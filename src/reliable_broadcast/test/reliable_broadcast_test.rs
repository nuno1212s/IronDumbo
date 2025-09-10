use crate::quorum_info::quorum_info::QuorumInfo;
use crate::rbc::ReliableBroadcastSendNode;
use crate::reliable_broadcast::messages::ReliableBroadcastMessage;
use crate::reliable_broadcast::reliable_broadcast::{
    ReliableBroadcastInstance, ReliableBroadcastResult,
};
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage};
use std::cell::RefCell;
use std::sync::Arc;

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

impl ReliableBroadcastSendNode<MsgType> for MockNetwork {
    fn send(
        &self,
        message: ReliableBroadcastMessage<MsgType>,
        target: NodeId,
        flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.send_signed(message, target, flush)
    }
    fn send_signed(
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
    fn broadcast_signed<I>(
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

fn quorum_info(n: usize, f: usize) -> QuorumInfo {
    QuorumInfo::new(n, f, (0..n).map(NodeId::from).collect())
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

const N: usize = 4;
const F: usize = 1;

#[test]
fn test_send_phase() {
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum);
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(42);
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum);
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(42);

    // Simulate SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    network: &Arc<MockNetwork>,
    digest: Digest,
) {
    for i in 0..(quorum.quorum_size() - quorum.f()) {
        let echo_msg = stored_msg(
            NodeId::from(i),
            sender,
            ReliableBroadcastMessage::Echo(digest),
        );
        rbc.process_message(echo_msg, &network);
    }
}

#[test]
fn test_ready_phase_and_deliver() {
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(42);

    // Simulate SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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

    assert_eq!(requests.len(), 0, "No requests should be finalized");
    assert_eq!(digest, digest, "Digest should match the one sent");
}

#[test]
fn test_not_enough_echoes_no_ready() {
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(1);

    // SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(2);

    // SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(3);

    // SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(4);
    let wrong_digest = make_digest(99);

    // SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(5);

    // First SEND
    let send_msg = stored_msg(
        sender,
        sender,
        ReliableBroadcastMessage::Send(vec![], digest),
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
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
    let quorum = quorum_info(N, F);
    let sender = sender_from_quorum(&quorum);
    let mut rbc = ReliableBroadcastInstance::<MsgType>::new(sender, quorum.clone());
    let network = Arc::new(MockNetwork::new());
    let digest = make_digest(7);

    // READY before SEND
    let ready_msg = stored_msg(NodeId(1), sender, ReliableBroadcastMessage::Ready(digest));
    let result = rbc.process_message(ready_msg, &network);

    assert!(
        matches!(result, ReliableBroadcastResult::MessageQueued),
        "READY before SEND should be queued"
    );
}
