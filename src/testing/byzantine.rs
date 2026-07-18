use atlas_common::node_id::NodeId;

/// Behaviors a faulty node can exhibit in a simulated cluster.
#[derive(Debug, Clone)]
pub enum ByzantineBehavior {
    /// Never sends any message.
    Silent,
    /// Sends different values to different targets.
    Equivocate { values: Vec<Vec<u8>> },
}

#[derive(Debug, Clone)]
pub struct ByzantineNode {
    pub id: NodeId,
    pub behavior: ByzantineBehavior,
}

impl ByzantineNode {
    pub fn new(id: NodeId, behavior: ByzantineBehavior) -> Self {
        Self { id, behavior }
    }

    pub fn is_silent(&self) -> bool {
        matches!(self.behavior, ByzantineBehavior::Silent)
    }
}
