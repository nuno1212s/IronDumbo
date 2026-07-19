use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::error::Error;
use std::fmt::Debug;

/// Consistent Broadcast (Algorithm 6): "similar to RBC, but it does not
/// provide Totality" (paper Section 3). A single round of SEND -> ECHO
/// (signature shares sent back to the owner only) -> the owner combines
/// `2f+1` shares into a self-certifying signature and multicasts FINISH.
/// Any node -- including one that never saw SEND -- can independently
/// verify FINISH and output the value. Used by MVBA (Section 5.1/Fig. 4) to
/// let every node "echo" its own candidate proposal before the
/// leader-permutation agreement loop runs.
///
/// Unlike this crate's PRBC (which wraps a full RBC and augments it with a
/// Done/Finish `f+1`-threshold phase), CBC is a standalone single-round
/// protocol and uses a genuine `2f+1` threshold -- see `ThresholdKeys`'s
/// doc comment for why `f+1` would be unsafe here.
pub trait CBCProtocol<V>: Debug {
    type Message: SerMsg;
    type Error: Error + Send + Sync + 'static;

    fn new(owner_id: NodeId, quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self;

    fn new_with_propose<NT>(
        owner_id: NodeId,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        value: V,
        network: &NT,
    ) -> Self
    where
        NT: CBCSendNode<Self::Message>;

    /// CBC's SEND/ECHO/FINISH are all handled unconditionally by state --
    /// there is never anything to defer, so this always returns `None`.
    /// Kept for interface symmetry with this crate's other sub-protocols;
    /// not currently called anywhere (the only consumer, MVBA, drives
    /// `ConsistentBroadcastInstance` concretely rather than through this
    /// trait generically).
    #[allow(dead_code)]
    fn poll(&mut self) -> Option<StoredMessage<Self::Message>>;

    #[allow(dead_code)]
    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<CBCResult, Self::Error>
    where
        NT: CBCSendNode<Self::Message>;

    /// Returns the delivered value, its digest, and the self-certifying
    /// `2f+1`-threshold signature proving Consistency.
    fn finalize(self) -> Result<(V, Digest, CombinedSignature), Self::Error>;
}

pub enum CBCResult {
    MessageIgnored,
    Processed,
    /// The combined signature was just assembled (or adopted from a peer's
    /// `Finish`). No payload: callers that need the assembled signature
    /// (and the delivered value/digest) call `finalize()`, which returns
    /// all three together.
    Finalized,
}

pub trait CBCSendNode<M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, flush: bool) -> atlas_common::error::Result<()>;

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;
}
