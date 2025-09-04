use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult};
use atlas_common::node_id::NodeId;

// Import test utilities from the existing test file
use super::async_bin_agreement_test::{get_aux_message, get_conf_message, get_val_message, perform_all_rounds_until_conf_success, TestData};

const N: usize = 4;
const F: usize = 1;

/// Tests handling of a message from a future round, which should be queued
#[test]
fn test_future_round_message_is_queued() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    // Create a message from a future round (round 1)
    let future_message = get_val_message(INITIAL_ESTIMATE, Some(1));

    // Process the message
    let result = test_data.accept_message(NodeId(1), future_message);

    // The message should be queued
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageQueued));

    test_data.advance_round(INITIAL_ESTIMATE);

    // Verify we've advanced to round 1
    assert_eq!(1, test_data.aba.round());

    // Process any pending messages - the queued message should now be processed
    let pending = test_data.aba.poll();
    assert!(pending.is_some(), "Expected a queued message, but found none");

    let pending_msg = pending.unwrap();
    assert_eq!(1, pending_msg.message().round(), "Expected the queued message to be for round 1");
}

/// Tests handling of a message from a past round, which should be ignored
#[test]
fn test_past_round_message_is_ignored() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    test_data.advance_round(INITIAL_ESTIMATE);

    assert_eq!(1, test_data.aba.round());

    // Now try to process a message from round 0 (past round)
    let past_message = get_val_message(INITIAL_ESTIMATE, Some(0));
    let result = test_data.accept_message(NodeId(1), past_message);

    // The message should be ignored
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
}

/// Tests that a message is queued when received out of order within a round
#[test]
fn test_out_of_order_message_is_queued() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    // In round 0, state starts with CollectingVal
    // Try to send an Aux message which is not expected yet
    let aux_message = get_aux_message(vec![INITIAL_ESTIMATE], Some(0));
    let result = test_data.accept_message(NodeId(1), aux_message);

    // The message should be queued because we're not in the right state yet
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageQueued));
}

/// Tests that duplicate messages are ignored
#[test]
fn test_duplicate_messages_are_ignored() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    // Send a VAL message from node 1
    let val_message = get_val_message(INITIAL_ESTIMATE, Some(0));
    let result = test_data.accept_message(NodeId(1), val_message.clone());

    // The message should be processed
    assert!(matches!(result, AsyncBinaryAgreementResult::Processed));

    // Send the same message again from the same node
    let result = test_data.accept_message(NodeId(1), val_message);

    // The duplicate message should be ignored
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
}

/// Test that erroneous messages in the Finishing state are properly handled
#[test]
fn test_erroneous_messages_in_finishing_state() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F, INITIAL_ESTIMATE);

    // Bring the protocol to the Finishing state
    let round = perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);

    // Now we're in the Finishing state, send a Val message which should be queued
    let val_message = get_val_message(INITIAL_ESTIMATE, Some(round));
    let result = test_data.accept_message(NodeId(1), val_message);

    // The message should be ignored because we're past that state
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));

    // Send an Aux message which should also be queued
    let aux_message = get_aux_message(vec![INITIAL_ESTIMATE], Some(round));
    let result = test_data.accept_message(NodeId(1), aux_message);

    // The message should be ignored because we're past that state
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));

    // Send a Conf message which should be ignored in Finishing state
    let conf_message = get_conf_message(
        vec![INITIAL_ESTIMATE],
        &test_data.key_set,
        NodeId(1),
        Some(round),
    );
    let result = test_data.accept_message(NodeId(1), conf_message);

    // The message should be ignored
    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
}
