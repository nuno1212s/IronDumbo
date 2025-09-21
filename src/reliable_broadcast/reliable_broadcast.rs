use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::ReliableBroadcastSendNode;
use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use atlas_common::collections::HashSet;
use atlas_common::crypto::hash::Digest;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use getset::{Getters, MutGetters};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReliableBroadcastState {
    Init,
    /// We have received a SEND message and are waiting for echoes.
    Proposed,
    /// We have received enough echoes and are waiting for readies.
    Echoed,
    /// We have received enough readies and are ready to finalize.
    Ready,
}

/// An instance of the reliable broadcast protocol.
///
/// It holds the state of the protocol for a specific sender and quorum.
/// It tracks the proposed messages, message tracking information, and pending messages.
///
#[derive(Debug, Getters)]
pub(super) struct ReliableBroadcastInstance<RQ> {
    #[get = "pub(super)"]
    sender: NodeId,
    #[get = ""]
    quorum_info: QuorumInfo,
    #[get = ""]
    proposed_messages: Option<(Vec<StoredMessage<RQ>>, Digest)>,
    #[get = ""]
    message_tracking: MessageTracking,
    #[get = ""]
    reliable_broadcast_state: ReliableBroadcastState,
    #[get = ""]
    pending_messages: PendingMessages<RQ>,
}

