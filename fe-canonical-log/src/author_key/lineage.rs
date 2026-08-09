//! The §4 active-key state machine: pure in-memory logic over a caller-supplied causal view.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::envelope::{Hash32, Scope};

/// The causal facts the lineage resolver needs; the caller owns the DAG and its storage.
pub trait CausalOperationView {
    /// Parents of `operation`, or `None` when the operation is not present at all.
    fn parents(&self, operation: Hash32) -> Option<Vec<Hash32>>;

    /// Reports whether `operation` completed SPEC-1 admission.
    fn is_admitted(&self, operation: Hash32) -> bool;

    /// Reports whether `target` is `from` itself or one of its causal ancestors.
    fn reaches(&self, from: Hash32, target: Hash32) -> bool;
}

/// One admitted rotation reduced to the facts §4 resolution needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RotationRecord {
    /// Content address of the rotation operation.
    pub op_id: Hash32,
    /// Envelope scope of the rotation; §2.5 propagates it to descendant scopes.
    pub scope: Scope,
    /// The sole envelope parent, which induces the state the rotation is judged against.
    pub parent_op_id: Hash32,
    /// Key retired by this rotation.
    pub predecessor_public_key: [u8; 32],
    /// Key activated by this rotation.
    pub successor_public_key: [u8; 32],
}

/// What recording a rotation did to the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RotationOutcome {
    /// The successor becomes active on causal paths reaching this rotation.
    Applied,
    /// §4.6 rotation fork: every named successor stays inactive until a resolver exists.
    Fork {
        /// Every rotation in the fork group, this one included, in `op_id` order.
        conflicting_op_ids: Vec<Hash32>,
    },
}

/// State of one key at one causal point in one scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// The last non-superseded key in its lineage as resolved from this point.
    Active,
    /// Superseded by an admitted rotation reachable from this point.
    Retired {
        /// Key that replaced it.
        successor_public_key: [u8; 32],
        /// Rotation that retired it.
        rotation_op_id: Hash32,
    },
    /// Named by a §4.6 rotation fork, so neither side may be treated as active.
    ForkedUnresolved {
        /// Rotations in the fork group, in `op_id` order.
        rotation_op_ids: Vec<Hash32>,
    },
    /// No lineage evidence reaches this point for this key in this scope.
    Unknown,
}

/// Every reason a rotation cannot be recorded.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LineageError {
    /// The same `op_id` was recorded twice.
    #[error("rotation {op_id:?} is already recorded")]
    DuplicateRotationOperation {
        /// The repeated operation.
        op_id: Hash32,
    },
    /// Predecessor and successor were the same key.
    #[error("the successor key must differ from the predecessor key")]
    SuccessorEqualsPredecessor,
    /// The successor already occurs earlier in the predecessor's lineage (§3 rule 4).
    #[error("the successor key already occurs in the predecessor's lineage")]
    LineageCycle,
    /// Another rotation already transitions into this successor at an overlapping scope.
    #[error("successor key already activated by rotation {existing_op_id:?}")]
    DuplicateSuccessorTransition {
        /// The rotation that already names this successor.
        existing_op_id: Hash32,
    },
    /// The predecessor was not active at the parent-induced state (§3 rule 2, §4.6).
    #[error("the predecessor is not active at the parent-induced state: {state:?}")]
    PredecessorNotActive {
        /// The state actually resolved.
        state: KeyState,
    },
}

/// Scope-local lineage of admitted rotations; resolution is always causal, never by arrival
/// order or wall clock.
#[derive(Clone, Debug, Default)]
pub struct LineageIndex {
    rotations: BTreeMap<Hash32, RotationRecord>,
    roots: Vec<(Scope, [u8; 32])>,
}

impl LineageIndex {
    /// Builds an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a key that is known active in `scope` without a preceding rotation.
    pub fn register_root_key(&mut self, scope: Scope, public_key: [u8; 32]) {
        if !self
            .roots
            .iter()
            .any(|(known_scope, known_key)| *known_scope == scope && *known_key == public_key)
        {
            self.roots.push((scope, public_key));
        }
    }

