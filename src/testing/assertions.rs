use atlas_common::node_id::NodeId;
use std::fmt::Debug;

/// All decided nodes decided the same value.
pub fn assert_agreement<T: Eq + Debug>(decisions: &[(NodeId, T)]) {
    let Some((_, first)) = decisions.first() else {
        return;
    };

    for (node, value) in decisions.iter().skip(1) {
        assert_eq!(
            value, first,
            "node {node:?} disagreed with the decision reached by {:?}",
            decisions[0].0
        );
    }
}

/// The decided value was proposed by some honest node.
pub fn assert_validity<T: Eq>(decided: &T, proposed: &[T]) {
    assert!(
        proposed.iter().any(|value| value == decided),
        "decided value was not proposed by any honest node"
    );
}

/// All honest nodes eventually decided.
pub fn assert_liveness<T>(decisions: &[(NodeId, T)], honest_count: usize) {
    assert_eq!(
        decisions.len(),
        honest_count,
        "only {} of {honest_count} honest nodes reached a decision",
        decisions.len()
    );
}
