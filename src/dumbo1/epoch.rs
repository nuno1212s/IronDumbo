use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult};
use crate::committee_election::{CommitteeElectionProtocol, CommitteeElectionResult};
use crate::dumbo1::message::DumboMessageType;
use crate::dumbo1::network::SendNodeWrapperRef;
use crate::dumbo1::protocol::{DumboPMessage, DumboPSerialization};
use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::{ReliableBroadcast, ReliableBroadcastResult};
use atlas_common::collections::HashMap;
use atlas_common::error::Result;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::ordering_protocol::ShareableConsensusMessage;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::fmt::Debug;
use std::sync::Arc;

pub(super) struct DumboRound<CE, RQ, R, A> {
    // The current epoch number.
    epoch_num: SeqNo,
    // The state of each node in the protocol.
    node_states: HashMap<NodeId, NodeState<RQ, R, A>>,
    // The state of the committee election protocol.
    committee_election: CommitteeState<CE>,
    // The information about the quorum.
    quorum_info: QuorumInfo,
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

        let committee_election_protocol = CE::new(quorum_info.clone(), required_committee);

        Self {
            epoch_num,
            node_states: HashMap::default(),
            committee_election: CommitteeState::RunningCE(committee_election_protocol),
            quorum_info,
        }
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableConsensusMessage<RQ, DumboPSerialization<RQ, R, A, CE>>,
        network: &Arc<NT>,
    ) -> Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, R, A, CE>>,
    {
        match message.message().message_type() {
            DumboMessageType::ReliableBroadcast(rbc_msg) => {
                let node_state = self.node_states.get_mut(&message.header().from());

                if let Some(node_state) = node_state {
                    match node_state {
                        NodeState::RunningRBC(rbc) => {
                            let stored_message =
                                StoredMessage::new(message.header().clone(), rbc_msg.clone());

                            let network = SendNodeWrapperRef::new(self.epoch_num.clone(), network);

                            let result = rbc.process_message(stored_message, &network);

                            match result {
                                ReliableBroadcastResult::MessageQueued => {
                                    Ok(EpochResult::MessageQueued)
                                }
                                ReliableBroadcastResult::MessageIgnored => {
                                    Ok(EpochResult::MessageIgnored)
                                }
                                ReliableBroadcastResult::Processed => {
                                    Ok(EpochResult::MessageProcessed)
                                }
                                ReliableBroadcastResult::Finalized => {
                                    // Replace with a dummy variable so we can then swap into the new state
                                    let rbc = std::mem::replace(rbc, R::new());

                                    let mut next_state = NodeState::RunningABA {
                                        completed_rbc: rbc.finalize(),
                                        aba: A::new(true),
                                    };

                                    std::mem::swap(node_state, &mut next_state);

                                    Ok(EpochResult::MessageProcessed)
                                }
                            }
                        }
                        NodeState::RunningABA { aba, .. } => Ok(EpochResult::MessageIgnored),
                        NodeState::Completed { .. } => Ok(EpochResult::MessageIgnored),
                    }
                } else {
                    Ok(EpochResult::MessageIgnored)
                }
            }
            DumboMessageType::AsyncBinaryAgreement(aba_msg) => {
                let node_state = self.node_states.get_mut(&message.header().from());

                if let Some(node_state) = node_state {
                    let result = match node_state {
                        NodeState::RunningABA { aba, completed_rbc } => {
                            let stored_message =
                                StoredMessage::new(message.header().clone(), aba_msg.clone());

                            let network = SendNodeWrapperRef::new(self.epoch_num.clone(), network);

                            aba.process_message(stored_message, &network)?
                        }
                        NodeState::RunningRBC(_) => {
                            // ABA message received before RBC completion, ignore or queue
                            //TODO: Queue message somewhere
                            return Ok(EpochResult::MessageQueued);
                        }
                        NodeState::Completed { .. } => {
                            // Node already completed, ignore message
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
                            let NodeState::RunningABA { aba, completed_rbc } = node_state else {
                                unreachable!("Checked above that we are in RunningABA state");
                            };

                            let protocol = std::mem::replace(aba, A::new(false));

                            let result = protocol.finalize()?;

                            //TODO prevent cloning the complete_rbc
                            *node_state = NodeState::Completed {
                                completed_rbc: completed_rbc.clone(),
                                value: result,
                            };

                            Ok(EpochResult::MessageProcessed)
                        }
                    }
                } else {
                    // Message from unknown node, ignore or queue
                    Ok(EpochResult::MessageIgnored)
                }
            }
            DumboMessageType::CommitteeElectionMessage(ce_msg) => {
                let network = SendNodeWrapperRef::new(self.epoch_num.clone(), network);

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
