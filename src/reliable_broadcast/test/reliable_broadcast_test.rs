use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::{ReliableBroadcast, ReliableBroadcastSendNode};
use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use crate::reliable_broadcast::reliable_broadcast::{
    ReliableBroadcastInstance, ReliableBroadcastResult,
};
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage};
use std::cell::RefCell;
use std::collections::HashMap;

// Mock network to capture broadcasts
struct MockNetwork {
    sent: RefCell<Vec<(ReliableBroadcastMessage<u8>, Vec<NodeId>)>>,
}

impl MockNetwork {
    fn new() -> Self {
        Self {
            sent: RefCell::new(vec![]),
        }
    }
}

type MsgType = u8;

impl ReliableBroadcastSendNode<ReliableBroadcastMessage<MsgType>> for MockNetwork {
    fn send(
        &self,
        message: ReliableBroadcastMessage<MsgType>,
        target: NodeId,
        _flush: bool,
    ) -> atlas_common::error::Result<()> {
        let targets_vec: Vec<NodeId> = vec![target];
        self.sent.borrow_mut().push((message, targets_vec));
        Ok(())
    }

    fn broadcast<I>(
        &self,
        message: ReliableBroadcastMessage<MsgType>,
        targets: I,
    ) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        let targets_vec: Vec<NodeId> = targets.collect();
        self.sent.borrow_mut().push((message, targets_vec));
        Ok(())
    }
}

fn quorum_info(n: usize, f: usize, node: NodeId) -> QuorumInfo {
    QuorumInfo::new(n, f, (0..n).map(NodeId::from).collect(), node)
}

fn sender_from_quorum(quorum: &QuorumInfo) -> NodeId {
    quorum
        .quorum_members()
        .first()
        .cloned()
        .unwrap_or(NodeId(0))
}

fn make_digest(val: MsgType) -> Digest {
    let mut context = Context::new();
    context.update(&[val]);

    context.finish()
}

fn stored_msg(
    from: NodeId,
    to: NodeId,
    msg: ReliableBroadcastMessage<MsgType>,
) -> StoredMessage<ReliableBroadcastMessage<MsgType>> {
    let wire_msg = atlas_communication::message::WireMessage::new(
        from,
        to,
        MessageModule::Application,
        Buf::new(),
        0,
        Some(Digest::blank()),
        None,
    );

    StoredMessage::new(wire_msg.header().clone(), msg)
}

fn stored_msg_digest(
    from: NodeId,
    to: NodeId,
    msg: ReliableBroadcastMessage<MsgType>,
    digest: Option<Digest>,
) -> StoredMessage<ReliableBroadcastMessage<MsgType>> {
    let wire_msg = atlas_communication::message::WireMessage::new(
        from,
        to,
        MessageModule::Application,
        Buf::new(),
        0,
        Some(digest.unwrap_or(Digest::blank())),
        None,
    );

    StoredMessage::new(wire_msg.header().clone(), msg)
}

const N: usize = 4;
const F: usize = 1;

#[test]
fn test_send_phase() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum);
    let network = MockNetwork::new();
    let digest = make_digest(42);
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );

    // Process SEND
    let result = rbc.process_message(send_msg.clone(), &network);
    // Should broadcast ECHO
    assert!(matches!(result, ReliableBroadcastResult::Progressed(_)));
    let sent = &network.sent.borrow()[0];
    assert!(matches!(sent.0, ReliableBroadcastMessage::Echo(d) if d == digest));
}

#[test]
fn test_echo_phase() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum);
    let network = MockNetwork::new();
    let digest = make_digest(42);

    // Simulate SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // Simulate ECHO from 3 nodes (n-f)
    for i in 1..=3 {
        let echo_msg = stored_msg(NodeId(i), sender, ReliableBroadcastMessage::Echo(digest));
        rbc.process_message(echo_msg, &network);
    }
    // Should broadcast READY after n-f echoes
    let sent = network.sent.borrow();
    assert!(
        sent.iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(d) if *d == digest))
    );
}

fn simulate_echo(
    rbc: &mut ReliableBroadcastInstance<MsgType>,
    quorum: &QuorumInfo,
    sender: NodeId,
    network: &MockNetwork,
    digest: Digest,
) {
    for i in 0..(quorum.quorum_size() - quorum.f()) {
        let echo_msg = stored_msg(
            NodeId::from(i),
            sender,
            ReliableBroadcastMessage::Echo(digest),
        );
        rbc.process_message(echo_msg, network);
    }
}

