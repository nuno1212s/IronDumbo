use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;

/// A trait representing an asynchronous binary agreement protocol.
///
/// Event driven protocol where the orchestrator controls the execution of the protocol
/// The implementation is expected to queue messages from future rounds and ignore messages from past rounds.
/// The orchestrator polls regularly to check if there are any messages which are now ready to be processed
/// due to progress in the protocol.
/// See the [`AsyncBinaryAgreementResult`] enum for possible outcomes of the protocol a message.
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
