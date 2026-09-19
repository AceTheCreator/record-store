//! The Merkle tree a checkpoint commits over.
//!
//! A checkpoint has to do two jobs at once: commit to every record in its range
//! with a single digest that can be anchored externally, and let one record be
//! proved a member of that digest without handing over the rest of the log. A
//! binary hash tree does both, which is why the proof bundles in the next
//! workstream can name one object's records and nothing else.
//!
//! The construction is specified in `docs/reference/audit-chain.md` because an
//! independent verifier has to reproduce it exactly. Two details there are easy
//! to get wrong and are therefore pinned by tests here: leaves and interior
//! nodes are hashed with different prefixes, so an interior node can never be
//! presented as a leaf, and an odd node at any level is promoted unchanged
//! rather than duplicated, because duplicating it lets two different ranges
//! produce the same root.

use sha2::{Digest, Sha256};

use crate::chain::Digest32;

/// Domain separator for every hash in the tree.
const MERKLE_DOMAIN: &[u8] = b"record-store/audit-merkle/v1";
const LEAF_PREFIX: u8 = 0x00;
const NODE_PREFIX: u8 = 0x01;

/// Returns the tree leaf for one record hash.
#[must_use]
pub fn leaf_hash(record_hash: &Digest32) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(MERKLE_DOMAIN);
    hasher.update([LEAF_PREFIX]);
    hasher.update(record_hash);
    hasher.finalize().into()
}

/// Returns the interior node joining two children.
#[must_use]
pub fn node_hash(left: &Digest32, right: &Digest32) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(MERKLE_DOMAIN);
    hasher.update([NODE_PREFIX]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Which side of its parent a sibling sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The sibling is the left child, so the proved hash is the right one.
    Left,
    /// The sibling is the right child, so the proved hash is the left one.
    Right,
}

/// One step of an inclusion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PathStep {
    /// Which side the sibling is on.
    pub side: Side,
    /// The sibling's hash.
    #[serde(with = "crate::chain::digest_hex")]
    pub hash: Digest32,
}

/// A record's path from its leaf up to a checkpoint root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InclusionPath {
    /// Position of the record within the checkpoint's range.
    pub index: u64,
    /// Sibling hashes, innermost first.
    pub steps: Vec<PathStep>,
}

/// Returns the root committing to every record hash, in order.
///
/// `None` for an empty range: a checkpoint over no records would commit to
/// nothing while looking like it committed to something.
#[must_use]
pub fn root(record_hashes: &[Digest32]) -> Option<Digest32> {
    if record_hashes.is_empty() {
        return None;
    }
    let mut level: Vec<Digest32> = record_hashes.iter().map(leaf_hash).collect();
    while level.len() > 1 {
        level = combine(&level);
    }
    level.first().copied()
}

/// Reduces one level of the tree to its parents.
fn combine(level: &[Digest32]) -> Vec<Digest32> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut pairs = level.chunks_exact(2);
    for pair in &mut pairs {
        next.push(node_hash(&pair[0], &pair[1]));
    }
    // An odd node is carried up unchanged. Hashing it with itself instead would
    // make a range of three records produce the same root as a range of four
    // whose last entry repeats, so two different logs could agree.
    if let [odd] = pairs.remainder() {
        next.push(*odd);
    }
    next
}

/// Returns the path proving the record at `index` is under the root.
#[must_use]
pub fn inclusion_path(record_hashes: &[Digest32], index: usize) -> Option<InclusionPath> {
    if index >= record_hashes.len() {
        return None;
    }
    let mut level: Vec<Digest32> = record_hashes.iter().map(leaf_hash).collect();
    let mut position = index;
    let mut steps = Vec::new();
    while level.len() > 1 {
        // The promoted odd node has no sibling at this level, so it
        // contributes no step; it simply rises.
        let sibling = if position.is_multiple_of(2) {
            level.get(position + 1).map(|hash| PathStep {
                side: Side::Right,
                hash: *hash,
            })
        } else {
            level.get(position - 1).map(|hash| PathStep {
                side: Side::Left,
                hash: *hash,
            })
        };
        if let Some(step) = sibling {
            steps.push(step);
        }
        position /= 2;
        level = combine(&level);
    }
    Some(InclusionPath {
        index: index as u64,
        steps,
    })
}

/// Recomputes a root from one record hash and its path.
///
/// This is what an offline verifier runs: it never sees the other records, only
/// the digests on the way up.
#[must_use]
pub fn root_from_path(record_hash: &Digest32, path: &InclusionPath) -> Digest32 {
    let mut current = leaf_hash(record_hash);
    for step in &path.steps {
        current = match step.side {
            Side::Left => node_hash(&step.hash, &current),
            Side::Right => node_hash(&current, &step.hash),
        };
    }
    current
}

