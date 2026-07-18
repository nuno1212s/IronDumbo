use crate::async_bin_agreement::async_bin_agreement::AsyncBinaryAgreement;
use crate::committee_election::{
    CommitteeElectionProtocol, CommitteeElectionResult, CommitteeElectionSendNode,
};
use crate::dumbo1::config::Dumbo1Config;
use crate::dumbo1::epoch::DumboRound;
use crate::dumbo1::message::{DumboMessage, DumboMessageType, DumboSerialization};
use crate::dumbo1::protocol::{DumboPSerialization, IndexType, ShareableDumboPMessage};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use crate::reliable_broadcast::reliable_broadcast::ReliableBroadcastInstance;
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
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A minimal client-request stand-in. Dumbo1 happily proposes/agrees on empty
/// batches, so the integration tests below never need to populate real
/// requests through the (unused) request aggregator channel.
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

pub(super) type VR = ReliableBroadcastInstance<Vec<StoredMessage<TestRequest>>>;
pub(super) type IR = ReliableBroadcastInstance<IndexType>;
pub(super) type A = AsyncBinaryAgreement;
pub(super) type CE = TestCommitteeElection;
pub(super) type Ser = DumboPSerialization<TestRequest, VR, IR, A, CE>;
pub(super) type TestDumboMessage =
    <Ser as atlas_core::ordering_protocol::networking::serialize::OrderingProtocolMessage<
        TestRequest,
    >>::ProtocolMessage;
pub(super) type TestDumboRound = DumboRound<CE, TestRequest, VR, IR, A>;

/// A trivial committee election: the committee is a deterministic function of
/// the quorum alone (first `committee_size` members by NodeId), so every
/// correct node computes the same committee without needing to exchange any
/// real votes. A single kickoff message is still required to drive the
/// `WaitingForCommitteeElection -> Running` transition, since `DumboRound`
/// only advances that state machine in response to a processed message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CommitteeKickoff;

#[derive(Debug)]
pub(super) struct TestCommitteeElection {
    committee: Vec<NodeId>,
}

impl CommitteeElectionProtocol for TestCommitteeElection {
    type Message = CommitteeKickoff;
    type CEError = std::convert::Infallible;

    fn new(quorum_info: QuorumInfo, committee_size: usize) -> Self {
        let mut committee = quorum_info.quorum_members().clone();
        committee.sort();
        committee.truncate(committee_size.max(1));

        Self { committee }
    }

    fn poll(&mut self) -> Option<StoredMessage<Self::Message>> {
        None
    }

    fn process_message<NT>(
        &mut self,
        _message: StoredMessage<Self::Message>,
        _network: &NT,
    ) -> std::result::Result<CommitteeElectionResult, Self::CEError>
    where
        NT: CommitteeElectionSendNode<Self::Message>,
    {
        Ok(CommitteeElectionResult::Decided)
    }

    fn finalize(self) -> std::result::Result<Vec<NodeId>, Self::CEError> {
        Ok(self.committee)
    }
}

/// A shared, `Send + Sync` multi-node message bus for [`TestSendNode`],
/// mirroring `testing::network_sim::SimulatedNetwork` but usable from a type
/// that must satisfy `OrderProtocolSendNode: Send + Sync + 'static`.
pub(super) struct SharedBus {
    queues: Mutex<HashMap<NodeId, Vec<(NodeId, TestDumboMessage)>>>,
}

impl SharedBus {
    pub(super) fn new(nodes: &[NodeId]) -> Arc<Self> {
        let queues = nodes.iter().map(|&id| (id, Vec::new())).collect();

        Arc::new(Self {
            queues: Mutex::new(queues),
        })
    }

    fn send(&self, from: NodeId, to: NodeId, msg: TestDumboMessage) {
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
        msg: TestDumboMessage,
    ) {
        for to in targets {
            self.send(from, to, msg.clone());
        }
    }

    pub(super) fn deliver_next(&self, to: NodeId) -> Option<(NodeId, TestDumboMessage)> {
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
                format!("127.0.0.1:{}", 20000 + node_id.0)
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

    fn send(&self, message: TestDumboMessage, target: NodeId, _flush: bool) -> Result<()> {
        self.bus.send(self.own_id, target, message);
        Ok(())
    }

    fn send_signed(&self, message: TestDumboMessage, target: NodeId, flush: bool) -> Result<()> {
        self.send(message, target, flush)
    }

    fn broadcast<I>(
        &self,
        message: TestDumboMessage,
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
        message: TestDumboMessage,
        targets: I,
    ) -> std::result::Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.broadcast(message, targets)
    }

    fn serialize_digest_message(
        &self,
        message: TestDumboMessage,
    ) -> Result<(SerializedMessage<TestDumboMessage>, Digest)> {
        Ok((SerializedMessage::new(message, Buf::new()), Digest::blank()))
    }

    fn broadcast_serialized(
        &self,
        _messages: BTreeMap<NodeId, StoredSerializedMessage<TestDumboMessage>>,
    ) -> std::result::Result<(), Vec<NodeId>> {
        Ok(())
    }
}

