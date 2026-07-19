use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
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
/// phase: given each node's set of PRBC proofs (`MVBAProposal`), agree on
/// ONE node's proposed vector (Section 3's formal contract: "each party
/// proposes a (different) value... the protocol ensures that the decision
/// value was proposed by at least one party").
///
/// Implements the Cachin-Kursawe-Shoup/Abraham-Malkhi-Spiegelman
/// construction the paper cites (Fig. 4/Section 5.1): every node first
/// echoes its own candidate via Consistent Broadcast (`crate::cbc`), then
/// all nodes derive one shared random permutation of candidates (via
/// `crate::threshold_coin_tossing`, the same primitive Committee Election
/// uses) and try them in that order, running a single `AsyncBinaryAgreement`
/// per candidate until one is accepted -- giving an expected *constant*
/// number of ABA instances (matching the paper's "three consecutive
/// instances of ABA"), not one per quorum member.
pub trait MVBAProtocol: Debug {
    type Message: SerMsg;
    type Error: Error + Send + Sync + 'static;

    /// `round` scopes the coin-toss id: the permutation must vary across
    /// rounds of the same cluster/keyset or it becomes predictable after
    /// the first round, defeating the unpredictability the expected-O(1)
    /// termination bound relies on (the same reasoning that requires
    /// Committee Election's `round` parameter).
    fn new(quorum_info: QuorumInfo, threshold_keys: ThresholdKeys, round: SeqNo) -> Self;

    /// Called once this node has gathered its own candidate set of PRBC
    /// proofs (typically once it has `n-f` of them). Echoes it via CBC to
    /// kick off the agreement loop.
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
