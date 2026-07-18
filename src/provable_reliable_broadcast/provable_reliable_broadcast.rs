use crate::prbc::{PRBCProtocol, PRBCResult, PRBCSendNode};
use crate::provable_reliable_broadcast::messages::PRBCMessage;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rbc::{ReliableBroadcast, ReliableBroadcastSendNode};
use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use crate::reliable_broadcast::reliable_broadcast::{
    ReliableBroadcastError, ReliableBroadcastInstance, ReliableBroadcastResult,
};
use atlas_common::collections::HashMap;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{
    CombineSignatureError, CombinedSignature, PartialSignature,
};
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PRBCState {
    /// The inner reliable broadcast (SEND/ECHO/READY) hasn't finalized yet.
    Broadcasting,
    /// The inner broadcast finalized; collecting `Done` shares to combine
    /// into the threshold-signature proof.
    CollectingDone,
    /// A combined signature (ours or adopted from a peer's `Finish`) has
    /// been obtained.
    Finished,
}

/// Wraps a [`PRBCSendNode`] so the composed [`ReliableBroadcastInstance`] can
/// send through it, translating RBC-level messages into the corresponding
/// PRBC message variants (mirrors `dumbo1::network::SendNodeWrapperRef`).
struct RbcNetworkAdapter<'a, NT> {
    inner: &'a NT,
}

impl<'a, RQ, NT> ReliableBroadcastSendNode<ReliableBroadcastMessage<RQ>>
    for RbcNetworkAdapter<'a, NT>
where
    RQ: SerMsg,
    NT: PRBCSendNode<PRBCMessage<RQ>>,
{
    fn send(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.inner
            .send(rbc_message_to_prbc_message(message), target, flush)
    }

    fn broadcast<I>(
        &self,
        message: ReliableBroadcastMessage<RQ>,
        targets: I,
    ) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.inner
            .broadcast(rbc_message_to_prbc_message(message), targets)
    }
}

fn rbc_message_to_prbc_message<RQ>(message: ReliableBroadcastMessage<RQ>) -> PRBCMessage<RQ> {
    match message {
        ReliableBroadcastMessage::Send(value) => PRBCMessage::Send(value),
        ReliableBroadcastMessage::Echo(digest) => PRBCMessage::Echo(digest),
        ReliableBroadcastMessage::Ready(digest) => PRBCMessage::Ready(digest),
    }
}

/// An instance of the Provable Reliable Broadcast protocol: composes a
/// [`ReliableBroadcastInstance`] for the SEND/ECHO/READY phases, then adds a
/// Done/Finish threshold-signature phase on top, producing a combined
/// signature that proves at least `f+1` correct nodes received the value.
pub(crate) struct ProvableReliableBroadcastInstance<RQ> {
    inner: ReliableBroadcastInstance<RQ>,
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    state: PRBCState,
    finalized_digest: Option<Digest>,
    done_shares: HashMap<NodeId, PartialSignature>,
    combined_signature: Option<CombinedSignature>,
    pending: VecDeque<StoredMessage<PRBCMessage<RQ>>>,
}

