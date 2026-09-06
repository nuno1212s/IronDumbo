use crate::dumbo2::epoch::{Dumbo2Round, EpochResult};
use crate::dumbo2::message::{Dumbo2Message, Dumbo2MessageType, Dumbo2Serialization};
use crate::dumbo2::protocol::{Dumbo2PSerialization, DumboRQ, ShareableDumbo2PMessage};
use crate::multi_valued_byzantine_agreement::messages::MVBAMessage;
use crate::multi_valued_byzantine_agreement::mvba::MultiValuedByzantineAgreement;
use crate::provable_reliable_broadcast::messages::PRBCMessage;
use crate::provable_reliable_broadcast::provable_reliable_broadcast::ProvableReliableBroadcastInstance;
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::rq_aggregator::RequestAggregator;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::signature::{KeyPair, PublicKey};
use atlas_common::error::Result;
use atlas_common::node_id::{NodeId, NodeType};
use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::peer_addr::PeerAddr;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{
    Buf, SerializedMessage, StoredMessage, StoredSerializedMessage, WireMessage,
};
use atlas_communication::reconfiguration::{NetworkInformationProvider, NodeInfo};
use atlas_core::messages::{ClientRqInfo, ForwardedRequestsMessage, SessionBased};
use atlas_core::ordering_protocol::networking::OrderProtocolSendNode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TestRequest(pub u64);

impl Orderable for TestRequest {
    fn sequence_number(&self) -> SeqNo {
        SeqNo::from(self.0 as u32)
    }
}

impl SessionBased for TestRequest {
    fn session_number(&self) -> SeqNo {
        SeqNo::from(0u32)
    }
}

pub(super) type PR = ProvableReliableBroadcastInstance<DumboRQ<TestRequest>>;
pub(super) type MV = MultiValuedByzantineAgreement;
pub(super) type Ser = Dumbo2PSerialization<TestRequest, PR, MV>;
pub(super) type TestDumbo2Message =
    <Ser as atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage<
        TestRequest,
    >>::ProtocolMessage;
pub(super) type TestDumbo2Round = Dumbo2Round<TestRequest, PR, MV>;

pub(super) struct SharedBus {
    queues: Mutex<HashMap<NodeId, Vec<(NodeId, TestDumbo2Message)>>>,
}

impl SharedBus {
    pub(super) fn new(nodes: &[NodeId]) -> Arc<Self> {
        let queues = nodes.iter().map(|&id| (id, Vec::new())).collect();

        Arc::new(Self {
            queues: Mutex::new(queues),
        })
    }

    fn send(&self, from: NodeId, to: NodeId, msg: TestDumbo2Message) {
        self.queues
            .lock()
            .unwrap()
            .entry(to)
            .or_default()
            .push((from, msg));
    }

    fn broadcast(
        &self,
        from: NodeId,
        targets: impl Iterator<Item = NodeId>,
        msg: TestDumbo2Message,
    ) {
        for to in targets {
            self.send(from, to, msg.clone());
        }
    }

    pub(super) fn deliver_next(&self, to: NodeId) -> Option<(NodeId, TestDumbo2Message)> {
        let mut queues = self.queues.lock().unwrap();
        let queue = queues.get_mut(&to)?;
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    }
}

pub(super) struct TestNetworkInfo {
    own_node: NodeInfo,
    key: Arc<KeyPair>,
}

impl TestNetworkInfo {
    fn new(node_id: NodeId) -> Self {
        let key = KeyPair::from_bytes(&[0u8; 32]).expect("failed to build deterministic test key");

        let own_node = NodeInfo::new(
            node_id,
            NodeType::Replica,
            PublicKey::from(key.public_key()),
            PeerAddr::new(
                format!("127.0.0.1:{}", 21000 + node_id.0)
                    .parse()
                    .expect("valid socket addr"),
                String::from("localhost"),
            ),
        );

        Self {
            own_node,
            key: Arc::new(key),
        }
    }
}

