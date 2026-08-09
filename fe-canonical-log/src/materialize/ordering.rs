//! SPEC-4 §4.1-§4.7 deterministic causal replay order; see `src/AGENTS.md` §module-ownership
//! and `materialize/AGENTS.md`.
//!
//! The tie-break key is `(wall_ms, counter, author_public_key)` (§4.3). Topological order is
//! computed by repeatedly picking, among every currently-ready operation (every in-set parent
//! already emitted), the smallest ordering key -- so the result depends only on the DAG shape
//! and the ordering key, never on the order operations were inserted or arrived in. That
//! arrival-independence is what makes replay deterministic across peers and across restarts.

use std::collections::{BTreeSet, HashMap, HashSet};

use thiserror::Error;

use crate::envelope::Hash32;
use crate::materialize::traits::VerifiedEnvelopeMeta;

/// The §4.3 deterministic tie-break key for concurrent operations.
pub type OrderingKey = (u64, u32, [u8; 32]);

/// The ordering key for one verified operation.
pub fn ordering_key(meta: &VerifiedEnvelopeMeta) -> OrderingKey {
    (meta.wall_ms, meta.counter, meta.author_public_key)
}

/// Every reason a set of operations cannot be totally ordered.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum OrderingError {
    /// The in-set parent graph contains a cycle, which a valid DAG never does.
    #[error("parent graph among the supplied operations contains a cycle")]
    Cycle,
}

/// Orders `nodes` by topological dependency, breaking ties at each step by [`ordering_key`]
/// and finally by `op_id` for total determinism even in the (structurally impossible under a
/// well-formed DAG) case of two operations sharing an ordering key.
///
/// A parent not present in `nodes` is treated as already satisfied: callers pass the closed
/// set they want ordered together, typically a whole branch's admitted history or the closure
/// of one replay frontier.
pub fn deterministic_causal_order(
    nodes: &[VerifiedEnvelopeMeta],
) -> Result<Vec<Hash32>, OrderingError> {
    let known: HashSet<Hash32> = nodes.iter().map(|node| node.op_id).collect();
    let mut remaining_parents: HashMap<Hash32, HashSet<Hash32>> = nodes
        .iter()
        .map(|node| {
            let parents = node
                .parents
                .iter()
                .copied()
                .filter(|parent| known.contains(parent))
                .collect();
            (node.op_id, parents)
        })
        .collect();
    let by_id: HashMap<Hash32, &VerifiedEnvelopeMeta> =
        nodes.iter().map(|node| (node.op_id, node)).collect();

    let mut ready: BTreeSet<(u64, u32, [u8; 32], Hash32)> = BTreeSet::new();
    for node in nodes {
        if remaining_parents[&node.op_id].is_empty() {
            ready.insert(ready_tuple(node));
        }
    }

    let mut order = Vec::with_capacity(nodes.len());
    let mut emitted: HashSet<Hash32> = HashSet::with_capacity(nodes.len());
    while let Some(next) = ready.iter().next().copied() {
        ready.remove(&next);
        let op_id = next.3;
        order.push(op_id);
        emitted.insert(op_id);

        for node in nodes {
            if emitted.contains(&node.op_id) {
                continue;
            }
            let deps = remaining_parents
                .get_mut(&node.op_id)
                .expect("every node is tracked");
            if deps.remove(&op_id) && deps.is_empty() {
                ready.insert(ready_tuple(by_id[&node.op_id]));
            }
        }
    }

    if order.len() != nodes.len() {
        return Err(OrderingError::Cycle);
    }
    Ok(order)
}