    /// The rotation recorded under `op_id`, if any.
    pub fn rotation(&self, op_id: Hash32) -> Option<&RotationRecord> {
        self.rotations.get(&op_id)
    }

    /// Number of recorded rotations.
    pub fn len(&self) -> usize {
        self.rotations.len()
    }

    /// Reports whether no rotation is recorded.
    pub fn is_empty(&self) -> bool {
        self.rotations.is_empty()
    }

    /// Removes a rotation, for the §3.4 equivocation rule that materializes neither candidate.
    pub fn retract_rotation(&mut self, op_id: Hash32) -> bool {
        self.rotations.remove(&op_id).is_some()
    }

    /// Records one admitted rotation, rejecting every §3.4/§4 structural violation.
    pub fn record_rotation(
        &mut self,
        record: RotationRecord,
        view: &impl CausalOperationView,
    ) -> Result<RotationOutcome, LineageError> {
        if self.rotations.contains_key(&record.op_id) {
            return Err(LineageError::DuplicateRotationOperation {
                op_id: record.op_id,
            });
        }
        if record.predecessor_public_key == record.successor_public_key {
            return Err(LineageError::SuccessorEqualsPredecessor);
        }
        if self.lineage_reaches_key(
            record.predecessor_public_key,
            record.successor_public_key,
            record.scope,
        ) {
            return Err(LineageError::LineageCycle);
        }
        if let Some(existing) = self.rotations.values().find(|candidate| {
            candidate.successor_public_key == record.successor_public_key
                && scopes_overlap(candidate.scope, record.scope)
        }) {
            return Err(LineageError::DuplicateSuccessorTransition {
                existing_op_id: existing.op_id,
            });
        }

        let predecessor_state = self.active_key_at(
            record.parent_op_id,
            record.predecessor_public_key,
            record.scope,
            view,
        );
        if predecessor_state != KeyState::Active {
            return Err(LineageError::PredecessorNotActive {
                state: predecessor_state,
            });
        }

        let mut conflicting_op_ids: Vec<Hash32> = self
            .rotations
            .values()
            .filter(|candidate| {
                candidate.predecessor_public_key == record.predecessor_public_key
                    && scopes_overlap(candidate.scope, record.scope)
                    && !view.reaches(record.parent_op_id, candidate.op_id)
                    && !view.reaches(candidate.op_id, record.op_id)
            })
            .map(|candidate| candidate.op_id)
            .collect();

        self.rotations.insert(record.op_id, record);
        if conflicting_op_ids.is_empty() {
            return Ok(RotationOutcome::Applied);
        }
        conflicting_op_ids.push(record.op_id);
        conflicting_op_ids.sort_unstable();
        Ok(RotationOutcome::Fork { conflicting_op_ids })
    }

    /// Resolves the §4 state of `public_key` for `scope` as seen from `causal_point`.
    ///
    /// §2.5 propagation: a rotation applies when its own scope contains the queried scope, so a
    /// verse-scope rotation reaches descendant petals and resources while a narrower rotation
    /// never leaks outside its subtree or across verses.
    pub fn active_key_at(
        &self,
        causal_point: Hash32,
        public_key: [u8; 32],
        scope: Scope,
        view: &impl CausalOperationView,
    ) -> KeyState {
        let outgoing: Vec<Hash32> = self
            .reachable_rotations(causal_point, scope, view)
            .filter(|record| record.predecessor_public_key == public_key)
            .map(|record| record.op_id)
            .collect();
        if outgoing.len() > 1 {
            return KeyState::ForkedUnresolved {
                rotation_op_ids: outgoing,
            };
        }
        if let Some(rotation_op_id) = outgoing.first().copied() {
            let successor_public_key = self.rotations[&rotation_op_id].successor_public_key;
            return KeyState::Retired {
                successor_public_key,
                rotation_op_id,
            };
        }

        let incoming: Vec<Hash32> = self
            .reachable_rotations(causal_point, scope, view)
            .filter(|record| record.successor_public_key == public_key)
            .map(|record| record.op_id)
            .collect();
        for rotation_op_id in &incoming {
            let predecessor = self.rotations[rotation_op_id].predecessor_public_key;
            let siblings: Vec<Hash32> = self
                .reachable_rotations(causal_point, scope, view)
                .filter(|record| record.predecessor_public_key == predecessor)
                .map(|record| record.op_id)
                .collect();
            if siblings.len() > 1 {
                return KeyState::ForkedUnresolved {
                    rotation_op_ids: siblings,
                };
            }
        }
        if !incoming.is_empty() {
            return KeyState::Active;
        }

        if self
            .roots
            .iter()
            .any(|(root_scope, root_key)| *root_key == public_key && root_scope.contains(&scope))
        {
            return KeyState::Active;
        }
        KeyState::Unknown
    }

