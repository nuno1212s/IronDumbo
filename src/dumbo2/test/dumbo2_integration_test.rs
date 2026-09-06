use crate::dumbo2::epoch::EpochResult;
use crate::dumbo2::test::harness::Dumbo2TestCluster;
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;

const N: usize = 4;
const F: usize = 1;

#[test]
fn test_dumbo2_prbc_phase_completes() {
    let mut cluster = Dumbo2TestCluster::new(N, F);

    cluster.run_to_fixed_point(200_000);

    for &id in cluster.members().to_vec().iter() {
        assert_eq!(
            cluster.prbc_done_count(id),
            N,
            "every node should see all {N} PRBC broadcasts complete"
        );
    }
}

#[test]
fn test_dumbo2_mvba_decides_valid_subset() {
    let mut cluster = Dumbo2TestCluster::new(N, F);

    cluster.run_to_fixed_point(200_000);

    for &id in cluster.members().to_vec().iter() {
        let size = cluster
            .decided_size(id)
            .expect("MVBA should have decided a subset");
        assert!(
            size >= N - F,
            "the decided subset (size {size}) must contain at least n-f={} entries",
            N - F
        );
    }
}

#[test]
fn test_dumbo2_full_epoch_happy_path() {
    let mut cluster = Dumbo2TestCluster::new(N, F);

    let observed = cluster.run_to_fixed_point(200_000);

    let finalized_count = observed
        .iter()
        .filter(|(_, result)| matches!(result, EpochResult::Finalized))
        .count();

    assert_eq!(
        finalized_count, N,
        "every node should finalize a Dumbo2 epoch"
    );
}

#[test]
fn test_dumbo2_silent_byzantine_node() {
    let mut cluster = Dumbo2TestCluster::new(N, F);
    let silent = NodeId(0);

    let observed = cluster.run_to_fixed_point_excluding(Some(silent), 200_000);

    let finalized_nodes: std::collections::HashSet<NodeId> = observed
        .iter()
        .filter(|(_, result)| matches!(result, EpochResult::Finalized))
        .map(|(id, _)| *id)
        .collect();

    assert_eq!(
        finalized_nodes.len(),
        N - 1,
        "the remaining honest nodes should still finalize despite one silent node"
    );
}

#[test]
fn test_dumbo2_sequential_epochs() {
    for seq in [0u32, 1u32] {
        let mut cluster = Dumbo2TestCluster::new_at_seq_no(N, F, SeqNo::from(seq));

        let observed = cluster.run_to_fixed_point(200_000);

        let finalized_count = observed
            .iter()
            .filter(|(_, result)| matches!(result, EpochResult::Finalized))
            .count();

        assert_eq!(
            finalized_count, N,
            "epoch {seq} should finalize for every node"
        );
    }
}
