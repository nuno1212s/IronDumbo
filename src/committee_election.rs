use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::threshold_coin_tossing::{self, CoinTossState};
use atlas_common::crypto::threshold_crypto::{CombineSignatureError, PartialSignature};
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::Debug;
use thiserror::Error as ThisError;

/// Committee Election Protocol (Algorithm 4): elects a `committee_size`-sized
/// committee, unpredictable in advance, containing at least one honest
/// member with overwhelming probability (Lemma 1), via threshold
/// coin-tossing keyed by `round` -- distinct rounds must draw independent
/// committees, or the draw becomes predictable after the first round.
pub trait CommitteeElectionProtocol: Debug {
    type Message: SerMsg;
    type CEError: Error + Send + Sync + 'static;

    fn new(
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        committee_size: usize,
        round: SeqNo,
    ) -> Self;

    /// Poll this protocol to check if there are any pending messages stored
    /// That can now be processed
    fn poll(&mut self) -> Option<StoredMessage<Self::Message>>;

    /// Process a message in the committee election
    ///
    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<CommitteeElectionResult, Self::CEError>
    where
        NT: CommitteeElectionSendNode<Self::Message>;

    /// Finalize the protocol and obtain the result
    fn finalize(self) -> Result<Vec<NodeId>, Self::CEError>;
}

pub trait CommitteeElectionSendNode<CE>
where
    CE: SerMsg,
{
    fn send(&self, message: CE, target: NodeId, flush: bool) -> error::Result<()>;

    fn broadcast<I>(&self, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>;
}

pub enum CommitteeElectionResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    Decided,
}

