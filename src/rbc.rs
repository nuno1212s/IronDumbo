use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage;
use std::marker::PhantomData;
use std::sync::Arc;

/// A trait representing a reliable broadcast protocol.
/// The protocol ensures that messages broadcasted by a node are reliably delivered to all correct nodes in the network.
///
pub trait ReliableBroadcast<RQ> {
    type ReliableBroadcastMessage: SerMsg;
    fn new() -> Self;

    fn new_with_propose<NT>(request: RQ, network: &NT) -> Self
    where
        NT: ReliableBroadcastSendNode<Self::ReliableBroadcastMessage>;

    fn poll(&mut self) -> Option<Self::ReliableBroadcastMessage>;

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::ReliableBroadcastMessage>,
        network: &NT,
    ) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<Self::ReliableBroadcastMessage>;

    fn finalize(self) -> RQ;
}

pub enum ReliableBroadcastResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    Finalized,
}

pub(super) trait ReliableBroadcastSendNode<BCM>
where
    BCM: SerMsg,
{

    /// Sends a signed message to a given target
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the target or err if not. No other checks are made
    /// on the success of the message dispatch
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()>;

    /// Broadcast a message to all of the given targets
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the targets or err if not. No other checks are made
    /// on the success of the message dispatch
    fn broadcast<I>(&self, message: BCM, targets: I) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;

}
