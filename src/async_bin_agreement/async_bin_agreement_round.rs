use atlas_common::collections::{HashMap, HashSet, LinkedHashMap};
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::{PartialSignature, PublicKeySet};
use atlas_common::node_id::NodeId;
use getset::Getters;

/// Represents the state of the asynchronous binary agreement round.
/// It contains the current state, the quorum size, the estimate, and the received votes.
///
#[derive(Debug, Getters)]
pub(super) struct RoundData {
    #[get = "pub"]
    state: AsyncBinaryAgreementState,
    // The quorum size 2f + 1, where f is the maximum number of faulty nodes for this round
    f: usize,
    pub_key: PublicKeySet,
    #[get = "pub"]
    estimate: bool,
    #[get = "pub"]
    received_vals: LinkedHashMap<bool, HashSet<NodeId>>,
    // The estimates that have been broadcasted by our node in this round
    broadcast_estimates: HashSet<bool>,
    // The values that have been accepted by the round
    values_r: HashSet<bool>,
    #[get = "pub"]
    received_aux: LinkedHashMap<Vec<bool>, HashSet<NodeId>>,
    #[get = "pub"]
    received_conf: LinkedHashMap<Vec<bool>, HashMap<NodeId, PartialSignature>>,
}

impl RoundData {
    pub fn new(f: usize, pub_key_set: PublicKeySet, estimate: bool) -> Self {
        Self {
            state: AsyncBinaryAgreementState::default(),
            f,
            pub_key: pub_key_set,
            estimate,
            received_vals: LinkedHashMap::default(),
            broadcast_estimates: HashSet::default(),
            values_r: HashSet::default(),
            received_aux: LinkedHashMap::default(),
            received_conf: LinkedHashMap::default(),
        }
    }

