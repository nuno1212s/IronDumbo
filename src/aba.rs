use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;

pub trait ABAProtocol {
    type AsyncBinaryMessage: SerMsg;

    fn new(input_bit: bool) -> Self;

    fn process_message(&mut self, message: StoredMessage<Self::AsyncBinaryMessage>) -> AsyncBinaryAgreementResult<Self::AsyncBinaryMessage>;
}

pub enum AsyncBinaryAgreementResult<M> {
    MessageQueued,
    MessageIgnored,
    Processed(StoredMessage<M>),
    Decided(bool, StoredMessage<M>),
}

/// This trait defines the interface for sending messages in the context of an
/// asynchronous binary agreement protocol.
pub trait AsyncBinaryAgreementSendNode<M> {
    /// Broadcasts a message to all nodes in the network.
    fn broadcast_message<I>(&self, message: M, target: I) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item=NodeId>,
        M: SerMsg;
}


pub struct ABASendNode<NT, M>
{
    node: NT,
    phantom_data: std::marker::PhantomData<fn(M) -> M>,
}
