use crate::committee_election::CommitteeElectionProtocol;
use crate::dumbo1::test::harness::{Dumbo1TestCluster, TestCommitteeElection};
use crate::testing::fixtures;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;

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

/// The paper's CE (Algorithm 4) is a threshold-coin-tossing construction
/// whose entire security argument (Lemma 1: "the set C contains at least
/// one honest node except with negligible probability") rests on its
/// Unpredictability property: "Before invocation by one honest node, the
/// probability of the adversary to predict the returned committee is at
/// most 1/C(n,kappa)". `grep -rln "impl.*CommitteeElectionProtocol for"`
/// over this crate finds exactly one implementation, `TestCommitteeElection`
/// here in the test harness, and it is fully deterministic: "the committee
/// is a deterministic function of the quorum alone (first `committee_size`
/// members by NodeId)" per its own doc comment. There is currently no
/// production `CommitteeElectionProtocol` implementation anywhere in this
/// crate.
///
/// The `CommitteeElectionProtocol` trait as currently defined
/// (`committee_election.rs`) doesn't even accept a round/epoch identifier
/// in `new` -- only `quorum_info` and `committee_size`, both of which are
/// invariant across rounds for a fixed cluster. Nothing implementing this
/// trait can derive a per-round-distinct, coin-toss-seeded committee from
/// it, and `TestCommitteeElection` (the only implementation in the crate)
/// demonstrates the resulting determinism concretely.
///
/// This test asserts what Unpredictability actually requires: two
/// independent committee-election rounds must not produce the identical
/// committee. It currently fails, because there is no way -- via this
/// trait or this implementation -- for two rounds to differ at all.
#[test]
fn test_committee_election_output_should_vary_across_rounds() {
    let n = 7;
    let f = 2;
    let committee_size = f + 1;

    // Simulate two different rounds the only way the current trait allows:
    // there is no round/epoch parameter to vary, so we just call `new`
    // twice with the same (only available) inputs.
    let quorum_round_a = fixtures::quorum_info(n, f, NodeId(0));
    let quorum_round_b = fixtures::quorum_info(n, f, NodeId(0));

    let committee_a = TestCommitteeElection::new(quorum_round_a, committee_size)
        .finalize()
        .unwrap();
    let committee_b = TestCommitteeElection::new(quorum_round_b, committee_size)
        .finalize()
        .unwrap();

    assert_ne!(
        committee_a, committee_b,
        "two independent committee-election rounds produced the identical committee -- \
         Unpredictability (Pr[predict] <= 1/C(n,kappa)) requires each round's committee to be \
         an independent, coin-toss-seeded draw; this will keep failing until \
         CommitteeElectionProtocol::new (or an equivalent entry point) accepts a round/epoch id \
         and a real implementation derives the committee from a genuine per-round threshold \
         coin toss"
    );
}
