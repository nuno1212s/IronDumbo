use crate::aba::ABAProtocol;
use crate::dumbo1::protocol::IndexType;
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
            CommitteeState::RunningCE(ce) => write!(f, "RunningCE({ce:?})"),
            CommitteeState::Completed { committee } => write!(f, "Completed({committee:?})"),
        }
    }
}

/// Our POV of the state of a given node in the dumbo protocol
///
/// Committee nodes participate in both Value and Index RBC as well as having ABA protocol
/// Non Committee nodes only partake in the ValueRBC
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
            NodeState::CommitteeNode(state, ..) => write!(f, "CommitteeNode({state:?})"),
            NodeState::NonCommitteeNode(state, ..) => write!(f, "NonCommitteeNode({state:?})"),
        }
    }
}

/// The state of a committee node in the Dumbo protocol.
pub(super) enum CommitteeNodeExecuting<VR, IR, A> {
    RunningValueRBC(VR),
    WaitingForRBCs,
    RunningIndexRBC(IR),
    WaitingForValues,
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
            CommitteeNodeExecuting::RunningValueRBC(rbc) => write!(f, "RunningRBC({rbc:?})"),
            CommitteeNodeExecuting::WaitingForRBCs => write!(f, "WaitingForRBCs"),
            CommitteeNodeExecuting::RunningIndexRBC(rbc) => write!(f, "RunningIndexRBC({rbc:?})"),
            CommitteeNodeExecuting::WaitingForValues => write!(f, "WaitingForValues"),
            CommitteeNodeExecuting::RunningABA(aba) => write!(f, "RunningABA({aba:?})"),
            CommitteeNodeExecuting::Done => write!(f, "Done"),
        }
    }
}

pub(super) enum CommitteeNodeState<RQ> {
    Empty(Option<bool>),
    ValueRBC {
        value: RQ,
        pending_input: Option<bool>,
    },
    IndexRBC {
        value: RQ,
        index: IndexType,
        pending_input: Option<bool>,
    },
    ABA {
        value: RQ,
        index: IndexType,
        decision: bool,
    },
}

impl<RQ> CommitteeNodeState<RQ> {
    pub(super) fn received_value(&mut self, value: RQ) {
        *self = CommitteeNodeState::ValueRBC {
            value,
            pending_input: None,
        };
    }

    pub(super) fn received_index(&mut self, index: IndexType) {
        if let CommitteeNodeState::ValueRBC {
            value,
            pending_input,
        } = std::mem::replace(self, CommitteeNodeState::Empty(None))
        {
            *self = CommitteeNodeState::IndexRBC {
                value,
                index,
                pending_input,
            };
        } else {
            panic!("Invalid state transition: expected ValueRBC state");
        }
    }

    pub(super) fn received_decision(&mut self, decision: bool) {
        if let CommitteeNodeState::IndexRBC { value, index, .. } =
            std::mem::replace(self, CommitteeNodeState::Empty(None))
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

    pub(super) fn stored_pending_vote(&mut self, vote: bool) {
        match self {
            CommitteeNodeState::Empty(pending) => pending.insert(vote),
            CommitteeNodeState::ValueRBC { pending_input, .. }
            | CommitteeNodeState::IndexRBC { pending_input, .. } => pending_input.insert(vote),
            CommitteeNodeState::ABA { .. } => {
                unreachable!("Invalid state transition: expected ValueRBC state")
            }
        };
    }
}

impl<RQ> Debug for CommitteeNodeState<RQ> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeNodeState::Empty(pending) => write!(f, "Empty {pending:?}"),
            CommitteeNodeState::ValueRBC { pending_input, .. } => {
                write!(f, "ValueRBC {pending_input:?}")
            }
            CommitteeNodeState::IndexRBC {
                index,
                pending_input,
                ..
            } => {
                write!(
                    f,
                    "IndexRBC(index: {index:?}, pending_input: {pending_input:?})"
                )
            }
            CommitteeNodeState::ABA {
                index, decision, ..
            } => {
                write!(f, "ABA(index: {index:?}, decision: {decision:?})")
            }
        }
    }
}

impl<RQ> Default for CommitteeNodeState<RQ> {
    fn default() -> Self {
        CommitteeNodeState::Empty(None)
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
            NonCommitteeNodeExec::RunningValueRBC(rbc) => write!(f, "RunningRBC({rbc:?})"),
            NonCommitteeNodeExec::Completed => {
                write!(f, "Completed")
            }
        }
    }
}

#[derive(Default)]
pub(super) enum NonCommitteeNodeState<RQ> {
    #[default]
    Empty,
    ValueRBC {
        value: RQ,
    },
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
