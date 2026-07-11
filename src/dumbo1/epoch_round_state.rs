use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};
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
use crate::dumbo1::protocol::{DumboPSerialization, DumboRQ, IndexType, ShareableDumboPMessage};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::{ReliableBroadcast, ReliableBroadcastResult};
use atlas_common::collections::{HashMap, HashSet};
use atlas_common::crypto::hash::Digest;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::{Header, StoredMessage};
use atlas_core::messages::{ClientRqInfo, SessionBased};
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use getset::Getters;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

/// Struct of the state of a Dumbo round.
///
/// Holds either the committee election state or the running round state.
/// The running round state can only exist after the committee election is completed.
///
pub(super) enum DumboRoundState<CE, RQ, VR, IR, A> {
    WaitingForCommitteeElection(CommitteeElectionState<CE>),
    Running(RoundStateParts<RQ, VR, IR, A>),
    Done(RoundFinalData<RQ>),
}

impl<CE, RQ, VR, IR, A> DumboRoundState<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
{
    pub fn new_committee_election(committee_election: CE) -> Self {
        DumboRoundState::WaitingForCommitteeElection(
            CommitteeElectionState::CommitteeElectionRunning(committee_election),
        )
    }

    pub fn new_running<NT>(
        seq_no: SeqNo,
        committee: Vec<NodeId>,
        quorum_info: &QuorumInfo,
        requests: (DumboRQ<RQ>, Digest),
        network: &Arc<NT>,
    ) -> Self
    where
        CE: CommitteeElectionProtocol,
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        DumboRoundState::Running(RoundStateParts::new::<NT, CE>(
            seq_no,
            committee,
            quorum_info,
            requests,
            network,
        ))
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DumboRoundState::WaitingForCommitteeElection(committee_election) => {
                write!(f, "WaitingForCommitteeElection({committee_election:?})")
            }
            DumboRoundState::Running(round_parts) => {
                write!(f, "Running({round_parts:?})")
            }
            DumboRoundState::Done(round_final) => {
                write!(f, "Done({round_final:?})")
            }
        }
    }
}

/// The state of the committee election protocol
/// of this dumbo round
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
        RQ: SerMsg + SessionBased,
        VR: ReliableBroadcast<DumboRQ<RQ>>,
        IR: ReliableBroadcast<IndexType>,
        A: ABAProtocol,
    {
        let network = SendNodeWrapperRef::new(seq_number, own_node, network);

        match self {
            CommitteeElectionState::CommitteeElectionRunning(committee_election) => {
                let stored_message = StoredMessage::new(*message_header, ce_msg.clone());

                committee_election
                    .process_message(stored_message, &network)
                    .map_err(std::convert::Into::into)
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitteeElectionState::CommitteeElectionRunning(ce) => {
                write!(f, "CommitteeElectionRunning({ce:?})")
            }
            CommitteeElectionState::Done => write!(f, "Done"),
        }
    }
}

/// The ongoing round parts which store all relevant info to the round
pub struct RoundStateParts<RQ, VR, IR, A> {
    seq_no: SeqNo,
    committee: Vec<NodeId>,
    // The state of each node in the protocol. (excluding ourselves)
    node_states: HashMap<NodeId, NodeState<DumboRQ<RQ>, VR, IR, A>>,
    // Cache of client requests for which we have received via ValueRBCs from other nodes.
    client_request_tracker: ClientRequestTracker,
}

impl<RQ, VR, IR, A> Orderable for RoundStateParts<RQ, VR, IR, A> {
    fn sequence_number(&self) -> SeqNo {
        self.seq_no
    }
}

