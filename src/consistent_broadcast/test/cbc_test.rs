use crate::cbc::{CBCProtocol, CBCResult, CBCSendNode};
use crate::consistent_broadcast::consistent_broadcast::ConsistentBroadcastInstance;
use crate::consistent_broadcast::messages::CBCMessage;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::testing::fixtures;
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::PrivateKeySet;
use atlas_common::node_id::NodeId;
use atlas_common::serialization_helper::SerMsg;
use std::cell::RefCell;
use std::collections::HashMap;

type MsgType = u8;
type Cbc = ConsistentBroadcastInstance<MsgType>;

struct MockNetwork {
    sent: RefCell<Vec<(CBCMessage<MsgType>, Vec<NodeId>)>>,
}

impl MockNetwork {
    fn new() -> Self {
        Self {
            sent: RefCell::new(vec![]),
        }
    }
}

impl CBCSendNode<CBCMessage<MsgType>> for MockNetwork {
    fn send(
        &self,
        message: CBCMessage<MsgType>,
        target: NodeId,
        _flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.sent.borrow_mut().push((message, vec![target]));
        Ok(())
    }

    fn broadcast<I>(&self, message: CBCMessage<MsgType>, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.sent.borrow_mut().push((message, targets.collect()));
        Ok(())
    }
}

const N: usize = 4;
const F: usize = 1;

fn digest_of<V: SerMsg>(value: &V) -> Digest {
    let serialized = bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap();
    let mut ctx = Context::new();
    ctx.update(&serialized);
    ctx.finish()
}

fn setup(node: NodeId) -> (QuorumInfo, ThresholdKeys) {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let quorum = fixtures::quorum_info(N, F, node);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, node);
    (quorum, threshold_keys)
}

#[test]
fn test_cbc_send_phase_echoes_back_to_owner() {
    let owner = NodeId(0);
    let (quorum, threshold_keys) = setup(owner);
    let network = MockNetwork::new();

    let mut cbc = Cbc::new(owner, quorum, threshold_keys);
    let stored = fixtures::stored_msg(owner, owner, CBCMessage::Send(42));

    let result = cbc.process_message(stored, &network).unwrap();

    assert!(matches!(result, CBCResult::Processed));
    assert!(matches!(
        &network.sent.borrow()[0],
        (CBCMessage::Echo(_), targets) if targets == &vec![owner]
    ));
}

#[test]
fn test_cbc_send_from_non_owner_ignored() {
    let owner = NodeId(0);
    let impostor = NodeId(1);
    let (quorum, threshold_keys) = setup(owner);
    let network = MockNetwork::new();

    let mut cbc = Cbc::new(owner, quorum, threshold_keys);
    let stored = fixtures::stored_msg(impostor, owner, CBCMessage::Send(42));

    let result = cbc.process_message(stored, &network).unwrap();

    assert!(matches!(result, CBCResult::MessageIgnored));
}

#[test]
fn test_cbc_echo_below_2f_plus_1_does_not_finalize() {
    let owner = NodeId(0);
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let quorum = fixtures::quorum_info(N, F, owner);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, owner);
    let network = MockNetwork::new();

    let mut cbc = Cbc::new(owner, quorum.clone(), threshold_keys);
    cbc.process_message(
        fixtures::stored_msg(owner, owner, CBCMessage::Send(7)),
        &network,
    )
    .unwrap();

    let digest = digest_of(&7u8);

    // Only 2f = 2 echoes (need 2f+1 = 3).
    for &echoer in quorum.quorum_members().iter().take(2 * F) {
        let echoer_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, echoer);
        let share = echoer_keys
            .cbc_private_key()
            .partially_sign(digest.as_ref());

        let result = cbc
            .process_message(
                fixtures::stored_msg(echoer, owner, CBCMessage::Echo(share)),
                &network,
            )
            .unwrap();
        assert!(matches!(result, CBCResult::Processed));
    }

    assert!(
        !network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, CBCMessage::Finish(..))),
        "should not finalize before 2f+1 echoes"
    );
}

/// Drives an `n`-node CBC cluster (sharing one `2f`-threshold CBC keyset) to
/// a fixed point over a `SimulatedNetwork`, returning the finalized
/// instances plus the keysets used (so callers can independently verify
/// signatures against the right public key).
fn run_cbc_cluster(
    n: usize,
    f: usize,
    owner: NodeId,
    value: MsgType,
) -> (HashMap<NodeId, Cbc>, PrivateKeySet, PrivateKeySet) {
    let members = fixtures::node_ids(n);
    let keyset = fixtures::make_keyset(f);
    let cbc_keyset = fixtures::make_cbc_keyset(f);

    let bus = RefCell::new(SimulatedNetwork::<CBCMessage<MsgType>>::new(&members));

    let mut instances: HashMap<NodeId, Cbc> = members
        .iter()
        .filter(|&&id| id != owner)
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, id);
            (id, Cbc::new(owner, quorum, threshold_keys))
        })
        .collect();

    {
        let handle = NodeHandle::new(owner, &bus);
        let quorum = fixtures::quorum_info(n, f, owner);
        let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, owner);
        let cbc = Cbc::new_with_propose(owner, quorum, threshold_keys, value, &handle);
        instances.insert(owner, cbc);
    }

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
                    "CBC cluster simulation did not converge"
                );

                let handle = NodeHandle::new(id, &bus);
                let stored = fixtures::stored_msg(from, id, msg);
                instances
                    .get_mut(&id)
                    .unwrap()
                    .process_message(stored, &handle)
                    .unwrap();
            }
        }

        if !progressed {
            break;
        }
    }

    (instances, keyset, cbc_keyset)
}

#[test]
fn test_cbc_full_cluster_all_nodes_finalize_same_value() {
    let (instances, _keyset, cbc_keyset) = run_cbc_cluster(N, F, NodeId(0), 55);

    assert_eq!(instances.len(), N);

    let digest = digest_of(&55u8);

    for (_, instance) in instances {
        let (value, got_digest, signature) = instance.finalize().unwrap();
        assert_eq!(value, 55);
        assert_eq!(got_digest, digest);

        // Self-certifying: any node can verify the Finish signature purely
        // from the CBC public key, without ever having collected its own
        // echo shares.
        cbc_keyset
            .public_key_set()
            .verify(digest.as_ref(), &signature)
            .expect("Finish signature should independently verify");
    }
}

/// A node that misses SEND/ECHO entirely (e.g. joins late, or the owner
/// equivocates and never sends it a direct SEND) can still verify and
/// adopt a FINISH it receives, demonstrating CBC's self-certifying
/// property -- the whole point of using it as MVBA's proposal-echo step.
#[test]
fn test_cbc_node_that_never_saw_send_can_still_adopt_finish() {
    let owner = NodeId(0);
    let late_joiner_id = NodeId(3);

    let (mut instances, keyset, cbc_keyset) = run_cbc_cluster(N, F, owner, 9);
    let (_, _, finish_signature) = instances.remove(&NodeId(1)).unwrap().finalize().unwrap();

    let late_quorum = fixtures::quorum_info(N, F, late_joiner_id);
    let late_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, late_joiner_id);
    let mut late_joiner = Cbc::new(owner, late_quorum, late_keys);
    let network = MockNetwork::new();

    let finish_msg = CBCMessage::Finish(9, finish_signature);
    let result = late_joiner
        .process_message(
            fixtures::stored_msg(owner, late_joiner_id, finish_msg),
            &network,
        )
        .unwrap();

    assert!(matches!(result, CBCResult::Finalized));

    let (value, _, _) = late_joiner.finalize().unwrap();
    assert_eq!(value, 9);
}
