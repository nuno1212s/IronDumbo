use crate::prbc::{PRBCProtocol, PRBCResult, PRBCSendNode};
use crate::provable_reliable_broadcast::messages::PRBCMessage;
use crate::provable_reliable_broadcast::provable_reliable_broadcast::ProvableReliableBroadcastInstance;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::testing::fixtures;
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::node_id::NodeId;
use std::cell::RefCell;
use std::collections::HashMap;

type MsgType = u8;
type Prbc = ProvableReliableBroadcastInstance<MsgType>;

struct MockNetwork {
    sent: RefCell<Vec<(PRBCMessage, Vec<NodeId>)>>,
}

impl MockNetwork {
    fn new() -> Self {
        Self {
            sent: RefCell::new(vec![]),
        }
    }
}

impl PRBCSendNode<PRBCMessage> for MockNetwork {
    fn send(
        &self,
        message: PRBCMessage,
        target: NodeId,
        _flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.sent.borrow_mut().push((message, vec![target]));
        Ok(())
    }

    fn broadcast<I>(&self, message: PRBCMessage, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.sent.borrow_mut().push((message, targets.collect()));
        Ok(())
    }
}

const N: usize = 4;
const F: usize = 1;

fn setup(node: NodeId) -> (QuorumInfo, ThresholdKeys) {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let quorum = fixtures::quorum_info(N, F, node);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, node);
    (quorum, threshold_keys)
}

fn val_for(
    quorum: &QuorumInfo,
    recipient: NodeId,
    parts: &[crate::reliable_broadcast::messages::ErasureCodedPart],
) -> PRBCMessage {
    let idx = quorum
        .leaf_index_of(recipient)
        .expect("recipient must be a quorum member");
    PRBCMessage::Val(parts[idx].clone())
}

fn echo_from(
    quorum: &QuorumInfo,
    echoer: NodeId,
    parts: &[crate::reliable_broadcast::messages::ErasureCodedPart],
) -> PRBCMessage {
    let idx = quorum
        .leaf_index_of(echoer)
        .expect("echoer must be a quorum member");
    PRBCMessage::Echo(parts[idx].clone())
}

#[test]
fn test_prbc_val_phase() {
    let sender = NodeId(0);
    let (quorum, threshold_keys) = setup(sender);
    let network = MockNetwork::new();
    let (root, parts) = fixtures::make_erasure_coded_parts(&0u8, N, F);

    let mut prbc = Prbc::new(sender, quorum.clone(), threshold_keys);
    let val_msg = val_for(&quorum, sender, &parts);
    let stored = fixtures::stored_msg(sender, sender, val_msg);

    let result = prbc.process_message(stored, &network).unwrap();

    assert!(matches!(result, PRBCResult::Processed));
    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, PRBCMessage::Echo(part) if part.root == root))
    );
}

#[test]
fn test_prbc_echo_phase() {
    let sender = NodeId(0);
    let (quorum, threshold_keys) = setup(sender);
    let network = MockNetwork::new();
    let (root, parts) = fixtures::make_erasure_coded_parts(&7u8, N, F);

    let mut prbc = Prbc::new(sender, quorum.clone(), threshold_keys);
    prbc.process_message(
        fixtures::stored_msg(sender, sender, val_for(&quorum, sender, &parts)),
        &network,
    )
    .unwrap();

    for &echoer in quorum.quorum_members().iter().take(quorum.quorum_size()) {
        let echo = fixtures::stored_msg(echoer, sender, echo_from(&quorum, echoer, &parts));
        prbc.process_message(echo, &network).unwrap();
    }

    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, PRBCMessage::Ready(d) if *d == root)),
        "enough matching ECHOes should trigger a READY broadcast"
    );
}

#[test]
fn test_prbc_ready_phase() {
    let sender = NodeId(0);
    let (quorum, threshold_keys) = setup(sender);
    let network = MockNetwork::new();
    let (root, parts) = fixtures::make_erasure_coded_parts(&9u8, N, F);

    let mut prbc = Prbc::new(sender, quorum.clone(), threshold_keys);
    prbc.process_message(
        fixtures::stored_msg(sender, sender, val_for(&quorum, sender, &parts)),
        &network,
    )
    .unwrap();

    for &echoer in quorum.quorum_members().iter().take(quorum.quorum_size()) {
        let echo = fixtures::stored_msg(echoer, sender, echo_from(&quorum, echoer, &parts));
        prbc.process_message(echo, &network).unwrap();
    }

    for &ready_sender in quorum.quorum_members().iter().take(2 * quorum.f() + 1) {
        let ready = fixtures::stored_msg(ready_sender, sender, PRBCMessage::Ready(root));
        prbc.process_message(ready, &network).unwrap();
    }

    // Reaching READY quorum on the inner broadcast should immediately kick
    // off the Done phase: we broadcast our own threshold-signature share.
    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, PRBCMessage::Done(_))),
        "inner RBC finalizing should trigger our own Done broadcast"
    );
}

