use crate::dumbo1::message::{DumboMessage, DumboMessageType, DumboMessageTypeDiscriminants};
use atlas_common::collections::{HashMap, LinkedHashMap};
use atlas_common::node_id::NodeId;
use atlas_communication::message::StoredMessage;
use std::sync::Arc;
use strum::{IntoDiscriminant, IntoEnumIterator};

#[derive(Debug, PartialEq, Eq, Hash)]
struct PendingMessageKey(NodeId, DumboMessageTypeDiscriminants);

#[derive(Debug)]
pub(super) struct PendingMessages<M> {
    nodes_with_pending_messages: LinkedHashMap<NodeId, usize>,
    messages_by_node: HashMap<PendingMessageKey, Vec<M>>,
}

impl<M> PendingMessages<M> {
    pub(super) fn nodes_with_pending_messages(&self) -> impl Iterator<Item = NodeId> {
        self.nodes_with_pending_messages
            .iter()
            .map(|(node_id, _)| node_id)
            .copied()
    }

    pub(super) fn add_message(&mut self, message: M)
    where
        M: TDumboOneMessageType,
    {
        let message_key = PendingMessageKey(message.owner_id(), message.dumbo_message_type());

        let current_message_count = self
            .nodes_with_pending_messages
            .entry(message.owner_id())
            .or_insert(0);

        *current_message_count = current_message_count.overflowing_add(1).0;

        self.messages_by_node
            .entry(message_key)
            .or_default()
            .push(message);
    }

    pub(super) fn pop_message_by_type_and_owner(
        &mut self,
        owner_id: NodeId,
        message_type: DumboMessageTypeDiscriminants,
    ) -> Option<M> {
        let message_key = PendingMessageKey(owner_id, message_type);

        self.messages_by_node
            .get_mut(&message_key)
            .map(Vec::pop)
            .flatten()
    }

    pub(super) fn discard_messages_by_type_and_owner(
        &mut self,
        owner_id: NodeId,
        message_type: DumboMessageTypeDiscriminants,
    ) {
        let message_key = PendingMessageKey(owner_id, message_type);

        if let Some(messages) = self.messages_by_node.remove(&message_key) {
            if let Some(count) = self.nodes_with_pending_messages.get_mut(&owner_id) {
                *count = count.saturating_sub(messages.len());
                if *count == 0 {
                    self.nodes_with_pending_messages.remove(&owner_id);
                }
            }
        }
    }

    pub(super) fn discard_all_messages_by_owner(&mut self, owner_id: NodeId) {
        self.nodes_with_pending_messages.remove(&owner_id);

        DumboMessageTypeDiscriminants::iter().for_each(|discriminant| {
            let message_key = PendingMessageKey(owner_id, discriminant);
            self.messages_by_node.remove(&message_key);
        })
    }
}

impl<M> Default for PendingMessages<M> {
    fn default() -> Self {
        Self {
            nodes_with_pending_messages: LinkedHashMap::default(),
            messages_by_node: HashMap::default(),
        }
    }
}

/// Information about the message type contained in a DumboOne message
trait TDumboOneMessageType {
    fn owner_id(&self) -> NodeId;

    fn dumbo_message_type(&self) -> DumboMessageTypeDiscriminants;
}

impl<RBM, IRBM, AM, CEM> TDumboOneMessageType for DumboMessage<RBM, IRBM, AM, CEM> {
    fn owner_id(&self) -> NodeId {
        match self.message_type() {
            DumboMessageType::ReliableBroadcast(owner, _)
            | DumboMessageType::IndexReliableBroadcast(owner, _)
            | DumboMessageType::AsyncBinaryAgreement(owner, _) => *owner,
            _ => unreachable!("CommitteeElectionMessage has no owner id"),
        }
    }

    fn dumbo_message_type(&self) -> DumboMessageTypeDiscriminants {
        self.message_type().discriminant()
    }
}

impl<RBM, IRBM, AM, CEM> TDumboOneMessageType
    for Arc<StoredMessage<DumboMessage<RBM, IRBM, AM, CEM>>>
{
    fn owner_id(&self) -> NodeId {
        self.message().owner_id()
    }

    fn dumbo_message_type(&self) -> DumboMessageTypeDiscriminants {
        self.message().dumbo_message_type()
    }
}