#[test]
fn test_ready_phase_and_deliver() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(42);

    // Simulate SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // Simulate ECHO from n-f nodes to trigger READY
    simulate_echo(&mut rbc, &quorum, sender, &network, digest);

    // Simulate READY from 2f+1 nodes (3 nodes)
    let mut finalized = false;
    for i in 0..(quorum.f() * 2 + 1) {
        let ready_msg = stored_msg(
            NodeId::from(i),
            sender,
            ReliableBroadcastMessage::Ready(digest),
        );
        let result = rbc.process_message(ready_msg, &network);
        if let ReliableBroadcastResult::Finalized = result {
            finalized = true;
        }
    }

    assert!(finalized, "RBC should finalize after receiving 2f+1 READYs");

    let (requests, digest) = rbc.finalize().unwrap();

    assert_eq!(requests, 0, "No requests should be finalized");
    assert_eq!(digest, digest, "Digest should match the one sent");
}

#[test]
fn test_not_enough_echoes_no_ready() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(1);

    // SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // Only 1 ECHO (less than n-f)
    let echo_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Echo(digest));
    rbc.process_message(echo_msg, &network);

    // Should NOT broadcast READY
    let sent = network.sent.borrow();
    assert!(
        !sent
            .iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(_))),
        "Should not broadcast READY with insufficient ECHOs"
    );
}

#[test]
fn test_duplicate_echoes_ignored() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(2);

    // SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // ECHO from node 1 twice
    let echo_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Echo(digest));
    rbc.process_message(echo_msg.clone(), &network);
    rbc.process_message(echo_msg, &network);

    // Only one ECHO should be counted, so still not enough for READY
    let sent = network.sent.borrow();
    assert!(
        !sent
            .iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(_))),
        "Duplicate ECHO should not trigger READY"
    );
}

#[test]
fn test_duplicate_readies_ignored() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(3);

    // SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // Enough ECHOs to trigger READY
    simulate_echo(&mut rbc, &quorum, sender, &network, digest);

    // READY from node 1 twice
    let ready_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Ready(digest));
    let mut finalized = false;
    for _ in 0..2 {
        let result = rbc.process_message(ready_msg.clone(), &network);
        if let ReliableBroadcastResult::Finalized = result {
            finalized = true;
        }
    }
    // Not enough READYs for finalization
    assert!(!finalized, "Duplicate READY should not finalize");
}

#[test]
fn test_mismatched_digest_ignored() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(4);
    let wrong_digest = make_digest(99);

    // SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // ECHO with wrong digest
    let echo_msg = stored_msg(
        NodeId(1),
        sender,
        ReliableBroadcastMessage::Echo(wrong_digest),
    );
    let result = rbc.process_message(echo_msg, &network);

    // Should be queued/ignored
    assert!(
        matches!(result, ReliableBroadcastResult::MessageQueued),
        "Mismatched digest should be queued/ignored"
    );
}

#[test]
fn test_send_after_proposed_ignored() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(5);

    // First SEND
    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg.clone(), &network);

    // Second SEND (should be ignored)
    let result = rbc.process_message(send_msg, &network);
    assert!(
        matches!(result, ReliableBroadcastResult::MessageIgnored),
        "Second SEND should be ignored"
    );
}

#[test]
fn test_echo_before_send_queued() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(6);

    // ECHO before SEND
    let echo_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Echo(digest));
    let result = rbc.process_message(echo_msg, &network);

    assert!(
        matches!(result, ReliableBroadcastResult::MessageQueued),
        "ECHO before SEND should be queued"
    );
}

#[test]
fn test_ready_before_send_queued() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(7);

    // READY before SEND
    let ready_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Ready(digest));
    let result = rbc.process_message(ready_msg, &network);

    assert!(
        matches!(result, ReliableBroadcastResult::MessageQueued),
        "READY before SEND should be queued"
    );
}