impl<RQ> ProvableReliableBroadcastInstance<RQ>
where
    RQ: SerMsg,
{
    fn on_inner_finalized<NT>(&mut self, network: &NT) -> Result<PRBCResult, PRBCError>
    where
        NT: PRBCSendNode<PRBCMessage<RQ>>,
    {
        let digest = self
            .inner
            .get_current_digest()
            .expect("a finalized inner broadcast always has a digest");

        self.finalized_digest = Some(digest);
        self.state = PRBCState::CollectingDone;

        let own_id = self.quorum_info.own_node_id();
        let own_share = self
            .threshold_keys
            .private_key()
            .partially_sign(digest.as_ref());

        self.done_shares.insert(own_id, own_share.clone());

        let _ = network.broadcast(
            PRBCMessage::Done(own_share),
            self.quorum_info.quorum_members().iter().cloned(),
        );

        self.try_combine(network)
    }

    fn try_combine<NT>(&mut self, network: &NT) -> Result<PRBCResult, PRBCError>
    where
        NT: PRBCSendNode<PRBCMessage<RQ>>,
    {
        // The keyset is generated with `threshold = f`, so `f+1` shares are
        // required to combine (see `testing::fixtures::make_keyset`).
        if self.done_shares.len() <= self.quorum_info.f() {
            return Ok(PRBCResult::Processed);
        }

        let digest = self
            .finalized_digest
            .expect("digest is set once CollectingDone is reached");

        let shares = self
            .done_shares
            .iter()
            .map(|(node, sig)| (node.0 as usize, sig));

        let combined = self
            .threshold_keys
            .public_key()
            .combine_signatures(shares)
            .map_err(PRBCError::CombineFailed)?;

        self.combined_signature = Some(combined.clone());
        self.state = PRBCState::Finished;

        let _ = network.broadcast(
            PRBCMessage::Finish(combined.clone()),
            self.quorum_info.quorum_members().iter().cloned(),
        );

        let _ = digest;

        Ok(PRBCResult::Finalized(combined))
    }

    fn handle_done<NT>(
        &mut self,
        from: NodeId,
        share: PartialSignature,
        original: StoredMessage<PRBCMessage<RQ>>,
        network: &NT,
    ) -> Result<PRBCResult, PRBCError>
    where
        NT: PRBCSendNode<PRBCMessage<RQ>>,
    {
        match self.state {
            PRBCState::Broadcasting => {
                self.pending.push_back(original);
                Ok(PRBCResult::MessageQueued)
            }
            PRBCState::CollectingDone => {
                let digest = self
                    .finalized_digest
                    .expect("digest is set once CollectingDone is reached");

                if self
                    .threshold_keys
                    .public_key()
                    .verify_partial_signature(from.0 as usize, digest.as_ref(), &share)
                    .is_err()
                {
                    warn!("Rejecting invalid PRBC Done share from {from:?}");
                    return Ok(PRBCResult::MessageIgnored);
                }

                self.done_shares.insert(from, share);

                self.try_combine(network)
            }
            PRBCState::Finished => Ok(PRBCResult::MessageIgnored),
        }
    }

    fn handle_finish(
        &mut self,
        signature: CombinedSignature,
        original: StoredMessage<PRBCMessage<RQ>>,
    ) -> Result<PRBCResult, PRBCError> {
        match self.state {
            PRBCState::Broadcasting => {
                self.pending.push_back(original);
                Ok(PRBCResult::MessageQueued)
            }
            PRBCState::CollectingDone => {
                let digest = self
                    .finalized_digest
                    .expect("digest is set once CollectingDone is reached");

                if self
                    .threshold_keys
                    .public_key()
                    .verify(digest.as_ref(), &signature)
                    .is_err()
                {
                    warn!("Rejecting invalid PRBC Finish signature");
                    return Ok(PRBCResult::MessageIgnored);
                }

                self.combined_signature = Some(signature.clone());
                self.state = PRBCState::Finished;

                Ok(PRBCResult::Finalized(signature))
            }
            PRBCState::Finished => Ok(PRBCResult::MessageIgnored),
        }
    }
}

