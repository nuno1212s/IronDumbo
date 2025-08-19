use std::collections::VecDeque;
use atlas_communication::message::StoredMessage;
use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;

#[derive(Default)]
pub(super) struct PendingMessages {
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
