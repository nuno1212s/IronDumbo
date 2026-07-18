use crate::multi_valued_byzantine_agreement::messages::MVBAMessage;
use crate::multi_valued_byzantine_agreement::mvba::MultiValuedByzantineAgreement;
use crate::mvba::{MVBAProposal, MVBAProtocol, MVBAResult, MVBASendNode};
use crate::testing::fixtures;
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{CombinedSignature, PrivateKeySet};
use atlas_common::node_id::NodeId;
use std::cell::RefCell;
use std::collections::HashMap;

const N: usize = 4;
const F: usize = 1;

struct MockNetwork {
    sent: RefCell<Vec<(MVBAMessage, Vec<NodeId>)>>,
}

impl MockNetwork {
    fn new() -> Self {
        Self {
            sent: RefCell::new(vec![]),
        }
    }
}

impl MVBASendNode<MVBAMessage> for MockNetwork {
    fn send(
        &self,
        message: MVBAMessage,
        target: NodeId,
        _flush: bool,
    ) -> atlas_common::error::Result<()> {
        self.sent.borrow_mut().push((message, vec![target]));
        Ok(())
    }

    fn broadcast<I>(&self, message: MVBAMessage, targets: I) -> Result<(), Vec<NodeId>>
    where
        I: Iterator<Item = NodeId>,
    {
        self.sent.borrow_mut().push((message, targets.collect()));
        Ok(())
    }
}

/// Fabricates a valid PRBC-style proof for `owner`/`digest` by combining
/// `f+1` partial signatures from `keyset` directly, without running a full
/// PRBC instance (MVBA only cares that the proof verifies).
fn valid_entry(
    keyset: &PrivateKeySet,
    owner: NodeId,
    digest: Digest,
) -> (NodeId, Digest, CombinedSignature) {
    let shares: Vec<_> = (0..=F)
        .map(|i| {
            (
                i,
                keyset.private_key_part(i).partially_sign(digest.as_ref()),
            )
        })
        .collect();

    let combined = keyset
        .public_key_set()
        .combine_signatures(shares.iter().map(|(i, sig)| (*i, sig)))
        .expect("f+1 shares should combine");

    (owner, digest, combined)
}

#[test]
fn test_mvba_propose() {
    let keyset = fixtures::make_keyset(F);
    let node = NodeId(0);
    let quorum = fixtures::quorum_info(N, F, node);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, node);
    let network = MockNetwork::new();

    let digest = fixtures::make_digest(&[1]);
    let entry = valid_entry(&keyset, node, digest);

    let mut mvba = MultiValuedByzantineAgreement::new(quorum, threshold_keys);
    let result = mvba.propose(vec![entry], &network).unwrap();

    assert!(matches!(result, MVBAResult::Processed));
    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, MVBAMessage::Entry { owner, .. } if *owner == node)),
        "a valid proposed entry should be gossiped"
    );
    assert!(
        network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, MVBAMessage::Vote { .. })),
        "propose should kick off a per-owner ABA vote"
    );
}

#[test]
fn test_mvba_validity_predicate() {
    let keyset = fixtures::make_keyset(F);
    let bogus_keyset = fixtures::make_keyset(F);
    let node = NodeId(0);
    let quorum = fixtures::quorum_info(N, F, node);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, node);
    let network = MockNetwork::new();

    let digest = fixtures::make_digest(&[1]);
    // Signed with a *different* keyset: valid-looking shape, invalid signature.
    let bad_entry = valid_entry(&bogus_keyset, node, digest);

    let mut mvba = MultiValuedByzantineAgreement::new(quorum, threshold_keys);
    mvba.propose(vec![bad_entry], &network).unwrap();

    assert!(
        !network
            .sent
            .borrow()
            .iter()
            .any(|(msg, _)| matches!(msg, MVBAMessage::Entry { .. })),
        "a malformed proof must be rejected, not gossiped"
    );
}

