use crate::aba::ABAProtocol;
use crate::rbc::ReliableBroadcast;
use atlas_common::collections::HashMap;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use std::marker::PhantomData;

/// An instance of the Dumbo protocol.
/// Holds the state of the protocol for a specific epoch.
/// Tracks the state of each node in the protocol.
pub struct Dumbo<RQ, R, A> {
    // The current epoch number.
    epoch_num: SeqNo,
    // The state of each node in the protocol.
    node_states: HashMap<NodeId, NodeState<RQ, R, A>>,

    phantom_rq: PhantomData<fn(RQ) -> RQ>,
}

/// The state of a node in the Dumbo protocol.
enum NodeState<RQ, R, A> {
    RunningRBC(R),
    RunningABA { completed_rbc: RQ, aba: A },
    Completed { decided_value: RQ, value: bool },
}

impl<RQ, R, A> NodeState<RQ, R, A>
where
    R: ReliableBroadcast<RQ>,
    A: ABAProtocol,
{
}
