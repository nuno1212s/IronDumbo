use crate::quorum_info::quorum_info::ThresholdKeys;
use atlas_common::collections::HashMap;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_common::crypto::threshold_crypto::{
    CombineSignatureError, CombinedSignature, PartialSignature,
};
use atlas_common::node_id::NodeId;

/// Threshold coin-tossing (Algorithm 4's underlying CShare/CShareVerify/
/// CToss primitive, paper Appendix 8.2): collects f+1 valid partial
/// signature shares over a public `id` and combines them into a single,
/// unbiased value that is unpredictable in advance (before f+1 honest
/// shares exist) but that every node combining the same >=f+1 valid shares
/// derives identically (BLS threshold signatures are unique regardless of
/// which valid share subset combines them).
///
/// This is the same technique `async_bin_agreement_round::perform_coin_flip`
/// already uses for ABA's own single-bit common coin, factored out and
/// generalized so it can also drive Committee Election (Algorithm 4) and
/// MVBA's random permutation (Section 5.1/Fig. 4) without duplicating the
/// "collect shares, combine, derive pseudorandomness from the hash" logic.
#[derive(Debug, Default)]
pub(crate) struct CoinTossState {
    shares: HashMap<NodeId, PartialSignature>,
}

impl CoinTossState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `CShare_i(id)` -- Algorithm 4 line 3. `id` is public, domain-separated
    /// bytes; freshness/uniqueness across invocations comes from the caller
    /// choosing an `id` that varies per invocation (e.g. a round number),
    /// not from `id` being secret.
    pub(crate) fn own_share(threshold_keys: &ThresholdKeys, id: &[u8]) -> PartialSignature {
        threshold_keys.private_key().partially_sign(id)
    }

    /// `CShareVerify(id, j, sigma_j)` -- Algorithm 4 line 8.
    pub(crate) fn verify_share(
        threshold_keys: &ThresholdKeys,
        from: NodeId,
        id: &[u8],
        share: &PartialSignature,
    ) -> bool {
        threshold_keys
            .public_key()
            .verify_partial_signature(from.0 as usize, id, share)
            .is_ok()
    }

    /// Records an already-`verify_share`d share (idempotent: re-inserting
    /// the same sender overwrites, never double-counts). Returns `true`
    /// exactly the first time `|Sigma| == f+1` is crossed by this call.
    pub(crate) fn insert_share(&mut self, from: NodeId, share: PartialSignature, f: usize) -> bool {
        let was_below = self.shares.len() <= f;
        self.shares.insert(from, share);
        was_below && self.shares.len() > f
    }

    pub(crate) fn share_count(&self) -> usize {
        self.shares.len()
    }

    /// `CToss(id, Sigma)` -- Algorithm 4 line 6: combine `>= f+1` shares
    /// into a single, deterministic, unbiased signature. Requires at least
    /// `f+1` shares to have been inserted.
    pub(crate) fn toss(
        &self,
        threshold_keys: &ThresholdKeys,
    ) -> Result<CombinedSignature, CombineSignatureError> {
        let signatures = self.shares.iter().map(|(node, sig)| (node.0 as usize, sig));

        threshold_keys.public_key().combine_signatures(signatures)
    }
}

fn seed_from_coin(coin: &CombinedSignature) -> Digest {
    let serialized = bincode::serde::encode_to_vec(coin, bincode::config::standard())
        .expect("Failed to serialize combined signature");

    let mut ctx = Context::new();
    ctx.update(&serialized);
    ctx.finish()
}

/// Derives a shared, verifiable-after-the-fact random permutation of
/// `members` from a combined coin-toss signature. Every node that combines
/// the same coin gets the bit-identical permutation, but no one could have
/// predicted it before `f+1` honest shares existed. `members` must be
/// passed in the same canonical order by every caller (callers should sort
/// it first) so the derived permutation is consistent across the network.
pub(crate) fn derive_permutation(coin: &CombinedSignature, members: &[NodeId]) -> Vec<NodeId> {
    let seed = seed_from_coin(coin);

    let mut keyed: Vec<(Digest, NodeId)> = members
        .iter()
        .map(|&member| {
            let mut ctx = Context::new();
            ctx.update(seed.as_ref());
            ctx.update(&member.0.to_le_bytes());
            (ctx.finish(), member)
        })
        .collect();

    keyed.sort_by(|(a, _), (b, _)| a.cmp(b));

    keyed.into_iter().map(|(_, member)| member).collect()
}

