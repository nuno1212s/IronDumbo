use std::sync::Arc;
use crate::dumbo1::node_states::{CommitteeNodeExecuting, NodeState, NonCommitteeNodeExec};
use atlas_common::collections::{HashMap, HashSet};
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::Orderable;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::{Header, StoredMessage};
use atlas_core::messages::ClientRqInfo;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use thiserror::Error;
use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult};
use crate::committee_election::CommitteeElectionProtocol;
use crate::consensus_rqs::ConsensusRequest;
use crate::dumbo1::epoch::{DumboRound, EpochResult};
use crate::dumbo1::network::{SendNodeIBCMWrapperRef, SendNodeWrapperRef};
use crate::dumbo1::protocol::{DumboPSerialization, IndexType};
use crate::rbc::{ReliableBroadcast, ReliableBroadcastResult};

pub(super) enum DumboRoundState<CE, RQ, VR, IR, A> {
    WaitingForCommitteeElection {
        committee_election: CE,
    },
    Running {
        committee: Vec<NodeId>,
        // The state of each node in the protocol. (excluding ourselves)
        node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
        // Cache of client requests for which we have received via ValueRBCs from other nodes.
        received_request_cache: HashSet<ClientRqInfo>,
        waiting_for_values: HashMap<ClientRqInfo, Vec<NodeId>>,
    },
}

impl<CE, RQ, VR, IR, A> DumboRoundState<CE, RQ, VR, IR, A> {
    pub fn new(committee_election: CE) -> Self {
        DumboRoundState::WaitingForCommitteeElection { committee_election }
    }

    pub fn new_running(
        committee: Vec<NodeId>,
        node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
    ) -> Self {
        DumboRoundState::Running {
            committee,
            node_states,
            received_request_cache: HashSet::default(),
            waiting_for_values: HashMap::default(),
        }
    }

    pub fn get_committee_election(&self) -> Option<&CE> {
        match self {
            DumboRoundState::WaitingForCommitteeElection { committee_election } => {
                Some(committee_election)
            }
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

pub struct RoundStateParts<RQ, VR, IR, A> {
    committee: Vec<NodeId>,
    // The state of each node in the protocol. (excluding ourselves)
    node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
    // Cache of client requests for which we have received via ValueRBCs from other nodes.
    received_request_cache: HashSet<ClientRqInfo>,
    waiting_for_values: HashMap<ClientRqInfo, Vec<NodeId>>,
}

impl<RQ, VR, IR, A> RoundStateParts<RQ, VR, IR, A>where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol, {
    pub fn new(
        committee: Vec<NodeId>,
        node_states: HashMap<NodeId, NodeState<RQ, VR, IR, A>>,
        received_request_cache: HashSet<ClientRqInfo>,
        waiting_for_values: HashMap<ClientRqInfo, Vec<NodeId>>,
    ) -> Self {
        Self {
            committee,
            node_states,
            received_request_cache,
            waiting_for_values,
        }
    }

    fn process_value_rbc_message<NT, CE>(
        &mut self,
        round_data: &DumboRound<CE, RQ, VR, IR, A>,
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

                let network = SendNodeWrapperRef::new(round_data.sequence_number(), owner_id, network);

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
                Ok(EpochResult::MessageProcessed)
            }
        }
    }

    fn process_index_rbc_message<NT, CE>(
        &mut self,
        round_data: &DumboRound<CE, RQ, VR, IR, A>,
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

                let network =
                    SendNodeIBCMWrapperRef::new(round_data.sequence_number().clone(), owner_id, network);

                rbc.process_message(stored_message, &network)
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
            self.prepare_aba_with_input(network, owner_id, true)?;
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


    fn process_aba_message<NT, CE>(
        &mut self,
        round_data: &DumboRound<CE, RQ, VR, IR, A>,
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
            NodeState::CommitteeNode(committee_node, _) => match committee_node {
                CommitteeNodeExecuting::RunningABA(aba) => {
                    let stored_message =
                        StoredMessage::new(message_header.clone(), aba_msg.clone());

                    let network =
                        SendNodeWrapperRef::new(round_data.sequence_number(), owner_id.clone(), network);

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

        self.process_aba_result(owner_id, result)
    }

    fn process_aba_result(
        &mut self,
        owner_id: NodeId,
        result: AsyncBinaryAgreementResult,
    ) -> Result<EpochResult, A::ABAError> {
        let Some(node_state) = self.node_states.get_mut(&owner_id) else {
            return Ok(EpochResult::MessageIgnored);
        };

        match result {
            AsyncBinaryAgreementResult::MessageQueued => Ok(EpochResult::MessageQueued),
            AsyncBinaryAgreementResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
            AsyncBinaryAgreementResult::Processed => Ok(EpochResult::MessageProcessed),
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

                Ok(EpochResult::MessageProcessed)
            }
        }
    }

    fn prepare_aba_with_input<NT, CE>(
        &mut self,
        round_data: &DumboRound<CE, RQ, VR, IR, A>,
        network: &Arc<NT>,
        completed_node: NodeId,
        input: bool,
    ) -> Result<EpochResult, A::ABAError>
    where
        CE: CommitteeElectionProtocol,
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let node_state = self.node_states.get_mut(&completed_node);

        let network =
            SendNodeWrapperRef::new(round_data.sequence_number(), completed_node.clone(), network);

        if let Some(NodeState::CommitteeNode(committee_node_exec, _)) = node_state {
            let result = match committee_node_exec {
                CommitteeNodeExecuting::WaitingForValues => {
                    // Proceed to ABA
                    let mut aba = A::new(round_data.quorum_info().clone(), round_data.threshold_keys().clone());

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
        } else {
            Err(ABAPreparationError::NotPartOfCommittee)
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