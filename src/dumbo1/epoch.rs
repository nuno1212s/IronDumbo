use crate::aba::ABAProtocol;
use crate::committee_election::{CommitteeElectionProtocol, CommitteeElectionResult};
use crate::consensus_rqs::ConsensusRequest;
use crate::dumbo1::epoch_round_state::{
    CheckNodeStateError, CommitteeElectionState, DumboRoundState,
};
use crate::dumbo1::message::DumboMessageType;
use crate::dumbo1::pending_messages::PendingMessages;
use crate::dumbo1::protocol::{DumboPSerialization, IndexType};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::ReliableBroadcast;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use atlas_core::ordering_protocol::ShareableConsensusMessage;
use getset::Getters;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

#[derive(Getters)]
pub(super) struct DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    // The current epoch number.
    epoch_num: SeqNo,
    // The information about the quorum.
    #[get = "pub(super)"]
    quorum_info: QuorumInfo,
    // The threshold keys for the current quorum.
    #[get = "pub(super)"]
    threshold_keys: ThresholdKeys,
    // Pending messages to be processed later
    pending_message:
        PendingMessages<ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>>,
    // Round state of the current round
    dumbo_round_state: DumboRoundState<CE, RQ, VR, IR, A>,
}

impl<CE, RQ, VR, IR, A> Orderable for DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    fn sequence_number(&self) -> SeqNo {
        self.epoch_num
    }
}

impl<CE, RQ, VR, IR, A> DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    pub fn new(
        epoch_num: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
    ) -> Self {
        let required_committee = quorum_info.f() + 1;

        let committee_election_protocol = CE::new(quorum_info.clone(), required_committee);

        Self {
            epoch_num,
            quorum_info,
            threshold_keys,
            pending_message: PendingMessages::default(),
            dumbo_round_state: DumboRoundState::new(committee_election_protocol),
        }
    }
    
    fn node_id(&self) -> NodeId {
        self.quorum_info.own_node_id()
    }

    pub(super) fn poll(
        &mut self,
    ) -> Option<ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>> {
        match &self.dumbo_round_state {
            DumboRoundState::WaitingForCommitteeElection(_) => None,
            DumboRoundState::Running(round_state) => {
                round_state.pop_messages::<CE>(&mut self.pending_message)
            }
        }
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableConsensusMessage<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
        network: &Arc<NT>,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let seq_no = message.sequence_number();
        let node_id = self.node_id().clone();
        let quorum_info = self.quorum_info().clone();
        let threshold_keys = self.threshold_keys().clone();

        match &mut self.dumbo_round_state {
            DumboRoundState::WaitingForCommitteeElection(committee_election) => {
                if let DumboMessageType::CommitteeElectionMessage(ce_msg) =
                    message.message().message_type()
                {
                    let committee_result = committee_election
                        .process_committee_election_message::<_, RQ, VR, IR, A>(
                            seq_no,
                            node_id,
                            network,
                            message.header(),
                            ce_msg,
                        )?;

                    match committee_result {
                        CommitteeElectionResult::MessageQueued => Ok(EpochResult::MessageQueued),
                        CommitteeElectionResult::MessageIgnored => Ok(EpochResult::MessageIgnored),
                        CommitteeElectionResult::Processed => Ok(EpochResult::MessageProcessed),
                        CommitteeElectionResult::Decided => {
                            let state =
                                std::mem::replace(committee_election, CommitteeElectionState::Done);

                            let CommitteeElectionState::CommitteeElectionRunning(ce) = state else {
                                unreachable!("Checked above that we are in RunningCE state");
                            };

                            let committee = ce.finalize()?;

                            self.dumbo_round_state =
                                DumboRoundState::new_running(committee, self.quorum_info());

                            Ok(EpochResult::MessageProcessed)
                        }
                    }
                } else {
                    self.pending_message.add_message(message);

                    Ok(EpochResult::MessageQueued)
                }
            }
            DumboRoundState::Running(running_state) => {
                let result = match message.message().message_type() {
                    DumboMessageType::ReliableBroadcast(owner, rbc_msg) => running_state
                        .process_value_rbc_message::<_, CE>(
                            seq_no,
                            network,
                            *owner,
                            message.header(),
                            rbc_msg,
                        ),
                    DumboMessageType::IndexReliableBroadcast(owner_id, rbc_msg) => running_state
                        .process_index_rbc_message::<_, CE>(
                            seq_no,
                            quorum_info,
                            threshold_keys,
                            network,
                            *owner_id,
                            message.header(),
                            rbc_msg,
                        ),
                    DumboMessageType::AsyncBinaryAgreement(owner, aba_msg) => running_state
                        .process_aba_message::<_, CE>(
                            seq_no,
                            network,
                            *owner,
                            message.header(),
                            aba_msg,
                        ),
                    _ => Ok(EpochResult::MessageIgnored),
                }?;

                match result {
                    EpochResult::QueueMessage => {
                        self.pending_message.add_message(message);
                        Ok(EpochResult::MessageQueued)
                    }
                    _ => Ok(result),
                }
            }
        }
    }

}

impl<CE, RQ, VR, IR, A> Debug for DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + ConsensusRequest + Debug,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DumboRound")
            .field("epoch_num", &self.epoch_num)
            .field("node_id", &self.node_id())
            .field("quorum_info", &self.quorum_info)
            .field("dumbo_round_state", &self.dumbo_round_state)
            .finish()
    }
}

pub(super) enum EpochResult {
    MessageIgnored,
    MessageQueued,
    QueueMessage,
    MessageProcessed,
    Finalized,
}