#[test]
fn test_byzantine_sender_equivocation() {
    let quorum = quorum_info(N, F, NodeId(0));
    let members = quorum.quorum_members().clone();
    let proposer = NodeId(0);

    let digest_a = make_digest(1);
    let digest_b = make_digest(2);

    let bus = RefCell::new(SimulatedNetwork::<ReliableBroadcastMessage<MsgType>>::new(
        &members,
    ));
    let mut instances: HashMap<NodeId, ReliableBroadcastInstance<MsgType>> = members
        .iter()
        .map(|&id| {
            (
                id,
                ReliableBroadcastInstance::<MsgType>::new(proposer, quorum_info(N, F, id)),
            )
        })
        .collect();

    // Byzantine sender sends value A to half the quorum and value B to the other half.
    let (split_a, split_b) = members.split_at(2);

    for &member in split_a {
        let handle = NodeHandle::new(member, &bus);
        let send_msg = stored_msg_digest(
            proposer,
            member,
            ReliableBroadcastMessage::Send(1),
            Some(digest_a),
        );
        instances
            .get_mut(&member)
            .unwrap()
            .process_message(send_msg, &handle);
    }
    for &member in split_b {
        let handle = NodeHandle::new(member, &bus);
        let send_msg = stored_msg_digest(
            proposer,
            member,
            ReliableBroadcastMessage::Send(2),
            Some(digest_b),
        );
        instances
            .get_mut(&member)
            .unwrap()
            .process_message(send_msg, &handle);
    }

    let mut finalized_nodes = Vec::new();
    loop {
        let mut progressed = false;

        for &member in &members {
            loop {
                let next = bus.borrow_mut().deliver_next(member);
                let Some((from, msg)) = next else {
                    break;
                };
                progressed = true;

                let handle = NodeHandle::new(member, &bus);
                let stored = stored_msg(from, member, msg);
                let result = instances
                    .get_mut(&member)
                    .unwrap()
                    .process_message(stored, &handle);

                if matches!(result, ReliableBroadcastResult::Finalized) {
                    finalized_nodes.push(member);
                }
            }
        }

        if !progressed {
            break;
        }
    }

    // A 2/2 split under n=4,f=1 cannot gather 2f+1 matching READYs on either branch, so
    // neither value should ever finalize: equivocation must not let correct nodes disagree.
    assert!(
        finalized_nodes.is_empty(),
        "equivocation split should not allow any node to finalize, but {finalized_nodes:?} did"
    );
}

#[test]
fn test_concurrent_n_senders() {
    let receiver = NodeId(0);
    let quorum = quorum_info(N, F, receiver);
    let members = quorum.quorum_members().clone();
    let network = MockNetwork::new();

    let mut instances: HashMap<NodeId, ReliableBroadcastInstance<MsgType>> = members
        .iter()
        .map(|&sender| {
            (
                sender,
                ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone()),
            )
        })
        .collect();

    for &sender in &members {
        let digest = make_digest(sender.0 as u8);
        let rbc = instances.get_mut(&sender).unwrap();

        let send_msg = stored_msg_digest(
            sender,
            receiver,
            ReliableBroadcastMessage::Send(sender.0 as u8),
            Some(digest),
        );
        rbc.process_message(send_msg, &network);

        simulate_echo(rbc, &quorum, sender, &network, digest);

        let mut finalized = false;
        for i in 0..(quorum.f() * 2 + 1) {
            let ready_msg = stored_msg(
                NodeId::from(i),
                sender,
                ReliableBroadcastMessage::Ready(digest),
            );
            if let ReliableBroadcastResult::Finalized = rbc.process_message(ready_msg, &network) {
                finalized = true;
            }
        }

        assert!(
            finalized,
            "broadcast from sender {sender:?} should finalize independently of the others"
        );
    }

    for &sender in &members {
        let (value, digest) = instances.remove(&sender).unwrap().finalize().unwrap();
        assert_eq!(value, sender.0 as u8);
        assert_eq!(digest, make_digest(sender.0 as u8));
    }
}

#[test]
fn test_ready_amplification() {
    let quorum = quorum_info(N, F, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(8);

    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // A READY that arrives before we've echoed enough to reach the Echoed state must be
    // queued, not dropped, and must not itself trigger a READY broadcast.
    let early_ready = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Ready(digest));
    let result = rbc.process_message(early_ready, &network);
    assert!(matches!(result, ReliableBroadcastResult::MessageQueued));
    assert!(
        !network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(_))),
        "should not broadcast READY before reaching the Echoed state"
    );

    // Enough ECHOes arrive to reach Echoed (broadcasting our own READY) and the queued
    // early READY becomes eligible for processing.
    simulate_echo(&mut rbc, &quorum, sender, &network, digest);
    assert!(rbc.has_pending(), "the early READY should still be queued");

    let mut finalized = false;
    while let Some(pending) = rbc.poll() {
        if let ReliableBroadcastResult::Finalized = rbc.process_message(pending, &network) {
            finalized = true;
        }
    }
    for i in 2..=(quorum.f() * 2 + 1) {
        let ready_msg = stored_msg(
            NodeId::from(i),
            sender,
            ReliableBroadcastMessage::Ready(digest),
        );
        if let ReliableBroadcastResult::Finalized = rbc.process_message(ready_msg, &network) {
            finalized = true;
        }
    }

    assert!(
        finalized,
        "the queued early READY plus fresh READYs should reach 2f+1 and finalize"
    );
}

