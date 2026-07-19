use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::{ReliableBroadcast, ReliableBroadcastSendNode};
use crate::reliable_broadcast::erasure_coding::{self, ErasureParams};
use crate::reliable_broadcast::merkle;
use crate::reliable_broadcast::messages::{ErasureCodedPart, ReliableBroadcastMessage};
use atlas_common::collections::{HashMap, HashSet};
use atlas_common::crypto::hash::Digest;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::StoredMessage;
use std::fmt::{Debug, Formatter};
use thiserror::Error;
use tracing::warn;

/// Per-candidate-root bookkeeping. A node may end up tracking more than one
/// root at once (a Byzantine sender can equivocate the VAL it sends to
/// different recipients, or ECHO/READY traffic for a root this node never
/// received VAL for may arrive first) -- only one can ever reach quorum
/// (see the module-level safety argument in the RBC fix plan).
struct RootTracking<RQ> {
    /// Indexed by `QuorumInfo::leaf_index_of`; `None` until that party's
    /// shard has been seen and verified for this root.
    echo_shards: Vec<Option<Vec<u8>>>,
    ready_senders: HashSet<NodeId>,
    /// Set once this root's n-f ECHO threshold has been evaluated (whether
    /// or not reconstruction/verification actually succeeded) -- gates
    /// re-running the (relatively expensive) reconstruct+recompute step on
    /// every single subsequent ECHO for the same root.
    echo_threshold_crossed: bool,
    /// Set if reconstruction succeeded but the recomputed Merkle root
    /// didn't match (Algorithm 5 line 11) -- a provably inconsistent
    /// codeword under this root; it can never be legitimately decoded.
    aborted: bool,
    cached_decode: Option<RQ>,
}

impl<RQ> RootTracking<RQ> {
    fn new(n: usize) -> Self {
        Self {
            echo_shards: vec![None; n],
            ready_senders: HashSet::default(),
            echo_threshold_crossed: false,
            aborted: false,
            cached_decode: None,
        }
    }

    fn present_shard_count(&self) -> usize {
        self.echo_shards.iter().filter(|s| s.is_some()).count()
    }
}

/// An instance of the reliable broadcast protocol (Algorithm 5): erasure
/// coding + a Merkle tree let any node reconstruct the broadcast value from
/// n-2f ECHOes, without ever needing the sender's direct VAL -- this is
/// what lets a node the sender skips still satisfy Totality.
pub(crate) struct ReliableBroadcastInstance<RQ> {
    sender: NodeId,
    quorum_info: QuorumInfo,
    roots: HashMap<Digest, RootTracking<RQ>>,
    /// DoS guard: at most one "new root" slot per sender, so a Byzantine
    /// node can't grow `roots` unboundedly by inventing fabricated roots
    /// (a Merkle branch alone only proves self-consistency with a *claimed*
    /// root, not that the root is the real one).
    new_root_claimed_by: HashSet<NodeId>,
    /// Instance-level (not per-root): the Bracha single-vote invariant
    /// (never ECHO/READY two different values) must hold regardless of how
    /// many candidate roots this node ends up tracking.
    own_val_root: Option<Digest>,
    own_ready_root: Option<Digest>,
    finalized: Option<(Digest, RQ)>,
}

