use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::async_bin_agreement::network::AsyncBinaryAgreementSendNode;
use atlas_common::node_id::NodeId;
use atlas_communication::message::StoredMessage;
use getset::Getters;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
enum AsyncBinaryAgreementState {
    Init,
    CollectingEcho,
    CollectingReady,
    Finalized,
}

impl Default for AsyncBinaryAgreementState {
    fn default() -> Self {
        AsyncBinaryAgreementState::Init
    }
}

pub(super) struct AsyncBinaryAgreement {
    round: usize,
    input_bit: bool,
    current_round: RoundData,
    previous_rounds: Vec<RoundData>,
    pending_messages: PendingMessages,
}

impl AsyncBinaryAgreement {
    pub fn new(input_bit: bool) -> Self {
        Self {
            round: 0,
            input_bit,
            current_round: RoundData::default(),
            previous_rounds: Vec::new(),
            pending_messages: PendingMessages::default(),
        }
    }

    pub fn process_message<NT>(
        &mut self,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode,
    {
        let (header, async_bin_message) = message.clone().into_inner();

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
            AsyncBinaryAgreementMessageType::Est { estimate } => {
                match self
                    .current_round
                    .accept_estimate(header.from(), &async_bin_message)
                {
                    RoundDataVoteAcceptResult::Accepted => {
                        AsyncBinaryAgreementResult::Processed(message)
                    }
                    RoundDataVoteAcceptResult::Ignored => {
                        AsyncBinaryAgreementResult::MessageIgnored
                    }
                    RoundDataVoteAcceptResult::AlreadyAccepted => {
                        AsyncBinaryAgreementResult::MessageIgnored
                    }
                }
            }
            AsyncBinaryAgreementMessageType::Aux { estimate } => {
                match self.current_round.accept_auxiliary(header.from(), &async_bin_message) {
                    RoundDataVoteAcceptResult::Accepted => {
                        AsyncBinaryAgreementResult::Processed(message)
                    }
                    RoundDataVoteAcceptResult::Ignored => {
                        AsyncBinaryAgreementResult::MessageIgnored
                    }
                    RoundDataVoteAcceptResult::AlreadyAccepted => {
                        AsyncBinaryAgreementResult::MessageIgnored
                    }
                }
            }
        }
    }
}

pub(super) enum AsyncBinaryAgreementResult {
    MessageQueued,
    MessageIgnored,
    Processed(StoredMessage<AsyncBinaryAgreementMessage>),
    Decided,
}

#[derive(Debug, Default, Getters)]
struct RoundData {
    #[get = "pub"]
    state: AsyncBinaryAgreementState,
    #[get = "pub"]
    received_est: Vec<NodeId>,
    #[get = "pub"]
    received_aux: Vec<NodeId>,
}

impl RoundData {
    pub fn accept_estimate(
        &mut self,
        sender: NodeId,
        message: &AsyncBinaryAgreementMessage,
    ) -> RoundDataVoteAcceptResult {
        match self.state {
            AsyncBinaryAgreementState::Init => {}
            AsyncBinaryAgreementState::CollectingEcho => {}
            AsyncBinaryAgreementState::CollectingReady => {}
            AsyncBinaryAgreementState::Finalized => {}
        }
        todo!()
    }

    pub fn accept_auxiliary(
        &mut self,
        sender: NodeId,
        message: &AsyncBinaryAgreementMessage,
    ) -> RoundDataVoteAcceptResult {
        todo!()
    }
}

enum RoundDataVoteAcceptResult {
    Accepted,
    Ignored,
    AlreadyAccepted,
}

#[derive(Default)]
struct PendingMessages {
    current_round_base: usize,
    per_round_messages: VecDeque<Vec<StoredMessage<AsyncBinaryAgreementMessage>>>,
}

impl PendingMessages {
    pub fn new(current_round_base: usize) -> Self {
        Self {
            current_round_base,
            per_round_messages: VecDeque::new(),
        }
    }

    pub fn add_message(
        &mut self,
        round: usize,
        message: StoredMessage<AsyncBinaryAgreementMessage>,
    ) {
        while self.per_round_messages.len() <= round {
            self.per_round_messages.push_back(Vec::new());
        }
        if let Some(messages) = self.per_round_messages.get_mut(round) {
            messages.push(message);
        }
    }

    pub fn pop_message(&mut self) -> Option<StoredMessage<AsyncBinaryAgreementMessage>> {
        self.per_round_messages.front_mut()?.pop()
    }
}
