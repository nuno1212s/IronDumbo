use crate::aba::AsyncBinaryAgreementSendNode;
use crate::committee_election::CommitteeElectionSendNode;
use crate::dumbo1::message::{DumboMessage, DumboMessageType, DumboSerialization};
use crate::rbc::ReliableBroadcastSendNode;
use anyhow::anyhow;
use atlas_common::error;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use atlas_common::phantom::FPhantom;
use atlas_common::serialization_helper::{Ser, SerMsg};
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use std::marker::PhantomData;
use std::sync::Arc;

struct SendNode<RQ, ABA, BCM, IBCM, CE> {
    current_round: SeqNo,
    protocol_owner: NodeId,
    _phantom: FPhantom<(RQ, ABA, BCM, IBCM, CE)>,
}

impl<RQ, ABA, BCM, IBCM, CE> SendNode<RQ, ABA, BCM, IBCM, CE>
where
    RQ: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
{
    fn send_rbc<NT>(
        &self,
        node: &NT,
        message: BCM,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()>
    where
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(self.protocol_owner, message),
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
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(self.protocol_owner, message),
        );

        node.send_signed(message, target, flush)
    }

    fn broadcast_rbc<I, NT>(&self, node: &NT, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(self.protocol_owner, message),
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
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::ReliableBroadcast(self.protocol_owner, message),
        );

        node.broadcast_signed(message, targets)
    }

    fn broadcast_aba<I, NT>(&self, node: &NT, message: ABA, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::AsyncBinaryAgreement(self.protocol_owner, message),
        );

        node.broadcast(message, targets)
    }

    fn send_ce_msg_signed<NT>(
        &self,
        node: &NT,
        message: CE,
        target: NodeId,
        flush: bool,
    ) -> error::Result<()>
    where
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::CommitteeElectionMessage(message),
        );

        node.send_signed(message, target, flush)
    }

    fn broadcast_ce_msg<I, NT>(&self, node: &NT, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
        NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
    {
        let message = DumboMessage::new(
            self.current_round,
            DumboMessageType::CommitteeElectionMessage(message),
        );

        node.broadcast_signed(message, targets)
    }
}

pub(super) struct SendNodeWrapperRef<'a, RQ, ABA, BCM, IBCM, CE, NT> {
    inner: &'a Arc<NT>,
    inner_node: SendNode<RQ, ABA, BCM, IBCM, CE>,
}

impl<'a, RQ, ABA, BCM, IBCM, CE, NT> SendNodeWrapperRef<'a, RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
{
    pub(super) fn new(current_round: SeqNo, protocol_owner: NodeId, inner: &'a Arc<NT>) -> Self {
        Self {
            inner,
            inner_node: SendNode {
                current_round,
                protocol_owner,
                _phantom: PhantomData,
            },
        }
    }
}

impl<'a, RQ, BCM, IBCM, ABA, CE, NT> AsyncBinaryAgreementSendNode<ABA>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn broadcast_message<I>(&self, message: ABA, target: I) -> error::Result<()>
    where
        I: Iterator<Item = NodeId>,
        ABA: SerMsg,
    {
        //TODO: Improve error system
        self.inner_node
            .broadcast_aba(&**self.inner, message, target)
            .map_err(|failed| anyhow!("Failed to broadcast to some nodes: {:?}", failed))
    }
}

impl<'a, RQ, BCM, IBCM, ABA, CE, NT> CommitteeElectionSendNode<CE>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn send(&self, message: CE, target: NodeId, flush: bool) -> error::Result<()> {
        self.inner_node
            .send_ce_msg_signed(&**self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_ce_msg(&**self.inner, message, targets.into_iter())
    }
}

impl<'a, RQ, ABA, CE, BCM, IBCM, NT> ReliableBroadcastSendNode<BCM>
    for SendNodeWrapperRef<'a, RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc_signed::<NT>(&*self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc_signed::<I, NT>(&*self.inner, message, targets)
    }
}

pub(super) struct SendNodeWrapper<RQ, ABA, BCM, IBCM, CE, NT> {
    inner: Arc<NT>,
    inner_node: SendNode<RQ, ABA, BCM, IBCM, CE>,
}

impl<RQ, ABA, CE, BCM, IBCM, NT> ReliableBroadcastSendNode<BCM> for SendNodeWrapper<RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    ABA: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn send(&self, message: BCM, target: NodeId, flush: bool) -> atlas_common::error::Result<()> {
        self.inner_node
            .send_rbc_signed::<NT>(&*self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: BCM, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_rbc_signed::<I, NT>(&*self.inner, message, targets)
    }
}

impl<RQ, ABA, CE, BCM, IBCM, NT> AsyncBinaryAgreementSendNode<ABA>
    for SendNodeWrapper<RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn broadcast_message<I>(&self, message: ABA, target: I) -> error::Result<()>
    where
        I: Iterator<Item = NodeId>,
        ABA: SerMsg,
    {
        self.inner_node
            .broadcast_aba(&*self.inner, message, target)
            .map_err(|failed| anyhow!("Failed to broadcast to some nodes: {:?}", failed))
    }
}

impl<RQ, ABA, BCM, IBCM, CE, NT> CommitteeElectionSendNode<CE> for SendNodeWrapper<RQ, ABA, BCM, IBCM, CE, NT>
where
    RQ: SerMsg,
    ABA: SerMsg,
    BCM: SerMsg,
    IBCM: SerMsg,
    CE: SerMsg,
    NT: OrderProtocolSendNode<RQ, DumboSerialization<RQ, BCM, IBCM, ABA, CE>>,
{
    fn send(&self, message: CE, target: NodeId, flush: bool) -> error::Result<()> {
        self.inner_node
            .send_ce_msg_signed(&*self.inner, message, target, flush)
    }

    fn broadcast<I>(&self, message: CE, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: IntoIterator<Item = NodeId>,
    {
        self.inner_node
            .broadcast_ce_msg(&*self.inner, message, targets.into_iter())
    }
}

impl<RQ, ABA, BCM, IBCM, CE, NT> From<SendNodeWrapperRef<'_, RQ, ABA, BCM, IBCM, CE, NT>>
    for SendNodeWrapper<RQ, ABA, BCM, IBCM, CE, NT>
{
    fn from(value: SendNodeWrapperRef<'_, RQ, ABA, BCM, IBCM, CE, NT>) -> Self {
        Self {
            inner: value.inner.clone(),
            inner_node: value.inner_node,
        }
    }
}
