use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};
use crate::async_bin_agreement::async_bin_agreement_round::{RoundData, RoundDataVoteAcceptResult};
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::async_bin_agreement::pending_messages::PendingMessages;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::threshold_crypto::PartialSignature;
use atlas_communication::message::StoredMessage;
use getset::{CopyGetters, Getters};
use thiserror::Error;

/// Represents the state of an asynchronous binary agreement protocol.
/// It contains the current round, the input bit, the quorum information,
/// the current round data, the previous rounds, and the pending messages.
#[derive(Debug, Getters, CopyGetters)]
pub(crate) struct AsyncBinaryAgreement {
    #[get_copy = "pub"]
    round: usize,
    input_bit: Option<bool>,
    quorum_info: QuorumInfo,
    #[get = "pub(super)"]
    current_round: RoundData,
    previous_rounds: Vec<RoundData>,
    pending_messages: PendingMessages,
    threshold_key: ThresholdKeys,
    result: Option<bool>,
}

impl AsyncBinaryAgreement {
    pub(super) fn advance_round(&mut self, next_estimate: bool) {
        let f = self.quorum_info.f();

        let new_round = RoundData::new(
            f,
            self.threshold_key.public_key().clone(),
            Some(next_estimate),
        );
        let old_round = std::mem::replace(&mut self.current_round, new_round);

        self.previous_rounds.push(old_round);

        self.round += 1;
    }

    fn calculate_threshold_signature_for_round(&self, round: usize) -> PartialSignature {
        self.threshold_key
            .private_key()
            .partially_sign(&round.to_le_bytes()[..])
    }

    fn process_result<NT>(
        &mut self,
        network: &NT,
        message: Option<StoredMessage<<AsyncBinaryAgreement as ABAProtocol>::AsyncBinaryMessage>>,
        result: RoundDataVoteAcceptResult,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode<AsyncBinaryAgreementMessage>,
    {
        match result {
            RoundDataVoteAcceptResult::Accepted => AsyncBinaryAgreementResult::Processed,
            RoundDataVoteAcceptResult::Failed(next_estimate) => {
                // If we are in a failed state, we move to the next round, and must
                // broadcast our estimate for it: nobody else will vote in the new
                // round otherwise, since RoundData::new does not do this itself.
                self.advance_round(next_estimate);

                let est_message = AsyncBinaryAgreementMessage::new(
                    AsyncBinaryAgreementMessageType::Val {
                        estimate: next_estimate,
                    },
                    self.round,
                );

                network
                    .broadcast_message(
                        est_message,
                        self.quorum_info.quorum_members().iter().cloned(),
                    )
                    .expect("Failed to broadcast estimate message");

                AsyncBinaryAgreementResult::Processed
            }
            RoundDataVoteAcceptResult::Finalized(result) => {
                self.result = Some(result);
                AsyncBinaryAgreementResult::Decided
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

                AsyncBinaryAgreementResult::Processed
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

                AsyncBinaryAgreementResult::Processed
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

                AsyncBinaryAgreementResult::Processed
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

                AsyncBinaryAgreementResult::Processed
            }
            RoundDataVoteAcceptResult::Queue if message.is_some() => {
                // If we are collecting echoes, we queue the message for later processing
                self.pending_messages
                    .add_message(self.round, message.unwrap());
                AsyncBinaryAgreementResult::MessageQueued
            }
            RoundDataVoteAcceptResult::Ignored | RoundDataVoteAcceptResult::AlreadyAccepted | _ => {
                AsyncBinaryAgreementResult::MessageIgnored
            }
        }
    }
}

impl ABAProtocol for AsyncBinaryAgreement {
    type AsyncBinaryMessage = AsyncBinaryAgreementMessage;
    type ABAError = ABAError;

    fn new(quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self {
        let f = quorum_info.f();

        Self {
            round: 0,
            input_bit: None,
            quorum_info,
            current_round: RoundData::new(f, threshold_keys.public_key().clone(), None),
            previous_rounds: Vec::new(),
            pending_messages: PendingMessages::default(),
            threshold_key: threshold_keys,
            result: None,
        }
    }

    fn provide_input_bit<NT>(
        &mut self,
        input_bit: bool,
        network: &NT,
    ) -> Result<AsyncBinaryAgreementResult, Self::ABAError>
    where
        NT: AsyncBinaryAgreementSendNode<Self::AsyncBinaryMessage>,
    {
        if self.input_bit.is_some() {
            return Err(ABAError::AlreadyProvidedInputBit);
        }

        self.input_bit = Some(input_bit);

        let result = self.current_round.accept_input(input_bit);

        Ok(self.process_result(network, None, result))
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::AsyncBinaryMessage>> {
        self.pending_messages.pop_message(self.round)
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::AsyncBinaryMessage>,
        network: &NT,
    ) -> Result<AsyncBinaryAgreementResult, ABAError>
    where
        NT: AsyncBinaryAgreementSendNode<Self::AsyncBinaryMessage>,
    {
        let round = message.message().round();

        if round > self.round {
            // If the message is from a future round, we need to update our state
            self.pending_messages.add_message(round, message);

            return Ok(AsyncBinaryAgreementResult::MessageQueued);
        } else if round < self.round {
            // If the message is from a past round, we can ignore it
            return Ok(AsyncBinaryAgreementResult::MessageIgnored);
        } else if self.result.is_some() {
            // If we have already decided, we can ignore the message
            return Ok(AsyncBinaryAgreementResult::MessageIgnored);
        }

        let (header, async_bin_message) = message.clone().into_inner();

        let (_, message_type) = async_bin_message.into_inner();

        let sender = header.from();

        let result = match message_type {
            AsyncBinaryAgreementMessageType::Val { estimate } => {
                self.current_round.accept_estimate(sender, estimate)
            }
            AsyncBinaryAgreementMessageType::Aux { accepted_estimates } => self
                .current_round
                .accept_auxiliary(sender, accepted_estimates),
            AsyncBinaryAgreementMessageType::Conf {
                feasible_values,
                partial_signature,
            } => self
                .current_round
                .accept_confirmation(sender, feasible_values, partial_signature),
            AsyncBinaryAgreementMessageType::Finish { value } => {
                self.current_round.accept_finish(sender, value)
            }
        };

        Ok(self.process_result(network, Some(message), result))
    }

    fn finalize(self) -> Result<bool, Self::ABAError> {
        if let Some(result) = self.result {
            Ok(result)
        } else {
            Err(ABAError::FailedToFinalizeNotReady)
        }
    }
}

#[derive(Error, Debug)]
pub enum ABAError {
    #[error("The aba protocol has failed to finalize as it is not ready to do so")]
    FailedToFinalizeNotReady,
    #[error("The input bit has already been provided")]
    AlreadyProvidedInputBit,
}