impl<RQ> ReliableBroadcastInstance<RQ>
where
    RQ: SerMsg,
{
    pub fn new(sender: NodeId, quorum_info: QuorumInfo) -> Self {
        Self {
            sender,
            quorum_info,
            proposed_messages: None,
            message_tracking: MessageTracking::default(),
            reliable_broadcast_state: ReliableBroadcastState::Init,
            pending_messages: PendingMessages::<RQ>::default(),
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        match self.reliable_broadcast_state {
            ReliableBroadcastState::Proposed => !self.pending_messages.echoes.is_empty(),
            ReliableBroadcastState::Echoed => !self.pending_messages.readies.is_empty(),
            ReliableBroadcastState::Init | ReliableBroadcastState::Ready => false,
        }
    }

    pub(super) fn poll(&mut self) -> Option<StoredMessage<ReliableBroadcastMessage<RQ>>> {
        match self.reliable_broadcast_state {
            ReliableBroadcastState::Proposed => self.pending_messages.pop_echo(),
            ReliableBroadcastState::Echoed => self.pending_messages.pop_ready(),
            ReliableBroadcastState::Ready | ReliableBroadcastState::Init => None,
        }
    }

    /// Processes a message received from the network or queued in the pending messages.
    pub(super) fn process_message<NT>(
        &mut self,
        sys_msg: StoredMessage<ReliableBroadcastMessage<RQ>>,
        network: &Arc<NT>,
    ) -> ReliableBroadcastResult<RQ>
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage<RQ>>,
    {
        let (header, message) = sys_msg.clone().into_inner();

        match message {
            ReliableBroadcastMessage::Send(messages, digest)
                if self.proposed_messages.is_none()
                    && matches!(self.reliable_broadcast_state, ReliableBroadcastState::Init) =>
            {
                self.proposed_messages = Some((messages, digest));

                self.broadcast_echo_message(digest, network);

                self.reliable_broadcast_state = ReliableBroadcastState::Proposed;

                ReliableBroadcastResult::Progressed(sys_msg)
            }
            ReliableBroadcastMessage::Send(_, _) => {
                warn!("Received a send message when already proposed messages exist, ignoring.");

                ReliableBroadcastResult::MessageIgnored
            }
            ReliableBroadcastMessage::Echo(digest)
                if self.get_current_digest() == Some(digest)
                    && matches!(
                        self.reliable_broadcast_state,
                        ReliableBroadcastState::Proposed
                    ) =>
            {
                self.message_tracking.handle_received_echo(header.from());

                if self.message_tracking.received_echoes().len()
                    >= self.quorum_info().quorum_size() - self.quorum_info.f()
                    && !self.message_tracking.sent_echo()
                {
                    self.reliable_broadcast_state = ReliableBroadcastState::Echoed;
                    self.broadcast_ready_message(digest, network);

                    self.message_tracking.set_sent_echo();
                }

                ReliableBroadcastResult::Progressed(sys_msg)
            }
            ReliableBroadcastMessage::Ready(digest)
                if self.get_current_digest() == Some(digest)
                    && matches!(
                        self.reliable_broadcast_state,
                        ReliableBroadcastState::Echoed
                    ) =>
            {
                self.message_tracking.handle_received_ready(header.from());

                if self.message_tracking.received_readies().len() > 2 * self.quorum_info.f()
                    && !self.message_tracking.sent_ready()
                {
                    self.reliable_broadcast_state = ReliableBroadcastState::Ready;
                    self.message_tracking.set_sent_ready();

                    return ReliableBroadcastResult::Finalized;
                }

                ReliableBroadcastResult::Progressed(sys_msg)
            }
            _ => {
                self.pending_messages.queue_message(sys_msg);

                ReliableBroadcastResult::MessageQueued
            }
        }
    }

    fn get_current_digest(&self) -> Option<Digest> {
        self.proposed_messages.as_ref().map(|(_, digest)| *digest)
    }

    fn broadcast_echo_message<NT>(&self, digest: Digest, network: &Arc<NT>)
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage<RQ>>,
    {
        let message = ReliableBroadcastMessage::Echo(digest);

        if let Err(err) =
            network.broadcast(message, self.quorum_info.quorum_members().iter().cloned())
        {
            warn!("Failed to broadcast echo message: {err:?}");
        }
    }

    fn broadcast_ready_message<NT>(&self, digest: Digest, network: &Arc<NT>)
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage<RQ>>,
    {
        let message = ReliableBroadcastMessage::Ready(digest);

        if let Err(err) =
            network.broadcast(message, self.quorum_info.quorum_members().iter().cloned())
        {
            warn!("Failed to broadcast ready message: {err:?}");
        }
    }

    pub(super) fn finalize(
        self,
    ) -> Result<(Vec<StoredMessage<RQ>>, Digest), ReliableBroadcastError> {
        if matches!(self.reliable_broadcast_state, ReliableBroadcastState::Ready) {
            // We can finalize the broadcast
            if let Some((messages, digest)) = self.proposed_messages {
                Ok((messages, digest))
            } else {
                Err(ReliableBroadcastError::NoProposedMessages)
            }
        } else {
            warn!(
                "Attempted to finalize reliable broadcast in an invalid state: {:?}",
                self.reliable_broadcast_state
            );
            Err(ReliableBroadcastError::NotReadyToFinalize)
        }
    }
}

pub(super) enum ReliableBroadcastResult<RQ> {
    MessageIgnored,
    MessageQueued,
    Progressed(StoredMessage<ReliableBroadcastMessage<RQ>>),
    Finalized,
}

#[derive(MutGetters)]
struct PendingMessages<M> {
    #[get_mut]
    echoes: VecDeque<StoredMessage<ReliableBroadcastMessage<M>>>,
    #[get_mut]
    readies: VecDeque<StoredMessage<ReliableBroadcastMessage<M>>>,
}

impl<M> PendingMessages<M> {
    fn queue_message(&mut self, message: StoredMessage<ReliableBroadcastMessage<M>>) {
        match message.message() {
            ReliableBroadcastMessage::Echo(_) => {
                self.echoes.push_back(message);
            }
            ReliableBroadcastMessage::Ready(_) => {
                self.readies.push_back(message);
            }
            _ => {
                unreachable!("Only Echo and Ready messages should be queued here")
            }
        }
    }

    fn pop_echo(&mut self) -> Option<StoredMessage<ReliableBroadcastMessage<M>>> {
        self.echoes.pop_front()
    }

    fn pop_ready(&mut self) -> Option<StoredMessage<ReliableBroadcastMessage<M>>> {
        self.readies.pop_front()
    }
}

impl<M> Default for PendingMessages<M> {
    fn default() -> Self {
        Self {
            echoes: VecDeque::new(),
            readies: VecDeque::new(),
        }
    }
}

impl<M> Debug for PendingMessages<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingMessages")
            .field("echoes", &self.echoes.len())
            .field("readies", &self.readies.len())
            .finish()
    }
}

#[derive(Default, Debug, Getters)]
struct MessageTracking {
    #[get = "pub(super)"]
    received_echoes: HashSet<NodeId>,
    #[get = "pub(super)"]
    received_readies: HashSet<NodeId>,
    #[get = "pub(super)"]
    sent_echo: bool,
    #[get = "pub(super)"]
    sent_ready: bool,
}

impl MessageTracking {
    fn handle_received_echo(&mut self, from: NodeId) {
        self.received_echoes.insert(from);
    }

    fn handle_received_ready(&mut self, from: NodeId) {
        self.received_readies.insert(from);
    }

    fn set_sent_echo(&mut self) {
        self.sent_echo = true;
    }

    fn set_sent_ready(&mut self) {
        self.sent_ready = true;
    }
}

#[derive(Debug, Error)]
pub enum ReliableBroadcastError {
    #[error("Failed to finalize reliable broadcast")]
    NoProposedMessages,
    #[error("Reliable broadcast instance is not ready to finalize")]
    NotReadyToFinalize,
}