fn ready_tuple(node: &VerifiedEnvelopeMeta) -> (u64, u32, [u8; 32], Hash32) {
    let (wall_ms, counter, author_public_key) = ordering_key(node);
    (wall_ms, counter, author_public_key, node.op_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Identifier32, Scope};

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn meta(
        op_filler: u8,
        wall_ms: u64,
        counter: u32,
        author_filler: u8,
        parents: Vec<Hash32>,
    ) -> VerifiedEnvelopeMeta {
        VerifiedEnvelopeMeta {
            op_id: hash(op_filler),
            operation_kind: 1,
            scope: Scope::verse_wide(identifier(0x01)),
            branch_id: identifier(0x02),
            schema_hash: hash(0x03),
            parents,
            author_public_key: [author_filler; 32],
            wall_ms,
            counter,
        }
    }

    /// genesis -> {sibling_a, sibling_b} -> merge, where sibling_a and sibling_b are
    /// HLC-concurrent (identical wall_ms/counter, different author).
    fn sample_dag() -> Vec<VerifiedEnvelopeMeta> {
        let genesis = meta(0x01, 100, 0, 0x10, vec![]);
        let sibling_a = meta(0x02, 200, 0, 0x20, vec![genesis.op_id]);
        let sibling_b = meta(0x03, 200, 0, 0x30, vec![genesis.op_id]);
        let mut merge_parents = vec![sibling_a.op_id, sibling_b.op_id];
        merge_parents.sort();
        let merge = meta(0x04, 300, 0, 0x40, merge_parents);
        vec![genesis, sibling_a, sibling_b, merge]
    }

    #[test]
    fn order_is_identical_across_every_insertion_permutation() {
        let dag = sample_dag();
        let baseline = deterministic_causal_order(&dag).expect("acyclic");

        let mut reversed = dag.clone();
        reversed.reverse();

        let interleaved = vec![
            dag[2].clone(),
            dag[0].clone(),
            dag[3].clone(),
            dag[1].clone(),
        ];
        let another_interleaving = vec![
            dag[3].clone(),
            dag[1].clone(),
            dag[2].clone(),
            dag[0].clone(),
        ];

        for permutation in [dag.clone(), reversed, interleaved, another_interleaving] {
            assert_eq!(
                deterministic_causal_order(&permutation).expect("acyclic"),
                baseline,
                "order must not depend on insertion order"
            );
        }
    }

    #[test]
    fn concurrent_siblings_are_ordered_by_wall_ms_then_counter_then_author_key() {
        let dag = sample_dag();
        let order = deterministic_causal_order(&dag).expect("acyclic");

        let index_of = |op_id: Hash32| order.iter().position(|id| *id == op_id).unwrap();
        let genesis_index = index_of(dag[0].op_id);
        let sibling_a_index = index_of(dag[1].op_id);
        let sibling_b_index = index_of(dag[2].op_id);
        let merge_index = index_of(dag[3].op_id);

        assert!(genesis_index < sibling_a_index);
        assert!(genesis_index < sibling_b_index);
        assert!(sibling_a_index < merge_index);
        assert!(sibling_b_index < merge_index);
        // Both siblings share (wall_ms, counter); sibling_a's author key (0x20) sorts before
        // sibling_b's (0x30), so sibling_a must come first regardless of arrival order.
        assert!(sibling_a_index < sibling_b_index);
    }

    #[test]
    fn a_cycle_among_the_supplied_operations_is_rejected() {
        let mut first = meta(0x01, 1, 0, 0x10, vec![hash(0x02)]);
        let second = meta(0x02, 2, 0, 0x20, vec![hash(0x01)]);
        first.parents = vec![second.op_id];
        assert_eq!(
            deterministic_causal_order(&[first, second]),
            Err(OrderingError::Cycle)
        );
    }

    #[test]
    fn a_parent_outside_the_supplied_set_is_treated_as_already_satisfied() {
        let external_parent = hash(0xff);
        let lone = meta(0x01, 1, 0, 0x10, vec![external_parent]);
        assert_eq!(
            deterministic_causal_order(std::slice::from_ref(&lone)).expect("acyclic"),
            vec![lone.op_id]
        );
    }
}
