use atlas_common::collections::{HashMap, HashSet, LinkedHashMap};
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::{
    CombineSignatureError, PartialSignature, PublicKeySet,
};
use atlas_common::node_id::NodeId;
use getset::Getters;

/// Represents the state of the asynchronous binary agreement round.
/// It contains the current state, the quorum size, the estimate, and the received votes.
#[derive(Debug, Getters)]
pub(super) struct RoundData {
    #[get = "pub"]
    state: AsyncBinaryAgreementState,
    // The quorum size 2f + 1, where f is the maximum number of faulty nodes for this round
    f: usize,
    pub_key: PublicKeySet,
    #[get = "pub"]
    estimate: bool,
    // The values that have been accepted by the round
    values_r: HashSet<bool>,
    val_data: ValRoundData,
    aux_round_data: AuxRoundData,
    conf_round_data: ConfRoundData,
    finish_round_data: FinishRoundData,
}

impl RoundData {
    pub fn new(f: usize, pub_key_set: PublicKeySet, estimate: bool) -> Self {
        Self {
            state: AsyncBinaryAgreementState::default(),
            f,
            pub_key: pub_key_set,
            estimate,
            values_r: HashSet::default(),
            val_data: ValRoundData::default(),
            aux_round_data: AuxRoundData::default(),
            conf_round_data: ConfRoundData::default(),
            finish_round_data: FinishRoundData::default(),
        }
    }

    pub(super) fn accept_estimate(
        &mut self,
        sender: NodeId,
        estimate: bool,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::CollectingVal => self.insert_estimate(sender, estimate),
            // If we are collecting ready messages, we ignore the estimate as we already completed it
            _ => RoundDataVoteAcceptResult::Ignored,
        }
    }

    fn insert_estimate(&mut self, sender: NodeId, estimate: bool) -> RoundDataVoteAcceptResult {
        let current_votes = match self.val_data.insert_estimate(sender, estimate) {
            Ok(current_votes) => current_votes,
            Err(_) => return RoundDataVoteAcceptResult::AlreadyAccepted,
        };

        if current_votes >= 2 * self.f + 1 {
            self.values_r.insert(estimate);

            self.state = AsyncBinaryAgreementState::CollectingAux;

            return RoundDataVoteAcceptResult::BroadcastAux(
                self.values_r.clone().into_iter().collect(),
            );
        }

        if current_votes >= self.f + 1 && self.val_data.broadcast_estimates.insert(estimate) {
            // Broadcast the estimate to all nodes
            return RoundDataVoteAcceptResult::BroadcastEst(estimate);
        }

        RoundDataVoteAcceptResult::Accepted
    }

    pub(super) fn accept_auxiliary(
        &mut self,
        sender: NodeId,
        accepted_estimates: Vec<bool>,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::CollectingAux => self.insert_aux(sender, accepted_estimates),
            AsyncBinaryAgreementState::CollectingVal => RoundDataVoteAcceptResult::Queue,
            AsyncBinaryAgreementState::Finishing | AsyncBinaryAgreementState::CollectingConf => {
                RoundDataVoteAcceptResult::Ignored
            }
        }
    }

    fn insert_aux(
        &mut self,
        sender: NodeId,
        accepted_estimates: Vec<bool>,
    ) -> RoundDataVoteAcceptResult {
        let vote_count = match self
            .aux_round_data
            .insert_aux(sender, accepted_estimates.clone())
        {
            Ok(votes) => votes,
            Err(_) => return RoundDataVoteAcceptResult::AlreadyAccepted,
        };

        let accepted_estimates = accepted_estimates.into_iter().collect::<HashSet<_>>();

        if vote_count >= 2 * self.f + 1
            && (self.values_r.is_superset(&accepted_estimates)
                || self.values_r.eq(&accepted_estimates))
        {
            self.state = AsyncBinaryAgreementState::CollectingConf;

            return RoundDataVoteAcceptResult::BroadcastConf(
                self.values_r.clone().into_iter().collect(),
            );
        }

        RoundDataVoteAcceptResult::Accepted
    }

    pub(super) fn accept_confirmation(
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
            AsyncBinaryAgreementState::Finishing => RoundDataVoteAcceptResult::Ignored,
        }
    }

    fn insert_confirmation(
        &mut self,
        sender: NodeId,
        feasible_values: Vec<bool>,
        partial_signature: PartialSignature,
    ) -> RoundDataVoteAcceptResult {
        let vote_count = match self.conf_round_data.insert_confirmation(
            sender,
            feasible_values.clone(),
            partial_signature,
        ) {
            Ok(votes) => votes,
            Err(_) => return RoundDataVoteAcceptResult::AlreadyAccepted,
        };

        if vote_count >= 2 * self.f + 1 {
            let feasible_value_set = feasible_values.iter().cloned().collect::<HashSet<_>>();

            if self.values_r.is_superset(&feasible_value_set) || self.values_r == feasible_value_set
            {
                let signatures = self
                    .conf_round_data
                    .get_signatures_for_values(&feasible_values);

                return self
                    .perform_coin_flip(&feasible_values, signatures)
                    .unwrap_or_else(|_| RoundDataVoteAcceptResult::Failed(self.estimate));
            }
        }

        RoundDataVoteAcceptResult::Accepted
    }

    fn perform_coin_flip(
        &mut self,
        winning_set: &Vec<bool>,
        partial_signature: Vec<(NodeId, PartialSignature)>,
    ) -> Result<RoundDataVoteAcceptResult, CombineSignatureError> {
        let signatures = partial_signature
            .iter()
            .map(|(node, sig)| (node.0 as usize, sig));

        let combined_signature = self.pub_key.combine_signatures(signatures)?;

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
            // If the winning set is not a single value, we ignore it,
            // And move to the next round with the coin flip result as the estimate
            return Ok(RoundDataVoteAcceptResult::Failed(coin_flip_result));
        }

        if winning_set[0] == coin_flip_result {
            // If the winning set is the same as the coin flip result, we finalize
            self.state = AsyncBinaryAgreementState::Finishing;
            self.estimate = coin_flip_result;

            if self.finish_round_data.try_register_broadcast(self.estimate) {
                Ok(RoundDataVoteAcceptResult::BroadcastFinalized(self.estimate))
            } else {
                Ok(RoundDataVoteAcceptResult::Accepted)
            }
        } else {
            // If the winning set is not the same as the coin flip result, we ignore it
            // And move to the next round with the same estimate (as we have all agreed on it)
            Ok(RoundDataVoteAcceptResult::Failed(winning_set[0]))
        }
    }

    pub(super) fn accept_finish(
        &mut self,
        sender: NodeId,
        final_value: bool,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::Finishing => self.insert_finish(sender, final_value),
            AsyncBinaryAgreementState::CollectingAux
            | AsyncBinaryAgreementState::CollectingVal
            | AsyncBinaryAgreementState::CollectingConf => RoundDataVoteAcceptResult::Queue,
        }
    }

    fn insert_finish(&mut self, sender: NodeId, final_value: bool) -> RoundDataVoteAcceptResult {
        let vote_count = match self.finish_round_data.insert_finish(sender, final_value) {
            Ok(votes) => votes,
            Err(_) => return RoundDataVoteAcceptResult::AlreadyAccepted,
        };

        if vote_count >= 2 * self.f + 1 {
            return RoundDataVoteAcceptResult::Finalized(final_value);
        } else if vote_count >= self.f + 1
            && self.finish_round_data.try_register_broadcast(final_value)
        {
            return RoundDataVoteAcceptResult::BroadcastFinalized(final_value);
        }

        RoundDataVoteAcceptResult::Accepted
    }
}

