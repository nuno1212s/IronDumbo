use crate::aba::ABAProtocol;
use crate::async_bin_agreement::async_bin_agreement::AsyncBinaryAgreement;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::reliable_broadcast::reliable_broadcast::ReliableBroadcastInstance;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::PrivateKeySet;
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage, WireMessage};

pub fn node_ids(n: usize) -> Vec<NodeId> {
    (0..n).map(NodeId::from).collect()
}

pub fn quorum_info(n: usize, f: usize, node: NodeId) -> QuorumInfo {
    QuorumInfo::new(n, f, node_ids(n), node)
}

pub fn make_keyset(f: usize) -> PrivateKeySet {
    PrivateKeySet::gen_random(f)
}

pub fn make_threshold_keys(keyset: &PrivateKeySet, node: NodeId) -> ThresholdKeys {
    ThresholdKeys::new(
        keyset.public_key_set(),
        keyset.private_key_part(node.0 as usize),
    )
}

pub fn make_digest(val: &[u8]) -> Digest {
    let mut context = Context::new();
    context.update(val);
    context.finish()
}

pub fn stored_msg<T>(from: NodeId, to: NodeId, msg: T) -> StoredMessage<T> {
    stored_msg_with_digest(from, to, msg, Digest::blank())
}

pub fn stored_msg_with_digest<T>(
    from: NodeId,
    to: NodeId,
    msg: T,
    digest: Digest,
) -> StoredMessage<T> {
    let wire_msg = WireMessage::new(
        from,
        to,
        MessageModule::Application,
        Buf::new(),
        0,
        Some(digest),
        None,
    );

    StoredMessage::new(wire_msg.header().clone(), msg)
}

/// Bootstraps `n` [`ReliableBroadcastInstance`]s (tolerating `f` faults) that
/// share a common quorum, one per node, ready to have SEND/ECHO/READY
/// messages driven through them.
pub fn bootstrap_rbc_cluster(
    n: usize,
    f: usize,
) -> Vec<(NodeId, ReliableBroadcastInstance<Vec<u8>>)> {
    node_ids(n)
        .into_iter()
        .map(|id| {
            let quorum = quorum_info(n, f, id);

            (id, ReliableBroadcastInstance::<Vec<u8>>::new(id, quorum))
        })
        .collect()
}

/// Bootstraps `n` [`AsyncBinaryAgreement`]s (tolerating `f` faults) that
/// share a single threshold key set, along with that key set (needed by
/// tests that must hand out extra partial signatures).
pub fn bootstrap_aba_cluster(
    n: usize,
    f: usize,
) -> (Vec<(NodeId, AsyncBinaryAgreement)>, PrivateKeySet) {
    let keyset = make_keyset(f);

    let nodes = node_ids(n)
        .into_iter()
        .map(|id| {
            let quorum = quorum_info(n, f, id);
            let threshold_keys = make_threshold_keys(&keyset, id);

            (id, AsyncBinaryAgreement::new(quorum, threshold_keys))
        })
        .collect();

    (nodes, keyset)
}