impl<RQ> PRBCProtocol<RQ> for ProvableReliableBroadcastInstance<RQ>
where
    RQ: SerMsg,
{
    type Message = PRBCMessage<RQ>;
    type Error = PRBCError;

    fn new(owner_id: NodeId, quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self {
        Self {
            inner: ReliableBroadcastInstance::new(owner_id, quorum_info.clone()),
            quorum_info,
            threshold_keys,
            state: PRBCState::Broadcasting,
            finalized_digest: None,
            done_shares: HashMap::default(),
            combined_signature: None,
            pending: VecDeque::new(),
        }
    }

    fn new_with_propose<NT>(
        owner_id: NodeId,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        value: RQ,
        network: &NT,
    ) -> Self
    where
        NT: PRBCSendNode<Self::Message>,
    {
        let adapter = RbcNetworkAdapter { inner: network };

        let inner = ReliableBroadcastInstance::new_with_propose(
            owner_id,
            quorum_info.clone(),
            value,
            &adapter,
        );

        Self {
            inner,
            quorum_info,
            threshold_keys,
            state: PRBCState::Broadcasting,
            finalized_digest: None,
            done_shares: HashMap::default(),
            combined_signature: None,
            pending: VecDeque::new(),
        }
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::Message>> {
        if let Some(inner_msg) = self.inner.poll() {
            let (header, msg) = inner_msg.into_inner();

            return Some(StoredMessage::new(header, rbc_message_to_prbc_message(msg)));
        }

        if !matches!(self.state, PRBCState::Broadcasting) {
            return self.pending.pop_front();
        }

        None
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<PRBCResult, Self::Error>
    where
        NT: PRBCSendNode<Self::Message>,
    {
        if matches!(self.state, PRBCState::Finished)
            && matches!(
                message.message(),
                PRBCMessage::Send(_) | PRBCMessage::Echo(_) | PRBCMessage::Ready(_)
            )
        {
            return Ok(PRBCResult::MessageIgnored);
        }

        match message.message() {
            PRBCMessage::Send(_) | PRBCMessage::Echo(_) | PRBCMessage::Ready(_) => {
                let (header, msg) = message.into_inner();

                let rbc_msg = match msg {
                    PRBCMessage::Send(value) => ReliableBroadcastMessage::Send(value),
                    PRBCMessage::Echo(digest) => ReliableBroadcastMessage::Echo(digest),
                    PRBCMessage::Ready(digest) => ReliableBroadcastMessage::Ready(digest),
                    PRBCMessage::Done(_) | PRBCMessage::Finish(_) => {
                        unreachable!("checked above that this is a Send/Echo/Ready variant")
                    }
                };

                let adapter = RbcNetworkAdapter { inner: network };
                let stored = StoredMessage::new(header, rbc_msg);

                let result = self.inner.process_message(stored, &adapter);

                match result {
                    ReliableBroadcastResult::MessageIgnored => Ok(PRBCResult::MessageIgnored),
                    ReliableBroadcastResult::MessageQueued => Ok(PRBCResult::MessageQueued),
                    ReliableBroadcastResult::Progressed(_) => Ok(PRBCResult::Processed),
                    ReliableBroadcastResult::Finalized => self.on_inner_finalized(network),
                }
            }
            PRBCMessage::Done(_) | PRBCMessage::Finish(_) => {
                let from = message.header().from();
                let original = message.clone();
                let (_, msg) = message.into_inner();

                match msg {
                    PRBCMessage::Done(share) => self.handle_done(from, share, original, network),
                    PRBCMessage::Finish(signature) => self.handle_finish(signature, original),
                    PRBCMessage::Send(_) | PRBCMessage::Echo(_) | PRBCMessage::Ready(_) => {
                        unreachable!("checked above that this is a Done/Finish variant")
                    }
                }
            }
        }
    }

    fn finalize(self) -> Result<(RQ, Digest, CombinedSignature), Self::Error> {
        if !matches!(self.state, PRBCState::Finished) {
            return Err(PRBCError::NotReadyToFinalize);
        }

        let combined_signature = self
            .combined_signature
            .expect("Finished state implies combined_signature is set");

        let (value, digest) = self.inner.finalize()?;

        Ok((value, digest, combined_signature))
    }
}

impl<RQ> Debug for ProvableReliableBroadcastInstance<RQ> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvableReliableBroadcastInstance")
            .field("state", &self.state)
            .field("done_shares", &self.done_shares.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub(crate) enum PRBCError {
    #[error("Inner reliable broadcast error: {0}")]
    Inner(#[from] ReliableBroadcastError),
    #[error("Failed to combine PRBC done shares: {0}")]
    CombineFailed(#[from] CombineSignatureError),
    #[error("Attempted to finalize a PRBC instance before it reached the Finished state")]
    NotReadyToFinalize,
}
