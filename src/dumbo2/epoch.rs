use crate::dumbo2::epoch_round_state::{Dumbo2RoundError, RoundFinalData, RoundStateParts};
use crate::dumbo2::message::Dumbo2MessageType;
use crate::dumbo2::pending_messages::PendingMessages;
use crate::dumbo2::protocol::{Dumbo2PSerialization, DumboRQ, ShareableDumbo2PMessage};
use crate::mvba::MVBAProtocol;
use crate::prbc::PRBCProtocol;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rq_aggregator::RequestAggregator;
use atlas_common::error;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_core::messages::SessionBased;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use getset::Getters;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

/// A single Dumbo2 round: unlike Dumbo1's `DumboRound`, there is no
/// committee-election phase to wait through -- the round immediately starts
/// its own PRBC broadcast on construction (see `RoundStateParts::new`).
#[derive(Getters)]
pub(super) struct Dumbo2Round<RQ, PR, MV>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    epoch_num: SeqNo,
    #[get = "pub(super)"]
    quorum_info: QuorumInfo,
    #[get = "pub(super)"]
    threshold_keys: ThresholdKeys,
    pending_message: PendingMessages<ShareableDumbo2PMessage<RQ, PR, MV>>,
    /// `None` only after [`Self::take_final_data`] has consumed it.
    round_state: Option<RoundStateParts<RQ, PR, MV>>,
    request_aggregator: Arc<RequestAggregator<RQ>>,
}

impl<RQ, PR, MV> Orderable for Dumbo2Round<RQ, PR, MV>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    fn sequence_number(&self) -> SeqNo {
        self.epoch_num
    }
}

impl<RQ, PR, MV> Dumbo2Round<RQ, PR, MV>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    pub fn new<NT>(
        epoch_num: SeqNo,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        own_value: DumboRQ<RQ>,
        request_aggregator: Arc<RequestAggregator<RQ>>,
        network: &Arc<NT>,
    ) -> Self
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2PSerialization<RQ, PR, MV>>,
    {
        let round_state = RoundStateParts::new(
            epoch_num,
            quorum_info.clone(),
            threshold_keys.clone(),
            own_value,
            network,
        );

        Self {
            epoch_num,
            quorum_info,
            threshold_keys,
            pending_message: PendingMessages::default(),
            round_state: Some(round_state),
            request_aggregator,
        }
    }

    pub(super) fn poll(&mut self) -> Option<ShareableDumbo2PMessage<RQ, PR, MV>> {
        self.pending_message.pop_message()
    }

    pub(super) fn process_message<NT>(
        &mut self,
        message: ShareableDumbo2PMessage<RQ, PR, MV>,
        network: &Arc<NT>,
    ) -> error::Result<EpochResult>
    where
        NT: OrderProtocolSendNode<RQ, Dumbo2PSerialization<RQ, PR, MV>>,
    {
        let Some(round_state) = &mut self.round_state else {
            return Ok(EpochResult::MessageIgnored);
        };

        let result = match message.message().message_type() {
            Dumbo2MessageType::PRBC(owner, prbc_msg) => {
                round_state.process_prbc_message(network, *owner, message.header(), prbc_msg)?
            }
            Dumbo2MessageType::MVBA(mvba_msg) => {
                round_state.process_mvba_message(network, message.header(), mvba_msg)?
            }
        };

        if matches!(result, EpochResult::QueueMessage) {
            self.pending_message.add_message(message);
            return Ok(EpochResult::MessageQueued);
        }

        Ok(result)
    }

    /// Consumes the round's final data once it has reported
    /// [`EpochResult::Finalized`]. May only be called once.
    pub(super) fn take_final_data(&mut self) -> error::Result<RoundFinalData<DumboRQ<RQ>>> {
        let round_state = self
            .round_state
            .take()
            .ok_or(Dumbo2RoundError::NotFinished)?;

        round_state.finish().map_err(Into::into)
    }

    pub(super) fn prbc_done_count(&self) -> usize {
        self.round_state
            .as_ref()
            .map(|rs| rs.done_count())
            .unwrap_or(0)
    }

    pub(super) fn decided_size(&self) -> Option<usize> {
        self.round_state.as_ref().and_then(|rs| rs.decided_size())
    }
}

impl<RQ, PR, MV> Debug for Dumbo2Round<RQ, PR, MV>
where
    RQ: SerMsg + SessionBased + Debug,
    PR: PRBCProtocol<DumboRQ<RQ>> + Debug,
    MV: MVBAProtocol + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dumbo2Round")
            .field("epoch_num", &self.epoch_num)
            .field("round_state", &self.round_state)
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
