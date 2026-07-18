use crate::dumbo2::config::Dumbo2Config;
use crate::dumbo2::epoch::{Dumbo2Round, EpochResult};
use crate::dumbo2::message::{Dumbo2MessageType, Dumbo2Serialization};
use crate::mvba::MVBAProtocol;
use crate::prbc::PRBCProtocol;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rq_aggregator::RequestAggregator;
use atlas_common::Err;
use atlas_common::crypto::hash::Context;
use atlas_common::error::Result;
use atlas_common::maybe_vec::MaybeVec;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use atlas_core::messages::{ClientRqInfo, SessionBased};
use atlas_core::ordering_protocol::decision::{Decision, DecisionRequestBatch, DecisionRequests};
use atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage;
use atlas_core::ordering_protocol::networking::{
    NetworkedOrderProtocolInitializer, OrderProtocolSendNode,
};
use atlas_core::ordering_protocol::{
    DecisionsAhead, OPExResult, OPExecResult, OPPollResult, OPResult, OrderProtocolTolerance,
    OrderingProtocol, OrderingProtocolArgs, ShareableConsensusMessage,
};
use atlas_core::timeouts::timeout::{ModTimeout, TimeoutableMod};
use either::Either;
use getset::{Getters, Setters};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use tracing::warn;

static DUMBO2_MOD_NAME: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("Dumbo2"));

/// As in Dumbo1: batch support without complicating the individual
/// sub-protocols (PRBC), which broadcast a single opaque value.
pub(super) type DumboRQ<RQ> = Vec<StoredMessage<RQ>>;

pub(super) type ShareableDumbo2PMessage<RQ, PR, MV> =
    ShareableConsensusMessage<RQ, Dumbo2PSerialization<RQ, PR, MV>>;

pub type Dumbo2PSerialization<RQ, PR, MV> = Dumbo2Serialization<
    RQ,
    <PR as PRBCProtocol<DumboRQ<RQ>>>::Message,
    <MV as MVBAProtocol>::Message,
>;

/// An instance of the Dumbo2 protocol: PRBC + MVBA, without the
/// committee-election / IndexRBC machinery Dumbo1 uses.
#[derive(Getters, Setters)]
pub struct Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    epoch_num: SeqNo,
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    watermark: usize,
    request_aggregator: Arc<RequestAggregator<RQ>>,
    network: Arc<NT>,
    rounds: VecDeque<Dumbo2Round<RQ, PR, MV>>,
}

impl<RQ, PR, MV, NT> OrderProtocolTolerance for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    fn get_n_for_f(f: usize) -> usize {
        3 * f + 1
    }

    fn get_quorum_for_n(n: usize) -> usize {
        (n - 1) / 2
    }

    fn get_f_for_n(n: usize) -> usize {
        (n - 1) / 3
    }
}

impl<RQ, PR, MV, NT> Orderable for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    fn sequence_number(&self) -> SeqNo {
        self.epoch_num
    }
}

impl<RQ, PR, MV, NT> TimeoutableMod<OPExResult<RQ, Dumbo2PSerialization<RQ, PR, MV>>>
    for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    fn mod_name() -> Arc<str> {
        DUMBO2_MOD_NAME.clone()
    }

    fn handle_timeout(
        &mut self,
        _timeout: Vec<ModTimeout>,
    ) -> Result<OPExResult<RQ, Dumbo2PSerialization<RQ, PR, MV>>> {
        // Not yet implemented, matching Dumbo1's own current scope.
        todo!()
    }
}