/// Returns whether a record hash is proved to sit under `expected_root`.
#[must_use]
pub fn verify_inclusion(
    record_hash: &Digest32,
    path: &InclusionPath,
    expected_root: &Digest32,
) -> bool {
    root_from_path(record_hash, path) == *expected_root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(count: usize) -> Vec<Digest32> {
        (0..count)
            .map(|index| Sha256::digest(format!("record-{index}")).into())
            .collect()
    }

    #[test]
    fn an_empty_range_has_no_root() {
        assert!(root(&[]).is_none());
    }

    #[test]
    fn a_single_record_roots_at_its_leaf() {
        let leaves = hashes(1);
        assert_eq!(root(&leaves), Some(leaf_hash(&leaves[0])));
    }

    /// Leaves and interior nodes must not be interchangeable, or a proof could
    /// present an interior node as though it were a record.
    #[test]
    fn a_leaf_and_a_node_over_the_same_bytes_differ() {
        let value = Sha256::digest(b"same").into();
        assert_ne!(leaf_hash(&value), node_hash(&value, &value));
    }

    /// The promotion rule, pinned. Duplicating the odd node instead would let a
    /// three-record range collide with a four-record one whose tail repeats.
    #[test]
    fn an_odd_node_is_promoted_rather_than_duplicated() {
        let three = hashes(3);
        let duplicated_tail = vec![three[0], three[1], three[2], three[2]];
        assert_ne!(root(&three), root(&duplicated_tail));

        // And the promotion is what the implementation actually does.
        let leaves: Vec<Digest32> = three.iter().map(leaf_hash).collect();
        let expected = node_hash(&node_hash(&leaves[0], &leaves[1]), &leaves[2]);
        assert_eq!(root(&three), Some(expected));
    }

    #[test]
    fn a_balanced_tree_matches_a_hand_computed_root() {
        let four = hashes(4);
        let leaves: Vec<Digest32> = four.iter().map(leaf_hash).collect();
        let expected = node_hash(
            &node_hash(&leaves[0], &leaves[1]),
            &node_hash(&leaves[2], &leaves[3]),
        );
        assert_eq!(root(&four), Some(expected));
    }

    /// Every record in every range size must prove against the root it is
    /// actually under. The odd sizes are where promotion bites.
    #[test]
    fn every_record_proves_against_its_root_at_every_range_size() {
        for count in 1..=33_usize {
            let records = hashes(count);
            let expected = root(&records).expect("a non-empty range has a root");
            for index in 0..count {
                let path = inclusion_path(&records, index).expect("in range");
                assert!(
                    verify_inclusion(&records[index], &path, &expected),
                    "record {index} of {count} must prove against its root"
                );
            }
        }
    }

    /// A proof must not transfer to a record that was never in the tree.
    #[test]
    fn a_path_does_not_prove_a_record_that_is_not_in_the_tree() {
        let records = hashes(8);
        let expected = root(&records).expect("root");
        let path = inclusion_path(&records, 3).expect("in range");
        let absent: Digest32 = Sha256::digest(b"never written").into();
        assert!(!verify_inclusion(&absent, &path, &expected));
    }

    /// Tampering with any step of the path must break it.
    #[test]
    fn altering_a_path_step_breaks_the_proof() {
        let records = hashes(8);
        let expected = root(&records).expect("root");
        for step_index in 0..3 {
            let mut path = inclusion_path(&records, 5).expect("in range");
            path.steps[step_index].hash[0] ^= 0xff;
            assert!(
                !verify_inclusion(&records[5], &path, &expected),
                "step {step_index} altered"
            );
        }
    }

    /// Flipping which side a sibling sits on changes the order it is hashed in,
    /// which must change the root.
    #[test]
    fn flipping_a_sibling_side_breaks_the_proof() {
        let records = hashes(8);
        let expected = root(&records).expect("root");
        let mut path = inclusion_path(&records, 5).expect("in range");
        path.steps[0].side = match path.steps[0].side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        };
        assert!(!verify_inclusion(&records[5], &path, &expected));
    }

    #[test]
    fn an_index_outside_the_range_has_no_path() {
        assert!(inclusion_path(&hashes(4), 4).is_none());
        assert!(inclusion_path(&[], 0).is_none());
    }

    /// Changing any record changes the root, so a checkpoint cannot cover an
    /// edited range and still match what was anchored.
    #[test]
    fn changing_any_record_changes_the_root() {
        let records = hashes(9);
        let original = root(&records).expect("root");
        for index in 0..records.len() {
            let mut edited = records.clone();
            edited[index][0] ^= 0xff;
            assert_ne!(root(&edited), Some(original), "record {index}");
        }
    }

    #[test]
    fn a_path_round_trips_through_serialization() {
        let records = hashes(7);
        let path = inclusion_path(&records, 6).expect("in range");
        let encoded = serde_json::to_vec(&path).expect("encode");
        let decoded: InclusionPath = serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded, path);
    }
}