impl<RQ> ReliableBroadcastInstance<RQ>
where
    RQ: SerMsg,
{
    pub fn new(sender: NodeId, quorum_info: QuorumInfo) -> Self {
        Self {
            sender,
            quorum_info,
            roots: HashMap::default(),
            new_root_claimed_by: HashSet::default(),
            own_val_root: None,
            own_ready_root: None,
            finalized: None,
        }
    }

    fn propose_values<NT>(&mut self, network: &NT, value: RQ)
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        let n = self.quorum_info.quorum_members().len();
        let params = ErasureParams::for_quorum(n, self.quorum_info.f());

        let shards = match erasure_coding::encode(&value, &params) {
            Ok(shards) => shards,
            Err(err) => {
                warn!("Failed to erasure-code proposed value: {err:?}");
                return;
            }
        };

        let (root, branches) = merkle::build_tree(&shards);

        // Each recipient gets a *different* VAL message (their own
        // shard+branch), so this is a per-recipient unicast loop rather
        // than a single `broadcast`. Deliberately does not touch
        // `self.own_val_root`/`self.roots` here: the sender is a member of
        // its own quorum, so its own VAL loops back through the transport
        // and is handled by the exact same `handle_val` path as any other
        // recipient's.
        for (index, &member) in self.quorum_info.quorum_members().iter().enumerate() {
            let part = ErasureCodedPart {
                root,
                branch: branches[index].clone(),
                shard: shards[index].clone(),
            };

            if let Err(err) = network.send(ReliableBroadcastMessage::Val(part), member, true) {
                warn!("Failed to send VAL to {member:?}: {err:?}");
            }
        }
    }

    /// The root of the finalized value, if any -- a non-consuming peek used
    /// by composing protocols (e.g. PRBC) that need it before their own
    /// `finalize()` call.
    pub(crate) fn finalized_digest(&self) -> Option<Digest> {
        self.finalized.as_ref().map(|(root, _)| *root)
    }

    pub(crate) fn process_message<NT>(
        &mut self,
        sys_msg: StoredMessage<ReliableBroadcastMessage>,
        network: &NT,
    ) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        let (header, message) = sys_msg.into_inner();
        let from = header.from();

        match message {
            ReliableBroadcastMessage::Val(part) => self.handle_val(from, part, network),
            ReliableBroadcastMessage::Echo(part) => self.handle_echo(from, part, network),
            ReliableBroadcastMessage::Ready(root) => self.handle_ready(from, root, network),
        }
    }

    fn handle_val<NT>(
        &mut self,
        from: NodeId,
        part: ErasureCodedPart,
        network: &NT,
    ) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        if from != self.sender || self.own_val_root.is_some() {
            return ReliableBroadcastResult::MessageIgnored;
        }

        let n = self.quorum_info.quorum_members().len();
        let Some(my_index) = self
            .quorum_info
            .leaf_index_of(self.quorum_info.own_node_id())
        else {
            warn!("This node is not a member of its own quorum_info");
            return ReliableBroadcastResult::MessageIgnored;
        };

        if !merkle::verify_branch(part.root, n, my_index, &part.shard, &part.branch) {
            return ReliableBroadcastResult::MessageIgnored;
        }

        self.own_val_root = Some(part.root);
        self.broadcast_echo_message(part, network);

        ReliableBroadcastResult::Processed
    }

    fn handle_echo<NT>(
        &mut self,
        from: NodeId,
        part: ErasureCodedPart,
        network: &NT,
    ) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        if !self.quorum_info.is_member(from) {
            return ReliableBroadcastResult::MessageIgnored;
        }

        let n = self.quorum_info.quorum_members().len();
        let Some(leaf_index) = self.quorum_info.leaf_index_of(from) else {
            return ReliableBroadcastResult::MessageIgnored;
        };

        if !merkle::verify_branch(part.root, n, leaf_index, &part.shard, &part.branch) {
            return ReliableBroadcastResult::MessageIgnored;
        }

        let root = part.root;

        let Some(tracking) = self.ensure_root_tracked(root, from) else {
            return ReliableBroadcastResult::MessageIgnored;
        };

        tracking.echo_shards[leaf_index] = Some(part.shard);

        self.evaluate_root(root, network)
    }

    fn handle_ready<NT>(
        &mut self,
        from: NodeId,
        root: Digest,
        network: &NT,
    ) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        if !self.quorum_info.is_member(from) {
            return ReliableBroadcastResult::MessageIgnored;
        }

        let Some(tracking) = self.ensure_root_tracked(root, from) else {
            return ReliableBroadcastResult::MessageIgnored;
        };

        tracking.ready_senders.insert(from);

        self.evaluate_root(root, network)
    }

    fn ensure_root_tracked(&mut self, root: Digest, from: NodeId) -> Option<&mut RootTracking<RQ>> {
        if !self.roots.contains_key(&root) {
            if self.new_root_claimed_by.contains(&from) {
                return None;
            }

            self.new_root_claimed_by.insert(from);

            let n = self.quorum_info.quorum_members().len();
            self.roots.insert(root, RootTracking::new(n));
        }

        self.roots.get_mut(&root)
    }

    /// Algorithm 5 lines 10-11: interpolate all n shards, recompute the
    /// Merkle root, and abort this root if it doesn't match. Reconstructs
    /// *every* shard (not just the k data shards) so the recheck covers
    /// positions a data-only reconstruct would never touch.
    fn try_reconstruct(&mut self, root: Digest, params: &ErasureParams) {
        let Some(tracking) = self.roots.get_mut(&root) else {
            return;
        };

        tracking.echo_threshold_crossed = true;

        if tracking.aborted || tracking.cached_decode.is_some() {
            return;
        }

        let mut scratch = tracking.echo_shards.clone();

        if erasure_coding::reconstruct_all(&mut scratch, params).is_err() {
            // Not (yet) enough shards to reconstruct -- leave state as-is,
            // a later ECHO will retry.
            return;
        }

        let reconstructed: Vec<Vec<u8>> =
            scratch.into_iter().map(Option::unwrap_or_default).collect();
        let recomputed_root = merkle::compute_root(&reconstructed);

        if recomputed_root != root {
            if let Some(tracking) = self.roots.get_mut(&root) {
                tracking.aborted = true;
            }
            return;
        }

        match erasure_coding::decode::<RQ>(&reconstructed, params) {
            Ok(value) => {
                if let Some(tracking) = self.roots.get_mut(&root) {
                    tracking.cached_decode = Some(value);
                }
            }
            Err(err) => {
                warn!("Failed to decode reconstructed value for a verified root: {err:?}");
                if let Some(tracking) = self.roots.get_mut(&root) {
                    tracking.aborted = true;
                }
            }
        }
    }

    fn evaluate_root<NT>(&mut self, root: Digest, network: &NT) -> ReliableBroadcastResult
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        if self.finalized.is_some() {
            return ReliableBroadcastResult::Processed;
        }

        let n = self.quorum_info.quorum_members().len();
        let params = ErasureParams::for_quorum(n, self.quorum_info.f());

        let Some(tracking) = self.roots.get(&root) else {
            return ReliableBroadcastResult::Processed;
        };

        if tracking.aborted {
            return ReliableBroadcastResult::Processed;
        }

        // Lines 9-12: n-f ECHOes -> reconstruct+verify -> READY, once.
        if !tracking.echo_threshold_crossed
            && tracking.present_shard_count() >= self.quorum_info.quorum_size()
        {
            self.try_reconstruct(root, &params);
        }

        if self.roots.get(&root).map(|t| t.aborted).unwrap_or(true) {
            return ReliableBroadcastResult::Processed;
        }

        if self.own_ready_root.is_none()
            && self
                .roots
                .get(&root)
                .map(|t| t.echo_threshold_crossed)
                .unwrap_or(false)
        {
            self.broadcast_ready_message(root, network);
            self.own_ready_root = Some(root);
        }

        // Lines 13-14: f+1 matching READY -> amplify (pure vote count, no
        // data check needed -- see the RBC fix plan's safety argument).
        let ready_count = self
            .roots
            .get(&root)
            .map(|t| t.ready_senders.len())
            .unwrap_or(0);

        if self.own_ready_root.is_none() && ready_count > self.quorum_info.f() {
            self.broadcast_ready_message(root, network);
            self.own_ready_root = Some(root);
        }

        // Lines 15-16: 2f+1 READY -> wait for n-2f ECHOes -> decode.
        let ready_count = self
            .roots
            .get(&root)
            .map(|t| t.ready_senders.len())
            .unwrap_or(0);

        if ready_count > 2 * self.quorum_info.f() {
            let (has_cached, enough_shards) = self
                .roots
                .get(&root)
                .map(|t| {
                    (
                        t.cached_decode.is_some(),
                        t.present_shard_count() >= params.data_shards(),
                    )
                })
                .unwrap_or((false, false));

            if !has_cached && enough_shards {
                self.try_reconstruct(root, &params);
            }

            if let Some(value) = self
                .roots
                .get_mut(&root)
                .and_then(|tracking| tracking.cached_decode.take())
            {
                self.finalized = Some((root, value));
                return ReliableBroadcastResult::Finalized;
            }
        }

        ReliableBroadcastResult::Processed
    }

    fn broadcast_echo_message<NT>(&self, part: ErasureCodedPart, network: &NT)
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        let message = ReliableBroadcastMessage::Echo(part);

        if let Err(err) =
            network.broadcast(message, self.quorum_info.quorum_members().iter().cloned())
        {
            warn!("Failed to broadcast echo message: {err:?}");
        }
    }

    fn broadcast_ready_message<NT>(&self, root: Digest, network: &NT)
    where
        NT: ReliableBroadcastSendNode<ReliableBroadcastMessage>,
    {
        let message = ReliableBroadcastMessage::Ready(root);

        if let Err(err) =
            network.broadcast(message, self.quorum_info.quorum_members().iter().cloned())
        {
            warn!("Failed to broadcast ready message: {err:?}");
        }
    }
}

