use std::error::Error;
use atlas_common::error;
use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::{Header, StoredMessage};

/// Committee Election Protocol.
/// 
/// Meant to represent a committee election protocol and 
pub trait CommitteeElectionProtocol {
    type Message: SerMsg;
    type CEError: Error + Send + Sync + 'static;

    fn new(quorum_info: QuorumInfo, committee_size: usize) -> Self;

    /// Poll this protocol to check if there are any pending messages stored
    /// That can now be processed
    fn poll(&mut self) -> Option<StoredMessage<Self::Message>>;

    /// Process a message in the committee election
    ///
    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<CommitteeElectionResult, Self::CEError>
    where
        NT: CommitteeElectionSendNode<Self::Message>;

    /// Finalize the protocol and obtain the result
    fn finalize(self) -> Result<Vec<NodeId>, Self::CEError>;
}

pub trait CommitteeElectionSendNode<CE>
where
    CE: SerMsg,
{
    fn send(&self, message: CE, target: NodeId, flush: bool) -> error::Result<()>;

    fn broadcast<I>(&self, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>;
}

pub enum CommitteeElectionResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    Decided,
}
