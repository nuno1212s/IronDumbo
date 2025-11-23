use atlas_common::maybe_vec::MaybeVec;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::messages::{ClientRqInfo, SessionBased};

/// The trait representing the information we need for dumbo consensus requests
pub trait ConsensusRequest {
    /// Get the client request info associated with this consensus request
    fn get_client_rq_info(&self) -> MaybeVec<ClientRqInfo>;
}

impl<RQ> ConsensusRequest for Vec<StoredMessage<RQ>>
where
    RQ: SerMsg + SessionBased,
{
    fn get_client_rq_info(&self) -> MaybeVec<ClientRqInfo> {
        self.iter()
            .map(|message| ClientRqInfo::from(message))
            .collect()
    }
}
