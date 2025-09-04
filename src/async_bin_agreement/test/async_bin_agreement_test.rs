use crate::async_bin_agreement::async_bin_agreement::{
    AsyncBinaryAgreement,
};
use crate::async_bin_agreement::async_bin_agreement_round::AsyncBinaryAgreementState;
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{PrivateKeyPart, PrivateKeySet};
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage};
use getset::{Getters, MutGetters};
use std::cell::RefCell;
use std::collections::HashSet;
use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};

#[derive(Default)]
pub(super) struct MockNetwork {
    sent: RefCell<Vec<(AsyncBinaryAgreementMessage, Vec<NodeId>)>>,
}

impl AsyncBinaryAgreementSendNode<AsyncBinaryAgreementMessage> for MockNetwork {
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
pub(super) fn stored_msg<T>(from: NodeId, to: NodeId, msg: T) -> StoredMessage<T> {
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

pub(super) fn quorum_info(n: usize, f: usize) -> QuorumInfo {
    QuorumInfo::new(n, f, (0..n).map(NodeId::from).collect())
}

const N: usize = 4;
const F: usize = 1;

#[derive(Getters, MutGetters)]
pub(super) struct TestData {
    pub(super) node_id: NodeId,
    #[get = "pub"]
    pub(super) network: MockNetwork,
    #[get = "pub"]
    pub(super) key_set: PrivateKeySet,
    #[get_mut = "pub"]
    pub(super) aba: AsyncBinaryAgreement,
}

impl TestData {
    pub(super) fn new(id: NodeId, n: usize, f: usize, initial_estimate: bool) -> Self {
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
            key_set,
            aba,
        }
    }

    pub(super) fn get_private_key_part(&self, index: usize) -> PrivateKeyPart {
        self.key_set.private_key_part(index)
    }
    
    pub(super) fn advance_round(&mut self, estimate: bool) {
        self.aba.advance_round(estimate);
    }

    pub(super) fn accept_message(
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

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
    }

    // Send one more message, this should trigger a val broadcast
    let result = test_data.accept_message(NodeId::from(F + 1), test_message.clone());

    assert!(matches!(result, AsyncBinaryAgreementResult::Processed));
    assert_eq!(1, test_data.network().sent.borrow().len());

    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Val { estimate } if *estimate == INITIAL_ESTIMATE)));
}

pub(super) fn get_val_message(estimate: bool, round: Option<usize>) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Val { estimate },
        round.unwrap_or(0),
    )
}

pub(super) fn perform_full_val_round(test_data: &mut TestData, test_message: AsyncBinaryAgreementMessage) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
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

pub(crate) fn get_aux_message(
    accepted_estimates: Vec<bool>,
    round: Option<usize>,
) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Aux { accepted_estimates },
        round.unwrap_or(0),
    )
}

pub(super) fn perform_full_aux_round(test_data: &mut TestData, test_message: AsyncBinaryAgreementMessage) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
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
    assert!(matches!(
        test_data.aba.current_round().state(),
        AsyncBinaryAgreementState::CollectingConf { .. }
    ));
}

pub(super) fn get_conf_message(
    feasible_values: Vec<bool>,
    signature_set: &PrivateKeySet,
    node: NodeId,
    round: Option<usize>,
) -> AsyncBinaryAgreementMessage {
    let signature = signature_set
        .private_key_part(node.0 as usize)
        .partially_sign(&round.unwrap_or(0).to_le_bytes()[..]);

    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Conf {
            feasible_values,
            partial_signature: signature,
        },
        round.unwrap_or(0),
    )
}

pub(super) fn perform_full_conf_round(test_data: &mut TestData, initial_estimate: bool, round: Option<usize>) {
    for replica in 0..(2 * F + 1) {
        let conf_message = get_conf_message(
            vec![initial_estimate],
            &test_data.key_set,
            NodeId::from(replica),
            round,
        );

        let result = test_data.accept_message(NodeId::from(replica), conf_message);

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
    }
}

