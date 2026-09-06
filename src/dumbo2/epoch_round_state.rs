use crate::dumbo2::epoch::EpochResult;
use crate::dumbo2::message::{Dumbo2MessageType, Dumbo2Serialization};
use crate::dumbo2::network::{MvbaSendNodeWrapperRef, PrbcSendNodeWrapperRef};
use crate::dumbo2::node_states::NodeState2;
use crate::dumbo2::protocol::DumboRQ;
use crate::mvba::{MVBAProposal, MVBAProtocol, MVBAResult};
use crate::prbc::{PRBCProtocol, PRBCResult};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::collections::HashMap;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::{Header, StoredMessage};
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use getset::Getters;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for `n-f` of the quorum's PRBC broadcasts (including our own)
    /// to finish.
    CollectingPRBC,
    /// `n-f` PRBCs finished; MVBA has been proposed, waiting for it to decide.
    WaitingForMVBA,
    /// MVBA decided a subset `W'`; waiting for any of `W'`'s PRBCs that
    /// hadn't finished yet at decision time to catch up.
    CollectingValues,
    Done,
}

/// The ongoing state of a single Dumbo2 round: one PRBC instance per quorum
/// member (including ourselves), and -- once enough of those finish -- a
/// single MVBA instance agreeing on which `n-f` (or more) of them to include.
///
/// `RQ` here is the *user's* request type -- as in Dumbo1, each PRBC
/// instance actually broadcasts a batch (`DumboRQ<RQ>`), not a bare `RQ`.
pub(super) struct RoundStateParts<RQ, PR, MV> {
    seq_no: SeqNo,
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    phase: Phase,
    node_states: HashMap<NodeId, NodeState2<DumboRQ<RQ>, PR>>,
    mvba: Option<MV>,
    decided_w: Option<MVBAProposal>,
}

