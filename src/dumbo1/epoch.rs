use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult};
use crate::committee_election::{CommitteeElectionProtocol, CommitteeElectionResult};
use crate::dumbo1::message::DumboMessageType;
use crate::dumbo1::network::SendNodeWrapperRef;
use crate::dumbo1::node_states::{
    CommitteeNodeExecuting, CommitteeNodeState, CommitteeState, LocalDumboState, NodeState,
    NonCommitteeNodeExec, NonCommitteeNodeState,
};
use crate::dumbo1::protocol::{DumboPSerialization, IndexType};
use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::{ReliableBroadcast, ReliableBroadcastResult};
use atlas_common::collections::HashMap;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::ordering_protocol::ShareableConsensusMessage;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

pub(super) struct DumboRound<CE, RQ, VR, IR, A> {
    // The current epoch number.
    epoch_num: SeqNo,
    // Our own node ID.
    node_id: NodeId,
    // The state of each node in the protocol. (excluding ourselves)
    node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
    // Our local state in the protocol.
    local_state: LocalDumboState<RQ, VR, IR, A>,
    // The state of the committee election protocol.
    committee_election: CommitteeState<CE>,
    // The information about the quorum.
    quorum_info: QuorumInfo,
}

impl<CE, RQ, VR, IR, A> DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    pub fn new(epoch_num: SeqNo, node_id: NodeId, quorum_info: QuorumInfo) -> Self {
        let required_committee = quorum_info.f() + 1;

        let committee_election_protocol = CE::new(quorum_info.clone(), required_committee);

