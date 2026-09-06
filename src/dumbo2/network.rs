use crate::dumbo2::message::{Dumbo2Message, Dumbo2MessageType, Dumbo2Serialization};
use crate::mvba::MVBASendNode;
use crate::prbc::PRBCSendNode;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::phantom::FPhantom;
use atlas_common::serialization_helper::SerMsg;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::sync::Arc;

/// Adapts an outer [`OrderProtocolSendNode`] into a [`PRBCSendNode`] for a
/// single owner's PRBC instance, tagging every outgoing message with that
/// owner (mirrors `dumbo1::network::SendNodeWrapperRef`).
pub(super) struct PrbcSendNodeWrapperRef<'a, RQ, PM, MM, NT> {
    current_round: SeqNo,
    owner: NodeId,
    inner: &'a Arc<NT>,
    _phantom: FPhantom<(RQ, PM, MM)>,
}

impl<'a, RQ, PM, MM, NT> PrbcSendNodeWrapperRef<'a, RQ, PM, MM, NT> {
    pub(super) fn new(current_round: SeqNo, owner: NodeId, inner: &'a Arc<NT>) -> Self {
        Self {
            current_round,
            owner,
            inner,
            _phantom: FPhantom::default(),
        }
    }
}

impl<'a, RQ, PM, MM, NT> PRBCSendNode<PM> for PrbcSendNodeWrapperRef<'a, RQ, PM, MM, NT>
where
    RQ: SerMsg,
    PM: SerMsg,
    MM: SerMsg,
    NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PM, MM>>,
{
    fn send(&self, message: PM, target: NodeId, flush: bool) -> error::Result<()> {
        let message = Dumbo2Message::new(
            self.current_round,
            Dumbo2MessageType::PRBC(self.owner, message),
        );

        self.inner.send_signed(message, target, flush)
    }

    fn broadcast<I>(&self, message: PM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = Dumbo2Message::new(
            self.current_round,
            Dumbo2MessageType::PRBC(self.owner, message),
        );

        self.inner.broadcast_signed(message, targets)
    }
}

/// Adapts an outer [`OrderProtocolSendNode`] into an [`MVBASendNode`] for the
/// round's single MVBA instance (no per-owner tag needed).
pub(super) struct MvbaSendNodeWrapperRef<'a, RQ, PM, MM, NT> {
    current_round: SeqNo,
    inner: &'a Arc<NT>,
    _phantom: FPhantom<(RQ, PM, MM)>,
}

impl<'a, RQ, PM, MM, NT> MvbaSendNodeWrapperRef<'a, RQ, PM, MM, NT> {
    pub(super) fn new(current_round: SeqNo, inner: &'a Arc<NT>) -> Self {
        Self {
            current_round,
            inner,
            _phantom: FPhantom::default(),
        }
    }
}

impl<'a, RQ, PM, MM, NT> MVBASendNode<MM> for MvbaSendNodeWrapperRef<'a, RQ, PM, MM, NT>
where
    RQ: SerMsg,
    PM: SerMsg,
    MM: SerMsg,
    NT: OrderProtocolSendNode<RQ, Dumbo2Serialization<RQ, PM, MM>>,
{
    fn send(&self, message: MM, target: NodeId, flush: bool) -> error::Result<()> {
        let message = Dumbo2Message::new(self.current_round, Dumbo2MessageType::MVBA(message));

        self.inner.send_signed(message, target, flush)
    }

    fn broadcast<I>(&self, message: MM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = Dumbo2Message::new(self.current_round, Dumbo2MessageType::MVBA(message));

        self.inner.broadcast_signed(message, targets)
    }
}