#[test]
fn test_conf_round() {
    const INITIAL_ESTIMATE: bool = true;

    let mut achieved_results = HashSet::<AsyncBinaryAgreementState>::default();

    while achieved_results.len() < 2 {
        let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

        let val_message = get_val_message(INITIAL_ESTIMATE, None);

        perform_full_val_round(&mut test_data, val_message);

        let aux_message = get_aux_message(vec![INITIAL_ESTIMATE], None);

        perform_full_aux_round(&mut test_data, aux_message);

        perform_full_conf_round(&mut test_data, INITIAL_ESTIMATE, None);

        assert!(
            matches!(
                test_data.aba.current_round().state(),
                AsyncBinaryAgreementState::Finishing {}
            ) || matches!(
                test_data.aba.current_round().state(),
                AsyncBinaryAgreementState::CollectingVal { .. }
            )
        );

        if matches!(
            test_data.aba.current_round().state(),
            AsyncBinaryAgreementState::CollectingVal { .. }
        ) {
            assert_eq!(1, test_data.aba.round())
        }

        achieved_results.insert(test_data.aba.current_round().state().clone());
    }
}

pub(super) fn perform_all_rounds_until_conf_success(
    test_data: &mut TestData,
    initial_estimate: bool,
) -> usize {
    let mut round = 0;

    loop {
        let val_message = get_val_message(initial_estimate, Some(round));

        perform_full_val_round(test_data, val_message);

        let aux_message = get_aux_message(vec![initial_estimate], Some(round));

        perform_full_aux_round(test_data, aux_message);

        perform_full_conf_round(test_data, initial_estimate, Some(round));

        if matches!(
            test_data.aba.current_round().state(),
            AsyncBinaryAgreementState::Finishing {}
        ) {
            break round;
        }

        round += 1;
    }
}

pub(super) fn get_finish_message(final_value: bool, round: Option<usize>) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Finish { value: final_value },
        round.unwrap_or(0),
    )
}

#[test]
fn test_finish_round_f_1() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);
}

#[test]
fn test_finish_round_f_plus_1_broadcast() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);
    // First, we need to bring the protocol to the Finishing state
    let round = perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);

    // Record the current number of sent messages
    let sent_messages_before = test_data.network().sent.borrow().len();

    // Send F finish messages with the agreed value
    for i in 1..=F {
        let finish_message = get_finish_message(INITIAL_ESTIMATE, Some(round));
        let result = test_data.accept_message(NodeId::from(i), finish_message);
        assert!(matches!(result, AsyncBinaryAgreementResult::Processed));
    }

    // No broadcast should have happened yet
    assert_eq!(
        sent_messages_before,
        test_data.network().sent.borrow().len()
    );

    // Send one more message (F+1), which should trigger a broadcast
    let finish_message = get_finish_message(INITIAL_ESTIMATE, Some(round));
    let result = test_data.accept_message(NodeId::from(F + 1), finish_message);
    assert!(matches!(result, AsyncBinaryAgreementResult::Processed));

    // Verify the broadcast was a Finish message
    assert!(test_data.network().sent.borrow().iter().any(|(message, _)|
        matches!(message.message_type(), AsyncBinaryAgreementMessageType::Finish { value } if *value == INITIAL_ESTIMATE)));
}

#[test]
fn test_finish_round_2f_plus_1_finalization() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);
    // First, we need to bring the protocol to the Finishing state
    let round = perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);

    // Send 2F + 1 finish messages with the agreed value
    for i in 0..(2 * F + 1) {
        let finish_message = get_finish_message(INITIAL_ESTIMATE, Some(round));
        let result = test_data.accept_message(NodeId::from(i), finish_message);

        // All messages except possibly the last should be processed
        if i < 2 * F {
            assert!(matches!(result, AsyncBinaryAgreementResult::Processed));
        } else {
            // The final message should result in finalization
            assert!(
                matches!(result, AsyncBinaryAgreementResult::Decided(value, ..) if value == INITIAL_ESTIMATE)
            );
        }
    }
}
