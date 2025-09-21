use crate::aba::ABAProtocol;
use crate::committee_election::CommitteeElectionProtocol;
use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::ReliableBroadcast;
use atlas_common::collections::HashMap;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use std::fmt::Debug;
use std::sync::Arc;
use atlas_common::serialization_helper::SerMsg;
use atlas_common::error::Result;
use atlas_communication::message::StoredMessage;
use atlas_core::ordering_protocol::ShareableConsensusMessage;
use crate::dumbo1::message::DumboMessageType;
use crate::dumbo1::protocol::{DumboPMessage, DumboPSerialization};

pub(super) struct DumboRound<CE, RQ, R, A> {
    // The current epoch number.
    epoch_num: SeqNo,
    // The state of each node in the protocol.
    node_states: HashMap<NodeId, NodeState<RQ, R, A>>,
    // The state of the committee election protocol.
    committee_election: CommitteeState<CE>,
}

impl<CE, RQ, R, A> DumboRound<CE, RQ, R, A>
where
    RQ: SerMsg,
    R: ReliableBroadcast<RQ>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    pub fn new(epoch_num: SeqNo, quorum_info: QuorumInfo) -> Self {
        let required_committee = quorum_info.f() + 1;

        let committee_election_protocol = CE::new(quorum_info, required_committee);

        Self {
            epoch_num,
            node_states: HashMap::default(),
            committee_election: CommitteeState::RunningCE(committee_election_protocol),
        }
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableConsensusMessage<RQ, DumboPSerialization<RQ, R, A, CE>>,
        network: &Arc<NT>
    ) -> Result<EpochResult>{
        match message.message().message_type() {
            DumboMessageType::ReliableBroadcast(rbc_msg) => {
                let node_state = self.node_states.get_mut(&message.header().from());

                if let Some(node_state) = node_state {
                    match node_state {
                        NodeState::RunningRBC(rbc) => {
                            let stored_message = StoredMessage::new(
                                message.header().clone(),
                                rbc_msg.clone(),
                            );

                            todo!()
                        }
                        NodeState::RunningABA { .. } => {
                            todo!()
                        }
                        NodeState::Completed { .. } => {
                            todo!()
                        }
                    }
                } else {
                    todo!()
                }
            }
            DumboMessageType::AsyncBinaryAgreement(aba_msg) => {
                todo!()
            }
            DumboMessageType::CommitteeElectionMessage(ce_msg) => {
                todo!()
            }
        }

    }

    fn completed_node_count(&self) -> usize {
        self.node_states
            .iter()
            .filter(|(_, state)| matches!(state, NodeState::Completed { .. }))
            .count()
    }

    fn completed_rbc_count(&self) -> usize {
        self.node_states
            .iter()
            .filter(|(_, state)| {
                matches!(
                    state,
                    NodeState::RunningABA { .. } | NodeState::Completed { .. }
                )
            })
            .count()
    }
}

impl<CE, RQ, R, A> Debug for DumboRound<CE, RQ, R, A>
where
    CE: Debug,
    R: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DumboRound")
            .field("epoch_num", &self.epoch_num)
            .field("node_states", &self.node_states)
            .field("committee_election", &self.committee_election)
            .finish()
    }
}

/// The current state of the committee election protocol.
enum CommitteeState<CE> {
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

/// The state of a node in the Dumbo protocol.
enum NodeState<RQ, R, A> {
    RunningRBC(R),
    RunningABA { completed_rbc: RQ, aba: A },
    Completed { completed_rbc: RQ, value: bool },
}

impl<RQ, R, A> NodeState<RQ, R, A>
where
    R: ReliableBroadcast<RQ>,
    A: ABAProtocol,
{

}

impl<RQ, R, A> Debug for NodeState<RQ, R, A>
where
    R: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeState::RunningRBC(rbc) => write!(f, "RunningRBC({:?})", rbc),
            NodeState::RunningABA {
                completed_rbc: _,
                aba,
            } => write!(f, "RunningABA({:?})", aba),
            NodeState::Completed {
                completed_rbc: _,
                value,
            } => write!(f, "Completed({})", value),
        }
    }
}

pub(super) enum EpochResult {
    MessageIgnored,
    MessageQueued,
    MessageProcessed,
    Finalized,
}