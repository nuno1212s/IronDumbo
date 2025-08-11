use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use atlas_common::error::Result;
use atlas_common::node_id::NodeId;

/// This trait defines the interface for sending messages in the context of an
/// asynchronous binary agreement protocol.
pub(super) trait AsyncBinaryAgreementSendNode {
    /// Sends an estimate message to a given target.
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the target or err if not. No other checks are made
    /// on the success of the message dispatch.
    fn send_estimate(&self, message: AsyncBinaryAgreementMessage, target: NodeId) -> Result<()>;

    /// Sends an auxiliary message to a given target.
    /// Does not block on the message sent. Returns a result that is
    /// Ok if there is a current connection to the target or err if not. No other checks are made
    /// on the success of the message dispatch.
    fn send_auxiliary(&self, message: AsyncBinaryAgreementMessage, target: NodeId) -> Result<()>;
}