impl<RQ, VR, IR, A> RoundStateParts<RQ, VR, IR, A>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
{
    pub fn new<NT, CE>(
        seq_no: SeqNo,
        committee: Vec<NodeId>,
        quorum_info: &QuorumInfo,
        requests: (DumboRQ<RQ>, Digest),
        network: &Arc<NT>,
    ) -> Self
    where
        CE: CommitteeElectionProtocol,
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let mut node_states = quorum_info
            .quorum_members()
            .iter()
            .filter(|node| **node != quorum_info.own_node_id())
            .map(|node_id| {
                (
                    *node_id,
                    Self::init_other_node_state_for(
                        *node_id,
                        committee.contains(node_id),
                        quorum_info,
                    ),
                )
            })
            .collect::<HashMap<NodeId, NodeState<DumboRQ<RQ>, VR, IR, A>>>();

        let own_state = Self::init_own_node_state_for::<NT, CE>(
            seq_no,
            committee.contains(&quorum_info.own_node_id()),
            quorum_info.clone(),
            requests,
            network,
        );

        node_states.insert(quorum_info.own_node_id(), own_state);

        Self {
            seq_no,
            committee,
            node_states,
            client_request_tracker: ClientRequestTracker::default(),
        }
    }

    fn init_other_node_state_for(
        node_id: NodeId,
        is_committee_node: bool,
        quorum_info: &QuorumInfo,
    ) -> NodeState<DumboRQ<RQ>, VR, IR, A> {
        let rbc = VR::new(node_id, quorum_info.clone());

        Self::init_node_state_for(rbc, is_committee_node)
    }

    fn init_own_node_state_for<NT, CE>(
        seq_no: SeqNo,
        is_committee_node: bool,
        quorum_info: QuorumInfo,
        requests: (DumboRQ<RQ>, Digest),
        network: &Arc<NT>,
    ) -> NodeState<DumboRQ<RQ>, VR, IR, A>
    where
        CE: CommitteeElectionProtocol,
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let network = SendNodeWrapperRef::new(seq_no, quorum_info.own_node_id(), network);

        let rbc =
            VR::new_with_propose(quorum_info.own_node_id(), quorum_info, requests.0, &network);

        Self::init_node_state_for(rbc, is_committee_node)
    }

    fn init_node_state_for(rbc: VR, is_committee_node: bool) -> NodeState<DumboRQ<RQ>, VR, IR, A> {
        if is_committee_node {
            NodeState::CommitteeNode(
                CommitteeNodeExecuting::<VR, IR, A>::RunningValueRBC(rbc),
                CommitteeNodeState::<DumboRQ<RQ>>::default(),
            )
        } else {
            NodeState::NonCommitteeNode(
                NonCommitteeNodeExec::RunningValueRBC(rbc),
                NonCommitteeNodeState::default(),
            )
        }
    }

    pub(super) fn pop_messages<CE>(
        &self,
        pending_messages: &mut PendingMessages<ShareableDumboPMessage<RQ, VR, IR, A, CE>>,
    ) -> Option<ShareableDumboPMessage<RQ, VR, IR, A, CE>>
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
                            node,
                            DumboMessageTypeDiscriminants::ReliableBroadcast,
                        ),
                    CommitteeNodeExecuting::RunningIndexRBC(_) => pending_messages
                        .pop_message_by_type_and_owner(
                            node,
                            DumboMessageTypeDiscriminants::IndexReliableBroadcast,
                        ),
                    CommitteeNodeExecuting::RunningABA(_) => pending_messages
                        .pop_message_by_type_and_owner(
                            node,
                            DumboMessageTypeDiscriminants::AsyncBinaryAgreement,
                        ),
                    CommitteeNodeExecuting::WaitingForRBCs
                    | CommitteeNodeExecuting::WaitingForValues => None,
                    CommitteeNodeExecuting::Done => {
                        pending_messages.discard_all_messages_by_owner(node);

                        continue;
                    }
                },
                NodeState::NonCommitteeNode(non_committee_node_exec, _) => {
                    match non_committee_node_exec {
                        NonCommitteeNodeExec::RunningValueRBC(_) => pending_messages
                            .pop_message_by_type_and_owner(
                                node,
                                DumboMessageTypeDiscriminants::ReliableBroadcast,
                            ),
                        NonCommitteeNodeExec::Completed => {
                            pending_messages.discard_all_messages_by_owner(node);

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
                let stored_message = StoredMessage::new(*message_header, rbc_msg.clone());

                let network = SendNodeWrapperRef::new(seq_no, owner_id, network);

                rbc.process_message(stored_message, &network)?
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
                let received_requests = match node_state {
                    NodeState::CommitteeNode(committee_node_exec, committee_node_state) => {
                        let value_rbc = std::mem::replace(
                            committee_node_exec,
                            CommitteeNodeExecuting::WaitingForRBCs,
                        );

                        let CommitteeNodeExecuting::RunningValueRBC(rbc) = value_rbc else {
                            unreachable!("Checked above that we are in RunningValueRBC state");
                        };

                        let (value_rbc, _) = rbc.finalize()?;

                        let contained_rqs = value_rbc.get_client_rq_info();

                        committee_node_state.received_value(value_rbc);

                        contained_rqs
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

                        let (completed_rbc, _) = rbc.finalize()?;

                        let contained_requests = completed_rbc.get_client_rq_info();

                        non_committee_node_state.received_value(completed_rbc);

                        contained_requests
                    }
                }
                .into_vec();

                self.client_request_tracker
                    .register_received_requests(received_requests.as_slice());

                EpochResult::MessageProcessed
            }
        })
    }

    fn handle_value_rbc_finished(&mut self, owner_id: NodeId) {
        if self.is_part_of_committee(owner_id) {}
    }

    pub(super) fn process_index_rbc_message<NT, CE>(
        &mut self,
        round_data_arguments: RoundDataArguments,
        network: &Arc<NT>,
        owner_id: NodeId,
        message_header: &Header,
        rbc_msg: &IR::ReliableBroadcastMessage,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        CE: CommitteeElectionProtocol,
    {
        let RoundDataArguments {
            seq_no,
            quorum_info,
            threshold_keys,
        } = round_data_arguments;

        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        let result = match node_state {
            NodeState::CommitteeNode(CommitteeNodeExecuting::RunningIndexRBC(rbc), _) => {
                let stored_message = StoredMessage::new(*message_header, rbc_msg.clone());

                let network = SendNodeIBCMWrapperRef::new(seq_no, owner_id, network);

                rbc.process_message(stored_message, &network)?
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
                return Ok(EpochResult::MessageQueued);
            }
            ReliableBroadcastResult::MessageIgnored => {
                return Ok(EpochResult::MessageIgnored);
            }
            ReliableBroadcastResult::Processed => return Ok(EpochResult::MessageProcessed),
            ReliableBroadcastResult::Finalized => match node_state {
                NodeState::CommitteeNode(committee_node_exec, committee_node_state) => {
                    let value_rbc = std::mem::replace(
                        committee_node_exec,
                        CommitteeNodeExecuting::WaitingForValues,
                    );

                    let CommitteeNodeExecuting::RunningIndexRBC(rbc) = value_rbc else {
                        unreachable!("Checked above that we are in RunningValueRBC state");
                    };

                    let (index_rbc, _) = rbc.finalize()?;

                    committee_node_state.received_index(index_rbc.clone());

                    index_rbc
                }
                _ => {
                    unreachable!("Only committee nodes run Index RBC");
                }
            },
        };

        let missing_values = self.client_request_tracker.check_missing_values(&indexes.1);

        if missing_values.is_empty() {
            self.prepare_aba_with_input::<_, CE>(
                seq_no,
                quorum_info,
                threshold_keys,
                network,
                owner_id,
                true,
            )
            .map_err(Into::into)
        } else {
            self.client_request_tracker
                .register_missing_values(owner_id, missing_values);
            Ok(EpochResult::MessageProcessed)
        }
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
        let network = SendNodeWrapperRef::new(seq_no, owner_id, network);

        let result = match node_state {
            NodeState::CommitteeNode(CommitteeNodeExecuting::RunningABA(aba), _) => {
                let stored_message = StoredMessage::new(*message_header, aba_msg.clone());

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

        self.process_aba_result(owner_id, &network, &result)
            .map_err(std::convert::Into::into)
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

        let network = SendNodeWrapperRef::new(seq_no, completed_node, network);

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

        self.process_aba_result(completed_node, &network, &result)
            .map_err(std::convert::Into::into)
    }

    fn process_aba_result<NT>(
        &mut self,
        owner_id: NodeId,
        network: &NT,
        result: &AsyncBinaryAgreementResult,
    ) -> Result<EpochResult, A::ABAError>
    where
        NT: AsyncBinaryAgreementSendNode<A::AsyncBinaryMessage>,
    {
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            warn!("Received invalid node state for {:?}", owner_id);
            return Ok(EpochResult::MessageIgnored);
        };

        let (epoch_result, finished_aba) = match result {
            AsyncBinaryAgreementResult::MessageQueued => (EpochResult::MessageQueued, false),
            AsyncBinaryAgreementResult::MessageIgnored => (EpochResult::MessageIgnored, false),
            AsyncBinaryAgreementResult::Processed => (EpochResult::MessageProcessed, false),
            AsyncBinaryAgreementResult::Decided => {
                let NodeState::CommitteeNode(committee_node_exec, committee_node_state) =
                    node_state
                else {
                    unreachable!("Checked above that we are in RunningABA state");
                };

                let aba_state =
                    std::mem::replace(committee_node_exec, CommitteeNodeExecuting::Done);

                let CommitteeNodeExecuting::RunningABA(aba) = aba_state else {
                    unreachable!("Checked above that we are in RunningABA state");
                };

                let protocol_result = aba.finalize()?;

                committee_node_state.received_decision(protocol_result);

                if protocol_result {
                    // When we have finalized 1 of the running ABAs with a
                    // Positive outcome (included) then we want to send
                    // Input of 1 to all others which are not yet completed
                    self.send_negative_input_to_all_pending_aba(network)?;
                }

                (EpochResult::MessageProcessed, true)
            }
        };

        if finished_aba && self.are_all_aba_finished() {
            let final_index = self.get_final_index();

            let missing_values = final_index
                .iter()
                .flat_map(|(_, requests)| {
                    self.client_request_tracker
                        .check_missing_values(requests)
                        .into_iter()
                        .cloned()
                })
                .collect::<Vec<_>>();

            if missing_values.is_empty() {
                return Ok(EpochResult::Finalized);
            }

            self.client_request_tracker
                .register_missing_values(owner_id, missing_values.as_slice());
        }

        Ok(epoch_result)
    }

    fn send_negative_input_to_all_pending_aba<NT>(
        &mut self,
        network: &NT,
    ) -> Result<HashMap<NodeId, EpochResult>, A::ABAError>
    where
        NT: AsyncBinaryAgreementSendNode<A::AsyncBinaryMessage>,
    {
        let mut aba_results = Vec::new();

        for committee_node in &self.committee {
            let Some(node_state) = self.node_states.get_mut(committee_node) else {
                continue;
            };

            match node_state {
                NodeState::CommitteeNode(executing, state) => match (executing, state) {
                    (CommitteeNodeExecuting::RunningABA(aba), _) => {
                        let result = aba.provide_input_bit(false, network)?;

                        aba_results.push((*committee_node, result));
                    }
                    (_, state) if !matches!(state, CommitteeNodeState::Empty(_)) => {
                        state.stored_pending_vote(false);
                    }
                    (_, _) => (),
                },
                NodeState::NonCommitteeNode(_, _) => {
                    unreachable!("A committee node has been setup as a non committee node")
                }
            }
        }

        let mut final_results = HashMap::default();

        for (node, results) in aba_results {
            let epoch_result = self.process_aba_result(node, network, &results)?;
            final_results.insert(node, epoch_result);
        }

        Ok(final_results)
    }

    fn finish(mut self) -> Result<RoundFinalData<RQ>, FinalizeRoundError> {
        let index = self.get_final_index();

        let mut requests = Vec::with_capacity(index.len());

        for (node, _) in &index {
            if let Some(node_state) = self.node_states.remove(node) {
                let mut node_requests = match node_state {
                    NodeState::CommitteeNode(_, state) => match state {
                        CommitteeNodeState::ValueRBC { value, .. }
                        | CommitteeNodeState::IndexRBC { value, .. }
                        | CommitteeNodeState::ABA { value, .. } => value,
                        _ => return Err(FinalizeRoundError::AccessValueOfNonAvailableNode(*node)),
                    },
                    NodeState::NonCommitteeNode(_, state) => match state {
                        NonCommitteeNodeState::ValueRBC { value } => value,
                        NonCommitteeNodeState::Empty => {
                            return Err(FinalizeRoundError::AccessValueOfNonAvailableNode(*node));
                        }
                    },
                };

                requests.append(&mut node_requests);
            }
        }

        Ok(RoundFinalData::new(requests, index, self.committee))
    }

    fn node_received_all_pending_requests(&mut self, node: NodeId) {
        let Some(node) = self.node_states.get_mut(&node) else {
            return;
        };

        match node {
            NodeState::CommitteeNode(executing, round_state) => match executing {
                CommitteeNodeExecuting::RunningValueRBC(_) => {
                    todo!()
                }
                CommitteeNodeExecuting::WaitingForRBCs => {}
                CommitteeNodeExecuting::RunningIndexRBC(_) => {}
                CommitteeNodeExecuting::WaitingForValues => {}
                CommitteeNodeExecuting::RunningABA(_) => {}
                CommitteeNodeExecuting::Done => {}
            },
            NodeState::NonCommitteeNode(_, _) => {}
        }
    }

    fn are_all_aba_finished(&self) -> bool {
        self.committee_nodes()
            .any(|(_, state)| !matches!(state, CommitteeNodeState::ABA { .. }))
    }

    fn get_final_index(&self) -> Vec<IndexType> {
        self.committee_nodes()
            .filter_map(|(_, state)| {
                if let CommitteeNodeState::ABA {
                    decision, index, ..
                } = state
                {
                    if *decision { Some(index.clone()) } else { None }
                } else {
                    None
                }
            })
            .collect()
    }

    fn committee_nodes(
        &self,
    ) -> impl Iterator<
        Item = (
            &CommitteeNodeExecuting<VR, IR, A>,
            &CommitteeNodeState<DumboRQ<RQ>>,
        ),
    > {
        self.committee
            .iter()
            .filter_map(|node_id| self.node_states.get(node_id))
            .filter_map(|node_state| {
                if let NodeState::CommitteeNode(exec, state) = node_state {
                    Some((exec, state))
                } else {
                    None
                }
            })
    }

    fn completed_value_rbc_count(&self) -> usize {
        self.node_states
            .iter()
            .filter(|(_, state)| match state {
                NodeState::CommitteeNode(_, committee_node_state) => {
                    !matches!(committee_node_state, CommitteeNodeState::Empty(_))
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

    fn is_part_of_committee(&self, node_id: NodeId) -> bool {
        self.committee.contains(&node_id)
    }

    fn check_nodes_ready(&self, quorum_info: &QuorumInfo) -> bool {
        if !self.is_part_of_committee(quorum_info.own_node_id()) {
            return false;
        }

        self.completed_value_rbc_count() >= quorum_info.quorum_size()
    }

    fn check_all_committee_nodes_finished(&self) -> bool {
        let finished_committee_nodes = self
            .committee
            .iter()
            .filter_map(|node| self.node_states.get(node))
            .filter(|node_state| {
                matches!(
                    node_state,
                    NodeState::CommitteeNode(
                        CommitteeNodeExecuting::Done,
                        CommitteeNodeState::ABA { .. }
                    )
                )
            })
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoundStateParts")
            .field("committee", &self.committee)
            .field("node_states", &self.node_states)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct ClientRequestTracker {
    // Cache of client requests for which we have received via ValueRBCs from other nodes.
    received_request_cache: HashSet<ClientRqInfo>,
    waiting_for_values: HashMap<ClientRqInfo, HashSet<NodeId>>,
    reverse_waiting_values: HashMap<NodeId, HashSet<ClientRqInfo>>,
}

impl ClientRequestTracker {
    pub(super) fn check_missing_values<'a>(
        &self,
        index: &'a [ClientRqInfo],
    ) -> Vec<&'a ClientRqInfo> {
        index
            .iter()
            .filter(|rq_info| !self.received_request_cache.contains(rq_info))
            .collect()
    }

    fn register_received_requests(&mut self, requests: &[ClientRqInfo]) -> Vec<NodeId> {
        let mut finished_nodes = Vec::new();

        for request in requests {
            self.received_request_cache.insert(request.clone());

            if let Some(waiting_for_request) = self.waiting_for_values.remove(request) {
                let requests = waiting_for_request.into_iter().collect::<Vec<_>>();

                let mut finished =
                    self.handle_request_received_for_nodes(requests.as_slice(), request);

                if !finished.is_empty() {
                    finished_nodes.append(&mut finished);
                }
            }
        }

        finished_nodes
    }

    fn handle_request_received_for_nodes(
        &mut self,
        nodes: &[NodeId],
        request: &ClientRqInfo,
    ) -> Vec<NodeId> {
        let mut finished_nodes = Vec::new();

        for node in nodes {
            if let Some(pending_requests) = self.reverse_waiting_values.get_mut(node) {
                pending_requests.remove(request);

                if pending_requests.is_empty() {
                    finished_nodes.push(*node);
                }
            }
        }

        finished_nodes
    }

    fn register_missing_values<'a, R>(&mut self, owning_node: NodeId, missing_values: R)
    where
        R: IntoIterator<Item = &'a ClientRqInfo>,
    {
        let waiting_for_requests = self.reverse_waiting_values.entry(owning_node).or_default();

        missing_values.into_iter().for_each(|client_rq| {
            waiting_for_requests.insert(client_rq.clone());

            self.waiting_for_values
                .entry(client_rq.clone())
                .or_default()
                .insert(owning_node);
        });
    }
}

#[derive(Debug)]
pub(super) enum RoundProcessResult {
    QueueMessage,
    MessageIgnored,
    MessageProcessed,
    ReadyToFinalize,
}

#[derive(Getters)]
pub(super) struct RoundFinalData<RQ> {
    #[get = "pub"]
    requests: DumboRQ<RQ>,
    #[get = "pub"]
    indexes: Vec<IndexType>,
    #[get = "pub"]
    committee: Vec<NodeId>,
}

impl<RQ> RoundFinalData<RQ> {
    fn new(requests: DumboRQ<RQ>, indexes: Vec<IndexType>, committee: Vec<NodeId>) -> Self {
        Self {
            requests,
            indexes,
            committee,
        }
    }

    fn is_part_of_committee(&self, node_id: NodeId) -> bool {
        self.committee.contains(&node_id)
    }

    fn into_inner(self) -> (DumboRQ<RQ>, Vec<IndexType>, Vec<NodeId>) {
        (self.requests, self.indexes, self.committee)
    }
}

impl<RQ> Debug for RoundFinalData<RQ> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoundFinalData")
            .field("committee", &self.committee)
            .field("indexes", &self.indexes)
            .finish_non_exhaustive()
    }
}

pub(super) struct RoundDataArguments {
    seq_no: SeqNo,
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
}

impl RoundDataArguments {
    pub(super) fn new(
        seq_no: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
    ) -> Self {
        Self {
            seq_no,
            quorum_info,
            threshold_keys,
        }
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

#[derive(Debug, Error)]
pub(super) enum FinalizeRoundError {
    #[error("Failed to obtain requests from node state {0:?} as it is not in a supported state")]
    AccessValueOfNonAvailableNode(NodeId),
}
