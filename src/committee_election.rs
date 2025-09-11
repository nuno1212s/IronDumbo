use atlas_common::error::Result;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::Header;
use crate::quorum_info::quorum_info::QuorumInfo;

pub trait CommitteeElectionProtocol {

    type Message: SerMsg;

    fn new(quorum_info: QuorumInfo, committee_size: usize) -> Self;

    fn process_message(&mut self, header: Header, message: Self::Message) -> Result<CommitteeElectionResult>;

    fn finalize(self) -> Result<Vec<NodeId>>;
    
}

pub enum CommitteeElectionResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    Decided,
}