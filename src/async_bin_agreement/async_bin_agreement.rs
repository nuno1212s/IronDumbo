use crate::async_bin_agreement::async_bin_agreement_round::{RoundData, RoundDataVoteAcceptResult};
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::async_bin_agreement::pending_messages::PendingMessages;
use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::threshold_crypto::{PartialSignature, PrivateKeyPart, PublicKeySet};
use atlas_communication::message::StoredMessage;
use getset::{CopyGetters, Getters};
use crate::aba::{AsyncBinaryAgreementSendNode};

pub(super) type AsyncBinaryAgreementResult = crate::aba::AsyncBinaryAgreementResult<AsyncBinaryAgreementMessage>;

/// Represents the keys used in the threshold cryptography for the asynchronous binary agreement.
#[derive(Debug)]
pub(super) struct ThresholdKeys(PublicKeySet, PrivateKeyPart);

/// Represents the state of an asynchronous binary agreement protocol.
/// It contains the current round, the input bit, the quorum information,
/// the current round data, the previous rounds, and the pending messages.
#[derive(Debug, Getters, CopyGetters)]
pub(super) struct AsyncBinaryAgreement {
    #[get_copy = "pub"]
    round: usize,
    input_bit: bool,
    quorum_info: QuorumInfo,
    #[get = "pub(super)"]
    current_round: RoundData,
    previous_rounds: Vec<RoundData>,
    pending_messages: PendingMessages,
    threshold_key: ThresholdKeys,
}

impl AsyncBinaryAgreement {
    pub fn new(
        input_bit: bool,
        quorum_info: QuorumInfo,
        public_key_set: PublicKeySet,
        threshold_key: PrivateKeyPart,
    ) -> Self {
        let f = quorum_info.f();

        Self {
            round: 0,
            input_bit,
            quorum_info,
            current_round: RoundData::new(f, public_key_set.clone(), input_bit),
            previous_rounds: Vec::new(),
            pending_messages: PendingMessages::default(),
            threshold_key: ThresholdKeys(public_key_set, threshold_key),
        }
    }

    pub fn poll(&mut self) -> Option<StoredMessage<AsyncBinaryAgreementMessage>> {
        self.pending_messages.pop_message(self.round)
    }

    pub fn process_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode<AsyncBinaryAgreementMessage>,
    {

        let round = message.message().round();

        if round > self.round {
            // If the message is from a future round, we need to update our state
            self.pending_messages
                .add_message(round, message);

            return AsyncBinaryAgreementResult::MessageQueued;
        } else if round < self.round {
            // If the message is from a past round, we can ignore it
            return AsyncBinaryAgreementResult::MessageIgnored;
        }

        let (header, async_bin_message) = message.clone().into_inner();

        let (_, message_type) = async_bin_message.into_inner();

        let sender = header.from();

        let result = match message_type {
            AsyncBinaryAgreementMessageType::Val { estimate } => {
                self.current_round.accept_estimate(sender, estimate)
            }
            AsyncBinaryAgreementMessageType::Aux { accepted_estimates } => {
                self.current_round.accept_auxiliary(sender, accepted_estimates)
            }
            AsyncBinaryAgreementMessageType::Conf { feasible_values, partial_signature } => {
                self.current_round.accept_confirmation(sender, feasible_values, partial_signature)
            }
            AsyncBinaryAgreementMessageType::Finish { value } => {
                self.current_round.accept_finish(sender, value)
            }
        };

        match result {
            RoundDataVoteAcceptResult::Accepted => AsyncBinaryAgreementResult::Processed(message),
            RoundDataVoteAcceptResult::Failed(next_estimate) => {
                // If we are in a failed state, we move to the next round
                self.advance_round(next_estimate);
                AsyncBinaryAgreementResult::Processed(message)
            }
            RoundDataVoteAcceptResult::Finalized(result) => {
                AsyncBinaryAgreementResult::Decided(result, message)
            }
            RoundDataVoteAcceptResult::BroadcastEst(estimate) => {
                // If we are collecting echoes, we broadcast the estimate
                let est_message = AsyncBinaryAgreementMessage::new(
                    AsyncBinaryAgreementMessageType::Val { estimate },
                    self.round,
                );

                network
                    .broadcast_message(
                        est_message,
                        self.quorum_info.quorum_members().iter().cloned(),
                    )
                    .expect("Failed to broadcast estimate message");

                AsyncBinaryAgreementResult::Processed(message)
            }
            RoundDataVoteAcceptResult::BroadcastAux(accepted_estimates) => {
                // If we are collecting echoes, we broadcast the estimate
                let est_message = AsyncBinaryAgreementMessage::new(
                    AsyncBinaryAgreementMessageType::Aux { accepted_estimates },
                    self.round,
                );

                network
                    .broadcast_message(
                        est_message,
                        self.quorum_info.quorum_members().iter().cloned(),
                    )
                    .expect("Failed to broadcast estimate message");

                AsyncBinaryAgreementResult::Processed(message)
            }
            RoundDataVoteAcceptResult::BroadcastConf(feasible_values) => {
                // If we are collecting echoes, we broadcast the estimate
                let partial_signature = self.calculate_threshold_signature_for_round(self.round);

                let conf_message = AsyncBinaryAgreementMessage::new(
                    AsyncBinaryAgreementMessageType::Conf {
                        feasible_values,
                        partial_signature,
                    },
                    self.round,
                );

                network
                    .broadcast_message(
                        conf_message,
                        self.quorum_info.quorum_members().iter().cloned(),
                    )
                    .expect("Failed to broadcast confirmation message");

                AsyncBinaryAgreementResult::Processed(message)
            }
            RoundDataVoteAcceptResult::BroadcastFinalized(value) => {
                // If we are collecting echoes, we broadcast the estimate
                let finish_message = AsyncBinaryAgreementMessage::new(
                    AsyncBinaryAgreementMessageType::Finish { value },
                    self.round,
                );

                network
                    .broadcast_message(
                        finish_message,
                        self.quorum_info.quorum_members().iter().cloned(),
                    )
                    .expect("Failed to broadcast finalized message");

                AsyncBinaryAgreementResult::Processed(message)
            }
            RoundDataVoteAcceptResult::Queue => {
                // If we are collecting echoes, we queue the message for later processing
                self.pending_messages.add_message(self.round, message);
                AsyncBinaryAgreementResult::MessageQueued
            }
            RoundDataVoteAcceptResult::Ignored | RoundDataVoteAcceptResult::AlreadyAccepted => {
                AsyncBinaryAgreementResult::MessageIgnored
            }
        }
    }

    pub(super) fn advance_round(&mut self, next_estimate: bool) {
        let f = self.quorum_info.f();

        let new_round = RoundData::new(f, self.threshold_key.0.clone(), next_estimate);
        let old_round = std::mem::replace(&mut self.current_round, new_round);

        self.previous_rounds.push(old_round);

        self.round += 1;
    }

    fn calculate_threshold_signature_for_round(&self, round: usize) -> PartialSignature {
        self.threshold_key
            .1
            .partially_sign(&round.to_le_bytes()[..])
    }
}