/// Demonstrates the divergence from Algorithm 5 line 9: the paper requires
/// n-f distinct ECHOes before a node may multicast READY -- the exact
/// Bracha bound that guarantees any two ECHO-quorums intersect in a
/// strictly-honest node. `ReliableBroadcastInstance::process_message`
/// instead gates the transition on
/// `quorum_info.quorum_size() - quorum_info.f()`, and `quorum_size()` is
/// itself already `n - f` (see `QuorumInfo::new`), so the actual threshold
/// evaluated is `(n - f) - f == n - 2f`, i.e. `f + 1` for n = 3f+1 -- far
/// below the required n-f = 2f+1.
///
/// This test asserts the *spec-mandated* behavior (READY must not be
/// broadcast before n-f ECHOes, and must be broadcast once the n-f'th
/// arrives). It currently fails: the implementation broadcasts READY after
/// only f+1 ECHOes (see the threshold computation above), so the
/// "must-not-broadcast-yet" assertion trips well before n-f is reached.
#[test]
fn test_echo_threshold_matches_paper_spec_n_minus_f() {
    const N7: usize = 7;
    const F2: usize = 2;
    let paper_required_echoes = N7 - F2; // n - f = 5, per Algorithm 5 line 9

    let quorum = quorum_info(N7, F2, NodeId(0));
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = MockNetwork::new();
    let digest = make_digest(77);

    let send_msg = stored_msg_digest(
        sender,
        sender,
        ReliableBroadcastMessage::Send(0),
        Some(digest),
    );
    rbc.process_message(send_msg, &network);

    // One short of n-f: READY must NOT have been broadcast yet.
    for i in 1..paper_required_echoes {
        let echo_msg = stored_msg(NodeId::from(i), sender, ReliableBroadcastMessage::Echo(digest));
        rbc.process_message(echo_msg, &network);
    }
    assert!(
        !network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(_))),
        "READY must not be broadcast before n-f={paper_required_echoes} distinct ECHOes have \
         been received (Algorithm 5 line 9); the implementation broadcasts it far earlier, \
         after only f+1 ECHOes"
    );

    // The n-f'th ECHO should be the one that triggers READY.
    let echo_msg = stored_msg(
        NodeId::from(paper_required_echoes),
        sender,
        ReliableBroadcastMessage::Echo(digest),
    );
    rbc.process_message(echo_msg, &network);

    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, ReliableBroadcastMessage::Ready(d) if *d == digest)),
        "READY should be broadcast once n-f={paper_required_echoes} distinct ECHOes have arrived"
    );
}