impl NetworkInformationProvider for TestNetworkInfo {
    fn own_node_info(&self) -> &NodeInfo {
        &self.own_node
    }

    fn get_key_pair(&self) -> &Arc<KeyPair> {
        &self.key
    }

    fn get_node_info(&self, _node: &NodeId) -> Option<NodeInfo> {
        None
    }
}

pub(super) struct TestSendNode {
    own_id: NodeId,
    network_info: Arc<TestNetworkInfo>,
    bus: Arc<SharedBus>,
}

impl TestSendNode {
    pub(super) fn new(own_id: NodeId, bus: Arc<SharedBus>) -> Self {
        Self {
            own_id,
            network_info: Arc::new(TestNetworkInfo::new(own_id)),
            bus,
        }
    }
}

impl OrderProtocolSendNode<TestRequest, Ser> for TestSendNode {
    type NetworkInfoProvider = TestNetworkInfo;

    fn id(&self) -> NodeId {
        self.own_id
    }

    fn network_info_provider(&self) -> &Arc<Self::NetworkInfoProvider> {
        &self.network_info
    }

    fn forward_requests<I>(
        &self,
        _fwd_requests: ForwardedRequestsMessage<TestRequest>,
        _targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        Ok(())
    }

    fn send(&self, message: TestDumbo2Message, target: NodeId, _flush: bool) -> Result<()> {
        self.bus.send(self.own_id, target, message);
        Ok(())
    }

    fn send_signed(&self, message: TestDumbo2Message, target: NodeId, flush: bool) -> Result<()> {
        self.send(message, target, flush)
    }

    fn broadcast<I>(
        &self,
        message: TestDumbo2Message,
        targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.bus.broadcast(self.own_id, targets, message);
        Ok(())
    }

    fn broadcast_signed<I>(
        &self,
        message: TestDumbo2Message,
        targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.broadcast(message, targets)
    }

    fn serialize_digest_message(
        &self,
        message: TestDumbo2Message,
    ) -> Result<(SerializedMessage<TestDumbo2Message>, Digest)> {
        Ok((SerializedMessage::new(message, Buf::new()), Digest::blank()))
    }

    fn broadcast_serialized(
        &self,
        _messages: BTreeMap<NodeId, StoredSerializedMessage<TestDumbo2Message>>,
    ) -> std::result::Result<(), Vec<NodeId>> {
        Ok(())
    }
}

