use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};
use crate::async_bin_agreement::async_bin_agreement::AsyncBinaryAgreement;
use crate::async_bin_agreement::async_bin_agreement_round::AsyncBinaryAgreementState;
use crate::async_bin_agreement::messages::{
    AsyncBinaryAgreementMessage, AsyncBinaryAgreementMessageType,
};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{PrivateKeyPart, PrivateKeySet};
use atlas_common::node_id::NodeId;
use atlas_communication::lookup_table::MessageModule;
use atlas_communication::message::{Buf, StoredMessage};
use getset::{Getters, MutGetters};
use std::cell::RefCell;
use std::collections::HashSet;

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

pub(super) fn quorum_info(n: usize, f: usize, node: NodeId) -> QuorumInfo {
    QuorumInfo::new(n, f, (0..n).map(NodeId::from).collect(), node)
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
    pub(super) fn new(id: NodeId, n: usize, f: usize) -> Self {
        let qi = quorum_info(n, f, id);
        let key_set = PrivateKeySet::gen_random(f);
        let pk_set = key_set.public_key_set();
        // ABA itself never touches the CBC keyset; generated here only
        // because ThresholdKeys::new requires one.
        let cbc_key_set = PrivateKeySet::gen_random(2 * f);

        let threshold_keys = ThresholdKeys::new(
            pk_set.clone(),
            key_set.private_key_part(id.0 as usize),
            cbc_key_set.public_key_set(),
            cbc_key_set.private_key_part(id.0 as usize),
        );

        let aba = AsyncBinaryAgreement::new(qi.clone(), threshold_keys);

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

        self.aba.process_message(stored, &self.network).unwrap()
    }
}

#[test]
fn test_val_round_first_stage() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

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

pub(super) fn perform_full_val_round(
    test_data: &mut TestData,
    test_message: AsyncBinaryAgreementMessage,
) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
    }
}

#[test]
fn test_val_round_second_stage() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

    let test_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, test_message);

    assert_eq!(2, test_data.network().sent.borrow().len());
    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Val { estimate } if *estimate == INITIAL_ESTIMATE)));
    assert!(test_data.network().sent.borrow().iter().any(|(message, _)| matches!(message.message_type(), AsyncBinaryAgreementMessageType::Aux { accepted_estimates } if accepted_estimates.len() == 1 && accepted_estimates.contains(&INITIAL_ESTIMATE))));
}

#[test]
fn test_val_round_redundant_vote_for_already_accepted_value_is_processed_not_ignored() {
    // A Val vote for a value that has *already* crossed 2f+1 (from a
    // sender who hasn't voted yet) is legitimate, harmless bookkeeping --
    // Algorithm 7 lines 6-7 have no phase gate, so this must be accepted
    // (a no-op past the threshold), not blanket-ignored. Blanket-ignoring
    // every post-CollectingVal Val vote (the old behavior this test used
    // to pin) is exactly the bug that let a late vote for a *different*
    // value get dropped too, see `test_late_val_for_second_value_is_still_counted_toward_values_r`.
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

    let test_message = get_val_message(INITIAL_ESTIMATE, None);

    perform_full_val_round(&mut test_data, test_message.clone());

    let sent_before = test_data.network().sent.borrow().len();

    // Send one more vote for the *same*, already-accepted value.
    let result = test_data.accept_message(NodeId::from(2 * F + 1), test_message.clone());

    assert!(!matches!(
        result,
        AsyncBinaryAgreementResult::MessageIgnored
    ));
    // No new broadcast: the value was already in values_r and AUX was
    // already sent once.
    assert_eq!(sent_before, test_data.network().sent.borrow().len());
}

#[test]
fn test_val_round_message_after_finishing_is_ignored() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

    let round = perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);

    for i in 0..(2 * F + 1) {
        let finish_message = get_finish_message(INITIAL_ESTIMATE, Some(round));
        test_data.accept_message(NodeId::from(i), finish_message);
    }

    assert!(matches!(
        test_data.aba.current_round().state(),
        AsyncBinaryAgreementState::Finishing {}
    ));

    let result = test_data.accept_message(
        NodeId::from(2 * F + 5),
        get_val_message(!INITIAL_ESTIMATE, Some(round)),
    );

    assert!(matches!(result, AsyncBinaryAgreementResult::MessageIgnored));
}

pub(super) fn get_aux_message(
    accepted_estimates: Vec<bool>,
    round: Option<usize>,
) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Aux { accepted_estimates },
        round.unwrap_or(0),
    )
}

pub(super) fn perform_full_aux_round(
    test_data: &mut TestData,
    test_message: AsyncBinaryAgreementMessage,
) {
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), test_message.clone());

        assert!(matches!(result, AsyncBinaryAgreementResult::Processed))
    }
}

#[test]
fn test_aux_round() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

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

pub(super) fn perform_full_conf_round(
    test_data: &mut TestData,
    initial_estimate: bool,
    round: Option<usize>,
) {
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
        let mut test_data = TestData::new(NodeId(0), N, F);

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

pub(super) fn get_finish_message(
    final_value: bool,
    round: Option<usize>,
) -> AsyncBinaryAgreementMessage {
    AsyncBinaryAgreementMessage::new(
        AsyncBinaryAgreementMessageType::Finish { value: final_value },
        round.unwrap_or(0),
    )
}

#[test]
fn test_finish_round_f_1() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);

    perform_all_rounds_until_conf_success(&mut test_data, INITIAL_ESTIMATE);
}

