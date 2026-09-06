use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};
use crate::async_bin_agreement::async_bin_agreement::{ABAError, AsyncBinaryAgreement};
use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use crate::cbc::{CBCProtocol, CBCResult, CBCSendNode};
use crate::consistent_broadcast::consistent_broadcast::{
    CBCError, ConsistentBroadcastInstance, compute_digest,
};
use crate::consistent_broadcast::messages::CBCMessage;
use crate::multi_valued_byzantine_agreement::messages::MVBAMessage;
use crate::mvba::{MVBAProposal, MVBAProtocol, MVBAResult, MVBASendNode};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::threshold_coin_tossing::{self, CoinTossState};
use anyhow::anyhow;
use atlas_common::collections::{HashMap, HashSet};
use atlas_common::crypto::threshold_crypto::CombineSignatureError;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_communication::message::{Header, StoredMessage};
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    WaitingForProposals,
    CollectingCommitments,
    CollectingCoinShares,
    AgreementLoop,
    Done,
}

enum CbcSlot {
    Running(Box<ConsistentBroadcastInstance<MVBAProposal>>),
    Done,
}

fn mvba_coin_id(round: SeqNo) -> Vec<u8> {
    let mut id = b"mvba-pi-round-".to_vec();
    id.extend_from_slice(&u32::from(round).to_le_bytes());
    id
}

/// Verifies the paper's `Q_r` predicate (Algorithm 3): at least `n-f`
/// *distinct* owners in `proposal` carry a combined signature that
/// independently verifies against the quorum's (PRBC/Done, `f`-threshold)
/// public key. Guards against a Byzantine candidate padding duplicate
/// entries to inflate the count.
fn verify_q_r(
    quorum_info: &QuorumInfo,
    threshold_keys: &ThresholdKeys,
    proposal: &MVBAProposal,
) -> bool {
    let mut seen = HashSet::default();
    let mut valid = 0usize;

    for (owner, digest, signature) in proposal {
        if !seen.insert(*owner) {
            continue;
        }

        if threshold_keys
            .public_key()
            .verify(digest.as_ref(), signature)
            .is_ok()
        {
            valid += 1;
        }
    }

    valid >= quorum_info.quorum_size()
}

/// Wraps an [`MVBASendNode`] so a per-owner [`ConsistentBroadcastInstance`]
/// can send through it, tagging every outgoing message with which owner's
/// CBC it belongs to.
struct CbcNetworkAdapter<'a, NT> {
    owner: NodeId,
    inner: &'a NT,
}

impl<'a, NT> CBCSendNode<CBCMessage<MVBAProposal>> for CbcNetworkAdapter<'a, NT>
where
    NT: MVBASendNode<MVBAMessage>,
{
    fn send(
        &self,
        message: CBCMessage<MVBAProposal>,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.inner.send(
            MVBAMessage::Cbc {
                owner: self.owner,
                message,
            },
            target,
            flush,
        )
    }

    fn broadcast<I>(&self, message: CBCMessage<MVBAProposal>, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner.broadcast(
            MVBAMessage::Cbc {
                owner: self.owner,
                message,
            },
            targets,
        )
    }
}

/// Wraps an [`MVBASendNode`] so a per-candidate [`AsyncBinaryAgreement`]
/// instance can send through it, tagging every outgoing message with which
/// candidate's agreement-loop round it belongs to.
struct AbaNetworkAdapter<'a, NT> {
    candidate: NodeId,
    inner: &'a NT,
}

impl<'a, NT> AsyncBinaryAgreementSendNode<AsyncBinaryAgreementMessage> for AbaNetworkAdapter<'a, NT>
where
    NT: MVBASendNode<MVBAMessage>,
{
    fn broadcast_message<I>(
        &self,
        message: AsyncBinaryAgreementMessage,
        target: I,
    ) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner
            .broadcast(
                MVBAMessage::Aba {
                    candidate: self.candidate,
                    message,
                },
                target,
            )
            .map_err(|failed| anyhow!("Failed to broadcast to some nodes: {failed:?}"))
    }
}