/// Represents the data for the val part of the round in the asynchronous binary agreement protocol.
#[derive(Debug, Clone, Default, Getters)]
struct ValRoundData {
    #[get = "pub"]
    received_vals: LinkedHashMap<bool, HashSet<NodeId>>,
    // The estimates that have been broadcasted by our node in this round
    broadcast_estimates: HashSet<bool>,
}

impl ValRoundData {
    fn insert_estimate(&mut self, sender: NodeId, estimate: bool) -> Result<usize, ()> {
        let entry = self.received_vals.entry(estimate).or_default();

        if entry.insert(sender) {
            Ok(entry.len())
        } else {
            Err(())
        }
    }
}

/// Represents the data for the aux part of the round in the asynchronous binary agreement protocol.
#[derive(Debug, Clone, Default, Getters)]
struct AuxRoundData {
    #[get = "pub"]
    received_aux: LinkedHashMap<Vec<bool>, HashSet<NodeId>>,
}

impl AuxRoundData {
    fn insert_aux(&mut self, sender: NodeId, accepted_estimates: Vec<bool>) -> Result<usize, ()> {
        let entry = self.received_aux.entry(accepted_estimates).or_default();

        if entry.insert(sender) {
            Ok(entry.len())
        } else {
            Err(())
        }
    }
}

#[derive(Debug, Clone, Default, Getters)]
struct ConfRoundData {
    #[get = "pub"]
    received_conf: LinkedHashMap<Vec<bool>, HashMap<NodeId, PartialSignature>>,
}

impl ConfRoundData {
    fn insert_confirmation(
        &mut self,
        sender: NodeId,
        feasible_values: Vec<bool>,
        partial_signature: PartialSignature,
    ) -> Result<usize, ()> {
        let entry = self.received_conf.entry(feasible_values).or_default();

        if entry.contains_key(&sender) {
            Err(())
        } else {
            entry.insert(sender, partial_signature);
            Ok(entry.len())
        }
    }

    fn get_signatures_for_values(&self, values: &Vec<bool>) -> Vec<(NodeId, PartialSignature)> {
        if let Some(signatures) = self.received_conf.get(values) {
            signatures
                .iter()
                .map(|(node, sig)| (*node, sig.clone()))
                .collect()
        } else {
            vec![]
        }
    }
}

#[derive(Debug, Clone, Default, Getters)]
struct FinishRoundData {
    #[get = "pub"]
    received_finish: LinkedHashMap<bool, HashSet<NodeId>>,
    broadcast_finish: HashSet<bool>,
}

impl FinishRoundData {
    fn insert_finish(&mut self, sender: NodeId, final_value: bool) -> Result<usize, ()> {
        let entry = self.received_finish.entry(final_value).or_default();

        if entry.insert(sender) {
            Ok(entry.len())
        } else {
            Err(())
        }
    }

    fn try_register_broadcast(&mut self, final_value: bool) -> bool {
        self.broadcast_finish.insert(final_value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum AsyncBinaryAgreementState {
    CollectingVal,
    CollectingAux,
    CollectingConf,
    Finishing,
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
    BroadcastFinalized(bool),
    Ignored,
    AlreadyAccepted,
    Queue,
    Failed(bool),
    Finalized(bool),
}

const VOTE_VALUES: [bool; 2] = [false, true];