fn stored_message(
    from: NodeId,
    to: NodeId,
    msg: TestDumboMessage,
) -> StoredMessage<TestDumboMessage> {
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

/// A cluster of `n` Dumbo1 replicas (tolerating `f` faults) driven entirely
/// in-process: each node's [`DumboRound`] is real production code, wired to
/// the others only through a [`SharedBus`], with no actual networking, TCP,
/// or signature verification involved.
pub(super) struct Dumbo1TestCluster {
    pub(super) nodes: HashMap<NodeId, TestDumboRound>,
    pub(super) send_nodes: HashMap<NodeId, Arc<TestSendNode>>,
    bus: Arc<SharedBus>,
    members: Vec<NodeId>,
    seq_no: SeqNo,
}

impl Dumbo1TestCluster {
    pub(super) fn new(n: usize, f: usize) -> Self {
        Self::new_at_seq_no(n, f, SeqNo::from(0u32))
    }

    /// Builds a cluster whose rounds are all pinned to the given sequence
    /// number. Used to independently exercise a "second epoch" without
    /// needing the full `Dumbo`/`install_seq_no` pipeline (this harness
    /// drives `DumboRound` directly, one round per node, not the multi-round
    /// `Dumbo` wrapper).
    pub(super) fn new_at_seq_no(n: usize, f: usize, seq_no: SeqNo) -> Self {
        let members: Vec<NodeId> = (0..n).map(NodeId::from).collect();
        let bus = SharedBus::new(&members);

        let mut nodes = HashMap::new();
        let mut send_nodes = HashMap::new();

        for &id in &members {
            let quorum_info = QuorumInfo::new(n, f, members.clone(), id);
            let keyset = crate::testing::fixtures::make_keyset(f);
            let threshold_keys = ThresholdKeys::new(
                keyset.public_key_set(),
                keyset.private_key_part(id.0 as usize),
            );

            let (_tx, rx) = atlas_common::channel::sync::new_bounded_sync(1, None::<String>);
            let batch_output = rx.into();
            let request_aggregator =
                Arc::new(RequestAggregator::new(batch_output, quorum_info.clone()));

            let round = DumboRound::new(seq_no, quorum_info, threshold_keys, request_aggregator);

            nodes.insert(id, round);
            send_nodes.insert(id, Arc::new(TestSendNode::new(id, bus.clone())));
        }

        Self {
            nodes,
            send_nodes,
            bus,
            members,
            seq_no,
        }
    }

    /// Feeds the deterministic committee-election kickoff to every node,
    /// transitioning each round from `WaitingForCommitteeElection` to `Running`.
    pub(super) fn kickoff_committee_election(&mut self) {
        for &id in &self.members {
            let msg = DumboMessage::new(
                self.seq_no,
                DumboMessageType::CommitteeElectionMessage(CommitteeKickoff),
            );
            let stored = stored_message(id, id, msg);
            let shareable: ShareableDumboPMessage<TestRequest, VR, IR, A, CE> = Arc::new(stored);

            let network = self.send_nodes[&id].clone();
            self.nodes
                .get_mut(&id)
                .unwrap()
                .process_message(shareable, &network)
                .expect("committee election kickoff must be accepted");
        }
    }

    /// Delivers every currently-queued message to its destination, repeating
    /// until no node's inbox has anything left (a synchronous fixed point).
    /// Returns the sequence of `(node, EpochResult)` observed along the way.
    pub(super) fn run_to_fixed_point(
        &mut self,
        max_messages: usize,
    ) -> Vec<(NodeId, crate::dumbo1::epoch::EpochResult)> {
        let mut observed = Vec::new();
        let mut delivered = 0usize;

        loop {
            let mut progressed = false;

            for &id in &self.members {
                loop {
                    let Some((from, msg)) = self.bus.deliver_next(id) else {
                        break;
                    };
                    progressed = true;
                    delivered += 1;
                    assert!(
                        delivered < max_messages,
                        "Dumbo1 cluster simulation did not converge within {delivered} messages"
                    );

                    let stored = stored_message(from, id, msg);
                    let shareable: ShareableDumboPMessage<TestRequest, VR, IR, A, CE> =
                        Arc::new(stored);

                    let network = self.send_nodes[&id].clone();
                    let result = self
                        .nodes
                        .get_mut(&id)
                        .unwrap()
                        .process_message(shareable, &network)
                        .expect("dumbo1 round should not error processing a well-formed message");

                    observed.push((id, result));
                }
            }

            if !progressed {
                break;
            }
        }

        observed
    }

    pub(super) fn members(&self) -> &[NodeId] {
        &self.members
    }

    /// See `RoundStateParts::all_value_rbcs_complete`: true once every node's
    /// ValueRBC has finished, on every replica's local view.
    pub(super) fn all_value_rbcs_complete(&self) -> bool {
        self.members
            .iter()
            .all(|id| self.nodes[id].all_value_rbcs_complete())
    }

    pub(super) fn debug_dump(&self) -> String {
        let mut out = String::new();
        for &id in &self.members {
            out.push_str(&format!("{id:?}: {:?}\n", self.nodes[&id]));
        }
        out
    }
}
