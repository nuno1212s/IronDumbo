use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use atlas_common::error::Result;
use atlas_common::node_id::NodeId;

/// This trait defines the interface for sending messages in the context of an
/// asynchronous binary agreement protocol.
pub(super) trait AsyncBinaryAgreementSendNode {

    /// Broadcasts a message to all nodes in the network.
    fn broadcast_message<I>(&self, message: AsyncBinaryAgreementMessage, target: I) -> Result<()>
    where
        I: Iterator<Item = NodeId>;
}