        Self {
            epoch_num,
            node_id,
            node_states: HashMap::default(),
            local_state: LocalDumboState::WaitingForCommittee,
            committee_election: CommitteeState::RunningCE(committee_election_protocol),
            quorum_info,
        }
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        network: &Arc<NT>,
    ) -> atlas_common::error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        match message.message().message_type() {
            DumboMessageType::CommitteeElectionMessage(ce_msg) => {
                let network =
                    SendNodeWrapperRef::new(self.epoch_num.clone(), self.node_id, network);

                let committee_result = match &mut self.committee_election {
                    CommitteeState::RunningCE(committee_election) => {
                        let stored_message =
                            StoredMessage::new(message.header().clone(), ce_msg.clone());

                        committee_election.process_message(stored_message, &network)?
                    }
                    CommitteeState::Completed { .. } => return Ok(EpochResult::MessageIgnored),
                };

                match committee_result {
                    CommitteeElectionResult::MessageQueued => Ok(EpochResult::MessageQueued),
                    CommitteeElectionResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
                    CommitteeElectionResult::Processed => Ok(EpochResult::MessageProcessed),
                    CommitteeElectionResult::Decided => {
                        let CommitteeState::RunningCE(ce) = &mut self.committee_election else {
                            unreachable!("Checked above that we are in RunningCE state");
                        };

                        let current_ce = std::mem::replace(
                            ce,
                            CE::new(self.quorum_info.clone(), self.quorum_info.f() + 1),
                        );

                        let committee = current_ce.finalize()?;

                        self.committee_election = CommitteeState::Completed { committee };

                        Ok(EpochResult::MessageProcessed)
                    }
                }
            }
            DumboMessageType::ReliableBroadcast(owner, rbc_msg) => {
                // Get the state of the corresponding reliable broadcast instance
                let node_state = self.node_states.get_mut(owner);

                if node_state.is_none() {
                    return Ok(EpochResult::MessageIgnored)
                };

                let node_state = node_state.unwrap();

                let result = match node_state {
                    NodeState::CommitteeNode(
                        CommitteeNodeExecuting::RunningValueRBC(rbc),
                        _,
                    )
                    | NodeState::NonCommitteeNode(
                        NonCommitteeNodeExec::RunningValueRBC(rbc),
                        _,
                    ) => {
                        let stored_message =
                            StoredMessage::new(message.header().clone(), rbc_msg.clone());

                        let network = SendNodeWrapperRef::new(
                            self.epoch_num.clone(),
                            owner.clone(),
                            network,
                        );

                        rbc.process_message(stored_message, &network)
                    }
                    _ => return Ok(EpochResult::MessageIgnored),
                };

                match result {
                    ReliableBroadcastResult::MessageQueued => Ok(EpochResult::MessageQueued),
                    ReliableBroadcastResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
                    ReliableBroadcastResult::Processed => Ok(EpochResult::MessageProcessed),
                    ReliableBroadcastResult::Finalized => {
                        match node_state {
                            NodeState::CommitteeNode(
                                committee_node_exec,
                                committee_node_state,
                            ) => {
                                let value_rbc = std::mem::replace(
                                    committee_node_exec,
                                    CommitteeNodeExecuting::WaitingForRBCs,
                                );

                                let CommitteeNodeExecuting::RunningValueRBC(rbc) = value_rbc
                                else {
                                    unreachable!(
                                        "Checked above that we are in RunningValueRBC state"
                                    );
                                };

                                let value_rbc = rbc.finalize();

                                committee_node_state.received_value(value_rbc);
                            }
                            NodeState::NonCommitteeNode(
                                non_committee_node_exec,
                                non_committee_node_state,
                            ) => {
                                let value_rbc = std::mem::replace(
                                    non_committee_node_exec,
                                    NonCommitteeNodeExec::Completed,
                                );

                                let NonCommitteeNodeExec::RunningValueRBC(rbc) = value_rbc
                                else {
                                    unreachable!(
                                        "Checked above that we are in RunningValueRBC state"
                                    );
                                };

                                let completed_rbc = rbc.finalize();

                                non_committee_node_state.received_value(completed_rbc);
                            }
                        }
                        Ok(EpochResult::MessageProcessed)
                    }
                }
            }
            DumboMessageType::IndexReliableBroadcast(owner_id, rbc_msg) => {
                todo!()
            }
            DumboMessageType::AsyncBinaryAgreement(owner_id, aba_msg) => {
                let node_state = self.node_states.get_mut(&owner_id);

                if node_state.is_none() {
                    return Ok(EpochResult::MessageIgnored)
                };

                let node_state = node_state.unwrap();

                let result = match node_state {
                    NodeState::CommitteeNode(committee_node, _) => match committee_node {
                        CommitteeNodeExecuting::RunningABA(aba) => {
                            let stored_message =
                                StoredMessage::new(message.header().clone(), aba_msg.clone());

                            let network = SendNodeWrapperRef::new(
                                self.epoch_num.clone(),
                                owner_id.clone(),
                                network,
                            );

                            aba.process_message(stored_message, &network)?
                        }
                        CommitteeNodeExecuting::Done => return Ok(EpochResult::MessageIgnored),
                        _ => {
                            todo!();
                            return Ok(EpochResult::MessageQueued);
                        }
                    },
                    NodeState::NonCommitteeNode(_, _) => {
                        // Non-committee nodes do not have ABA, ignore message
                        return Ok(EpochResult::MessageIgnored);
                    }
                };

                match result {
                    AsyncBinaryAgreementResult::MessageQueued => Ok(EpochResult::MessageQueued),
                    AsyncBinaryAgreementResult::MessageIgnored => {
                        Ok(EpochResult::MessageIgnored)
                    }
                    AsyncBinaryAgreementResult::Processed => Ok(EpochResult::MessageProcessed),
                    AsyncBinaryAgreementResult::Decided => {
                        let NodeState::CommitteeNode(committee_node_exec, committee_node_state) =
                            node_state
                        else {
                            unreachable!("Checked above that we are in RunningABA state");
                        };

                        let CommitteeNodeExecuting::RunningABA(aba) = std::mem::replace(
                            committee_node_exec,
                            CommitteeNodeExecuting::Done,
                        ) else {
                            unreachable!("Checked above that we are in RunningABA state");
                        };

                        let protocol_result = aba.finalize()?;

                        committee_node_state.received_decision(protocol_result);

                        Ok(EpochResult::MessageProcessed)
                    }
                }
            }
        }
    }

    fn prepare_aba(&mut self) {}

    fn is_part_of_committee(&self) -> Result<bool, CheckNodeStateError> {
        if let CommitteeState::Completed { committee } = &self.committee_election {
            Ok(committee.contains(&self.node_id))
        } else {
            Err(CheckNodeStateError::CommitteeNotCompleted)
        }
    }

    fn check_nodes_ready(&mut self) -> Result<bool, CheckNodeStateError> {
        if !self.is_part_of_committee()? {
            return Ok(false);
        }

        Ok(self.completed_rbc_count() >= self.quorum_info.quorum_size())
    }

    fn completed_rbc_count(&self) -> usize {
        self.node_states
            .iter()
            .filter(|(_, state)| match state {
                NodeState::CommitteeNode(_, committee_node_state) => {
                    !matches!(committee_node_state, CommitteeNodeState::Empty)
                }
                NodeState::NonCommitteeNode(_, non_committee_node_state) => {
                    matches!(
                        non_committee_node_state,
                        NonCommitteeNodeState::ValueRBC { .. }
                    )
                }
            })
            .count()
    }
}

impl<CE, RQ, VR, IR, A> Debug for DumboRound<CE, RQ, VR, IR, A>
where
    CE: Debug,
    VR: Debug,
    IR: Debug,
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

pub(super) enum EpochResult {
    MessageIgnored,
    MessageQueued,
    MessageProcessed,
    Finalized,
}

/// Error when checking if the node is part of the committee
#[derive(Debug, Error)]
enum CheckNodeStateError {
    #[error("Committee election protocol not completed yet")]
    CommitteeNotCompleted,
}
