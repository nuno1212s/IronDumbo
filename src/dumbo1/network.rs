use crate::dumbo1::message::{DumboMessage, DumboMessageType, DumboSerialization};
use crate::dumbo1::protocol::{DumboPMessage, DumboPSerialization};
use crate::rbc::ReliableBroadcastSendNode;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::marker::PhantomData;
use std::sync::Arc;

struct SendNodeWrapperRef<'a, RQ, ABA, CE, NT> {
    inner: &'a Arc<NT>,
    current_round: SeqNo,
    _phantom: PhantomData<fn(RQ, ABA, CE) -> ()>,
}

impl<'a, RQ, ABA, CE, BCM, NT> ReliableBroadcastSendNode<BCM> for SendNodeWrapperRef<'a, RQ, ABA, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.send(message, target, flush)
    }

    fn send_signed(
        &self,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.send_signed(message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.broadcast(message, targets)
    }

    fn broadcast_signed<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.broadcast_signed(message, targets)
    }
}

struct SendNodeWrapper< RQ, ABA, CE, NT> {
    inner: Arc<NT>,
    current_round: SeqNo,
    _phantom: PhantomData<fn(RQ, ABA, CE) -> ()>,
}

impl<RQ, ABA, CE, BCM, NT> ReliableBroadcastSendNode<BCM> for SendNodeWrapper<RQ, ABA, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.send(message, target, flush)
    }

    fn send_signed(
        &self,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.send_signed(message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.broadcast(message, targets)
    }

    fn broadcast_signed<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        self.inner.broadcast_signed(message, targets)
    }
}

impl<RQ, ABA, CE, NT> From<SendNodeWrapperRef<'_, RQ, ABA, CE, NT>> for SendNodeWrapper<RQ, ABA, CE, NT>
{
    fn from(value: SendNodeWrapperRef<'_, RQ, ABA, CE, NT>) -> Self {
        Self {
            inner: value.inner.clone(),
            current_round: value.current_round,
            _phantom: PhantomData,
        }
    }
}