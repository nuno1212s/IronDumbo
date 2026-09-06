use atlas_common::crypto::threshold_crypto::{PrivateKeyPart, PublicKeyPart, PublicKeySet};
use atlas_common::node_id::NodeId;
use getset::{CopyGetters, Getters};

#[derive(Debug, Clone, PartialEq, Eq, Getters, CopyGetters)]
pub struct QuorumInfo {
    #[get_copy = "pub"]
    own_node_id: NodeId,
    #[get_copy = "pub"]
    f: usize,
    #[get_copy = "pub"]
    quorum_size: usize,
    #[get = "pub"]
    quorum_members: Vec<NodeId>,
}

impl QuorumInfo {
    pub fn new(n: usize, f: usize, quorum_members: Vec<NodeId>, own_node_id: NodeId) -> Self {
        assert!(n > 0 && f <= (n - 1) / 3, "Invalid quorum parameters");
        let quorum_size = n - f;
        Self {
            own_node_id,
            f,
            quorum_size,
            quorum_members,
        }
    }

    pub fn is_member(&self, node_id: NodeId) -> bool {
        self.quorum_members.contains(&node_id)
    }

    /// The position of `node` within `quorum_members` -- the single source
    /// of truth for which erasure-coded shard/Merkle-leaf slot belongs to
    /// which party. All nodes participating in a given protocol instance
    /// must construct their `QuorumInfo` with an identical `quorum_members`
    /// ordering for this to be consistent across the network.
    pub fn leaf_index_of(&self, node: NodeId) -> Option<usize> {
        self.quorum_members.iter().position(|&m| m == node)
    }
}

/// Represents the keys used in the threshold cryptography for the
/// asynchronous binary agreement, PRBC/Done, and threshold coin-tossing
/// (all against the `f`-threshold scheme, `public_key`/`private_key`), plus
/// a second, independently-generated `2f`-threshold scheme
/// (`cbc_public_key`/`cbc_private_key`) used only by Consistent Broadcast's
/// Echo/Finish combine step (Algorithm 6). CBC genuinely needs `2f+1`
/// shares to combine: reusing the `f`-threshold scheme there would let a
/// Byzantine CBC sender get two conflicting values each combined from
/// `f+1` shares (`2(f+1)-f = f+2 <= 2f+1` is satisfiable for `f>=1`),
/// whereas `2f+1` makes that impossible (`2(2f+1)-f = 3f+2` exceeds the
/// `2f+1` honest-node count for any `f>=1`). A `2f+1`-threshold scheme
/// can't be obtained by "waiting for more shares" against an `f`-threshold
/// keyset -- the secret-sharing polynomial's degree is fixed at
/// generation time -- so this requires a second, independent keyset.
#[derive(Debug, Clone, Getters)]
pub struct ThresholdKeys {
    #[get = "pub"]
    public_key: PublicKeySet,
    #[get = "pub"]
    private_key: PrivateKeyPart,
    #[get = "pub"]
    cbc_public_key: PublicKeySet,
    #[get = "pub"]
    cbc_private_key: PrivateKeyPart,
}

impl ThresholdKeys {
    pub fn new(
        public_key: PublicKeySet,
        private_key: PrivateKeyPart,
        cbc_public_key: PublicKeySet,
        cbc_private_key: PrivateKeyPart,
    ) -> Self {
        Self {
            public_key,
            private_key,
            cbc_public_key,
            cbc_private_key,
        }
    }

    pub fn get_public_key_for_node(&self, node_id: NodeId) -> PublicKeyPart {
        self.public_key.public_key_share(node_id.0 as usize)
    }
}
