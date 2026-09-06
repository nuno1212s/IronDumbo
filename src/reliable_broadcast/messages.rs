use crate::reliable_broadcast::merkle::MerkleBranch;
use atlas_common::crypto::hash::Digest;
use serde::{Deserialize, Serialize};

/// One erasure-coded shard plus its Merkle inclusion proof against a
/// claimed root. Carries no leaf-index/leaf-count field: the recipient
/// derives its shard index from the authenticated sender identity via
/// `QuorumInfo::leaf_index_of`, never trusting it from the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ErasureCodedPart {
    pub(crate) root: Digest,
    pub(crate) branch: MerkleBranch,
    pub(crate) shard: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReliableBroadcastMessage {
    /// Algorithm 5 line 4: a per-recipient unicast carrying that
    /// recipient's own shard + branch (renamed from `Send` to match the
    /// paper's terminology and avoid confusion with the `Send`/`Sync`
    /// auto traits).
    Val(ErasureCodedPart),
    /// Algorithm 5 line 6.
    Echo(ErasureCodedPart),
    /// Algorithm 5 lines 12/14.
    Ready(Digest),
}
