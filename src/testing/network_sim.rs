use crate::aba::AsyncBinaryAgreementSendNode;
use crate::cbc::CBCSendNode;
use crate::committee_election::CommitteeElectionSendNode;
use crate::mvba::MVBASendNode;
use crate::prbc::PRBCSendNode;
use crate::rbc::ReliableBroadcastSendNode;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

/// A multi-node synchronous message bus for driving full round-trip protocol
/// simulations in tests: send a message, run the recipient's `process_message`,
/// which may enqueue more messages, repeat until convergence.
pub struct SimulatedNetwork<M: Clone> {
    queues: HashMap<NodeId, VecDeque<(NodeId, M)>>,
}

impl<M: Clone> SimulatedNetwork<M> {
    pub fn new(nodes: &[NodeId]) -> Self {
        let queues = nodes.iter().map(|&node| (node, VecDeque::new())).collect();

        Self { queues }
    }

    pub fn send(&mut self, from: NodeId, to: NodeId, msg: M) {
        self.queues.entry(to).or_default().push_back((from, msg));
    }

    pub fn broadcast(&mut self, from: NodeId, targets: impl IntoIterator<Item = NodeId>, msg: M) {
        for to in targets {
            self.send(from, to, msg.clone());
        }
    }

    pub fn deliver_next(&mut self, to: NodeId) -> Option<(NodeId, M)> {
        self.queues.get_mut(&to)?.pop_front()
    }

    pub fn drain_all_to(&mut self, to: NodeId) -> Vec<(NodeId, M)> {
        self.queues
            .get_mut(&to)
            .map(|queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.queues.values().all(VecDeque::is_empty)
    }

    pub fn pending_for(&self, node: NodeId) -> usize {
        self.queues.get(&node).map(VecDeque::len).unwrap_or(0)
    }
}

/// A single node's handle onto a shared [`SimulatedNetwork`], implementing the
/// crate's `*SendNode` traits so protocol instances can be driven directly
/// against the bus without any production networking code.
pub struct NodeHandle<'a, M: Clone> {
    own_id: NodeId,
    bus: &'a RefCell<SimulatedNetwork<M>>,
}

impl<'a, M: Clone> NodeHandle<'a, M> {
    pub fn new(own_id: NodeId, bus: &'a RefCell<SimulatedNetwork<M>>) -> Self {
        Self { own_id, bus }
    }
}

impl<'a, M> ReliableBroadcastSendNode<M> for NodeHandle<'a, M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, _flush: bool) -> atlas_common::error::Result<()> {
        self.bus.borrow_mut().send(self.own_id, target, message);

        Ok(())
    }

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, targets, message);

        Ok(())
    }
}

impl<'a, M: Clone> AsyncBinaryAgreementSendNode<M> for NodeHandle<'a, M> {
    fn broadcast_message<I>(&self, message: M, target: I) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
        M: SerMsg,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, target, message);

        Ok(())
    }
}

impl<'a, M> PRBCSendNode<M> for NodeHandle<'a, M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, _flush: bool) -> atlas_common::error::Result<()> {
        self.bus.borrow_mut().send(self.own_id, target, message);

        Ok(())
    }

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, targets, message);

        Ok(())
    }
}

impl<'a, M> MVBASendNode<M> for NodeHandle<'a, M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, _flush: bool) -> atlas_common::error::Result<()> {
        self.bus.borrow_mut().send(self.own_id, target, message);

        Ok(())
    }

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, targets, message);

        Ok(())
    }
}

impl<'a, M> CBCSendNode<M> for NodeHandle<'a, M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, _flush: bool) -> atlas_common::error::Result<()> {
        self.bus.borrow_mut().send(self.own_id, target, message);

        Ok(())
    }

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, targets, message);

        Ok(())
    }
}

impl<'a, M> CommitteeElectionSendNode<M> for NodeHandle<'a, M>
where
    M: SerMsg,
{
    fn send(&self, message: M, target: NodeId, _flush: bool) -> atlas_common::error::Result<()> {
        self.bus.borrow_mut().send(self.own_id, target, message);

        Ok(())
    }

    fn broadcast<I>(&self, message: M, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>,
    {
        self.bus
            .borrow_mut()
            .broadcast(self.own_id, targets, message);

        Ok(())
    }
}