    pub fn accept_estimate(&mut self, sender: NodeId, estimate: bool) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::CollectingVal => self.insert_estimate(sender, estimate),
            // If we are collecting ready messages, we ignore the estimate as we already completed it
            _ => RoundDataVoteAcceptResult::Ignored,
        }
    }

    fn insert_estimate(&mut self, sender: NodeId, estimate: bool) -> RoundDataVoteAcceptResult {
        let success = self
            .received_vals
            .entry(estimate)
            .or_default()
            .insert(sender);

        if !success {
            return RoundDataVoteAcceptResult::AlreadyAccepted;
        }

        if self.received_vals[&estimate].len() >= 2 * self.f + 1 {
            self.values_r.insert(estimate);

            self.state = AsyncBinaryAgreementState::CollectingAux;

            return RoundDataVoteAcceptResult::BroadcastAux(
                self.values_r.clone().into_iter().collect(),
            );
        }

        if self.received_vals[&estimate].len() >= self.f + 1
            && self.broadcast_estimates.insert(estimate)
        {
            // Broadcast the estimate to all nodes
            return RoundDataVoteAcceptResult::BroadcastEst(estimate);
        }

        RoundDataVoteAcceptResult::Accepted
    }

    pub fn accept_auxiliary(
        &mut self,
        sender: NodeId,
        accepted_estimates: Vec<bool>,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::CollectingAux => self.insert_aux(sender, accepted_estimates),
            AsyncBinaryAgreementState::CollectingVal => RoundDataVoteAcceptResult::Queue,
            AsyncBinaryAgreementState::Finalized | AsyncBinaryAgreementState::CollectingConf => {
                RoundDataVoteAcceptResult::Ignored
            }
        }
    }

    fn insert_aux(
        &mut self,
        sender: NodeId,
        accepted_estimates: Vec<bool>,
    ) -> RoundDataVoteAcceptResult {
        let vote_count = {
            let entry_vote = self
                .received_aux
                .entry(accepted_estimates.clone())
                .or_default();

            // If the sender has already voted for this estimate, we ignore the message
            if !entry_vote.insert(sender) {
                return RoundDataVoteAcceptResult::AlreadyAccepted;
            }

            entry_vote.len()
        };

        let accepted_estimates = accepted_estimates.into_iter().collect::<HashSet<_>>();

        if vote_count >= 2 * self.f + 1
            && (self.values_r.is_superset(&accepted_estimates)
                || self.values_r.eq(&accepted_estimates))
        {
            return RoundDataVoteAcceptResult::BroadcastConf(
                self.values_r.clone().into_iter().collect(),
            );
        }

        RoundDataVoteAcceptResult::Accepted
    }

    fn accept_confirmation(
        &mut self,
        sender: NodeId,
        feasible_values: Vec<bool>,
        signature: PartialSignature,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::CollectingConf => {
                self.insert_confirmation(sender, feasible_values, signature)
            }
            AsyncBinaryAgreementState::CollectingAux | AsyncBinaryAgreementState::CollectingVal => {
                RoundDataVoteAcceptResult::Queue
            }
            AsyncBinaryAgreementState::Finalized => RoundDataVoteAcceptResult::Finalized,
        }
    }

    fn insert_confirmation(
        &mut self,
        sender: NodeId,
        feasible_values: Vec<bool>,
        partial_signature: PartialSignature,
    ) -> RoundDataVoteAcceptResult {
        let vote_count = {
            let entry_vote = self
                .received_conf
                .entry(feasible_values.clone())
                .or_default();

            if entry_vote.contains_key(&sender) {
                // If the sender has already voted for this confirmation, we ignore the message
                return RoundDataVoteAcceptResult::AlreadyAccepted;
            }

            entry_vote.insert(sender, partial_signature);

            entry_vote.len()
        };

        let feasible_value_set = feasible_values.into_iter().collect::<HashSet<_>>();

        if vote_count >= 2 * self.f + 1
            && (self.values_r.is_superset(&feasible_value_set)
                || self.values_r == feasible_value_set)
        {
            return RoundDataVoteAcceptResult::Finalized;
        }

        RoundDataVoteAcceptResult::Accepted
    }

    fn perform_coin_flip(
        &mut self,
        winning_set: Vec<bool>,
        partial_signature: Vec<(NodeId, PartialSignature)>,
    ) -> RoundDataVoteAcceptResult {
        let signatures = partial_signature
            .iter()
            .map(|(node, sig)| (node.0 as usize, sig));

        let combined_signature = self
            .pub_key
            .combine_signatures(signatures)
            .expect("Failed to combine signatures");

        // I want to hash the combined signature to get a deterministic value
        // and then use that value to % 2 to get the coin flip result
        let mut hash_ctx = Context::new();

        // I will need to serialize the combined signature
        let serialized_sig =
            bincode::serde::encode_to_vec(&combined_signature, bincode::config::standard())
                .expect("Failed to serialize combined signature");

        hash_ctx.update(&serialized_sig);

        let hash = hash_ctx.finish();

        let coin_flip_result = hash.as_ref()[Digest::LENGTH - 1] % 2 == 0;

        if winning_set.len() != 1 {
            // If the winning set is not a single value, we ignore it
            return RoundDataVoteAcceptResult::Failed(coin_flip_result);
        }

        if winning_set[0] == coin_flip_result {
            // If the winning set is the same as the coin flip result, we finalize
            self.state = AsyncBinaryAgreementState::Finalized;
            self.estimate = coin_flip_result;

            RoundDataVoteAcceptResult::Accepted
        } else {
            // If the winning set is not the same as the coin flip result, we ignore it
            RoundDataVoteAcceptResult::Failed(winning_set[0])
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AsyncBinaryAgreementState {
    CollectingVal,
    CollectingAux,
    CollectingConf,
    Finalized,
}

impl Default for AsyncBinaryAgreementState {
    fn default() -> Self {
        AsyncBinaryAgreementState::CollectingVal
    }
}

/// Represents the result of accepting a vote in the round data.
#[derive(Debug, Clone)]
pub(super) enum RoundDataVoteAcceptResult {
    Accepted,
    BroadcastEst(bool),
    BroadcastAux(Vec<bool>),
    BroadcastConf(Vec<bool>),
    Ignored,
    AlreadyAccepted,
    Queue,
    Failed(bool),
    Finalized,
}

const VOTE_VALUES: [bool; 2] = [false, true];