/// Drives an `n`-node MVBA cluster (sharing one threshold keyset) to a fixed
/// point, each proposing whatever `proposals` gives it (or nothing).
fn run_mvba_cluster(
    n: usize,
    f: usize,
    keyset: &PrivateKeySet,
    proposals: &HashMap<NodeId, MVBAProposal>,
) -> HashMap<NodeId, MVBAProposal> {
    let members = fixtures::node_ids(n);
    let bus = RefCell::new(SimulatedNetwork::<MVBAMessage>::new(&members));

    let mut instances: HashMap<NodeId, MultiValuedByzantineAgreement> = members
        .iter()
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(keyset, id);
            (
                id,
                MultiValuedByzantineAgreement::new(quorum, threshold_keys),
            )
        })
        .collect();

    for &id in &members {
        let handle = NodeHandle::new(id, &bus);
        let proposal = proposals.get(&id).cloned().unwrap_or_default();
        instances
            .get_mut(&id)
            .unwrap()
            .propose(proposal, &handle)
            .unwrap();
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
                    delivered < 200_000,
                    "MVBA cluster simulation did not converge"
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

    instances
        .into_iter()
        .filter_map(|(id, instance)| instance.finalize().ok().map(|proposal| (id, proposal)))
        .collect()
}

fn owners_of(proposal: &MVBAProposal) -> std::collections::BTreeSet<NodeId> {
    proposal.iter().map(|(owner, _, _)| *owner).collect()
}

#[test]
fn test_mvba_all_agree_same_subset() {
    let keyset = fixtures::make_keyset(F);
    let members = fixtures::node_ids(N);

    // Every node already has every entry: the simplest case where all
    // proposals are identical.
    let entries: Vec<_> = members
        .iter()
        .map(|&owner| valid_entry(&keyset, owner, fixtures::make_digest(&[owner.0 as u8])))
        .collect();

    let proposals: HashMap<NodeId, MVBAProposal> =
        members.iter().map(|&id| (id, entries.clone())).collect();

    let decisions = run_mvba_cluster(N, F, &keyset, &proposals);

    assert_eq!(decisions.len(), N, "every node should decide");

    let first = owners_of(&decisions[&members[0]]);
    for &id in &members {
        assert_eq!(
            owners_of(&decisions[&id]),
            first,
            "all nodes must agree on the same set of included owners"
        );
    }
    assert!(
        first.len() >= N - F,
        "the agreed set should contain at least n-f entries"
    );
}

/// The paper's headline efficiency claim for Dumbo2 (Section 5, Fig. 4) is
/// that its MVBA needs only an expected *constant* number of ABA instances
/// -- "three consecutive instances of ABA" -- independent of n, via a
/// leader-permutation-driven design (PRBC-phase -> CBC -> a single ABA per
/// round of a repeat loop). `MultiValuedByzantineAgreement::propose`
/// instead starts one full ABA per quorum member ("is owner's PRBC entry
/// included?", see `mvba.rs`'s own doc comment), so the ABA count scales
/// *linearly* with n.
///
/// This test doesn't pin an exact constant (the real design is
/// expected-case, not worst-case fixed) -- it asserts the one invariant
/// that actually is testable without a reference implementation: the count
/// must not grow between two very differently sized quorums. It currently
/// fails: the count equals n every time.
#[test]
fn test_mvba_aba_instance_count_is_independent_of_quorum_size() {
    let counts: Vec<usize> = [(4usize, 1usize), (10usize, 3usize)]
        .iter()
        .map(|&(n, f)| {
            let keyset = fixtures::make_keyset(f);
            let node = NodeId(0);
            let quorum = fixtures::quorum_info(n, f, node);
            let threshold_keys = fixtures::make_threshold_keys(&keyset, node);
            let network = MockNetwork::new();

            let mut mvba = MultiValuedByzantineAgreement::new(quorum, threshold_keys);
            mvba.propose(vec![], &network).unwrap();

            network
                .sent
                .borrow()
                .iter()
                .filter_map(|(msg, _)| match msg {
                    MVBAMessage::Vote { owner, .. } => Some(*owner),
                    MVBAMessage::Entry { .. } => None,
                })
                .collect::<std::collections::BTreeSet<NodeId>>()
                .len()
        })
        .collect();

    assert_eq!(
        counts[0], counts[1],
        "the number of ABA instances started by propose() must not depend on quorum size \
         (n=4 started {} instances, n=10 started {} instances) -- Dumbo2's efficiency claim is \
         an O(1)/expected-constant ABA count, not O(n)",
        counts[0], counts[1]
    );
}

