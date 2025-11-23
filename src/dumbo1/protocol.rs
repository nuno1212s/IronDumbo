use crate::aba::ABAProtocol;
use crate::committee_election::CommitteeElectionProtocol;
use crate::dumbo1::config::Dumbo1Config;
use crate::dumbo1::epoch::{DumboRound, EpochResult};
use crate::dumbo1::message::DumboSerialization;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::ReliableBroadcast;
use crate::rq_aggregator::RequestAggregator;
use atlas_common::Err;
use atlas_common::error::Result;
use atlas_common::maybe_vec::MaybeVec;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::messages::{ClientRqInfo, SessionBased};
use atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage;
use atlas_core::ordering_protocol::networking::{
    NetworkedOrderProtocolInitializer, OrderProtocolSendNode,
};
use atlas_core::ordering_protocol::{
    Decision, DecisionsAhead, OPExResult, OPExecResult, OPPollResult, OPResult,
    OrderProtocolTolerance, OrderingProtocol, OrderingProtocolArgs, ShareableConsensusMessage,
};
use atlas_core::timeouts::timeout::{ModTimeout, TimeoutableMod};
use either::Either;
use getset::{Getters, Setters};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use tracing::warn;

/// The name of the Dumbo1 module.
/// Used for logging and metrics.
static DUMBO1_MOD_NAME: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("Dumbo1"));

pub type IndexType = (NodeId, Vec<ClientRqInfo>);

/// Since we want Dumbo to support batching, and we don't want to complicate the design of the
/// Actual individual protocols (RBC, ABA) we need to short circuit this definition.
/// I still have to work on an Atlas level solution to allow for all protocols to be usable
/// with single requests or batched requests
pub(super) type DumboRQ<RQ> = Vec<StoredMessage<RQ>>;

pub(super) type ShareableDumboPMessage<
    RQ,
    R: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
> = ShareableConsensusMessage<RQ, DumboPSerialization<RQ, R, IR, A, CE>>;

pub type DumboPSerialization<
    RQ,
    R: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
> = DumboSerialization<
    RQ,
    R::ReliableBroadcastMessage,
    IR::ReliableBroadcastMessage,
    A::AsyncBinaryMessage,
    CE::Message,
>;

#[allow(dead_code)]
pub(super) type DumboPMessage<
    RQ: 'static,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
> = <DumboPSerialization<RQ, VR, IR, A, CE> as OrderingProtocolMessage<RQ>>::ProtocolMessage;

/// An instance of the Dumbo protocol.
/// Holds the state of the protocol for a specific epoch.
/// Tracks the state of each node in the protocol.
#[derive(Getters, Setters)]
pub struct Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
{
    // The current epoch number.
    epoch_num: SeqNo,

    // The current quorum information
    quorum_info: QuorumInfo,
    // The threshold keys for the current quorum.
    threshold_keys: ThresholdKeys,
    // The watermark for the number of rounds to keep in memory.
    watermark: usize,

    request_aggregator: Arc<RequestAggregator<RQ>>,

    network: Arc<NT>,
    // The rounds of the dumbo protocol.
    rounds: VecDeque<DumboRound<CE, RQ, VR, IR, A>>,
}

impl<CE, RQ, VR, IR, A, NT> OrderProtocolTolerance for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    RQ: SerMsg,
{
    fn get_n_for_f(f: usize) -> usize {
        3 * f + 1
    }

    fn get_quorum_for_n(n: usize) -> usize {
        // n = 2f + 1
        (n - 1) / 2
    }

    fn get_f_for_n(n: usize) -> usize {
        (n - 1) / 3
    }
}

impl<CE, RQ, VR, IR, A, NT> Orderable for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    RQ: SerMsg,
{
    fn sequence_number(&self) -> SeqNo {
        self.epoch_num
    }
}

impl<CE, RQ, VR, IR, A, NT> TimeoutableMod<OPExResult<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>>
    for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    RQ: SerMsg,
{
    fn mod_name() -> Arc<str> {
        DUMBO1_MOD_NAME.clone()
    }

    fn handle_timeout(
        &mut self,
        timeout: Vec<ModTimeout>,
    ) -> Result<OPExResult<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>> {
        todo!()
    }
}

impl<CE, RQ, VR, IR, A, NT> OrderingProtocol<RQ> for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
{
    type Serialization = DumboPSerialization<RQ, VR, IR, A, CE>;
    type Config = Dumbo1Config;

