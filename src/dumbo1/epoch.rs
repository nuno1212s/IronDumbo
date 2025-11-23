use crate::aba::ABAProtocol;
use crate::committee_election::{CommitteeElectionProtocol, CommitteeElectionResult};
use crate::dumbo1::epoch_round_state::{
    CommitteeElectionState, DumboRoundState, RoundDataArguments,
};
use crate::dumbo1::message::DumboMessageType;
use crate::dumbo1::pending_messages::PendingMessages;
use crate::dumbo1::protocol::{DumboPSerialization, DumboRQ, IndexType, ShareableDumboPMessage};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::ReliableBroadcast;
use crate::rq_aggregator::RequestAggregator;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_core::messages::SessionBased;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use getset::Getters;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

#[derive(Getters)]
pub(super) struct DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
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
    pending_message: PendingMessages<ShareableDumboPMessage<RQ, VR, IR, A, CE>>,
    // Round state of the current round
    round_state: DumboRoundState<CE, RQ, VR, IR, A>,
    // Request aggregator reference
    request_aggregator: Arc<RequestAggregator<RQ>>,
}

impl<CE, RQ, VR, IR, A> Orderable for DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
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
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    pub fn new(
        epoch_num: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        request_aggregator: Arc<RequestAggregator<RQ>>,
    ) -> Self {
        let required_committee = quorum_info.f() + 1;

        let committee_election_protocol = CE::new(quorum_info.clone(), required_committee);

        Self {
            epoch_num,
            quorum_info,
            threshold_keys,
            pending_message: PendingMessages::default(),
            round_state: DumboRoundState::new_committee_election(committee_election_protocol),
            request_aggregator,
        }
    }

    fn node_id(&self) -> NodeId {
        self.quorum_info.own_node_id()
    }

    pub(super) fn poll(&mut self) -> Option<ShareableDumboPMessage<RQ, VR, IR, A, CE>> {
        match &self.round_state {
            DumboRoundState::Running(round_state) => {
                round_state.pop_messages::<CE>(&mut self.pending_message)
            }
            _ => None,
        }
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableDumboPMessage<RQ, VR, IR, A, CE>,
        network: &Arc<NT>,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
    {
        let seq_no = message.sequence_number();
        let node_id = self.node_id();
        let quorum_info = self.quorum_info().clone();
        let threshold_keys = self.threshold_keys().clone();

        match &mut self.round_state {
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

                            let requests = self.request_aggregator.get_batch_and_reset();

                            self.round_state =
                                DumboRoundState::new_running(
                                    self.sequence_number(),
                                    committee,
                                    self.quorum_info(),
                                    requests,
                                    network,
                                );

                            Ok(EpochResult::MessageProcessed)
                        }
                    }
                } else {
                    self.pending_message.add_message(message);

                    Ok(EpochResult::MessageQueued)
                }
            }
            DumboRoundState::Running(running_state) => {
                let round_data = RoundDataArguments::new(seq_no, quorum_info, threshold_keys);

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
                            round_data,
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
            DumboRoundState::Done(..) => Ok(EpochResult::MessageIgnored),
        }
    }
}

impl<CE, RQ, VR, IR, A> Debug for DumboRound<CE, RQ, VR, IR, A>
where
    RQ: SerMsg + SessionBased + Debug,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DumboRound")
            .field("epoch_num", &self.epoch_num)
            .field("node_id", &self.node_id())
            .field("quorum_info", &self.quorum_info)
            .field("dumbo_round_state", &self.round_state)
            .finish_non_exhaustive()
    }
}

pub(super) enum EpochResult {
    MessageIgnored,
    MessageQueued,
    QueueMessage,
    MessageProcessed,
    Finalized,
}