/// Asserts RBC's Totality property ("if an honest node outputs v, then all
/// honest nodes output v", Section 3). The paper's Algorithm 5 lets *any*
/// node reconstruct v from n-2f ECHOes carrying erasure-coded chunks (line
/// 16), specifically so nodes the sender never directly reached can still
/// deliver. This currently fails: `ReliableBroadcastMessage::Echo`/`Ready`
/// carry only a `Digest` (no data), and `process_message`'s guards require
/// local state to already be `Proposed`/`Echoed` -- reachable only by
/// having received the sender's direct SEND -- before an ECHO/READY is
/// processed instead of just queued. A node the (possibly Byzantine)
/// sender skips is stuck in `Init` forever, no matter how many ECHO/READY
/// messages arrive for it.
#[test]
fn test_all_honest_nodes_finalize_even_if_sender_skips_one_totality() {
    let quorum = quorum_info(N, F, NodeId(0));
    let members = quorum.quorum_members().clone();
    let proposer = members[0];
    let skipped = members[3];
    let digest = make_digest(21);

    let bus = RefCell::new(SimulatedNetwork::<ReliableBroadcastMessage<MsgType>>::new(
        &members,
    ));
    let mut instances: HashMap<NodeId, ReliableBroadcastInstance<MsgType>> = members
        .iter()
        .map(|&id| {
            (
                id,
                ReliableBroadcastInstance::<MsgType>::new(proposer, quorum_info(N, F, id)),
            )
        })
        .collect();

    // The sender delivers SEND directly to everyone *except* `skipped`. The
    // system model (Section 2.1) only guarantees delivery between honest
    // nodes; the initial SEND is a direct unicast from the sender, who may
    // itself be faulty and simply omit some recipients.
    for &member in &members {
        if member == skipped {
            continue;
        }

        let handle = NodeHandle::new(member, &bus);
        let send_msg = stored_msg_digest(
            proposer,
            member,
            ReliableBroadcastMessage::Send(9),
            Some(digest),
        );
        instances
            .get_mut(&member)
            .unwrap()
            .process_message(send_msg, &handle);
    }

    let mut finalized_nodes = Vec::new();
    loop {
        let mut progressed = false;

        for &member in &members {
            loop {
                let next = bus.borrow_mut().deliver_next(member);
                let Some((from, msg)) = next else {
                    break;
                };
                progressed = true;

                let handle = NodeHandle::new(member, &bus);
                let stored = stored_msg(from, member, msg);
                let result = instances
                    .get_mut(&member)
                    .unwrap()
                    .process_message(stored, &handle);

                if matches!(result, ReliableBroadcastResult::Finalized) {
                    finalized_nodes.push(member);
                }
            }
        }

        if !progressed {
            break;
        }
    }

    // Spec-mandated (Section 3, RBC Totality): every honest node -- not
    // just the ones the sender directly reached -- must finalize.
    assert_eq!(
        finalized_nodes.len(),
        members.len(),
        "all {} honest nodes should have finalized, including the one the sender skipped over \
         (RBC Totality); only {:?} did",
        members.len(),
        finalized_nodes
    );

    for &member in &members {
        let (value, got_digest) = instances.remove(&member).unwrap().finalize().unwrap();
        assert_eq!(value, 9);
        assert_eq!(
            got_digest, digest,
            "node {member:?} should have recovered the same value as everyone else, including \
             the one the sender never sent SEND to directly -- e.g. via reconstruction from \
             n-2f ECHOes as Algorithm 5 line 16 describes"
        );
    }
}

#[test]
fn test_full_4node_rbc_simulation() {
    let quorum = quorum_info(N, F, NodeId(0));
    let members = quorum.quorum_members().clone();
    let proposer = members[0];
    let digest = make_digest(11);

    let bus = RefCell::new(SimulatedNetwork::<ReliableBroadcastMessage<MsgType>>::new(
        &members,
    ));
    let mut instances: HashMap<NodeId, ReliableBroadcastInstance<MsgType>> = members
        .iter()
        .map(|&id| {
            (
                id,
                ReliableBroadcastInstance::<MsgType>::new(proposer, quorum_info(N, F, id)),
            )
        })
        .collect();

    // The proposer's SEND is assumed already delivered to every node by the transport.
    for &member in &members {
        let handle = NodeHandle::new(member, &bus);
        let send_msg = stored_msg_digest(
            proposer,
            member,
            ReliableBroadcastMessage::Send(9),
            Some(digest),
        );
        instances
            .get_mut(&member)
            .unwrap()
            .process_message(send_msg, &handle);
    }

    let mut finalized_nodes = Vec::new();
    loop {
        let mut progressed = false;

        for &member in &members {
            loop {
                let next = bus.borrow_mut().deliver_next(member);
                let Some((from, msg)) = next else {
                    break;
                };
                progressed = true;

                let handle = NodeHandle::new(member, &bus);
                let stored = stored_msg(from, member, msg);
                let result = instances
                    .get_mut(&member)
                    .unwrap()
                    .process_message(stored, &handle);

                if matches!(result, ReliableBroadcastResult::Finalized) {
                    finalized_nodes.push(member);
                }
            }
        }

        if !progressed {
            break;
        }
    }

    assert_eq!(
        finalized_nodes.len(),
        members.len(),
        "every node should finalize the honestly broadcast value"
    );

    for &member in &members {
        let (value, got_digest) = instances.remove(&member).unwrap().finalize().unwrap();
        assert_eq!(value, 9);
        assert_eq!(got_digest, digest);
    }
}
