use crate::cbc::{CBCProtocol, CBCResult, CBCSendNode};
use crate::consistent_broadcast::messages::CBCMessage;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::collections::HashMap;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::{
    CombineSignatureError, CombinedSignature, PartialSignature,
};
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::fmt::{Debug, Formatter};
use thiserror::Error;
use tracing::warn;

/// Content digest CBC binds its threshold signature to. Exposed so callers
/// (e.g. MVBA, verifying a `VVote`'s embedded `(value, signature)` proof
/// without running a full `CBCProtocol` instance) can independently
/// recompute the exact same digest a `Finish` signature was produced over.
pub(crate) fn compute_digest<V: SerMsg>(value: &V) -> Digest {
    let serialized = bincode::serde::encode_to_vec(value, bincode::config::standard())
        .expect("Failed to serialize CBC value");

    let mut ctx = Context::new();
    ctx.update(&serialized);
    ctx.finish()
}

enum CBCState<V> {
    AwaitingSend,
    AwaitingFinish {
        value: V,
        digest: Digest,
    },
    Finished {
        value: V,
        digest: Digest,
        signature: CombinedSignature,
    },
}

/// An instance of the Consistent Broadcast protocol (Algorithm 6).
pub(crate) struct ConsistentBroadcastInstance<V> {
    owner_id: NodeId,
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    state: CBCState<V>,
    /// Only meaningful when `own_node_id == owner_id` -- only the owner
    /// ever collects echoes.
    echo_shares: HashMap<NodeId, PartialSignature>,
}

impl<V> ConsistentBroadcastInstance<V>
where
    V: SerMsg,
{
    fn propose_value<NT>(&self, network: &NT, value: V)
    where
        NT: CBCSendNode<CBCMessage<V>>,
    {
        let _ = network.broadcast(
            CBCMessage::Send(value),
            self.quorum_info.quorum_members().iter().cloned(),
        );
    }

    pub(crate) fn process_message<NT>(
        &mut self,
        sys_msg: StoredMessage<CBCMessage<V>>,
        network: &NT,
    ) -> Result<CBCResult, CBCError>
    where
        NT: CBCSendNode<CBCMessage<V>>,
    {
        let (header, message) = sys_msg.into_inner();
        let from = header.from();

        match message {
            CBCMessage::Send(value) => self.handle_send(from, value, network),
            CBCMessage::Echo(share) => self.handle_echo(from, share, network),
            CBCMessage::Finish(value, signature) => self.handle_finish(from, value, signature),
        }
    }

    fn handle_send<NT>(
        &mut self,
        from: NodeId,
        value: V,
        network: &NT,
    ) -> Result<CBCResult, CBCError>
    where
        NT: CBCSendNode<CBCMessage<V>>,
    {
        if from != self.owner_id || !matches!(self.state, CBCState::AwaitingSend) {
            return Ok(CBCResult::MessageIgnored);
        }

        let digest = compute_digest(&value);
        self.state = CBCState::AwaitingFinish { value, digest };

        let share = self
            .threshold_keys
            .cbc_private_key()
            .partially_sign(digest.as_ref());

        let _ = network.send(CBCMessage::Echo(share), self.owner_id, true);

        Ok(CBCResult::Processed)
    }

    fn handle_echo<NT>(
        &mut self,
        from: NodeId,
        share: PartialSignature,
        network: &NT,
    ) -> Result<CBCResult, CBCError>
    where
        NT: CBCSendNode<CBCMessage<V>>,
    {
        if self.quorum_info.own_node_id() != self.owner_id {
            return Ok(CBCResult::MessageIgnored);
        }

        let digest = match &self.state {
            CBCState::AwaitingFinish { digest, .. } => *digest,
            _ => return Ok(CBCResult::MessageIgnored),
        };

        if self
            .threshold_keys
            .cbc_public_key()
            .verify_partial_signature(from.0 as usize, digest.as_ref(), &share)
            .is_err()
        {
            warn!("Rejecting invalid CBC Echo share from {from:?}");
            return Ok(CBCResult::MessageIgnored);
        }

        self.echo_shares.insert(from, share);

        if self.echo_shares.len() > 2 * self.quorum_info.f() {
            let shares = self
                .echo_shares
                .iter()
                .map(|(node, sig)| (node.0 as usize, sig));

            let signature = self
                .threshold_keys
                .cbc_public_key()
                .combine_signatures(shares)
                .map_err(CBCError::Combine)?;

            let CBCState::AwaitingFinish { value, digest } =
                std::mem::replace(&mut self.state, CBCState::AwaitingSend)
            else {
                unreachable!("checked above that state is AwaitingFinish");
            };

            let _ = network.broadcast(
                CBCMessage::Finish(value.clone(), signature.clone()),
                self.quorum_info.quorum_members().iter().cloned(),
            );

            self.state = CBCState::Finished {
                value,
                digest,
                signature,
            };

            return Ok(CBCResult::Finalized);
        }

        Ok(CBCResult::Processed)
    }

    fn handle_finish(
        &mut self,
        from: NodeId,
        value: V,
        signature: CombinedSignature,
    ) -> Result<CBCResult, CBCError> {
        if from != self.owner_id || matches!(self.state, CBCState::Finished { .. }) {
            return Ok(CBCResult::MessageIgnored);
        }

        let digest = compute_digest(&value);

        if self
            .threshold_keys
            .cbc_public_key()
            .verify(digest.as_ref(), &signature)
            .is_err()
        {
            warn!("Rejecting invalid CBC Finish signature");
            return Ok(CBCResult::MessageIgnored);
        }

        self.state = CBCState::Finished {
            value,
            digest,
            signature,
        };

        Ok(CBCResult::Finalized)
    }
}

