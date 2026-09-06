use atlas_common::crypto::hash::{Context, Digest};

/// A Merkle inclusion proof: sibling hashes from leaf to root, in that
/// order. Verifying a branch requires no other leaves -- only the claimed
/// root, the total leaf count, this leaf's index, its data, and this list.
pub(crate) type MerkleBranch = Vec<Digest>;

fn leaf_hash(shard: &[u8]) -> Digest {
    let mut ctx = Context::new();
    ctx.update(&[0x00]);
    ctx.update(shard);
    ctx.finish()
}

fn internal_hash(left: &Digest, right: &Digest) -> Digest {
    let mut ctx = Context::new();
    ctx.update(&[0x01]);
    ctx.update(left.as_ref());
    ctx.update(right.as_ref());
    ctx.finish()
}

/// The largest power of two strictly less than `len` (RFC 6962's `MTH`/
/// `PATH` split point). Requires `len > 1`.
fn split_point(len: usize) -> usize {
    debug_assert!(len > 1);

    1usize << (usize::BITS - 1 - (len - 1).leading_zeros())
}

enum MerkleNode {
    Leaf(Digest),
    Internal {
        hash: Digest,
        left: Box<MerkleNode>,
        right: Box<MerkleNode>,
    },
}

impl MerkleNode {
    fn hash(&self) -> Digest {
        match self {
            MerkleNode::Leaf(h) => *h,
            MerkleNode::Internal { hash, .. } => *hash,
        }
    }
}

fn build_node(leaf_hashes: &[Digest]) -> MerkleNode {
    if leaf_hashes.len() == 1 {
        return MerkleNode::Leaf(leaf_hashes[0]);
    }

    let k = split_point(leaf_hashes.len());
    let left = build_node(&leaf_hashes[..k]);
    let right = build_node(&leaf_hashes[k..]);
    let hash = internal_hash(&left.hash(), &right.hash());

    MerkleNode::Internal {
        hash,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// Walks the tree top-down, threading a shared sibling stack. At each leaf,
/// the branch is `reverse(stack)` -- `PATH`'s recursive definition builds
/// deepest-sibling-first by recursing before appending, which is exactly
/// what a top-down walk collects in reverse.
fn collect_branches(
    node: &MerkleNode,
    leaf_start: usize,
    leaf_count: usize,
    stack: &mut Vec<Digest>,
    out: &mut [MerkleBranch],
) {
    match node {
        MerkleNode::Leaf(_) => {
            let mut branch = stack.clone();
            branch.reverse();
            out[leaf_start] = branch;
        }
        MerkleNode::Internal { left, right, .. } => {
            let k = split_point(leaf_count);

            stack.push(right.hash());
            collect_branches(left, leaf_start, k, stack, out);
            stack.pop();

            stack.push(left.hash());
            collect_branches(right, leaf_start + k, leaf_count - k, stack, out);
            stack.pop();
        }
    }
}

/// Builds a Merkle tree over `shards` (sender-side). Returns the root and
/// one branch per shard, in the same order/index as `shards`.
pub(crate) fn build_tree(shards: &[Vec<u8>]) -> (Digest, Vec<MerkleBranch>) {
    assert!(
        !shards.is_empty(),
        "cannot build a Merkle tree over zero shards"
    );

    let leaf_hashes: Vec<Digest> = shards.iter().map(|s| leaf_hash(s)).collect();
    let root_node = build_node(&leaf_hashes);
    let root = root_node.hash();

    let mut branches = vec![Vec::new(); shards.len()];
    let mut stack = Vec::new();
    collect_branches(&root_node, 0, shards.len(), &mut stack, &mut branches);

    (root, branches)
}

/// Verifies that `shard` is the leaf at `leaf_index` (of `num_leaves` total)
/// under `expected_root`, given its `branch`. Needs nothing but this one
/// shard and its branch -- no other leaves required.
pub(crate) fn verify_branch(
    expected_root: Digest,
    num_leaves: usize,
    leaf_index: usize,
    shard: &[u8],
    branch: &MerkleBranch,
) -> bool {
    if leaf_index >= num_leaves {
        return false;
    }

    let mut cursor = 0usize;
    let computed = verify_recursive(
        0,
        num_leaves,
        leaf_index,
        leaf_hash(shard),
        branch,
        &mut cursor,
    );

    computed == expected_root && cursor == branch.len()
}

fn verify_recursive(
    range_start: usize,
    range_len: usize,
    target_index: usize,
    leaf_hash: Digest,
    branch: &MerkleBranch,
    cursor: &mut usize,
) -> Digest {
    if range_len == 1 {
        return leaf_hash;
    }

    let k = split_point(range_len);

    if target_index - range_start < k {
        let left = verify_recursive(range_start, k, target_index, leaf_hash, branch, cursor);
        let Some(&sibling) = branch.get(*cursor) else {
            // Malformed/short branch: return a value that can never match a
            // real root (the caller compares against `expected_root`).
            return Digest::blank();
        };
        *cursor += 1;

        internal_hash(&left, &sibling)
    } else {
        let right = verify_recursive(
            range_start + k,
            range_len - k,
            target_index,
            leaf_hash,
            branch,
            cursor,
        );
        let Some(&sibling) = branch.get(*cursor) else {
            return Digest::blank();
        };
        *cursor += 1;

        internal_hash(&sibling, &right)
    }
}

/// Recomputes the root over a fully-reconstructed shard set (Algorithm 5
/// line 11's "recompute Merkle root h'"). Used after RS reconstruction to
/// detect a Byzantine sender that dispersed an inconsistent codeword.
pub(crate) fn compute_root(shards: &[Vec<u8>]) -> Digest {
    let leaf_hashes: Vec<Digest> = shards.iter().map(|s| leaf_hash(s)).collect();
    build_node(&leaf_hashes).hash()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_leaf() {
        let shards = vec![vec![1, 2, 3]];
        let (root, branches) = build_tree(&shards);

        assert_eq!(branches.len(), 1);
        assert!(branches[0].is_empty());
        assert!(verify_branch(root, 1, 0, &shards[0], &branches[0]));
    }

    #[test]
    fn test_various_leaf_counts_round_trip() {
        for n in 1..=17 {
            let shards: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8; 4]).collect();
            let (root, branches) = build_tree(&shards);

            assert_eq!(branches.len(), n);

            for (i, shard) in shards.iter().enumerate() {
                assert!(
                    verify_branch(root, n, i, shard, &branches[i]),
                    "leaf {i} of {n} failed to verify"
                );
            }
        }
    }

    #[test]
    fn test_tampered_shard_fails_verification() {
        let shards: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i; 4]).collect();
        let (root, branches) = build_tree(&shards);

        let tampered = vec![99u8; 4];
        assert!(!verify_branch(root, 5, 2, &tampered, &branches[2]));
    }

    #[test]
    fn test_branch_for_wrong_index_fails_verification() {
        let shards: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i; 4]).collect();
        let (root, branches) = build_tree(&shards);

        // Branch for leaf 2 shouldn't verify leaf 3's data at index 3.
        assert!(!verify_branch(root, 5, 3, &shards[2], &branches[2]));
    }

    #[test]
    fn test_compute_root_matches_build_tree() {
        let shards: Vec<Vec<u8>> = (0..7u8).map(|i| vec![i; 3]).collect();
        let (root, _) = build_tree(&shards);

        assert_eq!(root, compute_root(&shards));
    }
}
