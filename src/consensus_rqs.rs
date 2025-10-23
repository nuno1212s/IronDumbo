use atlas_common::maybe_vec::MaybeVec;
use atlas_core::messages::ClientRqInfo;

/// The trait representing the information we need for dumbo consensus requests
pub trait ConsensusRequest {

    /// Get the client request info associated with this consensus request
    fn get_client_rq_info(&self) -> MaybeVec<ClientRqInfo>;

}