impl<RQ> ReliableBroadcast<RQ> for ReliableBroadcastInstance<RQ>
where
    RQ: SerMsg,
{
    type ReliableBroadcastMessage = ReliableBroadcastMessage;
    type Error = ReliableBroadcastError;

    fn new(owner_id: NodeId, quorum_info: QuorumInfo) -> Self {
        Self::new(owner_id, quorum_info)
    }

    fn new_with_propose<NT>(
        owner_id: NodeId,
        quorum_info: QuorumInfo,
        request: RQ,
        network: &NT,
    ) -> Self
    where
        NT: ReliableBroadcastSendNode<Self::ReliableBroadcastMessage>,
    {
        let mut rbc = Self::new(owner_id, quorum_info);

        rbc.propose_values(network, request);

        rbc
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::ReliableBroadcastMessage>> {
        // ECHO/READY are now processed unconditionally regardless of local
        // state (that's precisely what fixes Totality), so there is never
        // anything to defer.
        None
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::ReliableBroadcastMessage>,
        network: &NT,
    ) -> Result<crate::rbc::ReliableBroadcastResult, Self::Error>
    where
        NT: ReliableBroadcastSendNode<Self::ReliableBroadcastMessage>,
    {
        let result = self.process_message(message, network);

        Ok(match result {
            ReliableBroadcastResult::MessageIgnored => {
                crate::rbc::ReliableBroadcastResult::MessageIgnored
            }
            ReliableBroadcastResult::Processed => crate::rbc::ReliableBroadcastResult::Processed,
            ReliableBroadcastResult::Finalized => crate::rbc::ReliableBroadcastResult::Finalized,
        })
    }

    fn finalize(self) -> Result<(RQ, Digest), Self::Error> {
        self.finalized
            .map(|(root, value)| (value, root))
            .ok_or(ReliableBroadcastError::NotReadyToFinalize)
    }
}

impl<RQ> Debug for ReliableBroadcastInstance<RQ> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReliableBroadcastInstance")
            .field("sender", &self.sender)
            .field("tracked_roots", &self.roots.len())
            .field("finalized", &self.finalized.is_some())
            .finish_non_exhaustive()
    }
}

pub(crate) enum ReliableBroadcastResult {
    MessageIgnored,
    Processed,
    Finalized,
}

#[derive(Debug, Error)]
pub enum ReliableBroadcastError {
    #[error("Reliable broadcast instance is not ready to finalize")]
    NotReadyToFinalize,
}
