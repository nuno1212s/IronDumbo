use std::collections::VecDeque;

/// A simple FIFO queue for messages that arrived before they could be
/// processed. Unlike Dumbo1's `PendingMessages` (which indexes by owner and
/// message-type discriminant across four sub-protocols), Dumbo2 only ever
/// needs to queue MVBA messages that arrive before the round's single MVBA
/// instance has been constructed, so a plain queue is sufficient.
#[derive(Debug)]
pub(super) struct PendingMessages<M> {
    queue: VecDeque<M>,
}

impl<M> PendingMessages<M> {
    pub(super) fn add_message(&mut self, message: M) {
        self.queue.push_back(message);
    }

    pub(super) fn pop_message(&mut self) -> Option<M> {
        self.queue.pop_front()
    }
}

impl<M> Default for PendingMessages<M> {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}
