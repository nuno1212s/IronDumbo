use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::error::Error;
use std::fmt::Debug;

/// A candidate entry contributed to MVBA: a PRBC proof (owner, digest of the
/// value it broadcast, and the combined threshold signature proving `f+1`
/// correct nodes received it).
///
/// NOTE: the adopted plan sketched `Vec<(NodeId, ThresholdSignature)>`, but a
/// combined signature alone cannot be verified without knowing which digest
/// it was produced over -- each owner broadcasts a different value/digest.
/// The digest is added here so `MVBAProtocol::propose` can actually run the
/// validity predicate it's specified to run.
pub type MVBAProposal = Vec<(NodeId, Digest, CombinedSignature)>;

/// A Multi-Valued Byzantine Agreement protocol, as used by Dumbo2's second
/// phase: given each node's set of PRBC proofs (`MVBAProposal`), agree on a
/// common subset containing at least `n-f` entries, each independently
/// verified.
///
/// This implementation runs one [`crate::aba::ABAProtocol`] instance per
/// quorum member `owner`, each deciding "is `owner`'s PRBC entry included in
/// the agreed set?". A node votes `true` for `owner` if it holds a valid PRBC
/// proof for `owner` at the time [`MVBAProtocol::propose`] is called, `false`
/// otherwise -- matching Dumbo2's algorithm 3.
pub trait MVBAProtocol: Debug {
    type Message: SerMsg;
    type Error: Error + Send + Sync + 'static;

    fn new(quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self;

    /// Called once this node has gathered its own candidate set of PRBC
    /// proofs (typically once it has `n-f` of them). Kicks off one ABA vote
    /// per quorum member.
    fn propose<NT>(
        &mut self,
        proposal: MVBAProposal,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>;

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>;

    /// Finalize the protocol and obtain the agreed-upon subset.
    fn finalize(self) -> Result<MVBAProposal, Self::Error>;
}

pub enum MVBAResult {
    MessageQueued,
    MessageIgnored,
    Processed,
    /// Every per-owner ABA has decided; the agreed subset is ready via `finalize`.
    Decided,
}

pub trait MVBASendNode<M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, flush: bool) -> atlas_common::error::Result<()>;

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>;
}