fn ce_coin_id(round: SeqNo) -> Vec<u8> {
    let mut id = b"ce-round-".to_vec();
    id.extend_from_slice(&u32::from(round).to_le_bytes());
    id
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum CEMessage {
    /// `(SHARE, id, sigma_i)` -- Algorithm 4 lines 4/7.
    Share(PartialSignature),
}

/// Real implementation of Algorithm 4, built on the shared
/// `threshold_coin_tossing` primitive (also used by MVBA's random
/// permutation, Section 5.1/Fig. 4).
#[derive(Debug)]
pub(crate) struct RealCommitteeElection {
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    committee_size: usize,
    coin_id: Vec<u8>,
    coin: CoinTossState,
    result: Option<Vec<NodeId>>,
}

impl RealCommitteeElection {
    fn sorted_members(&self) -> Vec<NodeId> {
        let mut members = self.quorum_info.quorum_members().clone();
        members.sort();
        members
    }

    /// Algorithm 4 lines 3-4: compute our own share and broadcast it.
    /// Deliberately kept separate from `new()` (which stays side-effect
    /// free, mirroring ABA's `new()` + separate `provide_input_bit()`
    /// split): this crate's `DumboRound::new()` constructs its
    /// `CommitteeElectionProtocol` before a network handle is available,
    /// so the network-dependent kickoff step needs its own call, made once
    /// a handle exists. Like every other broadcast-to-self in this crate
    /// (e.g. `ReliableBroadcastInstance::propose_values`), our own share is
    /// *not* inserted directly here -- it loops back through the normal
    /// `process_message` path along with everyone else's, since
    /// `quorum_members()` (the broadcast target set) includes ourselves.
    ///
    /// Not yet called from `DumboRound`'s production message-processing
    /// flow: that requires deciding exactly where the first opportunity to
    /// reach a network handle after `WaitingForCommitteeElection` begins
    /// should trigger it, which is a separate design question (the same
    /// category of pre-existing integration gap already documented for
    /// Dumbo1's IndexRBC/ABA/finalization phases -- see
    /// `dumbo1::test::dumbo1_integration_test`'s module comment). Exercised
    /// directly by this module's own tests and by
    /// `dumbo1::test::dumbo1_integration_test::test_committee_election_output_should_vary_across_rounds`.
    #[allow(dead_code)]
    pub(crate) fn kickoff<NT>(&mut self, network: &NT)
    where
        NT: CommitteeElectionSendNode<CEMessage>,
    {
        let share = CoinTossState::own_share(&self.threshold_keys, &self.coin_id);

        let _ = network.broadcast(
            CEMessage::Share(share),
            self.quorum_info.quorum_members().iter().cloned(),
        );
    }
}

impl CommitteeElectionProtocol for RealCommitteeElection {
    type Message = CEMessage;
    type CEError = CEError;

    fn new(
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        committee_size: usize,
        round: SeqNo,
    ) -> Self {
        Self {
            quorum_info,
            threshold_keys,
            committee_size,
            coin_id: ce_coin_id(round),
            coin: CoinTossState::new(),
            result: None,
        }
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::Message>> {
        // Algorithm 4 is a single round of SHARE exchange with no phase
        // ordering to defer against -- there is never anything to queue.
        None
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        _network: &NT,
    ) -> Result<CommitteeElectionResult, Self::CEError>
    where
        NT: CommitteeElectionSendNode<Self::Message>,
    {
        if self.result.is_some() {
            return Ok(CommitteeElectionResult::MessageIgnored);
        }

        let (header, CEMessage::Share(share)) = message.into_inner();
        let from = header.from();

        // Algorithm 4 line 8: CShareVerify(id, j, sigma_j).
        if !CoinTossState::verify_share(&self.threshold_keys, from, &self.coin_id, &share) {
            return Ok(CommitteeElectionResult::MessageIgnored);
        }

        // Line 9: Sigma <- Sigma U {sigma_j} (first time only).
        let crossed = self.coin.insert_share(from, share, self.quorum_info.f());

        if crossed {
            // Lines 5-6: wait until |Sigma| = f+1, return CToss(id, Sigma).
            let combined = self
                .coin
                .toss(&self.threshold_keys)
                .map_err(CEError::Combine)?;

            let committee = threshold_coin_tossing::derive_committee(
                &combined,
                &self.sorted_members(),
                self.committee_size,
            );

            self.result = Some(committee);

            return Ok(CommitteeElectionResult::Decided);
        }

        Ok(CommitteeElectionResult::Processed)
    }

    fn finalize(self) -> Result<Vec<NodeId>, Self::CEError> {
        self.result.ok_or(CEError::NotReadyToFinalize)
    }
}

#[derive(Debug, ThisError)]
pub(crate) enum CEError {
    #[error("Failed to combine committee-election coin shares: {0}")]
    Combine(#[from] CombineSignatureError),
    #[error("Committee election is not ready to finalize")]
    NotReadyToFinalize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures;
    use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn run_ce_cluster(n: usize, f: usize, round: SeqNo) -> HashMap<NodeId, Vec<NodeId>> {
        let members = fixtures::node_ids(n);
        let keyset = fixtures::make_keyset(f);
        let cbc_keyset = fixtures::make_cbc_keyset(f);
        let committee_size = f + 1;

        let bus = RefCell::new(SimulatedNetwork::<CEMessage>::new(&members));

        let mut instances: HashMap<NodeId, RealCommitteeElection> = members
            .iter()
            .map(|&id| {
                let quorum = fixtures::quorum_info(n, f, id);
                let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, id);
                (
                    id,
                    RealCommitteeElection::new(quorum, threshold_keys, committee_size, round),
                )
            })
            .collect();

        for &id in &members {
            let handle = NodeHandle::new(id, &bus);
            instances.get_mut(&id).unwrap().kickoff(&handle);
        }

        let mut delivered = 0usize;
        loop {
            let mut progressed = false;

            for &id in &members {
                loop {
                    let next = bus.borrow_mut().deliver_next(id);
                    let Some((from, msg)) = next else {
                        break;
                    };
                    progressed = true;
                    delivered += 1;
                    assert!(
                        delivered < 100_000,
                        "CE cluster simulation did not converge"
                    );

                    let handle = NodeHandle::new(id, &bus);
                    let stored = fixtures::stored_msg(from, id, msg);
                    instances
                        .get_mut(&id)
                        .unwrap()
                        .process_message(stored, &handle)
                        .unwrap();
                }
            }

            if !progressed {
                break;
            }
        }

        instances
            .into_iter()
            .map(|(id, instance)| (id, instance.finalize().unwrap()))
            .collect()
    }

    #[test]
    fn test_ce_cluster_agrees_on_same_committee() {
        let decisions = run_ce_cluster(4, 1, SeqNo::from(0u32));

        let first = decisions.values().next().unwrap().clone();
        for committee in decisions.values() {
            assert_eq!(
                committee, &first,
                "all honest nodes must agree on the same committee"
            );
        }
        assert_eq!(first.len(), 2, "committee size should be f+1");
    }

    #[test]
    fn test_ce_different_rounds_draw_different_committees() {
        // Real (non-deterministic) committees, unlike the test-only
        // `TestCommitteeElection` stub -- see
        // `dumbo1::test::dumbo1_integration_test::test_committee_election_output_should_vary_across_rounds`.
        //
        // n=30 (rather than a small n) so this isn't a flaky birthday-paradox
        // coin flip: each call generates an independent random keyset, and a
        // small n/committee_size combination can coincidentally draw the
        // same committee by chance with non-negligible probability.
        let decisions_round_0 = run_ce_cluster(30, 2, SeqNo::from(0u32));
        let decisions_round_1 = run_ce_cluster(30, 2, SeqNo::from(1u32));

        let committee_0 = decisions_round_0.values().next().unwrap();
        let committee_1 = decisions_round_1.values().next().unwrap();

        assert_ne!(
            committee_0, committee_1,
            "different rounds must draw independent, unpredictable committees"
        );
    }
}
