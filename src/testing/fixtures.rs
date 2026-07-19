use crate::aba::ABAProtocol;
use crate::async_bin_agreement::async_bin_agreement::AsyncBinaryAgreement;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::reliable_broadcast::erasure_coding::{self, ErasureParams};
use crate::reliable_broadcast::merkle;
use crate::reliable_broadcast::messages::ErasureCodedPart;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::PrivateKeySet;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
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

/// The independent `2f`-threshold keyset CBC's Echo/Finish combine step
/// needs (see `ThresholdKeys`'s doc comment) -- a distinct polynomial from
/// `make_keyset`, not derivable from it.
pub fn make_cbc_keyset(f: usize) -> PrivateKeySet {
    PrivateKeySet::gen_random(2 * f)
}

pub fn make_threshold_keys(
    keyset: &PrivateKeySet,
    cbc_keyset: &PrivateKeySet,
    node: NodeId,
) -> ThresholdKeys {
    ThresholdKeys::new(
        keyset.public_key_set(),
        keyset.private_key_part(node.0 as usize),
        cbc_keyset.public_key_set(),
        cbc_keyset.private_key_part(node.0 as usize),
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

/// Erasure-codes + Merkle-trees `value` for a quorum of size `n` tolerating
/// `f` faults (Algorithm 5), returning the root and one [`ErasureCodedPart`]
/// per leaf index -- callers pick `parts[quorum.leaf_index_of(member)]` when
/// constructing `Val`/`Echo` messages for a given recipient/sender.
pub fn make_erasure_coded_parts<RQ: SerMsg>(
    value: &RQ,
    n: usize,
    f: usize,
) -> (Digest, Vec<ErasureCodedPart>) {
    let params = ErasureParams::for_quorum(n, f);
    let shards = erasure_coding::encode(value, &params).expect("encode should succeed in tests");
    let (root, branches) = merkle::build_tree(&shards);

    let parts = shards
        .into_iter()
        .zip(branches)
        .map(|(shard, branch)| ErasureCodedPart {
            root,
            branch,
            shard,
        })
        .collect();

    (root, parts)
}

/// Bootstraps `n` [`AsyncBinaryAgreement`]s (tolerating `f` faults) that
/// share a single threshold key set, along with that key set (needed by
/// tests that must hand out extra partial signatures).
pub fn bootstrap_aba_cluster(
    n: usize,
    f: usize,
) -> (Vec<(NodeId, AsyncBinaryAgreement)>, PrivateKeySet) {
    let keyset = make_keyset(f);
    let cbc_keyset = make_cbc_keyset(f);

    let nodes = node_ids(n)
        .into_iter()
        .map(|id| {
            let quorum = quorum_info(n, f, id);
            let threshold_keys = make_threshold_keys(&keyset, &cbc_keyset, id);

            (id, AsyncBinaryAgreement::new(quorum, threshold_keys))
        })
        .collect();

    (nodes, keyset)
}