impl<V> CBCProtocol<V> for ConsistentBroadcastInstance<V>
where
    V: SerMsg,
{
    type Message = CBCMessage<V>;
    type Error = CBCError;

    fn new(owner_id: NodeId, quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self {
        Self {
            owner_id,
            quorum_info,
            threshold_keys,
            state: CBCState::AwaitingSend,
            echo_shares: HashMap::default(),
        }
    }

    fn new_with_propose<NT>(
        owner_id: NodeId,
        quorum_info: QuorumInfo,
        threshold_keys: ThresholdKeys,
        value: V,
        network: &NT,
    ) -> Self
    where
        NT: CBCSendNode<Self::Message>,
    {
        let cbc = Self::new(owner_id, quorum_info, threshold_keys);

        cbc.propose_value(network, value);

        cbc
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::Message>> {
        // A single round of SEND/ECHO/FINISH, each unconditionally handled
        // by state -- there is never anything to defer.
        None
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<CBCResult, Self::Error>
    where
        NT: CBCSendNode<Self::Message>,
    {
        self.process_message(message, network)
    }

    fn finalize(self) -> Result<(V, Digest, CombinedSignature), Self::Error> {
        match self.state {
            CBCState::Finished {
                value,
                digest,
                signature,
            } => Ok((value, digest, signature)),
            _ => Err(CBCError::NotReadyToFinalize),
        }
    }
}

impl<V> Debug for ConsistentBroadcastInstance<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let state = match &self.state {
            CBCState::AwaitingSend => "AwaitingSend",
            CBCState::AwaitingFinish { .. } => "AwaitingFinish",
            CBCState::Finished { .. } => "Finished",
        };

        f.debug_struct("ConsistentBroadcastInstance")
            .field("owner_id", &self.owner_id)
            .field("state", &state)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub(crate) enum CBCError {
    #[error("Failed to combine CBC echo shares: {0}")]
    Combine(#[from] CombineSignatureError),
    #[error("Attempted to finalize a CBC instance before it reached the Finished state")]
    NotReadyToFinalize,
}
