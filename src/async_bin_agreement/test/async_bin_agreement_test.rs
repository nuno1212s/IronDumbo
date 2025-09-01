use crate::async_bin_agreement::async_bin_agreement::{
    AsyncBinaryAgreement, AsyncBinaryAgreementResult,
};
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::async_bin_agreement::network::AsyncBinaryAgreementSendNode;
use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{PartialSignature, PrivateKeyPart, PrivateKeySet, PublicKeySet};
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage};
use getset::{Getters, MutGetters};
use std::cell::RefCell;
use crate::async_bin_agreement::async_bin_agreement_round::AsyncBinaryAgreementState;

#[derive(Default)]
struct MockNetwork {
    sent: RefCell<Vec<(AsyncBinaryAgreementMessage, Vec<NodeId>)>>,
}

impl AsyncBinaryAgreementSendNode for MockNetwork {
    fn broadcast_message<I>(
        &self,
        message: AsyncBinaryAgreementMessage,
        target: I,
    ) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
    {
        self.sent.borrow_mut().push((message, target.collect()));

        Ok(())
    }
}
fn stored_msg<T>(from: NodeId, to: NodeId, msg: T) -> StoredMessage<T> {
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

fn quorum_info(n: usize, f: usize) -> QuorumInfo {
    QuorumInfo::new(n, f, (0..n).map(NodeId::from).collect())
}

const N: usize = 4;
const F: usize = 1;

#[derive(Getters, MutGetters)]
struct TestData {
    node_id: NodeId,
    #[get = "pub"]
    network: MockNetwork,
    #[get = "pub"]
    qi: QuorumInfo,
    #[get = "pub"]
    key_set: PrivateKeySet,
    #[get = "pub"]
    pk_set: PublicKeySet,
    #[get_mut = "pub"]
    aba: AsyncBinaryAgreement,
}

impl TestData {
    fn new(id: NodeId, n: usize, f: usize, initial_estimate: bool) -> Self {
        let qi = quorum_info(n, f);
        let key_set = PrivateKeySet::gen_random(f);
        let pk_set = key_set.public_key_set();

        let aba = AsyncBinaryAgreement::new(
            initial_estimate,
            qi.clone(),
            pk_set.clone(),
            key_set.private_key_part(id.0 as usize),
        );

        Self {
            node_id: id,
            network: MockNetwork::default(),
            qi,
            key_set,
            pk_set,
            aba,
        }
    }

    fn get_private_key_part(&self, index: usize) -> PrivateKeyPart {
        self.key_set.private_key_part(index)
    }

    fn accept_message(
        &mut self,
        from: NodeId,
        msg: AsyncBinaryAgreementMessage,
    ) -> AsyncBinaryAgreementResult {
        let stored = stored_msg(from, self.node_id.clone(), msg);

        self.aba.process_message(stored, &self.network)
    }
}

#[test]
fn test_val_round_first_stage() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    let test_message = AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Val {
            estimate: INITIAL_ESTIMATE,
        },
        0,
    );

    // send F valid messages from different nodes
    for i in 1..=F {
        let result = test_data.accept_message(NodeId::from(i), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed(_)))
    }

    // Send one more message, this should trigger a val broadcast
    let result = test_data.accept_message(NodeId::from(F + 1), test_message.clone());

    assert!(matches!(result, AsyncBinaryAgreementResult::Processed(_)));
    assert_eq!(1, test_data.network().sent.borrow().len());

    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Val { estimate } if *estimate == INITIAL_ESTIMATE)));
}

fn get_val_message(estimate: bool, round: Option<usize>) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Val { estimate },
        round.unwrap_or(0),
    )
}

fn perform_full_val_round(test_data: &mut TestData, test_message: AsyncBinaryAgreementMessage) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed(_)))
    }
}

#[test]
fn test_val_round_second_stage() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    let test_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, test_message);

    assert_eq!(2, test_data.network().sent.borrow().len());
    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Val { estimate } if *estimate == INITIAL_ESTIMATE)));
    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Aux { accepted_estimates } if accepted_estimates.len() == 1 && accepted_estimates.contains(&INITIAL_ESTIMATE))));
}


#[test]
fn test_val_round_ignored() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    let test_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, test_message.clone());

    // Send one more message, this should be ignored
    let result = test_data.accept_message(NodeId::from(2 * F + 1), test_message.clone());

    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
    assert_eq!(2, test_data.network().sent.borrow().len());
}

fn get_aux_message(accepted_estimates: Vec<bool>, round: Option<usize>) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Aux { accepted_estimates },
        round.unwrap_or(0),
    )
}

fn perform_full_aux_round(test_data: &mut TestData, test_message: AsyncBinaryAgreementMessage) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed(_)))
    }
}

#[test]
fn test_aux_round() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    let val_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, val_message);

    let aux_message = get_aux_message(vec![INITIAL_ESTIMATE], None);

    // send F valid messages from different nodes
    perform_full_aux_round(&mut test_data, aux_message.clone());

    // Send one more message, this should trigger an aux broadcast
    let result = test_data.accept_message(NodeId::from(F + 1), aux_message.clone());

    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
    assert_eq!(3, test_data.network().sent.borrow().len());

    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Aux { accepted_estimates } if accepted_estimates.len() == 1 && accepted_estimates.contains(&INITIAL_ESTIMATE))));
    assert!(matches!(test_data.aba.current_round().state(),AsyncBinaryAgreementState::CollectingConf { .. } ));
}

fn get_conf_message(feasible_values: Vec<bool>, signature_set: &PrivateKeySet, node: NodeId, round: Option<usize>) -> AsyncBinaryAgreementMessage {
    let signature = signature_set
        .private_key_part(node.0 as usize)
        .partially_sign(&round.unwrap_or(0).to_le_bytes()[..]);

    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Conf { feasible_values, partial_signature: signature },
        round.unwrap_or(0),
    )
}

#[test]
fn test_conf_round() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    let val_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, val_message);

    let aux_message = get_aux_message(vec![INITIAL_ESTIMATE], None);

    perform_full_aux_round(&mut test_data, aux_message);

    for replica in 0..(2 * F + 1) {
        let conf_message = get_conf_message(vec![INITIAL_ESTIMATE], &test_data.key_set, NodeId::from(replica), None);
        let result = test_data.accept_message(NodeId::from(replica), conf_message);

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed(_)))
    }

}
