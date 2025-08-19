use crate::async_bin_agreement::async_bin_agreement_round::{RoundData, RoundDataVoteAcceptResult};
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::async_bin_agreement::network::AsyncBinaryAgreementSendNode;
use crate::async_bin_agreement::pending_messages::PendingMessages;
use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::threshold_crypto::{PartialSignature, PrivateKeyPart, PublicKeySet};
use atlas_communication::message::StoredMessage;

/// Represents the keys used in the threshold cryptography for the asynchronous binary agreement.
pub(super) struct ThresholdKeys(PublicKeySet, PrivateKeyPart);

/// Represents the state of an asynchronous binary agreement protocol.
/// It contains the current round, the input bit, the quorum information,
/// the current round data, the previous rounds, and the pending messages.
pub(super) struct AsyncBinaryAgreement {
    round: usize,
    input_bit: bool,
    quorum_info: QuorumInfo,
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

    pub fn process_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        let async_bin_message = message.message();

        if async_bin_message.round() > self.round {
            // If the message is from a future round, we need to update our state
            self.pending_messages
                .add_message(async_bin_message.round(), message);

            return AsyncBinaryAgreementResult::MessageQueued;
        } else if async_bin_message.round() < self.round {
            // If the message is from a past round, we can ignore it
            return AsyncBinaryAgreementResult::MessageIgnored;
        }

        match async_bin_message.message_type() {
            AsyncBinaryAgreementMessageType::Val { .. } => {
                self.process_val_message(message, network)
            }
            AsyncBinaryAgreementMessageType::Aux { .. } => {
                self.process_aux_message(message, network)
            }
            AsyncBinaryAgreementMessageType::Conf { .. } => {
                self.process_conf_message(message, network)
            },
            AsyncBinaryAgreementMessageType::Finish { .. } => todo!()
        }
    }

    fn process_val_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        let header = message.header();

        let AsyncBinaryAgreementMessageType::Val { estimate } = message.message().message_type()
        else {
            unreachable!()
        };

        match self.current_round.accept_estimate(header.from(), *estimate) {
            RoundDataVoteAcceptResult::Accepted => AsyncBinaryAgreementResult::Processed(message),
            RoundDataVoteAcceptResult::Failed(next_estimate) => {
                // If we are in a failed state, we ignore the message
                self.advance_round(next_estimate);
                AsyncBinaryAgreementResult::MessageIgnored
            }
            RoundDataVoteAcceptResult::Finalized => {
                // If we are in a finalized state, we ignore the message
                AsyncBinaryAgreementResult::Decided(message)
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
            RoundDataVoteAcceptResult::Queue => {
                // If we are collecting echoes, we queue the message for later processing
                self.pending_messages.add_message(self.round, message);
                AsyncBinaryAgreementResult::MessageQueued
            }
            RoundDataVoteAcceptResult::Ignored | RoundDataVoteAcceptResult::AlreadyAccepted => {
                AsyncBinaryAgreementResult::MessageIgnored
            }
            RoundDataVoteAcceptResult::BroadcastConf(_) => unreachable!(),
        }
    }

    fn process_aux_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        let header = message.header();

        let AsyncBinaryAgreementMessageType::Aux { accepted_estimates } =
            message.message().message_type()
        else {
            unreachable!()
        };

        match self
            .current_round
            .accept_auxiliary(header.from(), accepted_estimates.clone())
        {
            RoundDataVoteAcceptResult::Accepted => AsyncBinaryAgreementResult::Processed(message),
            RoundDataVoteAcceptResult::Failed(next_estimate) => {
                // If we are in a failed state, we ignore the message
                self.advance_round(next_estimate);
                AsyncBinaryAgreementResult::MessageIgnored
            }
            RoundDataVoteAcceptResult::BroadcastConf(feasible_values) => {
                // If we are collecting ready messages, we broadcast the confirmation
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
            RoundDataVoteAcceptResult::Finalized => {
                // If we are in a finalized state, we ignore the message
                AsyncBinaryAgreementResult::Decided(message)
            }
            RoundDataVoteAcceptResult::Ignored | RoundDataVoteAcceptResult::AlreadyAccepted => {
                AsyncBinaryAgreementResult::MessageIgnored
            }
            RoundDataVoteAcceptResult::Queue => {
                // If we are collecting echoes, we queue the message for later processing
                self.pending_messages.add_message(self.round, message);
                AsyncBinaryAgreementResult::MessageQueued
            }
            RoundDataVoteAcceptResult::BroadcastEst(_)
            | RoundDataVoteAcceptResult::BroadcastAux(_) => {
                unreachable!()
            }
        }
    }

    fn process_conf_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        todo!();
    }

    fn process_finish_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        todo!();
    }

    fn advance_round(&mut self, next_estimate: bool) {
        let f = self.quorum_info.f();

        let new_round = RoundData::new(f, self.threshold_key.0.clone(), next_estimate);
        let old_round = std::mem::replace(&mut self.current_round, new_round);
        self.previous_rounds.push(old_round);
        self.round += 1;
    }

    fn calculate_threshold_signature_for_round(&self, round: usize) -> PartialSignature {
        self.threshold_key.1.partially_sign(&round.to_le_bytes()[..])
    }
}

pub(super) enum AsyncBinaryAgreementResult {
    MessageQueued,
    MessageIgnored,
    Processed(StoredMessage<AsyncBinaryAgreementMessage>),
    Decided(StoredMessage<AsyncBinaryAgreementMessage>),
}
