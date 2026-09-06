use crate::aba::ABAProtocol;
use crate::async_bin_agreement::async_bin_agreement::AsyncBinaryAgreement;
use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use crate::testing::assertions::assert_agreement;
use crate::testing::fixtures::{bootstrap_aba_cluster, stored_msg};
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::node_id::NodeId;
use std::cell::RefCell;
use std::collections::HashMap;

const N: usize = 4;
const F: usize = 1;

/// Drives a bootstrapped ABA cluster to a fixed point over a [`SimulatedNetwork`],
/// optionally treating one node as silent (never given input, never processed).
/// Returns the decided value for every node that reached one.
fn run_cluster_to_convergence(
    nodes: Vec<(NodeId, AsyncBinaryAgreement)>,
    inputs: &HashMap<NodeId, bool>,
    silent: Option<NodeId>,
) -> HashMap<NodeId, bool> {
    let ids: Vec<NodeId> = nodes.iter().map(|(id, _)| *id).collect();
    let bus = RefCell::new(SimulatedNetwork::<AsyncBinaryAgreementMessage>::new(&ids));
    let mut instances: HashMap<NodeId, AsyncBinaryAgreement> = nodes.into_iter().collect();

    for &id in &ids {
        if Some(id) == silent {
            continue;
        }

        let handle = NodeHandle::new(id, &bus);
        instances
            .get_mut(&id)
            .unwrap()
            .provide_input_bit(inputs[&id], &handle)
            .unwrap();
    }

    let mut iterations = 0usize;
    loop {
        let mut progressed = false;

        for &id in &ids {
            if Some(id) == silent {
                continue;
            }

            loop {
                let next = bus.borrow_mut().deliver_next(id);
                let Some((from, msg)) = next else {
                    break;
                };
                progressed = true;
                iterations += 1;
                assert!(
                    iterations < 100_000,
                    "ABA cluster simulation did not converge within {iterations} message deliveries"
                );

                let handle = NodeHandle::new(id, &bus);
                let stored = stored_msg(from, id, msg);
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

    ids.into_iter()
        .filter(|id| Some(*id) != silent)
        .filter_map(|id| {
            instances
                .remove(&id)
                .and_then(|instance| instance.finalize().ok())
                .map(|value| (id, value))
        })
        .collect()
}

#[test]
fn test_conflicting_inputs_converge() {
    let (nodes, _keyset) = bootstrap_aba_cluster(N, F);
    let ids: Vec<NodeId> = nodes.iter().map(|(id, _)| *id).collect();

    // Half the quorum starts with `true`, half with `false`.
    let inputs: HashMap<NodeId, bool> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i % 2 == 0))
        .collect();

    let decisions = run_cluster_to_convergence(nodes, &inputs, None);

    assert_eq!(decisions.len(), N, "every node should reach a decision");
    assert_agreement(&decisions.into_iter().collect::<Vec<_>>());
}

#[test]
fn test_aba_with_silent_byzantine() {
    let (nodes, _keyset) = bootstrap_aba_cluster(N, F);
    let ids: Vec<NodeId> = nodes.iter().map(|(id, _)| *id).collect();
    let silent = ids[0];

    let inputs: HashMap<NodeId, bool> = ids.iter().map(|&id| (id, true)).collect();

    let decisions = run_cluster_to_convergence(nodes, &inputs, Some(silent));

    assert_eq!(
        decisions.len(),
        N - 1,
        "the remaining honest nodes should still terminate despite one silent node"
    );
    assert_agreement(&decisions.into_iter().collect::<Vec<_>>());
}

#[test]
fn test_full_4node_aba_simulation() {
    let (nodes, _keyset) = bootstrap_aba_cluster(N, F);
    let ids: Vec<NodeId> = nodes.iter().map(|(id, _)| *id).collect();

    let inputs: HashMap<NodeId, bool> = ids.iter().map(|&id| (id, true)).collect();

    let decisions = run_cluster_to_convergence(nodes, &inputs, None);

    assert_eq!(decisions.len(), N);
    assert_agreement(
        &decisions
            .iter()
            .map(|(&id, &v)| (id, v))
            .collect::<Vec<_>>(),
    );

    for &value in decisions.values() {
        assert!(
            value,
            "the only proposed value was true, so it must be decided"
        );
    }
}
