use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;

pub trait ABAProtocol {
    type AsyncBinaryMessage: SerMsg;

    fn new(input_bit: bool) -> Self;

    /// Polls the protocol for new messages or decisions.
    /// Returns Some(AsyncBinaryAgreementResult) if there is a new message to send or
    ///
    fn poll(&mut self) -> Option<StoredMessage<Self::AsyncBinaryMessage>>;

    /// Processes an incoming message.
    /// Returns an AsyncBinaryAgreementResult indicating the outcome of processing the message.
    ///
    ///
    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::AsyncBinaryMessage>,
        network: &NT,
    ) -> AsyncBinaryAgreementResult
    where
        NT: AsyncBinaryAgreementSendNode<Self::AsyncBinaryMessage>;
}

/// Represents the result of processing a message in the asynchronous binary agreement protocol.
/// Indicates whether the message was queued, ignored, processed, or led to a decision.
pub enum AsyncBinaryAgreementResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    Decided(bool),
}

/// This trait defines the interface for sending messages in the context of an
/// asynchronous binary agreement protocol.
pub trait AsyncBinaryAgreementSendNode<M> {
    /// Broadcasts a message to all nodes in the network.
    fn broadcast_message<I>(&self, message: M, target: I) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
        M: SerMsg;
}
