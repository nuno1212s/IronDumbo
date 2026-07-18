use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::error::Error;
use std::fmt::Debug;

/// A Provable Reliable Broadcast protocol: a reliable broadcast (as in
/// [`crate::rbc::ReliableBroadcast`]) that additionally produces a
/// combined-threshold-signature proof, verifiable by anyone holding the
/// quorum's public key, that at least `f+1` correct nodes received the
/// broadcast value. Used by Dumbo2 to build the `W` set handed to MVBA.
pub trait PRBCProtocol<RQ>: Debug {
    type Message: SerMsg;
    type Error: Error + Send + Sync + 'static;

    fn new(owner_id: NodeId, quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self;

    fn new_with_propose<NT>(
        owner_id: NodeId,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        value: RQ,
        network: &NT,
    ) -> Self
    where
        NT: PRBCSendNode<Self::Message>;

    fn poll(&mut self) -> Option<StoredMessage<Self::Message>>;

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<PRBCResult, Self::Error>
    where
        NT: PRBCSendNode<Self::Message>;

    /// Finalize the protocol and obtain the proposed value, its digest, and
    /// the combined threshold-signature proof of delivery.
    fn finalize(self) -> Result<(RQ, Digest, CombinedSignature), Self::Error>;
}

pub enum PRBCResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    /// The combined-signature proof was just assembled (or adopted from a
    /// peer's `Finish` message). The protocol is ready to be finalized.
    Finalized(CombinedSignature),
}

pub trait PRBCSendNode<M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, flush: bool) -> atlas_common::error::Result<()>;

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;
}
