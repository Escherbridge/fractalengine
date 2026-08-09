//! DAG frontier and parent-set rules (SPEC-1 §6); see `src/AGENTS.md` §module-ownership.

use thiserror::Error;

use crate::envelope::Hash32;

/// Every reason a frontier is not a well-formed D-CL19 frontier.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum FrontierError {
    /// A frontier always names at least one operation.
    #[error("a frontier must name at least one operation")]
    Empty,
    /// The same operation ID appeared twice.
    #[error("frontier contains a duplicate operation ID")]
    DuplicateOperationId {
        /// The repeated operation ID.
        op_id: Hash32,
    },
}

/// A strictly sorted, deduplicated, non-empty set of frontier operation IDs.
///
/// This is the single D-CL19 frontier primitive; SPEC-4 through SPEC-7 all commit to a
/// frontier through [`SortedFrontier::commitment`], never through their own ordering.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SortedFrontier(Vec<Hash32>);

impl SortedFrontier {
    /// Sorts the input and rejects an empty set or any duplicate; input order is irrelevant.
    pub fn try_new(op_ids: impl IntoIterator<Item = Hash32>) -> Result<Self, FrontierError> {
        let mut op_ids: Vec<Hash32> = op_ids.into_iter().collect();
        if op_ids.is_empty() {
            return Err(FrontierError::Empty);
        }
        op_ids.sort_unstable();
        if let Some(index) = op_ids.windows(2).position(|pair| pair[0] == pair[1]) {
            return Err(FrontierError::DuplicateOperationId {
                op_id: op_ids[index],
            });
        }
        Ok(Self(op_ids))
    }

    /// The operation IDs in byte-lexicographic order.
    pub fn as_slice(&self) -> &[Hash32] {
        &self.0
    }

    /// Number of operations in the frontier; always at least one.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false; a frontier is never empty.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// BLAKE3 over the concatenated 32-byte operation IDs in lexicographic order.
    pub fn commitment(&self) -> Hash32 {
        let mut hasher = blake3::Hasher::new();
        for op_id in &self.0 {
            hasher.update(op_id.as_bytes());
        }
        Hash32(*hasher.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op_id(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    #[test]
    fn commitment_is_independent_of_input_order() {
        let ascending = SortedFrontier::try_new([op_id(1), op_id(2), op_id(3)]).expect("frontier");
        let shuffled = SortedFrontier::try_new([op_id(3), op_id(1), op_id(2)]).expect("frontier");

        assert_eq!(ascending, shuffled);
        assert_eq!(ascending.as_slice(), shuffled.as_slice());
        assert_eq!(ascending.commitment(), shuffled.commitment());
        assert_eq!(ascending.as_slice(), &[op_id(1), op_id(2), op_id(3)][..]);
    }

    #[test]
    fn rejects_duplicates_and_the_empty_frontier() {
        assert_eq!(
            SortedFrontier::try_new(Vec::new()),
            Err(FrontierError::Empty)
        );
        assert_eq!(
            SortedFrontier::try_new([op_id(4), op_id(4)]),
            Err(FrontierError::DuplicateOperationId { op_id: op_id(4) })
        );
    }

    #[test]
    fn a_single_operation_frontier_commits_to_a_hash_of_its_own() {
        let only = op_id(9);
        let frontier = SortedFrontier::try_new([only]).expect("frontier");

        assert_eq!(frontier.len(), 1);
        assert!(!frontier.is_empty());
        assert_ne!(frontier.commitment(), only);
        assert_eq!(frontier.commitment(), Hash32::of(only.as_bytes()));
    }

    #[test]
    fn commitment_hashes_the_concatenated_identifiers() {
        let frontier = SortedFrontier::try_new([op_id(2), op_id(1)]).expect("frontier");
        let mut concatenated = Vec::new();
        concatenated.extend_from_slice(op_id(1).as_bytes());
        concatenated.extend_from_slice(op_id(2).as_bytes());
        assert_eq!(frontier.commitment(), Hash32::of(&concatenated));
    }

    #[test]
    fn different_frontiers_commit_differently() {
        let left = SortedFrontier::try_new([op_id(1), op_id(2)]).expect("frontier");
        let right = SortedFrontier::try_new([op_id(1), op_id(3)]).expect("frontier");
        assert_ne!(left.commitment(), right.commitment());
    }
}
