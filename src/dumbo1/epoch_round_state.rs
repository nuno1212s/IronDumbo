use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult};
use crate::committee_election::{CommitteeElectionProtocol, CommitteeElectionResult};
use crate::consensus_rqs::ConsensusRequest;
use crate::dumbo1::epoch::EpochResult;
use crate::dumbo1::message::DumboMessageTypeDiscriminants;
use crate::dumbo1::network::{SendNodeIBCMWrapperRef, SendNodeWrapperRef};
use crate::dumbo1::node_states::{
    CommitteeNodeExecuting, CommitteeNodeState, NodeState, NonCommitteeNodeExec,
    NonCommitteeNodeState,
};
use crate::dumbo1::pending_messages::PendingMessages;
use crate::dumbo1::protocol::{DumboPSerialization, IndexType};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::{ReliableBroadcast, ReliableBroadcastResult};
use atlas_common::collections::{HashMap, HashSet};
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::{Header, StoredMessage};
use atlas_core::messages::ClientRqInfo;
use atlas_core::ordering_protocol::ShareableConsensusMessage;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

/// Struct of the state of a Dumbo round.
///
/// Holds either the committee election state or the running round state.
/// The running round state can only exist after the committee election is completed.
///
pub(super) enum DumboRoundState<CE, RQ, VR, IR, A> {
    WaitingForCommitteeElection(CommitteeElectionState<CE>),
    Running(RoundStateParts<RQ, VR, IR, A>),
}

impl<CE, RQ, VR, IR, A> DumboRoundState<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
{
    pub fn new(committee_election: CE) -> Self {
        DumboRoundState::WaitingForCommitteeElection(
            CommitteeElectionState::CommitteeElectionRunning(committee_election),
        )
    }

    pub fn new_running(committee: Vec<NodeId>, quorum_info: &QuorumInfo) -> Self {
        DumboRoundState::Running(RoundStateParts::new(committee, quorum_info))
    }

    pub fn get_committee_election(&self) -> Option<&CE> {
        match self {
            DumboRoundState::WaitingForCommitteeElection(
                CommitteeElectionState::CommitteeElectionRunning(committee_election),
            ) => Some(committee_election),
            _ => None,
        }
    }

    pub fn is_waiting_for_committee_election(&self) -> bool {
        matches!(self, DumboRoundState::WaitingForCommitteeElection { .. })
    }

    pub fn is_committee_member(&self, node_id: &NodeId) -> Result<bool, CheckNodeStateError>
    where
        CE: CommitteeElectionProtocol,
    {
        match self {
            DumboRoundState::WaitingForCommitteeElection(_) => {
                Err(CheckNodeStateError::CommitteeNotCompleted)
            }
            DumboRoundState::Running(round_state) => Ok(round_state.is_part_of_committee(node_id)),
        }
    }

    pub fn completed_rbc_count(&self) -> usize
    where
        CE: CommitteeElectionProtocol,
    {
        match self {
            DumboRoundState::WaitingForCommitteeElection(_) => 0,
            DumboRoundState::Running(round_state) => round_state.completed_rbc_count(),
        }
    }

    pub fn is_running_fully(&self) -> bool {
        matches!(self, DumboRoundState::Running { .. })
    }
}

impl<CE, RQ, VR, IR, A> Debug for DumboRoundState<CE, RQ, VR, IR, A>
where
    CE: Debug,
    RQ: Debug,
    VR: Debug,
    IR: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DumboRoundState::WaitingForCommitteeElection(committee_election) => {
                write!(f, "WaitingForCommitteeElection({:?})", committee_election)
            }
            DumboRoundState::Running(round_parts) => {
                write!(f, "Running({:?})", round_parts)
            }
        }
    }
}

pub enum CommitteeElectionState<CE> {
    CommitteeElectionRunning(CE),
    Done,
}

