use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use atlas_communication::message::StoredMessage;
use std::collections::VecDeque;

#[derive(Default, Debug)]
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
        if round < self.current_round_base {
            // Ignore messages from rounds that have already been processed
            return;
        }

        let round_index = round - self.current_round_base;

        while self.per_round_messages.len() <= round_index {
            self.per_round_messages.push_back(Vec::new());
        }

        if let Some(messages) = self.per_round_messages.get_mut(round_index) {
            messages.push(message);
        }
    }

    pub fn pop_message(
        &mut self,
        round: usize,
    ) -> Option<StoredMessage<AsyncBinaryAgreementMessage>> {
        // discard all older rounds until we reach the requested round
        if round > self.current_round_base {
            // Calculate how many rounds to skip
            let rounds_to_skip = round - self.current_round_base;

            // Remove all rounds up to (but not including) the requested round
            self.per_round_messages
                .drain(0..rounds_to_skip)
                .for_each(|_| ()); // Just drain, we don't need the values

            // Update our base to the requested round
            self.current_round_base = round;
        }

        // Now we're at the requested round, get a message if it exists
        self.per_round_messages.front_mut()?.pop()
    }
}