/// Algorithm 4's `G: R -> {1,...,n}^s` committee draw: the first
/// `committee_size` entries of the same permutation.
pub(crate) fn derive_committee(
    coin: &CombinedSignature,
    members: &[NodeId],
    committee_size: usize,
) -> Vec<NodeId> {
    derive_permutation(coin, members)
        .into_iter()
        .take(committee_size)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures;

    #[test]
    fn test_toss_requires_f_plus_1_shares_and_agrees_across_nodes() {
        let n = 4;
        let f = 1;
        let keyset = fixtures::make_keyset(f);
        let cbc_keyset = fixtures::make_cbc_keyset(f);
        let members = fixtures::node_ids(n);
        let id = b"test-round-0";

        let mut state_a = CoinTossState::new();
        let mut state_b = CoinTossState::new();

        // Two different (but overlapping-by-at-least-one-honest) subsets of
        // f+1 shares should combine to the identical coin.
        for &member in &members[0..=f] {
            let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, member);
            let share = CoinTossState::own_share(&threshold_keys, id);
            assert!(CoinTossState::verify_share(
                &threshold_keys,
                member,
                id,
                &share
            ));
            state_a.insert_share(member, share, f);
        }

        for &member in &members[1..=(f + 1)] {
            let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, member);
            let share = CoinTossState::own_share(&threshold_keys, id);
            state_b.insert_share(member, share, f);
        }

        let verifier_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, members[0]);
        let coin_a = state_a.toss(&verifier_keys).unwrap();
        let coin_b = state_b.toss(&verifier_keys).unwrap();

        assert_eq!(
            coin_a, coin_b,
            "any valid f+1 share subset must combine identically"
        );
    }

    #[test]
    fn test_derive_permutation_is_a_permutation_and_deterministic() {
        let n = 7;
        let f = 2;
        let keyset = fixtures::make_keyset(f);
        let cbc_keyset = fixtures::make_cbc_keyset(f);
        let members = {
            let mut m = fixtures::node_ids(n);
            m.sort();
            m
        };

        let mut state = CoinTossState::new();
        for &member in members.iter().take(f + 1) {
            let threshold_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, member);
            let share = CoinTossState::own_share(&threshold_keys, b"perm-round");
            state.insert_share(member, share, f);
        }

        let verifier_keys = fixtures::make_threshold_keys(&keyset, &cbc_keyset, members[0]);
        let coin = state.toss(&verifier_keys).unwrap();

        let perm_a = derive_permutation(&coin, &members);
        let perm_b = derive_permutation(&coin, &members);

        assert_eq!(perm_a, perm_b, "must be deterministic given the same coin");

        let mut sorted_perm = perm_a.clone();
        sorted_perm.sort();
        assert_eq!(
            sorted_perm, members,
            "must be a permutation of the input set"
        );
    }

    #[test]
    fn test_derive_committee_varies_with_the_coin() {
        // Compare the *full* permutation (not just a small truncated
        // committee) over a large-ish `n`, so this test isn't a flaky
        // birthday-paradox coin flip: two independent random keysets
        // producing the identical n=30 permutation by chance has
        // probability ~1/30! -- indistinguishable from zero -- versus a
        // small truncated committee over a small n, which can collide with
        // non-negligible probability purely by chance.
        let n = 30;
        let f = 2;
        let keyset_a = fixtures::make_keyset(f);
        let keyset_b = fixtures::make_keyset(f);
        let cbc_keyset = fixtures::make_cbc_keyset(f);
        let members = {
            let mut m = fixtures::node_ids(n);
            m.sort();
            m
        };

        let toss = |keyset: &atlas_common::crypto::threshold_crypto::PrivateKeySet, id: &[u8]| {
            let mut state = CoinTossState::new();
            for &member in members.iter().take(f + 1) {
                let threshold_keys = fixtures::make_threshold_keys(keyset, &cbc_keyset, member);
                let share = CoinTossState::own_share(&threshold_keys, id);
                state.insert_share(member, share, f);
            }
            let verifier_keys = fixtures::make_threshold_keys(keyset, &cbc_keyset, members[0]);
            state.toss(&verifier_keys).unwrap()
        };

        let coin_a = toss(&keyset_a, b"round");
        let coin_b = toss(&keyset_b, b"round");

        let permutation_a = derive_permutation(&coin_a, &members);
        let permutation_b = derive_permutation(&coin_b, &members);

        assert_ne!(
            permutation_a, permutation_b,
            "independent keysets (independent randomness) should draw different permutations"
        );

        let committee_a = derive_committee(&coin_a, &members, f + 1);
        assert_eq!(committee_a.len(), f + 1);
        assert_eq!(
            committee_a,
            permutation_a[..f + 1],
            "the committee should be exactly the permutation's first f+1 entries"
        );
    }
}
