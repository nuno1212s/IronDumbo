use atlas_common::node_id::NodeId;
use getset::{CopyGetters, Getters};

#[derive(Debug, Clone, PartialEq, Eq, Getters, CopyGetters)]
pub(super) struct AsyncBinaryAgreementMessage {
    #[get = "pub (super)"]
    message_type: AsyncBinaryAgreementMessageType,
    #[get_copy = "pub(super)"]
    round: usize,
    #[get_copy = "pub(super)"]
    sender: NodeId,
}

impl AsyncBinaryAgreementMessage {
    pub(super) fn new(
        message_type: AsyncBinaryAgreementMessageType,
        round: usize,
        sender: NodeId,
    ) -> Self {
        Self {
            message_type,
            round,
            sender,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AsyncBinaryAgreementMessageType {
    Est { estimate: bool },
    Aux { estimate: bool },
}