#[test]
fn test_finish_round_f_plus_1_broadcast() {
    const INITIAL_ESTIMATE: bool = true;

    let mut test_data = TestData::new(NodeId(0), N, F);
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

    let mut test_data = TestData::new(NodeId(0), N, F);
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
            assert!(matches!(result, AsyncBinaryAgreementResult::Decided));
        }
    }

    let result = test_data.aba.finalize().unwrap();

    assert_eq!(result, INITIAL_ESTIMATE);
}

/// Algorithm 7 lines 3-7 are *standing* event handlers: `values_r` must
/// keep accumulating Val_r votes for either bit throughout the whole round,
/// independent of which local phase this node has reached (the pseudocode
/// has no phase guard on them). `RoundData::accept_estimate` instead
/// dispatches on `self.state`, routing straight to `Ignored` for any state
/// other than `CollectingVal` (see `accept_estimate`'s match arm `_ =>
/// Ignored`). This test pushes a node past `CollectingVal` (2f+1 Val(true)
/// votes) and then asserts that a perfectly legitimate, never-before-seen
/// Val(false) vote still gets counted, rather than discarded. It currently
/// fails: the vote comes back `MessageIgnored`.
#[test]
fn test_late_val_for_second_value_is_still_counted_toward_values_r() {
    let mut test_data = TestData::new(NodeId(0), N, F);

    perform_full_val_round(&mut test_data, get_val_message(true, None));
    assert!(
        matches!(
            test_data.aba.current_round().state(),
            AsyncBinaryAgreementState::CollectingAux { .. }
        ),
        "precondition: the node should have left CollectingVal after 2f+1 Val(true) votes"
    );

    let result = test_data.accept_message(NodeId::from(N + 1), get_val_message(false, None));

    assert!(
        !matches!(result, AsyncBinaryAgreementResult::MessageIgnored),
        "a late, never-before-seen Val(false) vote must still be counted towards values_r \
         (Algorithm 7 lines 6-7 have no phase gate); it must not come back MessageIgnored"
    );
}

/// Direct consequence of the bug above: because a node's `values_r` can
/// never grow past whichever single value first crossed 2f+1 once it has
/// left `CollectingVal`, it can never accept an AUX message from a peer
/// whose own `values_r` legitimately grew to contain *both* values (Alg. 7
/// line 10: "wait until ... val_r ⊆ values_r"). Here we drive the node's
/// own `values_r` to {true,false} the same way the network would (2f+1
/// Val(true), then a later wave of 2f+1 Val(false)), then simulate n-f=3
/// honest peers who broadcast `AUX_r[{true,false}]`, and assert the node
/// reaches `CollectingConf`. It currently fails because the late Val(false)
/// votes are dropped (previous test), so `values_r` never grows past
/// `{true}` and the subset check in line 10 can never succeed.
#[test]
fn test_node_progresses_past_aux_once_values_r_reflects_both_confirmed_values() {
    let mut test_data = TestData::new(NodeId(0), N, F);

    perform_full_val_round(&mut test_data, get_val_message(true, None));
    assert!(matches!(
        test_data.aba.current_round().state(),
        AsyncBinaryAgreementState::CollectingAux { .. }
    ));

    let false_msg = get_val_message(false, None);
    for replica in (2 * F + 1)..(2 * F + 1 + 2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), false_msg.clone());
        assert!(
            !matches!(result, AsyncBinaryAgreementResult::MessageIgnored),
            "replica {replica}'s late Val(false) must be counted, not ignored"
        );
    }

    let mixed_aux = get_aux_message(vec![true, false], None);
    for replica in 0..(2 * F + 1) {
        let result = test_data.accept_message(NodeId::from(replica), mixed_aux.clone());
        assert!(
            matches!(result, AsyncBinaryAgreementResult::Processed),
            "matching AUX messages should at least be accepted/counted"
        );
    }

    assert!(
        matches!(
            test_data.aba.current_round().state(),
            AsyncBinaryAgreementState::CollectingConf { .. }
        ),
        "once n-f peers broadcast AUX[{{true,false}}], and values_r correctly reflects both \
         confirmed values (Algorithm 7 line 10: val_r ⊆ values_r), this node should reach \
         CollectingConf -- it is instead stuck in CollectingAux"
    );
}

#[test]
fn test_multi_round_coin_flip_convergence() {
    for initial_estimate in [true, false] {
        let mut test_data = TestData::new(NodeId(0), N, F);

        let round = perform_all_rounds_until_conf_success(&mut test_data, initial_estimate);

        assert!(matches!(
            test_data.aba.current_round().state(),
            AsyncBinaryAgreementState::Finishing {}
        ));

        for i in 0..(2 * F + 1) {
            let finish_message = get_finish_message(initial_estimate, Some(round));
            test_data.accept_message(NodeId::from(i), finish_message);
        }

        // Reaching `finalize()` at all (rather than looping forever in
        // `perform_all_rounds_until_conf_success`) is the termination guarantee under test.
        test_data.aba.finalize().unwrap();
    }
}