    fn handle_off_ctx_message(
        &mut self,
        message: ShareableConsensusMessage<RQ, Self::Serialization>,
    ) {
        todo!()
    }

    fn handle_execution_changed(&mut self, is_executing: bool) -> Result<()> {
        todo!()
    }

    fn poll(&mut self) -> Result<OPResult<RQ, Self::Serialization>> {
        let polled_message = self.rounds.front_mut().map(|round| round.poll()).flatten();

        match polled_message {
            None => Ok(OPPollResult::ReceiveMsg),
            Some(message) => Ok(OPPollResult::Exec(message)),
        }
    }

    fn process_message(
        &mut self,
        message: ShareableConsensusMessage<RQ, Self::Serialization>,
    ) -> Result<OPExResult<RQ, Self::Serialization>> {
        let message_seq_no = message.message().sequence_number();

        match message_seq_no.index(self.epoch_num) {
            Either::Left(_) => Ok(OPExecResult::MessageDropped),
            Either::Right(index) => {
                let result = self.rounds[index].process_message(message.clone(), &self.network)?;

                match result {
                    EpochResult::MessageIgnored => Ok(OPExecResult::MessageDropped),
                    EpochResult::MessageQueued | EpochResult::QueueMessage => {
                        Ok(OPExecResult::MessageQueued)
                    }
                    EpochResult::MessageProcessed => {
                        let decision =
                            Decision::decision_info_from_message(self.epoch_num, message);

                        Ok(OPExecResult::ProgressedDecision(
                            DecisionsAhead::Ignore,
                            MaybeVec::from_one(decision),
                        ))
                    }
                    EpochResult::Finalized => {
                        todo!()
                    }
                }
            }
        }
    }

    fn install_seq_no(&mut self, seq_no: SeqNo) -> Result<()> {
        match seq_no.index(self.epoch_num) {
            Either::Left(_) => {
                warn!(
                    "Tried to install an older seq no: {:?}, current: {:?}",
                    seq_no, self.epoch_num
                );

                Err!(InstallSeqNoError::InstalledOlderSeqNo {
                    current: self.epoch_num,
                    attempted: seq_no
                })
            }
            Either::Right(to_clear) => {
                for _ in 0..to_clear {
                    self.rounds.pop_front();
                }

                self.epoch_num = seq_no;

                let mut current_round_generated_count = 0;

                while self.rounds.len() <= self.watermark {
                    let new_round = DumboRound::new(
                        self.epoch_num + SeqNo::from(current_round_generated_count),
                        self.quorum_info.clone(),
                        self.threshold_keys.clone(),
                        self.request_aggregator.clone(),
                    );

                    self.rounds.push_back(new_round);
                    current_round_generated_count += 1;
                }

                Ok(())
            }
        }
    }
}

impl<CE, RQ, VR, IR, A, RP, NT> NetworkedOrderProtocolInitializer<RQ, RP, NT>
    for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    NT: OrderProtocolSendNode<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>,
{
    fn initialize(
        config: Self::Config,
        order_protocol_args: OrderingProtocolArgs<RQ, RP, NT>,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        let Dumbo1Config { threshold_keys } = config;

        let OrderingProtocolArgs(node_id, _timeout_mod, _rq_pp, batch_output, network, nodes) =
            order_protocol_args;

        let info = QuorumInfo::new(nodes.len(), Self::get_f_for_n(nodes.len()), nodes, node_id);

        let request_aggregator = Arc::new(RequestAggregator::new(batch_output, info.clone()));

        Ok(Self {
            epoch_num: SeqNo::from(0),
            quorum_info: info,
            threshold_keys,
            watermark: 1,
            request_aggregator,
            network,
            rounds: VecDeque::new(),
        })
    }
}

impl<CE, RQ, VR, IR, A, NT> Debug for Dumbo<CE, RQ, VR, IR, A, NT>
where
    RQ: SerMsg + SessionBased + Debug,
    VR: ReliableBroadcast<DumboRQ<RQ>>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dumbo(epoch_num: {:?}, rounds: {:?})",
            self.epoch_num, self.rounds
        )
    }
}

#[derive(Debug, Error)]
pub enum InstallSeqNoError {
    #[error("Attempted to install an older seq no. Current: {current:?}, Attempted: {attempted:?}")]
    InstalledOlderSeqNo { current: SeqNo, attempted: SeqNo },
}
