use crate::aba::AsyncBinaryAgreementSendNode;
use crate::committee_election::CommitteeElectionSendNode;
use crate::dumbo1::message::{DumboMessage, DumboMessageType, DumboSerialization};
use crate::dumbo1::protocol::{DumboPMessage, DumboPSerialization};
use crate::rbc::ReliableBroadcastSendNode;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::serialization_helper::SerMsg;
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::marker::PhantomData;
use std::sync::Arc;

struct SendNode<RQ, ABA, BCM, CE> {
    current_round: SeqNo,
    _phantom: PhantomData<fn(RQ, ABA, BCM, CE) -> ()>,
}

impl<RQ, ABA, BCM, CE> SendNode<RQ, ABA, BCM, CE>
where
    RQ: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
{
    fn send_rbc<NT>(
        &self,
        node: &NT,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()>
    where
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
        BCM: SerMsg,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        node.send(message, target, flush)
    }

    fn send_rbc_signed<NT>(
        &self,
        node: &NT,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()>
    where
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
        BCM: SerMsg,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        node.send_signed(message, target, flush)
    }

    fn broadcast_rbc<I, NT>(&self, node: &NT, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
        BCM: SerMsg,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        node.broadcast(message, targets)
    }

    fn broadcast_rbc_signed<I, NT>(
        &self,
        node: &NT,
        message: BCM,
        targets: I,
    ) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
        BCM: SerMsg,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(message),
        );

        node.broadcast_signed(message, targets)
    }
}
pub(super) struct SendNodeWrapperRef<'a, RQ, ABA, BCM, CE, NT> {
    inner: &'a Arc<NT>,
    inner_node: SendNode<RQ, ABA, BCM, CE>,
}

impl<'a, RQ, ABA, BCM, CE, NT> SendNodeWrapperRef<'a, RQ, ABA, BCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
{
    pub(super) fn new(current_round: SeqNo, inner: &'a Arc<NT>) -> Self {
        Self {
            inner,
            inner_node: SendNode {
                current_round,
                _phantom: PhantomData,
            },
        }
    }
}

impl<'a, RQ, BCM, ABA, CE, NT> AsyncBinaryAgreementSendNode<ABA>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn broadcast_message<I>(&self, message: ABA, target: I) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
        ABA: SerMsg,
    {
        todo!()
    }
}

impl<'a, RQ, BCM, ABA, CE, NT> CommitteeElectionSendNode<CE>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn send(&self, message: CE, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        todo!()
    }

    fn send_signed(
        &self,
        message: CE,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        todo!()
    }

    fn broadcast<I>(&self, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>,
    {
        todo!()
    }
}

impl<'a, RQ, ABA, CE, BCM, NT> ReliableBroadcastSendNode<BCM>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc::<NT>(&*self.inner, message, target, flush)
    }

    fn send_signed(
        &self,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc_signed::<NT>(&*self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc::<I, NT>(&*self.inner, message, targets)
    }

    fn broadcast_signed<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc_signed::<I, NT>(&*self.inner, message, targets)
    }
}

impl<RA, ABA, BCM, CE, NT> SendNodeWrapperRef<'_, RA, ABA, BCM, CE, NT>
where
    RA: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
{
    pub(super) fn current_round(&self) -> SeqNo {
        self.inner_node.current_round
    }
}

pub(super) struct SendNodeWrapper<RQ, ABA, BCM, CE, NT> {
    inner: Arc<NT>,
    inner_node: SendNode<RQ, ABA, BCM, CE>,
}

impl<RQ, ABA, CE, BCM, NT> ReliableBroadcastSendNode<BCM> for SendNodeWrapper<RQ, ABA, BCM, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc::<NT>(&*self.inner, message, target, flush)
    }

    fn send_signed(
        &self,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc_signed::<NT>(&*self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc::<I, NT>(&*self.inner, message, targets)
    }

    fn broadcast_signed<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc_signed::<I, NT>(&*self.inner, message, targets)
    }
}

impl<RQ, ABA, BCM, CE, NT> From<SendNodeWrapperRef<'_, RQ, ABA, BCM, CE, NT>>
    for SendNodeWrapper<RQ, ABA, BCM, CE, NT>
{
    fn from(value: SendNodeWrapperRef<'_, RQ, ABA, BCM, CE, NT>) -> Self {
        Self {
            inner: value.inner.clone(),
            inner_node: value.inner_node,
        }
    }
}