impl<CE> CommitteeElectionState<CE>
where
    CE: CommitteeElectionProtocol,
{
    pub(super) fn process_committee_election_message<NT, RQ, VR, IR, A>(
        &mut self,
        seq_number: SeqNo,
        own_node: NodeId,
        network: &Arc<NT>,
        message_header: &Header,
        ce_msg: &CE::Message,
    ) -> error::Result<CommitteeElectionResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        RQ: SerMsg + ConsensusRequest,
        VR: ReliableBroadcast<RQ>,
        IR: ReliableBroadcast<IndexType>,
        A: ABAProtocol,
    {
        let network = SendNodeWrapperRef::new(seq_number, own_node, network);

        match self {
            CommitteeElectionState::CommitteeElectionRunning(committee_election) => {
                let stored_message = StoredMessage::new(message_header.clone(), ce_msg.clone());

                committee_election
                    .process_message(stored_message, &network)
                    .map_err(|err| err.into())
            }
            CommitteeElectionState::Done => Ok(CommitteeElectionResult::MessageIgnored),
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, CommitteeElectionState::CommitteeElectionRunning(_))
    }

    pub fn is_done(&self) -> bool {
        matches!(self, CommitteeElectionState::Done)
    }
}

impl<CE> Debug for CommitteeElectionState<CE>
where
    CE: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeElectionState::CommitteeElectionRunning(ce) => {
                write!(f, "CommitteeElectionRunning({:?})", ce)
            }
            CommitteeElectionState::Done => write!(f, "Done"),
        }
    }
}

pub struct RoundStateParts<RQ, VR, IR, A> {
    committee: Vec<NodeId>,
    // The state of each node in the protocol. (excluding ourselves)
    node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
    // Cache of client requests for which we have received via ValueRBCs from other nodes.
    received_request_cache: HashSet<ClientRqInfo>,
    waiting_for_values: HashMap<ClientRqInfo, Vec<NodeId>>,
}

impl<RQ, VR, IR, A> RoundStateParts<RQ, VR, IR, A>
where
    VR: ReliableBroadcast<RQ>,
{
    pub fn new(committee: Vec<NodeId>, quorum_info: &QuorumInfo) -> Self {
        let node_states = quorum_info
            .quorum_members()
            .iter()
            .map(|node_id| {
                let node_state = if committee.contains(node_id) {
                    NodeState::CommitteeNode(
                        CommitteeNodeExecuting::<VR, IR, A>::RunningValueRBC(VR::new()),
                        CommitteeNodeState::<RQ>::Empty,
                    )
                } else {
                    NodeState::NonCommitteeNode(
                        NonCommitteeNodeExec::RunningValueRBC(VR::new()),
                        NonCommitteeNodeState::Empty,
                    )
                };
                (node_id.clone(), node_state)
            })
            .collect();

        RoundStateParts {
            committee,
            node_states,
            received_request_cache: HashSet::default(),
            waiting_for_values: HashMap::default(),
        }
    }
}

impl<RQ, VR, IR, A> RoundStateParts<RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
{
    pub(super) fn pop_messages<CE>(
        &self,
        pending_messages: &mut PendingMessages<
            ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        >,
    ) -> Option<ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>>
    where
        CE: CommitteeElectionProtocol,
    {
        let nodes_with_pending_messages = pending_messages
            .nodes_with_pending_messages()
            .collect::<Vec<_>>();

        for node in nodes_with_pending_messages {
            let Some(node_state) = self.node_states.get(&node) else {
                continue;
            };

            let popped_message = match node_state {
                NodeState::CommitteeNode(committee_node_exec, _) => match committee_node_exec {
                    CommitteeNodeExecuting::RunningValueRBC(_) => pending_messages
                        .pop_message_by_type_and_owner(
                            node.clone(),
                            DumboMessageTypeDiscriminants::ReliableBroadcast,
                        ),
                    CommitteeNodeExecuting::WaitingForRBCs => None,
                    CommitteeNodeExecuting::RunningIndexRBC(_) => pending_messages
                        .pop_message_by_type_and_owner(
                            node.clone(),
                            DumboMessageTypeDiscriminants::ReliableBroadcast,
                        ),
                    CommitteeNodeExecuting::WaitingForValues => None,
                    CommitteeNodeExecuting::RunningABA(_) => pending_messages
                        .pop_message_by_type_and_owner(
                            node.clone(),
                            DumboMessageTypeDiscriminants::AsyncBinaryAgreement,
                        ),
                    CommitteeNodeExecuting::Done => {
                        pending_messages.discard_all_messages_by_owner(node.clone());

                        continue;
                    }
                },
                NodeState::NonCommitteeNode(non_committee_node_exec, _) => {
                    match non_committee_node_exec {
                        NonCommitteeNodeExec::RunningValueRBC(_) => pending_messages
                            .pop_message_by_type_and_owner(
                                node.clone(),
                                DumboMessageTypeDiscriminants::ReliableBroadcast,
                            ),
                        NonCommitteeNodeExec::Completed => {
                            pending_messages.discard_all_messages_by_owner(node.clone());

                            continue;
                        }
                    }
                }
            };

            if let Some(message) = popped_message {
                return Some(message);
            }
        }

        None
    }