impl<RQ, PR, MV, NT> OrderingProtocol<RQ> for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
    NT: OrderProtocolSendNode<RQ, Dumbo2PSerialization<RQ, PR, MV>>,
{
    type Serialization = Dumbo2PSerialization<RQ, PR, MV>;
    type Config = Dumbo2Config;

    fn handle_off_ctx_message(
        &mut self,
        _message: ShareableConsensusMessage<RQ, Self::Serialization>,
    ) {
        // Not yet implemented, matching Dumbo1's own current scope.
        todo!()
    }

    fn handle_execution_changed(&mut self, _is_executing: bool) -> Result<()> {
        // Not yet implemented, matching Dumbo1's own current scope.
        todo!()
    }

    fn poll(&mut self) -> Result<OPResult<RQ, Self::Serialization>> {
        let polled_message = self.rounds.front_mut().and_then(|round| round.poll());

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
                let Some(round) = self.rounds.get_mut(index) else {
                    return Ok(OPExecResult::MessageDropped);
                };

                let result = round.process_message(message.clone(), &self.network)?;

                match result {
                    EpochResult::MessageIgnored => Ok(OPExecResult::MessageDropped),
                    EpochResult::MessageQueued | EpochResult::QueueMessage => {
                        Ok(OPExecResult::MessageQueued)
                    }
                    EpochResult::MessageProcessed => {
                        let decision = Decision::decision_from_message(self.epoch_num, message);

                        Ok(OPExecResult::ProgressedDecision(
                            DecisionsAhead::Ignore,
                            MaybeVec::from_one(decision),
                        ))
                    }
                    EpochResult::Finalized => {
                        let final_data = self.rounds[index].take_final_data()?;

                        let requests: Vec<StoredMessage<RQ>> =
                            final_data.values().iter().flatten().cloned().collect();
                        let client_rqs: Vec<ClientRqInfo> = final_data
                            .included()
                            .iter()
                            .map(|(owner, digest, _)| {
                                ClientRqInfo::new(*digest, *owner, self.epoch_num, self.epoch_num)
                            })
                            .collect();

                        let mut digest_ctx = Context::new();
                        requests
                            .iter()
                            .for_each(|rq| digest_ctx.update(rq.header().digest().as_ref()));
                        let batch_digest = digest_ctx.finish();

                        let decision_requests = DecisionRequests::new(
                            self.epoch_num,
                            DecisionRequestBatch::new_with_batch(self.epoch_num, requests),
                            client_rqs,
                            batch_digest,
                        );

                        let decision = Decision::completed_decision_with_requests(
                            self.epoch_num,
                            decision_requests,
                        );

                        let mut new_epoch_num = self.epoch_num;
                        for _ in 0..=index {
                            new_epoch_num = new_epoch_num.next();
                        }
                        self.install_seq_no(new_epoch_num)?;

                        Ok(OPExecResult::ProgressedDecision(
                            DecisionsAhead::Ignore,
                            MaybeVec::from_one(decision),
                        ))
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
                    let round_seq = self.epoch_num + SeqNo::from(current_round_generated_count);

                    let (batch, _digest) = self.request_aggregator.get_batch_and_reset();

                    let new_round = Dumbo2Round::new(
                        round_seq,
                        self.quorum_info.clone(),
                        self.threshold_keys.clone(),
                        batch,
                        self.request_aggregator.clone(),
                        &self.network,
                    );

                    self.rounds.push_back(new_round);
                    current_round_generated_count += 1;
                }

                Ok(())
            }
        }
    }
}

impl<RQ, PR, MV, RP, NT> NetworkedOrderProtocolInitializer<RQ, RP, NT> for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
    NT: OrderProtocolSendNode<RQ, Dumbo2PSerialization<RQ, PR, MV>>,
{
    fn initialize(
        config: Self::Config,
        order_protocol_args: OrderingProtocolArgs<RQ, RP, NT>,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        let Dumbo2Config { threshold_keys } = config;

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

impl<RQ, PR, MV, NT> Debug for Dumbo2<RQ, PR, MV, NT>
where
    RQ: SerMsg + SessionBased + Debug,
    PR: PRBCProtocol<DumboRQ<RQ>>,
    MV: MVBAProtocol,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dumbo2(epoch_num: {:?}, rounds: {:?})",
            self.epoch_num, self.rounds
        )
    }
}

#[derive(Debug, Error)]
pub enum InstallSeqNoError {
    #[error("Attempted to install an older seq no. Current: {current:?}, Attempted: {attempted:?}")]
    InstalledOlderSeqNo { current: SeqNo, attempted: SeqNo },
}
