use atlas_common::node_id::NodeId;
use getset::{CopyGetters, Getters};

#[derive(Debug, Clone, PartialEq, Eq, Getters, CopyGetters)]
pub struct QuorumInfo {
    #[get_copy = "pub"]
    f: usize,
    #[get_copy = "pub"]
    quorum_size: usize,
    #[get = "pub"]
    quorum_members: Vec<NodeId>,
}

impl QuorumInfo {
    pub fn new(n: usize, f: usize, quorum_members: Vec<NodeId>) -> Self {
        assert!(n > 0 && f <= (n - 1) / 3, "Invalid quorum parameters");
        let quorum_size = n - f;
        Self {
            f,
            quorum_size,
            quorum_members,
        }
    }

    pub fn is_member(&self, node_id: NodeId) -> bool {
        self.quorum_members.contains(&node_id)
    }
}