fn stored_message(
    from: NodeId,
    to: NodeId,
    msg: TestDumbo2Message,
) -> StoredMessage<TestDumbo2Message> {
    let wire_msg = WireMessage::new(
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

/// A cluster of `n` Dumbo2 replicas (tolerating `f` faults) driven entirely
/// in-process, mirroring `dumbo1::test::harness::Dumbo1TestCluster`. Unlike
/// Dumbo1, no separate "kickoff" step is needed: each round starts its own
/// PRBC broadcast immediately on construction.
pub(super) struct Dumbo2TestCluster {
    pub(super) nodes: HashMap<NodeId, TestDumbo2Round>,
    pub(super) send_nodes: HashMap<NodeId, Arc<TestSendNode>>,
    bus: Arc<SharedBus>,
    members: Vec<NodeId>,
}

impl Dumbo2TestCluster {
    pub(super) fn new(n: usize, f: usize) -> Self {
        Self::new_at_seq_no(n, f, SeqNo::from(0u32))
    }

    pub(super) fn new_at_seq_no(n: usize, f: usize, seq_no: SeqNo) -> Self {
        let members: Vec<NodeId> = (0..n).map(NodeId::from).collect();
        let bus = SharedBus::new(&members);
        // One shared keyset for the whole cluster: each node holds a
        // different private share of the *same* public key set, so
        // threshold signatures produced by one node verify for all others.
        let keyset = crate::testing::fixtures::make_keyset(f);
        let cbc_keyset = crate::testing::fixtures::make_cbc_keyset(f);

        let mut nodes = HashMap::new();
        let mut send_nodes = HashMap::new();

        for &id in &members {
            let quorum_info = QuorumInfo::new(n, f, members.clone(), id);
            let threshold_keys = ThresholdKeys::new(
                keyset.public_key_set(),
                keyset.private_key_part(id.0 as usize),
                cbc_keyset.public_key_set(),
                cbc_keyset.private_key_part(id.0 as usize),
            );

            let (_tx, rx) = atlas_common::channel::sync::new_bounded_sync(1, None::<String>);
            let batch_output = rx.into();
            let request_aggregator =
                Arc::new(RequestAggregator::new(batch_output, quorum_info.clone()));

            let send_node = Arc::new(TestSendNode::new(id, bus.clone()));

            let own_value = vec![stored_message_of_test_request(id, TestRequest(id.0 as u64))];

            let round: TestDumbo2Round = Dumbo2Round::new(
                seq_no,
                quorum_info,
                threshold_keys,
                own_value,
                request_aggregator,
                &send_node,
            );

            nodes.insert(id, round);
            send_nodes.insert(id, send_node);
        }

        Self {
            nodes,
            send_nodes,
            bus,
            members,
        }
    }

    pub(super) fn run_to_fixed_point(
        &mut self,
        max_messages: usize,
    ) -> Vec<(NodeId, crate::dumbo2::epoch::EpochResult)> {
        self.run_to_fixed_point_excluding(None, max_messages)
    }

    /// Like [`Self::run_to_fixed_point`], but never delivers to (or
    /// processes messages on behalf of) `silent`, simulating a node that
    /// never participates.
    pub(super) fn run_to_fixed_point_excluding(
        &mut self,
        silent: Option<NodeId>,
        max_messages: usize,
    ) -> Vec<(NodeId, crate::dumbo2::epoch::EpochResult)> {
        let mut observed = Vec::new();
        let mut delivered = 0usize;

        loop {
            let mut progressed = false;

            for &id in &self.members {
                if Some(id) == silent {
                    continue;
                }

                loop {
                    let Some((from, msg)) = self.bus.deliver_next(id) else {
                        break;
                    };
                    progressed = true;
                    delivered += 1;
                    assert!(
                        delivered < max_messages,
                        "Dumbo2 cluster simulation did not converge within {delivered} messages"
                    );

                    let stored = stored_message(from, id, msg);
                    let shareable: ShareableDumbo2PMessage<TestRequest, PR, MV> = Arc::new(stored);

                    let network = self.send_nodes[&id].clone();
                    let result = self
                        .nodes
                        .get_mut(&id)
                        .unwrap()
                        .process_message(shareable, &network)
                        .expect("dumbo2 round should not error processing a well-formed message");

                    observed.push((id, result));
                }
            }

            if !progressed {
                break;
            }
        }

        observed
    }

    pub(super) fn prbc_done_count(&self, id: NodeId) -> usize {
        self.nodes[&id].prbc_done_count()
    }

    pub(super) fn decided_size(&self, id: NodeId) -> Option<usize> {
        self.nodes[&id].decided_size()
    }

    pub(super) fn members(&self) -> &[NodeId] {
        &self.members
    }

    pub(super) fn debug_dump(&self) -> String {
        let mut out = String::new();
        for &id in &self.members {
            out.push_str(&format!("{id:?}: {:?}\n", self.nodes[&id]));
        }
        out
    }
}

fn stored_message_of_test_request(from: NodeId, rq: TestRequest) -> StoredMessage<TestRequest> {
    let wire_msg = WireMessage::new(
        from,
        from,
        MessageModule::Application,
        Buf::new(),
        0,
        Some(Digest::blank()),
        None,
    );

    StoredMessage::new(wire_msg.header().clone(), rq)
}
