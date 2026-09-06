use crate::committee_election::{CommitteeElectionProtocol, RealCommitteeElection};
use crate::dumbo1::test::harness::Dumbo1TestCluster;
use crate::testing::fixtures;
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use std::cell::RefCell;
use std::collections::HashMap;

const N: usize = 4;
const F: usize = 1;

// NOTE: These tests stop at ValueRBC-phase convergence, not full round
// finalization. `RoundStateParts::handle_value_rbc_finished` (the intended
// trigger for a committee node's WaitingForRBCs -> RunningIndexRBC
// transition) is an empty stub, and nothing anywhere in this crate ever
// constructs a `RunningIndexRBC` instance -- so the IndexRBC, ABA, and
// finalization phases of Dumbo1 are currently unreachable regardless of the
// test harness driving them. That's a real gap in the protocol
// implementation (requiring a genuine design decision: what triggers the
// transition, and what goes into the index), not something fixable as a
// test-writing task. See `RoundStateParts::all_value_rbcs_complete`'s doc
// comment for the same note in context.

#[test]
fn test_dumbo1_value_rbc_happy_path_4nodes() {
    let mut cluster = Dumbo1TestCluster::new(N, F);

    cluster.kickoff_committee_election();
    cluster.run_to_fixed_point(100_000);

    assert!(
        cluster.all_value_rbcs_complete(),
        "every node's ValueRBC should complete, on every replica's view:\n{}",
        cluster.debug_dump()
    );
}

#[test]
fn test_dumbo1_committee_election_determines_flow() {
    let mut cluster = Dumbo1TestCluster::new(N, F);

    cluster.kickoff_committee_election();
    cluster.run_to_fixed_point(100_000);

    // With N=4, F=1 the committee size is F+1=2. Committee and non-committee
    // nodes take different paths through `init_node_state_for`
    // (CommitteeNode vs NonCommitteeNode) purely as a function of the
    // deterministic committee this harness computes; the cluster converging
    // at all demonstrates both paths complete correctly.
    assert!(
        cluster.all_value_rbcs_complete(),
        "both committee and non-committee ValueRBC paths should converge:\n{}",
        cluster.debug_dump()
    );
}

#[test]
fn test_dumbo1_sequential_epochs() {
    // This harness drives individual `DumboRound`s directly (one per node),
    // not the multi-round `Dumbo`/`install_seq_no` pipeline in protocol.rs,
    // so "sequential epochs" here means: two independently-numbered rounds
    // (seq 0 and seq 1) each run committee election + ValueRBC to
    // convergence, back to back.
    for seq in [0u32, 1u32] {
        let mut cluster = Dumbo1TestCluster::new_at_seq_no(N, F, SeqNo::from(seq));

        cluster.kickoff_committee_election();
        cluster.run_to_fixed_point(100_000);

        assert!(
            cluster.all_value_rbcs_complete(),
            "epoch {seq} should converge for every node:\n{}",
            cluster.debug_dump()
        );
    }
}

/// Drives a real `RealCommitteeElection` cluster (Algorithm 4) to
/// convergence for a single `round`, sharing one keyset across nodes (the
/// same keys are reused across calls with different `round`s so that
/// `round` is the only thing that varies between two calls).
fn run_ce_cluster(
    n: usize,
    f: usize,
    round: SeqNo,
    keyset: &atlas_common::crypto::threshold_crypto::PrivateKeySet,
    cbc_keyset: &atlas_common::crypto::threshold_crypto::PrivateKeySet,
) -> Vec<NodeId> {
    let members = fixtures::node_ids(n);
    let committee_size = f + 1;

    let bus = RefCell::new(SimulatedNetwork::<
        <RealCommitteeElection as CommitteeElectionProtocol>::Message,
    >::new(&members));

    let mut instances: HashMap<NodeId, RealCommitteeElection> = members
        .iter()
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(keyset, cbc_keyset, id);
            (
                id,
                RealCommitteeElection::new(quorum, threshold_keys, committee_size, round),
            )
        })
        .collect();

    for &id in &members {
        let handle = NodeHandle::new(id, &bus);
        instances.get_mut(&id).unwrap().kickoff(&handle);
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
                    "CE cluster simulation did not converge"
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

    instances.remove(&members[0]).unwrap().finalize().unwrap()
}

/// The paper's CE (Algorithm 4) is a threshold-coin-tossing construction
/// whose entire security argument (Lemma 1: "the set C contains at least
/// one honest node except with negligible probability") rests on its
/// Unpredictability property: "Before invocation by one honest node, the
/// probability of the adversary to predict the returned committee is at
/// most 1/C(n,kappa)". This asserts what Unpredictability actually
/// requires: two independent committee-election rounds, driven by
/// `RealCommitteeElection` (`committee_election.rs`, Algorithm 4's real
/// threshold-coin-tossing implementation) against the *same* keyset, must
/// not produce the identical committee -- `round` alone must change the
/// draw.
///
/// (`Dumbo1TestCluster`'s `TestCommitteeElection` stub, exercised by the
/// other tests in this file, remains intentionally deterministic -- it's a
/// controllable test double for exercising Dumbo1's committee/non-committee
/// code paths, not a stand-in for CE's own security properties.)
#[test]
fn test_committee_election_output_should_vary_across_rounds() {
    // n=30 (rather than a small n) so this isn't a flaky birthday-paradox
    // coin flip: a small n/committee_size combination can coincidentally
    // draw the same committee across two rounds by chance with
    // non-negligible probability, even though the coin genuinely differs.
    let n = 30;
    let f = 2;
    let keyset = fixtures::make_keyset(f);
    let cbc_keyset = fixtures::make_cbc_keyset(f);

    let committee_round_0 = run_ce_cluster(n, f, SeqNo::from(0u32), &keyset, &cbc_keyset);
    let committee_round_1 = run_ce_cluster(n, f, SeqNo::from(1u32), &keyset, &cbc_keyset);

    assert_ne!(
        committee_round_0, committee_round_1,
        "two independent committee-election rounds (same keys, different round id) produced \
         the identical committee -- Unpredictability (Pr[predict] <= 1/C(n,kappa)) requires \
         each round's committee to be an independent, coin-toss-seeded draw"
    );
}