/// Drives `n` PRBC instances (sharing one threshold keyset), all tracking the
/// same proposer, to a fixed point over a `SimulatedNetwork`.
fn run_prbc_cluster(
    n: usize,
    f: usize,
    proposer: NodeId,
) -> (
    HashMap<NodeId, Prbc>,
    Vec<(NodeId, PRBCResult)>,
    atlas_common::crypto::threshold_crypto::PrivateKeySet,
) {
    let members = fixtures::node_ids(n);
    let keyset = fixtures::make_keyset(f);
    let cbc_keyset = fixtures::make_cbc_keyset(f);

    let bus = RefCell::new(SimulatedNetwork::<PRBCMessage>::new(&members));

    let mut instances: HashMap<NodeId, Prbc> = members
        .iter()
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, id);
            (id, Prbc::new(proposer, quorum, threshold_keys))
        })
        .collect();

    // The proposer kicks off the broadcast; since `quorum_members` includes
    // itself, this also seeds the proposer's own instance via loopback once
    // delivered below.
    {
        let handle = NodeHandle::new(proposer, &bus);
        let quorum = fixtures::quorum_info(n, f, proposer);
        let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, proposer);
        let prbc = Prbc::new_with_propose(proposer, quorum, threshold_keys, 5, &handle);
        instances.insert(proposer, prbc);
    }

    let mut observed = Vec::new();
    let mut delivered = 0usize;

    loop {
        let mut progressed = false;

        for &id in &members {
            loop {
                let next = bus.borrow_mut().deliver_next(id);
                let Some((from, msg)) = next else {
                    break;
                };
                progressed = true;
                delivered += 1;
                assert!(
                    delivered < 100_000,
                    "PRBC cluster simulation did not converge"
                );

                let handle = NodeHandle::new(id, &bus);
                let stored = fixtures::stored_msg(from, id, msg);
                let result = instances
                    .get_mut(&id)
                    .unwrap()
                    .process_message(stored, &handle)
                    .unwrap();

                observed.push((id, result));
            }
        }

        if !progressed {
            break;
        }
    }

    (instances, observed, keyset)
}

#[test]
fn test_prbc_done_phase() {
    let (_, observed, _keyset) = run_prbc_cluster(N, F, NodeId(0));

    let finalized_count = observed
        .iter()
        .filter(|(_, result)| matches!(result, PRBCResult::Finalized(_)))
        .count();

    assert!(
        finalized_count > 0,
        "at least one node should assemble/adopt a combined signature"
    );
}

#[test]
fn test_prbc_finalized_returns_proof() {
    let (mut instances, _, keyset) = run_prbc_cluster(N, F, NodeId(0));

    let proposer_instance = instances.remove(&NodeId(0)).unwrap();
    let (value, digest, combined_signature) = proposer_instance.finalize().unwrap();

    assert_eq!(value, 5);

    // The combined signature must independently verify against the quorum's
    // public key: any node can check this proof without re-collecting shares.
    keyset
        .public_key_set()
        .verify(digest.as_ref(), &combined_signature)
        .expect("the assembled signature should verify against the quorum's public key");
}

#[test]
fn test_prbc_full_4node_simulation() {
    let (mut instances, observed, _keyset) = run_prbc_cluster(N, F, NodeId(0));

    let finalized_nodes: std::collections::HashSet<NodeId> = observed
        .iter()
        .filter(|(_, result)| matches!(result, PRBCResult::Finalized(_)))
        .map(|(id, _)| *id)
        .collect();

    assert_eq!(
        finalized_nodes.len(),
        N,
        "every node should finalize with a combined signature"
    );

    for &id in fixtures::node_ids(N).iter() {
        let (value, _, _) = instances.remove(&id).unwrap().finalize().unwrap();
        assert_eq!(value, 5, "every node should agree on the proposed value");
    }
}