/// A real MVBA instance implementing the Cachin-Kursawe-Shoup construction
/// (see `crate::mvba`'s doc comment): CBC-echo candidate proposals, an
/// early-commitment round, a shared-coin-derived random permutation, then a
/// sequential agreement loop trying one candidate at a time with a single
/// `AsyncBinaryAgreement` each.
pub(crate) struct MultiValuedByzantineAgreement {
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    round: SeqNo,
    own_proposed: bool,
    phase: Phase,
    cbcs: HashMap<NodeId, CbcSlot>,
    /// `Q_r`-validated CBC completions: owner -> (value, CBC combined sig).
    proposals: HashMap<
        NodeId,
        (
            MVBAProposal,
            atlas_common::crypto::threshold_crypto::CombinedSignature,
        ),
    >,
    commits: HashMap<NodeId, Vec<bool>>,
    coin: CoinTossState,
    permutation: Option<Vec<NodeId>>,
    loop_position: usize,
    current_aba: Option<AsyncBinaryAgreement>,
    vvotes: HashMap<NodeId, HashMap<NodeId, bool>>,
    decided_value: Option<MVBAProposal>,
    pending: VecDeque<StoredMessage<MVBAMessage>>,
}

impl MultiValuedByzantineAgreement {
    fn sorted_members(&self) -> Vec<NodeId> {
        let mut members = self.quorum_info.quorum_members().clone();
        members.sort();
        members
    }

    fn drain_pending<NT>(&mut self, network: &NT) -> Result<(), MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let pending = std::mem::take(&mut self.pending);

        for message in pending {
            self.process_message(message, network)?;
        }

