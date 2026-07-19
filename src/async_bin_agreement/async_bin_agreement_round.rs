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
    estimate: Option<bool>,
    // The values that have been accepted by the round
    values_r: HashSet<bool>,
    val_data: ValRoundData,
    aux_round_data: AuxRoundData,
    conf_round_data: ConfRoundData,
    finish_round_data: FinishRoundData,
}

impl RoundData {
    pub fn new(f: usize, pub_key_set: PublicKeySet, estimate: Option<bool>) -> Self {
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

    pub(super) fn accept_input(&mut self, input: bool) -> RoundDataVoteAcceptResult {
        match self.estimate {
            None => {
                self.estimate = Some(input);

                match self.state {
                    AsyncBinaryAgreementState::CollectingVal => {
                        RoundDataVoteAcceptResult::BroadcastEst(input)
                    }
                    _ => RoundDataVoteAcceptResult::Ignored,
                }
            }
            Some(_) => RoundDataVoteAcceptResult::Ignored,
        }
    }

    pub(super) fn accept_estimate(
        &mut self,
        sender: NodeId,
        estimate: bool,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            // Algorithm 7 lines 3-7 are standing handlers active for the
            // whole round, not gated to a single phase: a Val vote must
            // keep being counted (and may grow values_r) even after this
            // node has moved on to collecting AUX/CONF votes. Only once
            // we've fully decided (Finishing) is a further Val vote moot.
            AsyncBinaryAgreementState::Finishing => RoundDataVoteAcceptResult::Ignored,
            _ => self.insert_estimate(sender, estimate),
        }
    }

    fn insert_estimate(&mut self, sender: NodeId, estimate: bool) -> RoundDataVoteAcceptResult {
        let current_votes = match self.val_data.insert_estimate(sender, estimate) {
            Ok(current_votes) => current_votes,
            Err(_) => return RoundDataVoteAcceptResult::AlreadyAccepted,
        };

        if current_votes > 2 * self.f {
            let is_new_value = self.values_r.insert(estimate);

            if is_new_value {
                if self.state == AsyncBinaryAgreementState::CollectingVal {
                    self.state = AsyncBinaryAgreementState::CollectingAux;

                    return RoundDataVoteAcceptResult::BroadcastAux(
                        self.values_r.clone().into_iter().collect(),
                    );
                }

                // A later value crossed 2f+1 after we already left
                // CollectingVal. AUX is only ever broadcast once (line
                // 8-9), so we don't re-broadcast here -- but values_r
                // growing may now let a peer's AUX/CONF vote that arrived
                // too early (and was merely `Accepted` at the time)
                // satisfy its subset check.
                if let Some(result) = self.recheck_pending_aux() {
                    return result;
                }

                if let Some(result) = self.recheck_pending_conf() {
                    return result;
                }
            }

            return RoundDataVoteAcceptResult::Accepted;
        }

        if current_votes > self.f && self.val_data.broadcast_estimates.insert(estimate) {
            // Broadcast the estimate to all nodes
            return RoundDataVoteAcceptResult::BroadcastEst(estimate);
        }

        RoundDataVoteAcceptResult::Accepted
    }

    /// Re-evaluates already-received AUX votes against the current
    /// `values_r` (Algorithm 7 line 10: `val_r ⊆ values_r`). A matching AUX
    /// vote may have arrived before `values_r` grew enough to satisfy this
    /// check; without this re-check it would sit "accepted but blocked"
    /// forever once no further AUX messages arrive.
    fn recheck_pending_aux(&mut self) -> Option<RoundDataVoteAcceptResult> {
        if self.state != AsyncBinaryAgreementState::CollectingAux {
            return None;
        }

        let f = self.f;
        let values_r = self.values_r.clone();

        let now_satisfied =
            self.aux_round_data
                .received_aux
                .iter()
                .any(|(accepted_estimates, voters)| {
                    if voters.len() <= 2 * f {
                        return false;
                    }

                    let accepted_set: HashSet<bool> = accepted_estimates.iter().cloned().collect();

                    values_r.is_superset(&accepted_set) || values_r == accepted_set
                });

        if now_satisfied {
            self.state = AsyncBinaryAgreementState::CollectingConf;

            Some(RoundDataVoteAcceptResult::BroadcastConf(
                values_r.into_iter().collect(),
            ))
        } else {
            None
        }
    }

    /// Symmetric re-check for CONF votes (Algorithm 7 line 12), for the
    /// rarer case where a very late Val vote grows `values_r` during the
    /// CONF phase.
    fn recheck_pending_conf(&mut self) -> Option<RoundDataVoteAcceptResult> {
        if self.state != AsyncBinaryAgreementState::CollectingConf {
            return None;
        }

        let f = self.f;
        let values_r = self.values_r.clone();

        let matching =
            self.conf_round_data
                .received_conf
                .iter()
                .find_map(|(feasible_values, signers)| {
                    if signers.len() <= 2 * f {
                        return None;
                    }

                    let feasible_set: HashSet<bool> = feasible_values.iter().cloned().collect();

                    if values_r.is_superset(&feasible_set) || values_r == feasible_set {
                        Some(feasible_values.clone())
                    } else {
                        None
                    }
                })?;

        let signatures = self.conf_round_data.get_signatures_for_values(&matching);

        Some(
            self.perform_coin_flip(&matching, signatures).unwrap_or(
                RoundDataVoteAcceptResult::Failed(
                    self.estimate
                        .unwrap_or_else(|| matching.first().cloned().unwrap_or_default()),
                ),
            ),
        )
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

        if vote_count > 2 * self.f
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

        if vote_count > 2 * self.f {
            let feasible_value_set = feasible_values.iter().cloned().collect::<HashSet<_>>();

            if self.values_r.is_superset(&feasible_value_set) || self.values_r == feasible_value_set
            {
                let signatures = self
                    .conf_round_data
                    .get_signatures_for_values(&feasible_values);

                return self
                    .perform_coin_flip(&feasible_values, signatures)
                    .unwrap_or(RoundDataVoteAcceptResult::Failed(
                        self.estimate.unwrap_or_else(|| {
                            feasible_values.first().cloned().unwrap_or_default()
                        }),
                    ));
            }
        }

        RoundDataVoteAcceptResult::Accepted
    }

    fn perform_coin_flip(
        &mut self,
        winning_set: &[bool],
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
            self.estimate = Some(coin_flip_result);

            if self
                .finish_round_data
                .try_register_broadcast(coin_flip_result)
            {
                Ok(RoundDataVoteAcceptResult::BroadcastFinalized(
                    coin_flip_result,
                ))
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

        if vote_count > 2 * self.f {
            return RoundDataVoteAcceptResult::Finalized(final_value);
        } else if vote_count > self.f && self.finish_round_data.try_register_broadcast(final_value)
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

        if let std::collections::hash_map::Entry::Vacant(e) = entry.entry(sender) {
            e.insert(partial_signature);
            Ok(entry.len())
        } else {
            Err(())
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(super) enum AsyncBinaryAgreementState {
    #[default]
    CollectingVal,
    CollectingAux,
    CollectingConf,
    Finishing,
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