    pub(super) fn process_value_rbc_message<NT, CE>(
        &mut self,
        seq_no: SeqNo,
        network: &Arc<NT>,
        owner_id: NodeId,
        message_header: &Header,
        rbc_msg: &VR::ReliableBroadcastMessage,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        CE: CommitteeElectionProtocol,
    {
        // Get the state of the corresponding reliable broadcast instance
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        let result = match node_state {
            NodeState::CommitteeNode(CommitteeNodeExecuting::RunningValueRBC(rbc), _)
            | NodeState::NonCommitteeNode(NonCommitteeNodeExec::RunningValueRBC(rbc), _) => {
                let stored_message = StoredMessage::new(message_header.clone(), rbc_msg.clone());

                let network = SendNodeWrapperRef::new(seq_no, owner_id, network);

                rbc.process_message(stored_message, &network)
            }
            // If we are not in the RunningValueRBC state, ignore the message
            // As the node must be in a more forward state (first state is the RunningValueRBC)
            _ => return Ok(EpochResult::MessageIgnored),
        };

        Ok(match result {
            ReliableBroadcastResult::MessageQueued => EpochResult::MessageQueued,
            ReliableBroadcastResult::MessageIgnored => EpochResult::MessageIgnored,
            ReliableBroadcastResult::Processed => EpochResult::MessageProcessed,
            ReliableBroadcastResult::Finalized => {
                match node_state {
                    NodeState::CommitteeNode(committee_node_exec, committee_node_state) => {
                        let value_rbc = std::mem::replace(
                            committee_node_exec,
                            CommitteeNodeExecuting::WaitingForRBCs,
                        );

                        let CommitteeNodeExecuting::RunningValueRBC(rbc) = value_rbc else {
                            unreachable!("Checked above that we are in RunningValueRBC state");
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

                        let NonCommitteeNodeExec::RunningValueRBC(rbc) = value_rbc else {
                            unreachable!("Checked above that we are in RunningValueRBC state");
                        };

                        let completed_rbc = rbc.finalize();

                        completed_rbc
                            .get_client_rq_info()
                            .into_iter()
                            .for_each(|rq_info| {
                                self.received_request_cache.insert(rq_info);
                            });

                        non_committee_node_state.received_value(completed_rbc);
                    }
                }

                EpochResult::MessageProcessed
            }
        })
    }

    pub(super) fn process_index_rbc_message<NT, CE>(
        &mut self,
        seq_no: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        network: &Arc<NT>,
        owner_id: NodeId,
        message_header: &Header,
        rbc_msg: &IR::ReliableBroadcastMessage,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        CE: CommitteeElectionProtocol,
    {
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        let result = match node_state {
            NodeState::CommitteeNode(CommitteeNodeExecuting::RunningIndexRBC(rbc), _) => {
                let stored_message = StoredMessage::new(message_header.clone(), rbc_msg.clone());

                let network = SendNodeIBCMWrapperRef::new(seq_no, owner_id, network);

                rbc.process_message(stored_message, &network)
            }
            NodeState::CommitteeNode(execution_state, _) => {
                return match execution_state {
                    CommitteeNodeExecuting::RunningValueRBC(_)
                    | CommitteeNodeExecuting::WaitingForRBCs => Ok(EpochResult::QueueMessage),
                    _ => Ok(EpochResult::MessageIgnored),
                };
            }
            _ => return Ok(EpochResult::MessageIgnored),
        };

        let indexes = match result {
            ReliableBroadcastResult::MessageQueued => {
                return Ok(EpochResult::MessageQueued.into());
            }
            ReliableBroadcastResult::MessageIgnored => {
                return Ok(EpochResult::MessageIgnored.into());
            }
            ReliableBroadcastResult::Processed => return Ok(EpochResult::MessageProcessed.into()),
            ReliableBroadcastResult::Finalized => match node_state {
                NodeState::CommitteeNode(committee_node_exec, committee_node_state) => {
                    let value_rbc = std::mem::replace(
                        committee_node_exec,
                        CommitteeNodeExecuting::WaitingForValues,
                    );

                    let CommitteeNodeExecuting::RunningIndexRBC(rbc) = value_rbc else {
                        unreachable!("Checked above that we are in RunningValueRBC state");
                    };

                    let index_rbc = rbc.finalize();

                    committee_node_state.received_index(index_rbc.clone());

                    index_rbc
                }
                _ => {
                    unreachable!("Only committee nodes run Index RBC");
                }
            },
        };

        let missing_values = self.check_missing_values(&indexes);

        if missing_values.is_empty() {
            self.prepare_aba_with_input::<_, CE>(
                seq_no,
                quorum_info,
                threshold_keys,
                network,
                owner_id,
                true,
            )?;
        } else {
            missing_values.into_iter().for_each(|rq_info| {
                self.waiting_for_values
                    .entry(rq_info.clone())
                    .or_default()
                    .push(owner_id.clone());
            });
        }

        todo!()
    }

    pub(super) fn process_aba_message<NT, CE>(
        &mut self,
        seq_no: SeqNo,
        network: &Arc<NT>,
        owner_id: NodeId,
        message_header: &Header,
        aba_msg: &A::AsyncBinaryMessage,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        CE: CommitteeElectionProtocol,
    {
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        let result = match node_state {
            NodeState::CommitteeNode(CommitteeNodeExecuting::RunningABA(aba), _) => {
                let stored_message = StoredMessage::new(message_header.clone(), aba_msg.clone());

                let network = SendNodeWrapperRef::new(seq_no, owner_id.clone(), network);

                aba.process_message(stored_message, &network)?
            }
            NodeState::NonCommitteeNode(_, _) => {
                // Non-committee nodes do not have ABA, ignore message
                return Ok(EpochResult::MessageIgnored);
            }
            NodeState::CommitteeNode(committee_node_exec, _) => {
                return match committee_node_exec {
                    CommitteeNodeExecuting::WaitingForValues
                    | CommitteeNodeExecuting::RunningValueRBC(_)
                    | CommitteeNodeExecuting::WaitingForRBCs
                    | CommitteeNodeExecuting::RunningIndexRBC(_) => Ok(EpochResult::QueueMessage),
                    _ => Ok(EpochResult::MessageIgnored),
                };
            }
        };

        self.process_aba_result(owner_id, result)
            .map_err(|err| err.into())
    }

    fn prepare_aba_with_input<NT, CE>(
        &mut self,
        seq_no: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        network: &Arc<NT>,
        completed_node: NodeId,
        input: bool,
    ) -> Result<EpochResult, ABAPreparationError<A::ABAError>>
    where
        CE: CommitteeElectionProtocol,
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let Some(NodeState::CommitteeNode(committee_node_exec, _)) =
            self.node_states.get_mut(&completed_node)
        else {
            return Err(ABAPreparationError::NotPartOfCommittee);
        };

        let network = SendNodeWrapperRef::new(seq_no, completed_node.clone(), network);

        let result = match committee_node_exec {
            CommitteeNodeExecuting::WaitingForValues => {
                // Proceed to ABA
                let mut aba = A::new(quorum_info, threshold_keys);

                let result = aba.provide_input_bit(input, &network)?;

                *committee_node_exec = CommitteeNodeExecuting::RunningABA(aba);

                result
            }
            CommitteeNodeExecuting::RunningABA(aba) => {
                // Already running ABA, just provide the input and return
                aba.provide_input_bit(input, &network)?
            }
            _ => return Err(ABAPreparationError::NotWaitingForValuesOrRunningABA),
        };

        self.process_aba_result(completed_node, result)
            .map_err(|err| err.into())
    }

    fn process_aba_result(
        &mut self,
        owner_id: NodeId,
        result: AsyncBinaryAgreementResult,
    ) -> Result<EpochResult, A::ABAError> {
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        Ok(match result {
            AsyncBinaryAgreementResult::MessageQueued => EpochResult::MessageQueued,
            AsyncBinaryAgreementResult::MessageIgnored => EpochResult::MessageIgnored,
            AsyncBinaryAgreementResult::Processed => EpochResult::MessageProcessed,
            AsyncBinaryAgreementResult::Decided => {
                let NodeState::CommitteeNode(committee_node_exec, committee_node_state) =
                    node_state
                else {
                    unreachable!("Checked above that we are in RunningABA state");
                };

                let CommitteeNodeExecuting::RunningABA(aba) =
                    std::mem::replace(committee_node_exec, CommitteeNodeExecuting::Done)
                else {
                    unreachable!("Checked above that we are in RunningABA state");
                };

                let protocol_result = aba.finalize()?;

                committee_node_state.received_decision(protocol_result);

                if protocol_result {
                    self.send_negative_input_to_all_pending_aba();
                }

                EpochResult::MessageProcessed
            }
        })
    }

    fn send_negative_input_to_all_pending_aba(&mut self) {

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

    pub(super) fn check_missing_values<'a>(&self, index: &'a IndexType) -> Vec<&'a ClientRqInfo> {
        index
            .iter()
            .filter(|rq_info| !self.received_request_cache.contains(rq_info))
            .collect()
    }

    fn is_part_of_committee(&self, node_id: &NodeId) -> bool {
        self.committee.contains(&node_id)
    }

    fn check_nodes_ready(&self, quorum_info: &QuorumInfo) -> bool {
        if !self.is_part_of_committee(&quorum_info.own_node_id()) {
            return false;
        }

        self.completed_rbc_count() >= quorum_info.quorum_size()
    }

    fn check_all_committee_nodes_finished(&self) -> bool {
        let finished_committee_nodes = self.committee.iter()
            .map(|node| self.node_states.get(node))
            .filter_map(|node_state| node_state)
            .filter(|node_state| matches!(node_state, NodeState::CommitteeNode(CommitteeNodeExecuting::Done, CommitteeNodeState::ABA {..})))
            .count();

        finished_committee_nodes == self.committee.len()
    }
}

impl<RQ, VR, IR, A> Debug for RoundStateParts<RQ, VR, IR, A>
where
    RQ: Debug,
    VR: Debug,
    IR: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoundStateParts")
            .field("committee", &self.committee)
            .field("node_states", &self.node_states)
            .finish()
    }
}

#[derive(Debug, Error)]
enum ABAPreparationError<A> {
    #[error("Node is not waiting for values or running ABA")]
    NotWaitingForValuesOrRunningABA,
    #[error("Node is not part of the committee")]
    NotPartOfCommittee,
    #[error("Node is not in the correct state to prepare ABA")]
    ABAError(#[from] A),
}

/// Error when checking if the node is part of the committee
#[derive(Debug, Error)]
pub(super) enum CheckNodeStateError {
    #[error("Committee election protocol not completed yet")]
    CommitteeNotCompleted,
}