impl<RQ, PR, MV> RoundStateParts<RQ, PR, MV>
where
    RQ: SerMsg,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    pub(super) fn new<NT>(
        seq_no: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        own_value: DumboRQ<RQ>,
        network: &Arc<NT>,
    ) -> Self
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PR::Message, MV::Message>>,
    {
        let own_id = quorum_info.own_node_id();

        let node_states = quorum_info
            .quorum_members()
            .iter()
            .map(|&node_id| {
                let state = if node_id == own_id {
                    let wrapper = PrbcSendNodeWrapperRef::<RQ, PR::Message, MV::Message, NT>::new(
                        seq_no, node_id, network,
                    );

                    NodeState2::RunningPRBC(PR::new_with_propose(
                        node_id,
                        quorum_info.clone(),
                        threshold_keys.clone(),
                        own_value.clone(),
                        &wrapper,
                    ))
                } else {
                    NodeState2::RunningPRBC(PR::new(
                        node_id,
                        quorum_info.clone(),
                        threshold_keys.clone(),
                    ))
                };

                (node_id, state)
            })
            .collect();

        Self {
            seq_no,
            quorum_info,
            threshold_keys,
            phase: Phase::CollectingPRBC,
            node_states,
            mvba: None,
            decided_w: None,
        }
    }

    pub(super) fn process_prbc_message<NT>(
        &mut self,
        network: &Arc<NT>,
        owner: NodeId,
        header: &Header,
        message: &PR::Message,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PR::Message, MV::Message>>,
    {
        if matches!(self.phase, Phase::Done) {
            return Ok(EpochResult::MessageIgnored);
        }

        let Some(node_state) = self.node_states.get_mut(&owner) else {
            return Ok(EpochResult::MessageIgnored);
        };

        let NodeState2::RunningPRBC(prbc) = node_state else {
            return Ok(EpochResult::MessageIgnored);
        };

        let stored = StoredMessage::new(*header, message.clone());
        let wrapper = PrbcSendNodeWrapperRef::<RQ, PR::Message, MV::Message, NT>::new(
            self.seq_no,
            owner,
            network,
        );

        let result = prbc
            .process_message(stored, &wrapper)
            .map_err(|e| Dumbo2RoundError::Prbc(Box::new(e)))?;

        match result {
            PRBCResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
            PRBCResult::MessageQueued => Ok(EpochResult::MessageQueued),
            PRBCResult::Processed => Ok(EpochResult::MessageProcessed),
            PRBCResult::Finalized(_) => {
                self.mark_prbc_done(owner)?;
                self.maybe_start_mvba(network)?;
                self.maybe_finish();

                if matches!(self.phase, Phase::Done) {
                    Ok(EpochResult::Finalized)
                } else {
                    Ok(EpochResult::MessageProcessed)
                }
            }
        }
    }

    fn mark_prbc_done(&mut self, owner: NodeId) -> error::Result<()> {
        let Some(node_state) = self.node_states.remove(&owner) else {
            return Ok(());
        };

        let NodeState2::RunningPRBC(prbc) = node_state else {
            self.node_states.insert(owner, node_state);
            return Ok(());
        };

        let (value, digest, signature) = prbc
            .finalize()
            .map_err(|e| Dumbo2RoundError::Prbc(Box::new(e)))?;

        self.node_states.insert(
            owner,
            NodeState2::Done {
                value,
                digest,
                signature,
            },
        );

        Ok(())
    }

    pub(super) fn done_count(&self) -> usize {
        self.node_states.values().filter(|s| s.is_done()).count()
    }

    pub(super) fn decided_size(&self) -> Option<usize> {
        self.decided_w.as_ref().map(Vec::len)
    }

    fn maybe_start_mvba<NT>(&mut self, network: &Arc<NT>) -> error::Result<()>
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PR::Message, MV::Message>>,
    {
        if !matches!(self.phase, Phase::CollectingPRBC) {
            return Ok(());
        }

        if self.done_count() < self.quorum_info.quorum_size() {
            return Ok(());
        }

        let proposal: MVBAProposal = self
            .node_states
            .iter()
            .filter_map(|(&owner, state)| match state {
                NodeState2::Done {
                    digest, signature, ..
                } => Some((owner, *digest, signature.clone())),
                NodeState2::RunningPRBC(_) => None,
            })
            .collect();

        let mut mvba = MV::new(
            self.quorum_info.clone(),
            self.threshold_keys.clone(),
            self.seq_no,
        );
        let wrapper =
            MvbaSendNodeWrapperRef::<RQ, PR::Message, MV::Message, NT>::new(self.seq_no, network);

        let result = mvba
            .propose(proposal, &wrapper)
            .map_err(|e| Dumbo2RoundError::Mvba(Box::new(e)))?;

        self.phase = Phase::WaitingForMVBA;
        self.mvba = Some(mvba);

        if matches!(result, MVBAResult::Decided) {
            self.handle_mvba_decided()?;
        }

        Ok(())
    }

    pub(super) fn process_mvba_message<NT>(
        &mut self,
        network: &Arc<NT>,
        header: &Header,
        message: &MV::Message,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PR::Message, MV::Message>>,
    {
        if matches!(self.phase, Phase::Done) {
            return Ok(EpochResult::MessageIgnored);
        }

        let Some(mvba) = &mut self.mvba else {
            return Ok(EpochResult::QueueMessage);
        };

        let stored = StoredMessage::new(*header, message.clone());
        let wrapper =
            MvbaSendNodeWrapperRef::<RQ, PR::Message, MV::Message, NT>::new(self.seq_no, network);

        let result = mvba
            .process_message(stored, &wrapper)
            .map_err(|e| Dumbo2RoundError::Mvba(Box::new(e)))?;

        match result {
            MVBAResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
            MVBAResult::MessageQueued => Ok(EpochResult::MessageQueued),
            MVBAResult::Processed => Ok(EpochResult::MessageProcessed),
            MVBAResult::Decided => {
                self.handle_mvba_decided()?;
                self.maybe_finish();

                if matches!(self.phase, Phase::Done) {
                    Ok(EpochResult::Finalized)
                } else {
                    Ok(EpochResult::MessageProcessed)
                }
            }
        }
    }

    fn handle_mvba_decided(&mut self) -> error::Result<()> {
        if self.decided_w.is_some() {
            return Ok(());
        }

        let Some(mvba) = self.mvba.take() else {
            return Ok(());
        };

        let decided = mvba
            .finalize()
            .map_err(|e| Dumbo2RoundError::Mvba(Box::new(e)))?;

        self.decided_w = Some(decided);
        self.phase = Phase::CollectingValues;

        Ok(())
    }

    fn maybe_finish(&mut self) {
        if !matches!(self.phase, Phase::CollectingValues) {
            return;
        }

        let Some(decided_w) = &self.decided_w else {
            return;
        };

        let all_ready = decided_w.iter().all(|(owner, _, _)| {
            self.node_states
                .get(owner)
                .map(NodeState2::is_done)
                .unwrap_or(false)
        });

        if all_ready {
            self.phase = Phase::Done;
        }
    }

    pub(super) fn finish(self) -> Result<RoundFinalData<DumboRQ<RQ>>, Dumbo2RoundError> {
        let Some(decided_w) = self.decided_w else {
            return Err(Dumbo2RoundError::NotFinished);
        };

        let mut node_states = self.node_states;

        let values = decided_w
            .iter()
            .filter_map(|(owner, _, _)| match node_states.remove(owner) {
                Some(NodeState2::Done { value, .. }) => Some(value),
                _ => None,
            })
            .collect();

        Ok(RoundFinalData::new(values, decided_w))
    }

    pub(super) fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }
}

impl<RQ, PR, MV> Debug for RoundStateParts<RQ, PR, MV>
where
    RQ: SerMsg,
    PR: PRBCProtocol<DumboRQ<RQ>> + Debug,
    MV: MVBAProtocol + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoundStateParts")
            .field("phase", &self.phase)
            .field("done_count", &self.done_count())
            .field("node_states", &self.node_states)
            .finish_non_exhaustive()
    }
}

#[derive(Getters)]
pub(super) struct RoundFinalData<RQ> {
    #[get = "pub"]
    values: Vec<RQ>,
    #[get = "pub"]
    included: MVBAProposal,
}

impl<RQ> RoundFinalData<RQ> {
    fn new(values: Vec<RQ>, included: MVBAProposal) -> Self {
        Self { values, included }
    }
}

#[derive(Debug, Error)]
pub(super) enum Dumbo2RoundError {
    #[error("PRBC error: {0}")]
    Prbc(Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("MVBA error: {0}")]
    Mvba(Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("Attempted to finish a round that has not decided a value")]
    NotFinished,
}