/// MVBA's Agreement property requires that *all* honest nodes that
/// terminate output the same value (Section 3: "All honest nodes that
/// terminate output the same value"). `MultiValuedByzantineAgreement::finalize`
/// (mvba.rs) builds its output by looking up each ABA-decided-true owner in
/// this node's own local `entries` map, which is populated only from this
/// node's initial proposal or from `Entry` gossip messages received over
/// the network -- and `MVBAResult::Decided` is signalled purely from
/// `all_decided()` (ABA completion), with no dependency on `entries` being
/// complete. If a decided-true owner's `Entry` gossip hasn't reached a node
/// yet, `finalize()` silently drops that owner via `filter_map` instead of
/// waiting or erroring.
///
/// This test simulates a network that delivers all ABA (`Vote`) traffic
/// normally but drops the `Entry` gossip destined for one node ("the
/// victim") for one specific owner, and asserts the victim's decided set
/// still agrees with another honest node's. It currently fails: the
/// victim's `finalize()` silently omits `target_owner`.
#[test]
fn test_mvba_finalize_agrees_across_honest_nodes_even_if_entry_gossip_is_dropped() {
    let n = 4;
    let f = 1;
    let keyset = fixtures::make_keyset(f);
    let members = fixtures::node_ids(n);
    let victim = members[3];
    let target_owner = members[2];

    let digest = fixtures::make_digest(&[42]);
    let entry = valid_entry(&keyset, target_owner, digest);

    let bus = RefCell::new(SimulatedNetwork::<MVBAMessage>::new(&members));
    let mut instances: HashMap<NodeId, MultiValuedByzantineAgreement> = members
        .iter()
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(&keyset, id);
            (
                id,
                MultiValuedByzantineAgreement::new(quorum, threshold_keys),
            )
        })
        .collect();

    // Every node except the victim already holds (and will gossip) the
    // entry for `target_owner`; the victim starts with nothing, so its own
    // initial vote for `target_owner` is `false` -- irrelevant to whether
    // the *network* ultimately decides `true` for it, since 3 of 4 nodes
    // vote `true`.
    for &id in &members {
        let handle = NodeHandle::new(id, &bus);
        let proposal = if id == victim {
            vec![]
        } else {
            vec![entry.clone()]
        };
        instances
            .get_mut(&id)
            .unwrap()
            .propose(proposal, &handle)
            .unwrap();
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

                // Drop only the Entry gossip for `target_owner` headed to
                // the victim. Everything else -- including all the ABA Vote
                // traffic that actually decides `target_owner`'s inclusion
                // -- is delivered normally.
                if id == victim
                    && matches!(&msg, MVBAMessage::Entry { owner, .. } if *owner == target_owner)
                {
                    progressed = true;
                    delivered += 1;
                    continue;
                }

                progressed = true;
                delivered += 1;
                assert!(delivered < 200_000, "MVBA cluster simulation did not converge");

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

    let victim_decision = instances.remove(&victim).unwrap().finalize().unwrap();
    let other_decision = instances.remove(&members[0]).unwrap().finalize().unwrap();

    let victim_owners = owners_of(&victim_decision);
    let other_owners = owners_of(&other_decision);

    assert!(
        other_owners.contains(&target_owner),
        "sanity check: a node that saw the Entry gossip should include target_owner in its \
         decided set"
    );
    assert_eq!(
        victim_owners, other_owners,
        "MVBA's Agreement property requires all honest nodes that terminate to output the same \
         value (Section 3); a node missing one Entry gossip message must not silently diverge \
         from everyone else's decided set. target_owner's per-owner ABA decided `true` \
         network-wide, so it must appear in the victim's output too, not just get silently \
         dropped by finalize()"
    );
}

#[test]
fn test_mvba_agreement_with_conflicting_proposals() {
    let keyset = fixtures::make_keyset(F);
    let members = fixtures::node_ids(N);

    let entries: HashMap<NodeId, (NodeId, Digest, CombinedSignature)> = members
        .iter()
        .map(|&owner| {
            (
                owner,
                valid_entry(&keyset, owner, fixtures::make_digest(&[owner.0 as u8])),
            )
        })
        .collect();

    // Each node proposes a different subset of the available entries.
    let proposals: HashMap<NodeId, MVBAProposal> = members
        .iter()
        .map(|&id| {
            let subset = members
                .iter()
                .filter(|&&owner| (owner.0 + id.0) % 2 == 0)
                .map(|owner| entries[owner].clone())
                .collect();
            (id, subset)
        })
        .collect();

    let decisions = run_mvba_cluster(N, F, &keyset, &proposals);

    assert_eq!(decisions.len(), N, "every node should still decide");

    let first = owners_of(&decisions[&members[0]]);
    for &id in &members {
        assert_eq!(
            owners_of(&decisions[&id]),
            first,
            "conflicting initial proposals must still converge on one agreed set"
        );
    }
}
