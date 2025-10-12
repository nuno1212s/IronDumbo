use crate::aba::ABAProtocol;
use crate::dumbo1::protocol::IndexType;
use crate::rbc::ReliableBroadcast;
use atlas_common::node_id::NodeId;
use std::fmt::Debug;

/// The current state of the committee election protocol.
pub(super) enum CommitteeState<CE> {
    RunningCE(CE),
    Completed { committee: Vec<NodeId> },
}

impl<CE> Debug for CommitteeState<CE>
where
    CE: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeState::RunningCE(ce) => write!(f, "RunningCE({:?})", ce),
            CommitteeState::Completed { committee } => write!(f, "Completed({:?})", committee),
        }
    }
}

/// Our local state in the Dumbo protocol.
pub(super) enum LocalDumboState<RQ, VR, IR, A> {
    WaitingForCommittee,
    CommitteeMember(CommitteeLocalState<RQ, VR, IR, A>),
    NonCommitteeMember(NonCommitteeLocalState<RQ, VR>),
}

impl<RQ, VR, IR, A> Default for LocalDumboState<RQ, VR, IR, A> {
    fn default() -> Self {
        Self::WaitingForCommittee
    }
}

/// The local state of a committee member in the Dumbo protocol.
/// Tracks the execution of the RBCs and ABA protocols that are
pub(super) enum CommitteeLocalState<RQ, VR, IR, A> {
    RunningValueRBC { rbc: VR },
    CompletedValueRBC { value_rbc: RQ },
    RunningIndexRBC { completed_value_rbc: RQ, rbc: IR },
    RunningABA { aba: A },
}

/// The local state of a non-committee member in the Dumbo protocol.
/// Does not host an index RBC nor the ABA protocol.
pub(super) enum NonCommitteeLocalState<RQ, R> {
    RunningRBC { rbc: R },
    Completed { completed_rbc: RQ },
}

/// The state of a node in the Dumbo protocol, distinguishing between committee and non-committee nodes.
///
/// Committee nodes participate in both Value and Index RBC as well as having ABA protocol
pub(super) enum NodeState<RQ, VR, IR, A> {
    CommitteeNode(CommitteeNodeExecuting<VR, IR, A>, CommitteeNodeState<RQ>),
    NonCommitteeNode(NonCommitteeNodeExec<VR>, NonCommitteeNodeState<RQ>),
}

impl<RQ, VR, IR, A> NodeState<RQ, VR, IR, A> where A: ABAProtocol {}

impl<RQ, VR, IR, A> Debug for NodeState<RQ, VR, IR, A>
where
    VR: Debug,
    IR: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeState::CommitteeNode(state, ..) => write!(f, "CommitteeNode({:?})", state),
            NodeState::NonCommitteeNode(state, ..) => write!(f, "NonCommitteeNode({:?})", state),
        }
    }
}

/// The state of a committee node in the Dumbo protocol.
pub(super) enum CommitteeNodeExecuting<VR, IR, A> {
    None,
    RunningValueRBC(VR),
    WaitingForRBCs,
    RunningIndexRBC(IR),
    RunningABA(A),
    Done,
}

impl<VR, IR, A> Debug for CommitteeNodeExecuting<VR, IR, A>
where
    VR: Debug,
    IR: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeNodeExecuting::None => write!(f, "None"),
            CommitteeNodeExecuting::RunningValueRBC(rbc) => write!(f, "RunningRBC({:?})", rbc),
            CommitteeNodeExecuting::WaitingForRBCs => write!(f, "WaitingForRBCs"),
            CommitteeNodeExecuting::RunningIndexRBC(rbc) => write!(f, "RunningIndexRBC({:?})", rbc),
            CommitteeNodeExecuting::RunningABA(aba) => write!(f, "RunningABA({:?})", aba),
            CommitteeNodeExecuting::Done => write!(f, "Done")
        }
    }
}

pub(super) enum CommitteeNodeState<RQ> {
    Empty,
    ValueRBC {
        value: RQ,
    },
    IndexRBC {
        value: RQ,
        index: IndexType,
    },
    ABA {
        value: RQ,
        index: IndexType,
        decision: bool,
    },
}

impl<RQ> CommitteeNodeState<RQ> {
    pub(super) fn received_value(&mut self, value: RQ) {
        *self = CommitteeNodeState::ValueRBC { value };
    }

    pub(super) fn received_index(&mut self, index: IndexType) {
        if let CommitteeNodeState::ValueRBC { value } =
            std::mem::replace(self, CommitteeNodeState::Empty)
        {
            *self = CommitteeNodeState::IndexRBC { value, index };
        } else {
            panic!("Invalid state transition: expected ValueRBC state");
        }
    }

    pub(super) fn received_decision(&mut self, decision: bool) {
        if let CommitteeNodeState::IndexRBC { value, index } =
            std::mem::replace(self, CommitteeNodeState::Empty)
        {
            *self = CommitteeNodeState::ABA {
                value,
                index,
                decision,
            };
        } else {
            panic!("Invalid state transition: expected IndexRBC state");
        }
    }
}

impl<RQ> Debug for CommitteeNodeState<RQ> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeNodeState::Empty => write!(f, "Empty"),
            CommitteeNodeState::ValueRBC { .. } => write!(f, "ValueRBC"),
            CommitteeNodeState::IndexRBC { index, .. } => {
                write!(f, "IndexRBC(index: {:?})", index)
            }
            CommitteeNodeState::ABA {
                index, decision, ..
            } => {
                write!(f, "ABA(index: {:?}, decision: {:?})", index, decision)
            }
        }
    }
}

/// The state of a non-committee node in the Dumbo protocol.
pub(super) enum NonCommitteeNodeExec<R> {
    RunningValueRBC(R),
    Completed,
}

impl<R> Debug for NonCommitteeNodeExec<R>
where
    R: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonCommitteeNodeExec::RunningValueRBC(rbc) => write!(f, "RunningRBC({:?})", rbc),
            NonCommitteeNodeExec::Completed => {
                write!(f, "Completed")
            }
        }
    }
}

pub(super) enum NonCommitteeNodeState<RQ> {
    Empty,
    ValueRBC { value: RQ },
}

impl<RQ> NonCommitteeNodeState<RQ> {
    pub(super) fn received_value(&mut self, value: RQ) {
        *self = NonCommitteeNodeState::ValueRBC { value };
    }
}

impl<RQ> Debug for NonCommitteeNodeState<RQ> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonCommitteeNodeState::Empty => write!(f, "Empty"),
            NonCommitteeNodeState::ValueRBC { .. } => write!(f, "ValueRBC"),
        }
    }
}
