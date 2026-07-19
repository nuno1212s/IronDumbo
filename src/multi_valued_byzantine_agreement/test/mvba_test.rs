use crate::consistent_broadcast::messages::CBCMessage;
use crate::multi_valued_byzantine_agreement::messages::MVBAMessage;
use crate::multi_valued_byzantine_agreement::mvba::MultiValuedByzantineAgreement;
use crate::mvba::{MVBAProposal, MVBAProtocol, MVBAResult, MVBASendNode};
use crate::testing::fixtures;
use crate::testing::network_sim::{NodeHandle, SimulatedNetwork};
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{CombinedSignature, PrivateKeySet};
use atlas_common::node_id::NodeId;
use atlas_common::ordering::SeqNo;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

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
    f: usize,
    owner: NodeId,
    digest: Digest,
) -> (NodeId, Digest, CombinedSignature) {
    let shares: Vec<_> = (0..=f)
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

fn owners_of(proposal: &MVBAProposal) -> BTreeSet<NodeId> {
    proposal.iter().map(|(owner, _, _)| *owner).collect()
}

#[test]
fn test_mvba_propose_only_kicks_off_own_cbc_echo() {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let node = NodeId(0);
    let quorum = fixtures::quorum_info(N, F, node);
    let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, node);
    let network = MockNetwork::new();

    let digest = fixtures::make_digest(&[1]);
    let entry = valid_entry(&keyset, F, node, digest);

    let mut mvba = MultiValuedByzantineAgreement::new(quorum, threshold_keys, SeqNo::from(0u32));
    let result = mvba.propose(vec![entry], &network).unwrap();

    assert!(matches!(result, MVBAResult::Processed));

    let sent = network.sent.borrow();
    assert_eq!(
        sent.len(),
        1,
        "propose() should only trigger the initial CBC Send broadcast, not a per-owner ABA vote \
         the way the old n-ABA design used to"
    );
    assert!(matches!(
        &sent[0].0,
        MVBAMessage::Cbc { owner, message: CBCMessage::Send(_) } if *owner == node
    ));
}

/// Drives an `n`-node MVBA cluster (sharing one threshold keyset and one
/// CBC keyset) to a fixed point, each proposing whatever `proposals` gives
/// it (or nothing).
fn run_mvba_cluster(
    n: usize,
    f: usize,
    keyset: &PrivateKeySet,
    cbc_keyset: &PrivateKeySet,
    round: SeqNo,
    proposals: &HashMap<NodeId, MVBAProposal>,
) -> HashMap<NodeId, MVBAProposal> {
    let (decisions, _winner) =
        run_mvba_cluster_capture_winner(n, f, keyset, cbc_keyset, round, proposals, None);
    decisions
}