    /// Rotations that apply to `scope` and are causally reachable from `causal_point`,
    /// in `op_id` order.
    fn reachable_rotations<'a, V: CausalOperationView>(
        &'a self,
        causal_point: Hash32,
        scope: Scope,
        view: &'a V,
    ) -> impl Iterator<Item = &'a RotationRecord> + 'a {
        self.rotations.values().filter(move |record| {
            record.scope.contains(&scope) && view.reaches(causal_point, record.op_id)
        })
    }

    /// Reports whether `candidate_key` is `start_key` or one of its lineage ancestors.
    fn lineage_reaches_key(
        &self,
        start_key: [u8; 32],
        candidate_key: [u8; 32],
        scope: Scope,
    ) -> bool {
        let mut current = start_key;
        let mut visited = BTreeSet::new();
        loop {
            if current == candidate_key {
                return true;
            }
            if !visited.insert(current) {
                return true;
            }
            match self.rotations.values().find(|record| {
                record.successor_public_key == current && scopes_overlap(record.scope, scope)
            }) {
                Some(record) => current = record.predecessor_public_key,
                None => return false,
            }
        }
    }
}

/// Reports whether either scope contains the other, which is when two lineage records touch
/// the same subtree.
fn scopes_overlap(left: Scope, right: Scope) -> bool {
    left.contains(&right) || right.contains(&left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::test_support::{
        hash, identifier, petal_scope, public_key, resource_scope, verse_scope, FakeDag,
    };

    /// `genesis -> first -> second` with `fork_left` and `fork_right` both branching off `first`.
    fn dag() -> FakeDag {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x02), vec![hash(0x01)]);
        dag.insert(hash(0x03), vec![hash(0x02)]);
        dag
    }

    fn record(
        op_id: u8,
        parent: u8,
        scope: Scope,
        predecessor: u8,
        successor: u8,
    ) -> RotationRecord {
        RotationRecord {
            op_id: hash(op_id),
            scope,
            parent_op_id: hash(parent),
            predecessor_public_key: public_key(predecessor),
            successor_public_key: public_key(successor),
        }
    }

    fn seeded_index() -> LineageIndex {
        let mut index = LineageIndex::new();
        index.register_root_key(verse_scope(), public_key(1));
        index
    }

    #[test]
    fn spec2_case_01_a_recorded_rotation_activates_exactly_the_announced_successor() {
        let dag = dag();
        let mut index = seeded_index();
        assert_eq!(
            index.record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag),
            Ok(RotationOutcome::Applied)
        );

        assert_eq!(
            index.active_key_at(hash(0x02), public_key(1), verse_scope(), &dag),
            KeyState::Retired {
                successor_public_key: public_key(2),
                rotation_op_id: hash(0x02),
            }
        );
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(2), verse_scope(), &dag),
            KeyState::Active
        );
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(3), verse_scope(), &dag),
            KeyState::Unknown
        );
    }

    #[test]
    fn spec2_case_06_an_old_key_operation_before_the_rotation_still_sees_it_active() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");

        assert_eq!(
            index.active_key_at(hash(0x01), public_key(1), verse_scope(), &dag),
            KeyState::Active,
            "a causal point that does not reach the rotation still sees the predecessor active"
        );
        assert_eq!(
            index.active_key_at(hash(0x03), public_key(1), verse_scope(), &dag),
            KeyState::Retired {
                successor_public_key: public_key(2),
                rotation_op_id: hash(0x02),
            },
            "a causal point after the rotation sees the predecessor retired"
        );
    }

    #[test]
    fn spec2_case_06_a_causally_later_rotation_by_a_retired_predecessor_is_rejected() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");

        assert_eq!(
            index.record_rotation(record(0x03, 0x02, verse_scope(), 1, 3), &dag),
            Err(LineageError::PredecessorNotActive {
                state: KeyState::Retired {
                    successor_public_key: public_key(2),
                    rotation_op_id: hash(0x02),
                },
            })
        );
    }

    #[test]
    fn spec2_case_03_a_lineage_cycle_is_rejected() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");
        assert_eq!(
            index.record_rotation(record(0x03, 0x02, verse_scope(), 2, 1), &dag),
            Err(LineageError::LineageCycle)
        );
    }

    #[test]
    fn spec2_case_03_a_duplicate_successor_transition_is_rejected() {
        let mut dag = dag();
        dag.insert(hash(0x04), vec![hash(0x01)]);
        let mut index = seeded_index();
        index.register_root_key(verse_scope(), public_key(5));
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");
        assert_eq!(
            index.record_rotation(record(0x04, 0x01, verse_scope(), 5, 2), &dag),
            Err(LineageError::DuplicateSuccessorTransition {
                existing_op_id: hash(0x02),
            })
        );
    }

    #[test]
    fn a_rotation_recorded_twice_is_rejected_and_a_self_rotation_is_rejected() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");
        assert_eq!(
            index.record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag),
            Err(LineageError::DuplicateRotationOperation { op_id: hash(0x02) })
        );
        assert_eq!(
            index.record_rotation(record(0x03, 0x02, verse_scope(), 2, 2), &dag),
            Err(LineageError::SuccessorEqualsPredecessor)
        );
    }

    #[test]
    fn spec2_case_04_a_rotation_by_an_unknown_predecessor_does_not_activate_its_successor() {
        let dag = dag();
        let mut index = seeded_index();
        assert_eq!(
            index.record_rotation(record(0x02, 0x01, verse_scope(), 7, 8), &dag),
            Err(LineageError::PredecessorNotActive {
                state: KeyState::Unknown,
            })
        );
        assert_eq!(
            index.active_key_at(hash(0x03), public_key(8), verse_scope(), &dag),
            KeyState::Unknown
        );
    }

    #[test]
    fn spec2_case_07_two_concurrent_rotations_fork_and_activate_neither_successor() {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x02), vec![hash(0x01)]);
        dag.insert(hash(0x03), vec![hash(0x01)]);
        dag.insert(hash(0x04), vec![hash(0x02), hash(0x03)]);

        let mut index = seeded_index();
        assert_eq!(
            index.record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag),
            Ok(RotationOutcome::Applied)
        );
        assert_eq!(
            index.record_rotation(record(0x03, 0x01, verse_scope(), 1, 3), &dag),
            Ok(RotationOutcome::Fork {
                conflicting_op_ids: vec![hash(0x02), hash(0x03)],
            })
        );

        let forked = KeyState::ForkedUnresolved {
            rotation_op_ids: vec![hash(0x02), hash(0x03)],
        };
        assert_eq!(
            index.active_key_at(hash(0x04), public_key(1), verse_scope(), &dag),
            forked
        );
        assert_eq!(
            index.active_key_at(hash(0x04), public_key(2), verse_scope(), &dag),
            forked,
            "neither successor becomes active while the fork is unresolved"
        );
        assert_eq!(
            index.active_key_at(hash(0x04), public_key(3), verse_scope(), &dag),
            forked
        );
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(2), verse_scope(), &dag),
            KeyState::Active,
            "a causal point that reaches only one side never observes the fork"
        );
    }

    #[test]
    fn a_verse_rotation_propagates_to_descendant_scopes_and_stops_at_the_verse_boundary() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");

        for scope in [verse_scope(), petal_scope(), resource_scope()] {
            assert_eq!(
                index.active_key_at(hash(0x02), public_key(1), scope, &dag),
                KeyState::Retired {
                    successor_public_key: public_key(2),
                    rotation_op_id: hash(0x02),
                },
                "a verse-scope rotation applies to every descendant scope"
            );
        }

        let other_verse = Scope::verse_wide(identifier(0x21));
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(1), other_verse, &dag),
            KeyState::Unknown,
            "a rotation has no effect in another verse"
        );
    }

    #[test]
    fn a_petal_rotation_never_leaks_up_to_the_verse_or_across_to_a_sibling_petal() {
        let dag = dag();
        let mut index = LineageIndex::new();
        index.register_root_key(petal_scope(), public_key(1));
        index
            .record_rotation(record(0x02, 0x01, petal_scope(), 1, 2), &dag)
            .expect("apply");

        assert_eq!(
            index.active_key_at(hash(0x02), public_key(1), resource_scope(), &dag),
            KeyState::Retired {
                successor_public_key: public_key(2),
                rotation_op_id: hash(0x02),
            }
        );
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(1), verse_scope(), &dag),
            KeyState::Unknown,
            "a narrower rotation never widens to the verse"
        );

        let sibling_petal =
            Scope::new(identifier(0x11), Some(identifier(0x19)), None).expect("sibling petal");
        assert_eq!(
            index.active_key_at(hash(0x02), public_key(1), sibling_petal, &dag),
            KeyState::Unknown
        );
    }

    #[test]
    fn retracting_a_rotation_restores_the_state_that_preceded_it() {
        let dag = dag();
        let mut index = seeded_index();
        index
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");
        assert!(index.retract_rotation(hash(0x02)));
        assert!(index.is_empty());
        assert_eq!(
            index.active_key_at(hash(0x03), public_key(1), verse_scope(), &dag),
            KeyState::Active
        );
        assert!(!index.retract_rotation(hash(0x02)));
    }

    #[test]
    fn spec2_case_11_a_regenerated_key_cannot_continue_a_lost_key_lineage() {
        let dag = dag();
        let mut index = seeded_index();
        let regenerated = record(0x02, 0x01, verse_scope(), 9, 2);
        assert_eq!(
            index.record_rotation(regenerated, &dag),
            Err(LineageError::PredecessorNotActive {
                state: KeyState::Unknown,
            }),
            "a key generated after secret loss has no lineage to continue"
        );
        assert_eq!(
            index.active_key_at(hash(0x03), public_key(9), verse_scope(), &dag),
            KeyState::Unknown
        );
    }

    #[test]
    fn spec2_case_10_two_independent_indexes_resolve_one_head_identically() {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x02), vec![hash(0x01)]);
        dag.insert(hash(0x03), vec![hash(0x02)]);

        let first_order = [
            record(0x02, 0x01, verse_scope(), 1, 2),
            record(0x03, 0x02, verse_scope(), 2, 3),
        ];
        let mut left = seeded_index();
        for entry in first_order {
            left.record_rotation(entry, &dag).expect("apply");
        }

        // The second index learns the same rotations through a different admission path: the
        // later rotation is presented first, rejected as not-yet-resolvable, and retried.
        let mut right = seeded_index();
        assert!(right
            .record_rotation(record(0x03, 0x02, verse_scope(), 2, 3), &dag)
            .is_err());
        right
            .record_rotation(record(0x02, 0x01, verse_scope(), 1, 2), &dag)
            .expect("apply");
        right
            .record_rotation(record(0x03, 0x02, verse_scope(), 2, 3), &dag)
            .expect("apply");

        for key in [public_key(1), public_key(2), public_key(3)] {
            assert_eq!(
                left.active_key_at(hash(0x03), key, verse_scope(), &dag),
                right.active_key_at(hash(0x03), key, verse_scope(), &dag)
            );
        }
        assert_eq!(left.len(), right.len());
    }
}