        Ok(())
    }

    fn advance_phase<NT>(&mut self, network: &NT) -> Result<(), MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        loop {
            match self.phase {
                Phase::WaitingForProposals => {
                    if self.proposals.len() < self.quorum_info.quorum_size() {
                        return Ok(());
                    }

                    let sorted = self.sorted_members();
                    let committed: Vec<bool> = sorted
                        .iter()
                        .map(|m| self.proposals.contains_key(m))
                        .collect();

                    self.commits
                        .insert(self.quorum_info.own_node_id(), committed.clone());

                    let _ = network.broadcast(
                        MVBAMessage::Commit { committed },
                        self.quorum_info.quorum_members().iter().cloned(),
                    );

                    self.phase = Phase::CollectingCommitments;
                }
                Phase::CollectingCommitments => {
                    let quorum_size = self.quorum_info.quorum_size();
                    let qualifying = self
                        .commits
                        .values()
                        .filter(|c| c.iter().filter(|&&b| b).count() >= quorum_size)
                        .count();

                    if qualifying < quorum_size {
                        return Ok(());
                    }

                    let id = mvba_coin_id(self.round);
                    let own_id = self.quorum_info.own_node_id();
                    let share = CoinTossState::own_share(&self.threshold_keys, &id);

                    self.coin
                        .insert_share(own_id, share.clone(), self.quorum_info.f());

                    let _ = network.broadcast(
                        MVBAMessage::CoinShare { share },
                        self.quorum_info.quorum_members().iter().cloned(),
                    );

                    self.phase = Phase::CollectingCoinShares;
                }
                Phase::CollectingCoinShares => {
                    if self.coin.share_count() <= self.quorum_info.f() {
                        return Ok(());
                    }

                    let combined = self
                        .coin
                        .toss(&self.threshold_keys)
                        .map_err(MVBAError::Combine)?;

                    let permutation = threshold_coin_tossing::derive_permutation(
                        &combined,
                        &self.sorted_members(),
                    );

                    self.permutation = Some(permutation);
                    self.loop_position = 0;
                    self.phase = Phase::AgreementLoop;

                    self.start_current_candidate(network)?;
                    self.drain_pending(network)?;

                    return Ok(());
                }
                Phase::AgreementLoop | Phase::Done => return Ok(()),
            }
        }
    }

    fn start_current_candidate<NT>(&mut self, network: &NT) -> Result<(), MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let permutation = self
            .permutation
            .clone()
            .expect("permutation must be set before entering AgreementLoop");

        if self.loop_position >= permutation.len() {
            return Err(MVBAError::AgreementLoopExhausted);
        }

        let candidate = permutation[self.loop_position];
        self.current_aba = None;

        let vote = self.proposals.contains_key(&candidate);
        let proof = self.proposals.get(&candidate).cloned();

        let _ = network.broadcast(
            MVBAMessage::VVote {
                candidate,
                vote,
                proof,
            },
            self.quorum_info.quorum_members().iter().cloned(),
        );

        Ok(())
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<MVBAMessage>,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        if !self.own_proposed {
            self.pending.push_back(message);
            return Ok(MVBAResult::MessageQueued);
        }

        if matches!(self.phase, Phase::Done) {
            return Ok(MVBAResult::MessageIgnored);
        }

        let (header, msg) = message.into_inner();

        match msg {
            MVBAMessage::Cbc { owner, message } => {
                self.handle_cbc_message(owner, header, message, network)
            }
            MVBAMessage::Commit { committed } => {
                self.handle_commit(header.from(), committed, network)
            }
            MVBAMessage::CoinShare { share } => {
                self.handle_coin_share(header.from(), share, network)
            }
            MVBAMessage::VVote {
                candidate,
                vote,
                proof,
            } => self.handle_vvote(header, candidate, vote, proof, network),
            MVBAMessage::Aba { candidate, message } => {
                self.handle_aba_message(header, candidate, message, network)
            }
        }
    }

    fn handle_cbc_message<NT>(
        &mut self,
        owner: NodeId,
        header: Header,
        message: CBCMessage<MVBAProposal>,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let Some(CbcSlot::Running(cbc)) = self.cbcs.get_mut(&owner) else {
            return Ok(MVBAResult::MessageIgnored);
        };

        let adapter = CbcNetworkAdapter {
            owner,
            inner: network,
        };
        let stored = StoredMessage::new(header, message);
        let result = cbc
            .process_message(stored, &adapter)
            .map_err(MVBAError::Cbc)?;

        let outcome = match result {
            CBCResult::MessageIgnored => MVBAResult::MessageIgnored,
            CBCResult::Processed => MVBAResult::Processed,
            CBCResult::Finalized => {
                let Some(CbcSlot::Running(cbc)) = self.cbcs.remove(&owner) else {
                    unreachable!("checked above that the slot is Running");
                };

                let (value, _digest, signature) = cbc.finalize().map_err(MVBAError::Cbc)?;
                self.cbcs.insert(owner, CbcSlot::Done);

                if verify_q_r(&self.quorum_info, &self.threshold_keys, &value) {
                    self.proposals.entry(owner).or_insert((value, signature));
                }
                // else: CBC-delivered garbage from a Byzantine owner --
                // never adopted, never votable.

                self.advance_phase(network)?;

                MVBAResult::Processed
            }
        };

        if self.decided_value.is_some() {
            Ok(MVBAResult::Decided)
        } else {
            Ok(outcome)
        }
    }

    fn handle_commit<NT>(
        &mut self,
        from: NodeId,
        committed: Vec<bool>,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        if committed.len() != self.quorum_info.quorum_members().len() {
            return Ok(MVBAResult::MessageIgnored);
        }

        self.commits.insert(from, committed);
        self.advance_phase(network)?;

        if self.decided_value.is_some() {
            Ok(MVBAResult::Decided)
        } else {
            Ok(MVBAResult::Processed)
        }
    }

    fn handle_coin_share<NT>(
        &mut self,
        from: NodeId,
        share: atlas_common::crypto::threshold_crypto::PartialSignature,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let id = mvba_coin_id(self.round);

        if !CoinTossState::verify_share(&self.threshold_keys, from, &id, &share) {
            return Ok(MVBAResult::MessageIgnored);
        }

        self.coin.insert_share(from, share, self.quorum_info.f());
        self.advance_phase(network)?;

        if self.decided_value.is_some() {
            Ok(MVBAResult::Decided)
        } else {
            Ok(MVBAResult::Processed)
        }
    }

    fn handle_vvote<NT>(
        &mut self,
        header: Header,
        candidate: NodeId,
        vote: bool,
        proof: Option<(
            MVBAProposal,
            atlas_common::crypto::threshold_crypto::CombinedSignature,
        )>,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let from = header.from();

        let Some(permutation) = self.permutation.clone() else {
            self.pending.push_back(StoredMessage::new(
                header,
                MVBAMessage::VVote {
                    candidate,
                    vote,
                    proof,
                },
            ));
            return Ok(MVBAResult::MessageQueued);
        };

        let Some(pos) = permutation.iter().position(|&c| c == candidate) else {
            return Ok(MVBAResult::MessageIgnored);
        };

        if pos > self.loop_position {
            self.pending.push_back(StoredMessage::new(
                header,
                MVBAMessage::VVote {
                    candidate,
                    vote,
                    proof,
                },
            ));
            return Ok(MVBAResult::MessageQueued);
        }

        if pos < self.loop_position {
            return Ok(MVBAResult::MessageIgnored);
        }

        if vote {
            let Some((value, signature)) = proof else {
                return Ok(MVBAResult::MessageIgnored);
            };

            let digest = compute_digest(&value);

            if self
                .threshold_keys
                .cbc_public_key()
                .verify(digest.as_ref(), &signature)
                .is_err()
            {
                return Ok(MVBAResult::MessageIgnored);
            }

            if !verify_q_r(&self.quorum_info, &self.threshold_keys, &value) {
                return Ok(MVBAResult::MessageIgnored);
            }

            self.proposals
                .entry(candidate)
                .or_insert((value, signature));
        }

        let vote_count = {
            let votes = self.vvotes.entry(candidate).or_default();

            if votes.insert(from, vote).is_some() {
                return Ok(MVBAResult::Processed);
            }

            votes.len()
        };

        if vote_count == self.quorum_info.quorum_size() && self.current_aba.is_none() {
            let own_input = self.proposals.contains_key(&candidate);

            let mut aba =
                AsyncBinaryAgreement::new(self.quorum_info.clone(), self.threshold_keys.clone());
            let adapter = AbaNetworkAdapter {
                candidate,
                inner: network,
            };
            let result = aba
                .provide_input_bit(own_input, &adapter)
                .map_err(MVBAError::Aba)?;

            self.current_aba = Some(aba);

            return self.on_aba_result(candidate, result, network);
        }

        Ok(MVBAResult::Processed)
    }

    fn handle_aba_message<NT>(
        &mut self,
        header: Header,
        candidate: NodeId,
        message: AsyncBinaryAgreementMessage,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        let Some(permutation) = self.permutation.clone() else {
            self.pending.push_back(StoredMessage::new(
                header,
                MVBAMessage::Aba { candidate, message },
            ));
            return Ok(MVBAResult::MessageQueued);
        };

        let Some(pos) = permutation.iter().position(|&c| c == candidate) else {
            return Ok(MVBAResult::MessageIgnored);
        };

        if pos > self.loop_position {
            self.pending.push_back(StoredMessage::new(
                header,
                MVBAMessage::Aba { candidate, message },
            ));
            return Ok(MVBAResult::MessageQueued);
        }

        if pos < self.loop_position {
            return Ok(MVBAResult::MessageIgnored);
        }

        let Some(aba) = self.current_aba.as_mut() else {
            self.pending.push_back(StoredMessage::new(
                header,
                MVBAMessage::Aba { candidate, message },
            ));
            return Ok(MVBAResult::MessageQueued);
        };

        let adapter = AbaNetworkAdapter {
            candidate,
            inner: network,
        };
        let stored = StoredMessage::new(header, message);
        let result = aba
            .process_message(stored, &adapter)
            .map_err(MVBAError::Aba)?;

        self.on_aba_result(candidate, result, network)
    }

    fn on_aba_result<NT>(
        &mut self,
        candidate: NodeId,
        result: AsyncBinaryAgreementResult,
        network: &NT,
    ) -> Result<MVBAResult, MVBAError>
    where
        NT: MVBASendNode<MVBAMessage>,
    {
        match result {
            AsyncBinaryAgreementResult::Decided => {
                let aba = self
                    .current_aba
                    .take()
                    .expect("ABA must be running to have decided");
                let decided_bit = aba.finalize().map_err(MVBAError::Aba)?;

                if decided_bit {
                    let (value, _sig) = self.proposals.get(&candidate).cloned().expect(
                        "ABA decided 1 for `candidate` => some honest node's VVote carried a \
                         valid proof for it => we adopted it into `proposals` before the ABA \
                         could even have received enough 1-inputs to decide 1",
                    );

                    self.decided_value = Some(value);
                    self.phase = Phase::Done;

                    Ok(MVBAResult::Decided)
                } else {
                    self.loop_position += 1;
                    self.start_current_candidate(network)?;
                    self.drain_pending(network)?;

                    if self.decided_value.is_some() {
                        Ok(MVBAResult::Decided)
                    } else {
                        Ok(MVBAResult::Processed)
                    }
                }
            }
            AsyncBinaryAgreementResult::MessageQueued => Ok(MVBAResult::MessageQueued),
            AsyncBinaryAgreementResult::MessageIgnored => Ok(MVBAResult::MessageIgnored),
            AsyncBinaryAgreementResult::Processed => Ok(MVBAResult::Processed),
        }
    }
}