/// Same as [`run_mvba_cluster`], but also (a) reports the first `candidate`
/// any `Aba` message was ever sent for (deterministic given fixed keys and
/// round, since the coin-derived permutation is order-independent -- any
/// valid `>=f+1`-share subset combines to the identical signature), and (b)
/// optionally drops all `Cbc` traffic for one `(victim, owner)` pair,
/// simulating a node that never completes that owner's own CBC instance.
fn run_mvba_cluster_capture_winner(
    n: usize,
    f: usize,
    keyset: &PrivateKeySet,
    cbc_keyset: &PrivateKeySet,
    round: SeqNo,
    proposals: &HashMap<NodeId, MVBAProposal>,
    drop_cbc_for: Option<(NodeId, NodeId)>,
) -> (HashMap<NodeId, MVBAProposal>, Option<NodeId>) {
    let members = fixtures::node_ids(n);
    let bus = RefCell::new(SimulatedNetwork::<MVBAMessage>::new(&members));

    let mut instances: HashMap<NodeId, MultiValuedByzantineAgreement> = members
        .iter()
        .map(|&id| {
            let quorum = fixtures::quorum_info(n, f, id);
            let threshold_keys = fixtures::make_threshold_keys(keyset, cbc_keyset, id);
            (
                id,
                MultiValuedByzantineAgreement::new(quorum, threshold_keys, round),
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

    let mut winner: Option<NodeId> = None;
    let mut delivered = 0usize;

    loop {
        let mut progressed = false;

        for &id in &members {
            loop {
                let next = bus.borrow_mut().deliver_next(id);
                let Some((from, msg)) = next else {
                    break;
                };

                if let (Some((victim, owner_to_drop)), MVBAMessage::Cbc { owner, .. }) =
                    (drop_cbc_for, &msg)
                    && id == victim
                    && *owner == owner_to_drop
                {
                    progressed = true;
                    delivered += 1;
                    continue;
                }

                progressed = true;
                delivered += 1;
                assert!(
                    delivered < 500_000,
                    "MVBA cluster simulation did not converge"
                );

                if let MVBAMessage::Aba { candidate, .. } = &msg {
                    winner = winner.or(Some(*candidate));
                }

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

    let decisions = instances
        .into_iter()
        .filter_map(|(id, instance)| instance.finalize().ok().map(|proposal| (id, proposal)))
        .collect();

    (decisions, winner)
}

#[test]
fn test_mvba_validity_predicate() {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let bogus_keyset = fixtures::make_keyset(F);
    let members = fixtures::node_ids(N);

    // 3 of 4 nodes propose an identical, mutually valid, n-f-sized proposal;
    // the 4th proposes a shape-valid but signed-with-the-wrong-keyset
    // (bogus) proposal.
    let valid_entries: Vec<_> = members
        .iter()
        .take(N - F)
        .map(|&owner| valid_entry(&keyset, F, owner, fixtures::make_digest(&[owner.0 as u8])))
        .collect();
    let bogus_entries: Vec<_> = members
        .iter()
        .take(N - F)
        .map(|&owner| {
            valid_entry(
                &bogus_keyset,
                F,
                owner,
                fixtures::make_digest(&[owner.0 as u8]),
            )
        })
        .collect();

    let mut proposals: HashMap<NodeId, MVBAProposal> = members
        .iter()
        .map(|&id| (id, valid_entries.clone()))
        .collect();
    proposals.insert(members[N - 1], bogus_entries);

    let decisions = run_mvba_cluster(N, F, &keyset, &cbc_keyset, SeqNo::from(0u32), &proposals);

    assert_eq!(
        decisions.len(),
        N,
        "Termination should be unaffected by one malformed candidate"
    );

    let first = &decisions[&members[0]];
    assert!(
        !first.is_empty(),
        "the decided value must be non-empty (the bogus candidate alone can never be decided, \
         since Q_r never holds for it)"
    );
    for (_, digest, signature) in first {
        keyset
            .public_key_set()
            .verify(digest.as_ref(), signature)
            .expect(
                "External-Validity: every entry in the decided value must independently verify",
            );
    }
}

#[test]
fn test_mvba_all_agree_same_subset() {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let members = fixtures::node_ids(N);

    // Every node already has every entry: the simplest case where all
    // proposals are identical.
    let entries: Vec<_> = members
        .iter()
        .map(|&owner| valid_entry(&keyset, F, owner, fixtures::make_digest(&[owner.0 as u8])))
        .collect();

    let proposals: HashMap<NodeId, MVBAProposal> =
        members.iter().map(|&id| (id, entries.clone())).collect();

    let decisions = run_mvba_cluster(N, F, &keyset, &cbc_keyset, SeqNo::from(0u32), &proposals);

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

#[test]
fn test_mvba_agreement_with_conflicting_proposals() {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let members = fixtures::node_ids(N);

    let entries: HashMap<NodeId, (NodeId, Digest, CombinedSignature)> = members
        .iter()
        .map(|&owner| {
            (
                owner,
                valid_entry(&keyset, F, owner, fixtures::make_digest(&[owner.0 as u8])),
            )
        })
        .collect();

    // Each node proposes a different, but still Q_r-valid (exactly n-f
    // entries), subset: omit a different single owner each time.
    let proposals: HashMap<NodeId, MVBAProposal> = members
        .iter()
        .map(|&id| {
            let subset = members
                .iter()
                .filter(|&&owner| owner != id)
                .map(|owner| entries[owner].clone())
                .collect();
            (id, subset)
        })
        .collect();

    let decisions = run_mvba_cluster(N, F, &keyset, &cbc_keyset, SeqNo::from(0u32), &proposals);

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

/// The paper's headline efficiency claim for Dumbo2 (Section 5, Fig. 4) is
/// that its MVBA needs only an expected *constant* number of ABA instances
/// -- "three consecutive instances of ABA" -- independent of n, via the
/// leader-permutation-driven design implemented here. This test drives a
/// full cluster where every node proposes an identical, fully-valid,
/// full-n-entry `W` (so the very first candidate in the permutation always
/// wins unanimously, via ABA's Strong Validity fast path) and asserts that
/// only ever exactly one distinct candidate's `Aba` traffic is ever sent --
/// for both a small and a much larger quorum.
#[test]
fn test_mvba_aba_instance_count_is_independent_of_quorum_size() {
    for &(n, f) in &[(4usize, 1usize), (10usize, 3usize)] {
        let keyset = fixtures::make_keyset(f);
        let cbc_keyset = fixtures::make_cbc_keyset(f);
        let members = fixtures::node_ids(n);

        let entries: Vec<_> = members
            .iter()
            .map(|&owner| valid_entry(&keyset, f, owner, fixtures::make_digest(&[owner.0 as u8])))
            .collect();
        let proposals: HashMap<NodeId, MVBAProposal> =
            members.iter().map(|&id| (id, entries.clone())).collect();

        let bus = RefCell::new(SimulatedNetwork::<MVBAMessage>::new(&members));
        let mut instances: HashMap<NodeId, MultiValuedByzantineAgreement> = members
            .iter()
            .map(|&id| {
                let quorum = fixtures::quorum_info(n, f, id);
                let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, id);
                (
                    id,
                    MultiValuedByzantineAgreement::new(quorum, threshold_keys, SeqNo::from(0u32)),
                )
            })
            .collect();

        for &id in &members {
            let handle = NodeHandle::new(id, &bus);
            instances
                .get_mut(&id)
                .unwrap()
                .propose(proposals[&id].clone(), &handle)
                .unwrap();
        }

        let mut distinct_candidates: BTreeSet<NodeId> = BTreeSet::new();
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
                        delivered < 500_000,
                        "MVBA cluster simulation did not converge"
                    );

                    if let MVBAMessage::Aba { candidate, .. } = &msg {
                        distinct_candidates.insert(*candidate);
                    }

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

        assert_eq!(
            distinct_candidates.len(),
            1,
            "n={n}: expected exactly one ABA candidate to ever be tried (every node proposed an \
             identical valid W, so the first permutation entry always wins unanimously) -- \
             Dumbo2's efficiency claim is an O(1)/expected-constant ABA count, not O(n)"
        );
    }
}

/// MVBA's Agreement property requires that *all* honest nodes that
/// terminate output the same value (Section 3). This drives the SAME
/// cluster construction twice from identical keys: once with nothing
/// dropped (to discover, deterministically, which candidate the
/// coin-derived permutation picks first -- BLS threshold-signature
/// combination is unique regardless of which valid share subset produces
/// it, so both runs derive the identical coin/permutation), and once more
/// with the winning candidate's own CBC traffic dropped for one victim
/// node, forcing that node to adopt the winning value purely via another
/// node's self-certifying `VVote` proof rather than its own CBC completion.
#[test]
fn test_mvba_finalize_agrees_even_when_a_victim_never_completes_the_winning_candidates_cbc() {
    let keyset = fixtures::make_keyset(F);
    let cbc_keyset = fixtures::make_cbc_keyset(F);
    let members = fixtures::node_ids(N);
    let victim = members[N - 1];

    let entries: Vec<_> = members
        .iter()
        .map(|&owner| valid_entry(&keyset, F, owner, fixtures::make_digest(&[owner.0 as u8])))
        .collect();
    let proposals: HashMap<NodeId, MVBAProposal> =
        members.iter().map(|&id| (id, entries.clone())).collect();

    let (_, winner) = run_mvba_cluster_capture_winner(
        N,
        F,
        &keyset,
        &cbc_keyset,
        SeqNo::from(0u32),
        &proposals,
        None,
    );
    let winner = winner.expect("cluster should have tried at least one candidate");

    let (decisions, _) = run_mvba_cluster_capture_winner(
        N,
        F,
        &keyset,
        &cbc_keyset,
        SeqNo::from(0u32),
        &proposals,
        Some((victim, winner)),
    );

    assert_eq!(
        decisions.len(),
        N,
        "every node -- including the victim -- should still decide"
    );

    let victim_owners = owners_of(&decisions[&victim]);
    let other_owners = owners_of(&decisions[&members[0]]);

    assert_eq!(
        victim_owners, other_owners,
        "MVBA's Agreement property requires all honest nodes that terminate to output the same \
         value; the victim never completed the winning candidate's own CBC instance, but it must \
         still adopt the identical decided value via a peer's self-certifying VVote proof"
    );
}
