use crate::aba::ABAProtocol;
use crate::committee_election::CommitteeElectionProtocol;
use crate::dumbo1::epoch::DumboRound;
use crate::dumbo1::message::DumboSerialization;
use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::ReliableBroadcast;
use atlas_common::error::Result;
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage;
use atlas_core::ordering_protocol::{
    OPExResult, OPResult, OrderProtocolTolerance, OrderingProtocol, ShareableConsensusMessage,
};
use atlas_core::timeouts::timeout::{ModTimeout, TimeoutableMod};
use getset::{Getters, Setters};
use std::collections::VecDeque;
use std::ops::Index;
use std::sync::{Arc, LazyLock};

/// The name of the Dumbo1 module.
/// Used for logging and metrics.
const DUMBO1_MOD_NAME: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("Dumbo1"));

pub type IndexType = usize;

pub type DumboPSerialization<
    RQ,
    R: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
> = DumboSerialization<RQ, R::ReliableBroadcastMessage, IR::ReliableBroadcastMessage, A::AsyncBinaryMessage, CE::Message>;

#[allow(dead_code)]
pub(super) type DumboPMessage<
    RQ: 'static,
    R: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
> = <DumboPSerialization<RQ, R, IR, A, CE> as OrderingProtocolMessage<RQ>>::ProtocolMessage;

/// An instance of the Dumbo protocol.
/// Holds the state of the protocol for a specific epoch.
/// Tracks the state of each node in the protocol.
#[derive(Debug, Getters, Setters)]
pub struct Dumbo<CE, RQ, VR, IR, A> {
    // The current epoch number.
    epoch_num: SeqNo,

    // The current quorum information
    quorum_info: QuorumInfo,

    // The rounds of the dumbo protocol.
    rounds: VecDeque<DumboRound<CE, RQ, VR, IR, A>>,
}

impl<CE, RQ, VR, IR, A> Dumbo<CE, RQ, VR, IR, A> {
    pub fn new(quorum_info: QuorumInfo) -> Self {
        Self {
            epoch_num: SeqNo::ONE,
            quorum_info,
            rounds: VecDeque::new(),
        }
    }
}

impl<CE, RQ, VR, IR, A> OrderProtocolTolerance for Dumbo<CE, RQ, VR, IR, A>
where
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<RQ>,
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

impl<CE, RQ, VR, IR, A> Orderable for Dumbo<CE, RQ, VR, IR, A>
where
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<RQ>,
    RQ: SerMsg,
{
    fn sequence_number(&self) -> SeqNo {
        self.epoch_num
    }
}

impl<CE, RQ, VR, IR, A> TimeoutableMod<OPExResult<RQ, DumboPSerialization<RQ, VR, IR, A, CE>>>
    for Dumbo<CE, RQ, VR, IR, A>
where
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<usize>,
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

impl<CE, RQ, VR, IR, A> OrderingProtocol<RQ> for Dumbo<CE, RQ, VR, IR, A>
where
    RQ: SerMsg,
    VR: ReliableBroadcast<RQ>,
    IR: ReliableBroadcast<IndexType>,
    A: ABAProtocol,
    CE: CommitteeElectionProtocol,
{
    type Config = ();
    type Serialization = DumboPSerialization<RQ, VR, IR, A, CE>;

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
        todo!()
    }

    fn process_message(
        &mut self,
        message: ShareableConsensusMessage<RQ, Self::Serialization>,
    ) -> Result<OPExResult<RQ, Self::Serialization>> {
        todo!()
    }

    fn install_seq_no(&mut self, seq_no: SeqNo) -> Result<()> {
        todo!()
    }
}
