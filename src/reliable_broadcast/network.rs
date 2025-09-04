use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use atlas_common::node_id::NodeId;

pub(super) trait ReliableBroadcastSendNode<RQ> {
    /// Sends a message to a given target.
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the target or err if not. No other checks are made
    /// on the success of the message dispatch
    fn send(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()>;

    /// Sends a signed message to a given target
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the target or err if not. No other checks are made
    /// on the success of the message dispatch
    fn send_signed(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()>;

    /// Broadcast a message to all of the given targets
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the targets or err if not. No other checks are made
    /// on the success of the message dispatch
    fn broadcast<I>(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;

    /// Broadcast a signed message for all of the given targets
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the targets or err if not. No other checks are made
    /// on the success of the message dispatch
    fn broadcast_signed<I>(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;
}