impl MVBAProtocol for MultiValuedByzantineAgreement {
    type Message = MVBAMessage;
    type Error = MVBAError;

    fn new(quorum_info: QuorumInfo, threshold_keys: ThresholdKeys, round: SeqNo) -> Self {
        let own_id = quorum_info.own_node_id();

        let cbcs = quorum_info
            .quorum_members()
            .iter()
            .filter(|&&id| id != own_id)
            .map(|&id| {
                (
                    id,
                    CbcSlot::Running(Box::new(ConsistentBroadcastInstance::new(
                        id,
                        quorum_info.clone(),
                        threshold_keys.clone(),
                    ))),
                )
            })
            .collect();

        Self {
            quorum_info,
            threshold_keys,
            round,
            own_proposed: false,
            phase: Phase::WaitingForProposals,
            cbcs,
            proposals: HashMap::default(),
            commits: HashMap::default(),
            coin: CoinTossState::new(),
            permutation: None,
            loop_position: 0,
            current_aba: None,
            vvotes: HashMap::default(),
            decided_value: None,
            pending: VecDeque::new(),
        }
    }

    fn propose<NT>(
        &mut self,
        proposal: MVBAProposal,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>,
    {
        if self.own_proposed {
            return Err(MVBAError::AlreadyProposed);
        }

        self.own_proposed = true;

        let own_id = self.quorum_info.own_node_id();
        let adapter = CbcNetworkAdapter {
            owner: own_id,
            inner: network,
        };
        let cbc = ConsistentBroadcastInstance::new_with_propose(
            own_id,
            self.quorum_info.clone(),
            self.threshold_keys.clone(),
            proposal,
            &adapter,
        );
        self.cbcs.insert(own_id, CbcSlot::Running(Box::new(cbc)));

        self.drain_pending(network)?;
        self.advance_phase(network)?;

        if self.decided_value.is_some() {
            Ok(MVBAResult::Decided)
        } else {
            Ok(MVBAResult::Processed)
        }
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>,
    {
        self.process_message(message, network)
    }

    fn finalize(self) -> Result<MVBAProposal, Self::Error> {
        self.decided_value.ok_or(MVBAError::NotReadyToFinalize)
    }
}

impl Debug for MultiValuedByzantineAgreement {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiValuedByzantineAgreement")
            .field("phase", &self.phase)
            .field("proposals", &self.proposals.len())
            .field("loop_position", &self.loop_position)
            .field("decided", &self.decided_value.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub(crate) enum MVBAError {
    #[error("Inner CBC error: {0}")]
    Cbc(CBCError),
    #[error("Inner ABA error: {0}")]
    Aba(ABAError),
    #[error("Failed to combine MVBA coin shares: {0}")]
    Combine(CombineSignatureError),
    #[error("Attempted to propose twice in the same MVBA instance")]
    AlreadyProposed,
    #[error("Agreement loop exhausted all candidates without deciding")]
    AgreementLoopExhausted,
    #[error("Attempted to finalize MVBA before it decided")]
    NotReadyToFinalize,
}
