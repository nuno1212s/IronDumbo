use atlas_common::crypto::threshold_crypto::{
    PrivateKeyPart, PrivateKeySet, PublicKeyPart, PublicKeySet,
};
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
}

/// The keyset that 
pub struct ThresholdKeySet {
    private_key_set: PrivateKeySet,
}

impl ThresholdKeySet {
    pub fn make_key_set_for(quorum_info: QuorumInfo) -> Self {
        let threshold = quorum_info.f + 1;
        let private_key_set = PrivateKeySet::gen_random(threshold);
        Self { private_key_set }
    }

    pub fn initialize_keys_for_node(&self, node_id: NodeId) -> ThresholdKeys {
        ThresholdKeys {
            public_key: self.private_key_set.public_key_set(),
            private_key: self.private_key_set.private_key_part(node_id.0 as usize),
        }
    }
}

/// Represents the keys used in the threshold cryptography for the asynchronous binary agreement.
#[derive(Debug, Clone, Getters)]
pub struct ThresholdKeys {
    #[get = "pub"]
    public_key: PublicKeySet,
    #[get = "pub"]
    private_key: PrivateKeyPart,
}

impl ThresholdKeys {

    pub fn new(public_key: PublicKeySet, private_key: PrivateKeyPart) -> Self {
        Self {
            public_key,
            private_key,
        }
    }

    pub fn get_public_key_for_node(&self, node_id: NodeId) -> PublicKeyPart {
        self.public_key.public_key_share(node_id.0 as usize)
    }
}
