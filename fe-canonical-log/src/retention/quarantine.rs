//! Bounded pre-admission quarantine (SPEC-5 §4); see `src/retention/AGENTS.md` §quarantine.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::envelope::{Hash32, Hlc, Scope};

/// Why a candidate operation cannot yet be admitted; see `src/retention/AGENTS.md` §quarantine
/// for the promotion path (or lack of one) each variant carries.
///
/// The crate's single quarantine vocabulary, shared with SPEC-2 lifecycle admission and SPEC-6
/// receipt refusal; see `src/AGENTS.md` §unified-vocabularies. Only `MissingParent`,
/// `UnknownSchema`, and `UnknownKind` have promotion paths here, and `AuthorEquivocation`
/// deliberately has none.
pub use crate::compose::{QuarantineReason, QuarantineReasonClass};

/// A candidate operation withheld from admission, with the evidence needed to re-evaluate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantineRecord {
    /// `op_id` the peer or sender claimed for this candidate.
    pub claimed_op_id: Hash32,
    /// `op_id` this process would derive by hashing `candidate_bytes` itself.
    pub derived_op_id: Hash32,
    /// The exact received candidate bytes, retained unmodified.
    pub candidate_bytes: Vec<u8>,
    /// Authorization scope the candidate declared (SPEC-3 §1.7 containment, disavow
    /// scope-matching, and lineage resolution all key off this per amendment M2).
    pub scope: Scope,
    /// Why the candidate is quarantined.
    pub reason: QuarantineReason,
    /// When this candidate was first observed.
    pub first_seen: Hlc,
    /// When this candidate's reason was last re-evaluated.
    pub last_validation: Hlc,
    /// Free-form provenance diagnostics (e.g. which peer or ingress path produced it).
    pub provenance: String,
}

/// The entry and byte budget for one [`QuarantineReasonClass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReasonBudget {
    /// Maximum number of records of this class the store may hold at once.
    pub max_entries: usize,
    /// Maximum sum of `candidate_bytes.len()` across held records of this class.
    pub max_total_bytes: usize,
}

/// One budget per reason class (D-CL28 gate G1).
///
/// A struct with a field per class, never a map: a map can omit a class, and an omitted class is
/// either unbounded or silently zero. Neither is a decision an operator should be able to make
/// by accident, so the type has no shape that leaves a class unbudgeted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerReasonBudgets {
    /// Budget for candidates awaiting their declared parent closure.
    pub missing_parent: ReasonBudget,
    /// Budget for candidates awaiting a registered schema.
    pub unknown_schema: ReasonBudget,
    /// Budget for candidates whose operation kind this build cannot interpret.
    pub unknown_kind: ReasonBudget,
    /// Budget shared by every other reason.
    pub other: ReasonBudget,
}

impl PerReasonBudgets {
    /// The budget `class` draws against.
    pub fn for_class(&self, class: QuarantineReasonClass) -> ReasonBudget {
        match class {
            QuarantineReasonClass::MissingParent => self.missing_parent,
            QuarantineReasonClass::UnknownSchema => self.unknown_schema,
            QuarantineReasonClass::UnknownKind => self.unknown_kind,
            QuarantineReasonClass::Other => self.other,
        }
    }
}

/// Reserved policy numbers (D-CL24): quarantine bounds are ALWAYS caller-supplied. There is no
/// `Default` impl and no constant here — see `src/retention/AGENTS.md` §reserved-policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuarantineBounds {
    /// Maximum number of records this store may hold at once, across every class.
    pub max_entries: usize,
    /// Maximum sum of `candidate_bytes.len()` across all held records, across every class.
    pub max_total_bytes: usize,
    /// Maximum age, in milliseconds of `Hlc::wall_ms`, before a record is eviction-eligible.
    pub max_age_ms: u64,
    /// Maximum number of missing parents a single `MissingParent` candidate may declare.
    pub max_parent_depth: usize,
    /// Minimum milliseconds between re-evaluation attempts for one record.
    pub retry_cadence_ms: u64,
    /// Per-class budgets, enforced before and independently of the pool-wide bounds above.
    pub per_reason: PerReasonBudgets,
}

/// Storage seam for quarantine records; Wave 3 implements this in `fe-database`.
pub trait QuarantineStore {
    /// Inserts or replaces the record keyed by `claimed_op_id`.
    fn insert(&mut self, record: QuarantineRecord);
    /// Removes and returns the record for `claimed_op_id`, if present.
    fn remove(&mut self, claimed_op_id: &Hash32) -> Option<QuarantineRecord>;
    /// Looks up the record for `claimed_op_id`, if present.
    fn get(&self, claimed_op_id: &Hash32) -> Option<&QuarantineRecord>;
    /// All records currently held, in unspecified order.
    fn entries(&self) -> Vec<&QuarantineRecord>;
    /// Number of records currently held.
    fn len(&self) -> usize;
    /// Whether no records are currently held.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Sum of `candidate_bytes.len()` across all held records.
    fn total_bytes(&self) -> usize;
    /// Records currently held whose reason draws against `class`.
    ///
    /// Derived from [`Self::entries`] by default; a storage-backed implementer SHOULD override
    /// both class accessors with an indexed query rather than scanning the pool.
    fn class_len(&self, class: QuarantineReasonClass) -> usize {
        self.entries()
            .into_iter()
            .filter(|record| record.reason.class() == class)
            .count()
    }
    /// Sum of `candidate_bytes.len()` across held records drawing against `class`.
    fn class_total_bytes(&self, class: QuarantineReasonClass) -> usize {
        self.entries()
            .into_iter()
            .filter(|record| record.reason.class() == class)
            .map(|record| record.candidate_bytes.len())
            .sum()
    }
}

/// In-memory [`QuarantineStore`] for unit tests and single-process use.
#[derive(Default)]
pub struct InMemoryQuarantineStore {
    records: BTreeMap<Hash32, QuarantineRecord>,
}

impl QuarantineStore for InMemoryQuarantineStore {
    fn insert(&mut self, record: QuarantineRecord) {
        self.records.insert(record.claimed_op_id, record);
    }

    fn remove(&mut self, claimed_op_id: &Hash32) -> Option<QuarantineRecord> {
        self.records.remove(claimed_op_id)
    }

    fn get(&self, claimed_op_id: &Hash32) -> Option<&QuarantineRecord> {
        self.records.get(claimed_op_id)
    }

    fn entries(&self) -> Vec<&QuarantineRecord> {
        self.records.values().collect()
    }

    fn len(&self) -> usize {
        self.records.len()
    }

    fn total_bytes(&self) -> usize {
        self.records
            .values()
            .map(|record| record.candidate_bytes.len())
            .sum()
    }
}

/// Every reason [`admit_candidate`] fails closed rather than admitting or evicting.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QuarantineAdmitError {
    /// Admitting this record would exceed `QuarantineBounds::max_entries`.
    #[error("quarantine entry capacity {limit} exceeded ({current} present)")]
    EntryCapacityExceeded {
        /// The configured bound.
        limit: usize,
        /// Entries already present before this attempt.
        current: usize,
    },
    /// Admitting this record would exceed `QuarantineBounds::max_total_bytes`.
    #[error("quarantine byte budget {limit} exceeded ({projected} bytes would be held)")]
    ByteBudgetExceeded {
        /// The configured bound.
        limit: usize,
        /// Bytes that would be held including this candidate.
        projected: usize,
    },
    /// The candidate declared more missing parents than `QuarantineBounds::max_parent_depth`.
    #[error("declared missing-parent count {declared} exceeds bound {limit}")]
    ParentDepthExceeded {
        /// Missing parents the candidate declared.
        declared: usize,
        /// The configured bound.
        limit: usize,
    },
    /// Admitting this record would exceed its own reason class's entry budget (gate G1).
    #[error("quarantine entry capacity {limit} for {class:?} exceeded ({current} present)")]
    ReasonEntryCapacityExceeded {
        /// The class whose budget is exhausted.
        class: QuarantineReasonClass,
        /// The configured bound for that class.
        limit: usize,
        /// Records of that class already present before this attempt.
        current: usize,
    },
    /// Admitting this record would exceed its own reason class's byte budget (gate G1).
    #[error(
        "quarantine byte budget {limit} for {class:?} exceeded ({projected} bytes would be held)"
    )]
    ReasonByteBudgetExceeded {
        /// The class whose budget is exhausted.
        class: QuarantineReasonClass,
        /// The configured bound for that class.
        limit: usize,
        /// Bytes of that class that would be held including this candidate.
        projected: usize,
    },
}

/// Admits `record` into `store`, failing closed on any bound rather than evicting existing
/// history. On error the store is left byte-for-byte unchanged: this function never removes an
/// already-admitted record to make room for a new one.
///
/// The record's own reason class is checked before the pool-wide bounds, so a class that has
/// exhausted its share is declined while the pool still has room for other classes (gate G1).
pub fn admit_candidate(
    store: &mut impl QuarantineStore,
    bounds: &QuarantineBounds,
    record: QuarantineRecord,
) -> Result<(), QuarantineAdmitError> {
    if let QuarantineReason::MissingParent { missing_parents } = &record.reason {
        if missing_parents.len() > bounds.max_parent_depth {
            return Err(QuarantineAdmitError::ParentDepthExceeded {
                declared: missing_parents.len(),
                limit: bounds.max_parent_depth,
            });
        }
    }
    let class = record.reason.class();
    let budget = bounds.per_reason.for_class(class);
    let class_current = store.class_len(class);
    if class_current >= budget.max_entries {
        return Err(QuarantineAdmitError::ReasonEntryCapacityExceeded {
            class,
            limit: budget.max_entries,
            current: class_current,
        });
    }
    let class_projected = store.class_total_bytes(class) + record.candidate_bytes.len();
    if class_projected > budget.max_total_bytes {
        return Err(QuarantineAdmitError::ReasonByteBudgetExceeded {
            class,
            limit: budget.max_total_bytes,
            projected: class_projected,
        });
    }
    if store.len() >= bounds.max_entries {
        return Err(QuarantineAdmitError::EntryCapacityExceeded {
            limit: bounds.max_entries,
            current: store.len(),
        });
    }
    let projected = store.total_bytes() + record.candidate_bytes.len();
    if projected > bounds.max_total_bytes {
        return Err(QuarantineAdmitError::ByteBudgetExceeded {
            limit: bounds.max_total_bytes,
            projected,
        });
    }
    store.insert(record);
    Ok(())
}

/// Evicts records older than `QuarantineBounds::max_age_ms`, then brings each reason class
/// within its own budget by evicting only records of that class, then evicts oldest-first
/// pool-wide. Only removes local quarantine copies; never touches admitted history, which this
/// module has no access to.
///
/// The per-class pass is what makes eviction reason-aware (gate G1): a flood of one class sheds
/// its own records, so it cannot displace another class's backlog even when its records are the
/// newest in the pool.
pub fn evict_expired_or_over_capacity(
    store: &mut impl QuarantineStore,
    bounds: &QuarantineBounds,
    now: Hlc,
) -> Vec<Hash32> {
    let mut evicted = Vec::new();

    let expired: Vec<Hash32> = store
        .entries()
        .into_iter()
        .filter(|record| now.wall_ms.saturating_sub(record.first_seen.wall_ms) > bounds.max_age_ms)
        .map(|record| record.claimed_op_id)
        .collect();
    for claimed_op_id in expired {
        if store.remove(&claimed_op_id).is_some() {
            evicted.push(claimed_op_id);
        }
    }

    for class in QuarantineReasonClass::ALL {
        let budget = bounds.per_reason.for_class(class);
        while store.class_len(class) > budget.max_entries
            || store.class_total_bytes(class) > budget.max_total_bytes
        {
            match oldest_claimed_op_id(store, Some(class)) {
                Some(claimed_op_id) => {
                    store.remove(&claimed_op_id);
                    evicted.push(claimed_op_id);
                }
                None => break,
            }
        }
    }

    while store.len() > bounds.max_entries || store.total_bytes() > bounds.max_total_bytes {
        match oldest_claimed_op_id(store, None) {
            Some(claimed_op_id) => {
                store.remove(&claimed_op_id);
                evicted.push(claimed_op_id);
            }
            None => break,
        }
    }

    evicted
}

/// The oldest held record by `(wall_ms, counter)`, optionally restricted to one reason class.
fn oldest_claimed_op_id<S: QuarantineStore + ?Sized>(
    store: &S,
    class: Option<QuarantineReasonClass>,
) -> Option<Hash32> {
    store
        .entries()
        .into_iter()
        .filter(|record| class.is_none_or(|class| record.reason.class() == class))
        .min_by_key(|record| (record.first_seen.wall_ms, record.first_seen.counter))
        .map(|record| record.claimed_op_id)
}

/// Reports whether `retry_cadence_ms` has elapsed since `record`'s last re-evaluation.
pub fn ready_for_reevaluation(
    record: &QuarantineRecord,
    bounds: &QuarantineBounds,
    now: Hlc,
) -> bool {
    now.wall_ms.saturating_sub(record.last_validation.wall_ms) >= bounds.retry_cadence_ms
}

/// Reports whether every locally admitted operation is available to satisfy `MissingParent`.
pub trait AdmittedOperationView {
    /// Reports whether `op_id` is locally admitted (and therefore its own parents, by
    /// construction of this process's admission order, are already satisfied).
    fn is_admitted(&self, op_id: &Hash32) -> bool;
}

/// Reports whether a `MissingParent` record's parent closure is now locally admitted. Returns
/// `false` for every other reason, including `AuthorEquivocation` (see that variant's doc).
pub fn missing_parent_promotion_ready(
    reason: &QuarantineReason,
    admitted: &impl AdmittedOperationView,
) -> bool {
    match reason {
        QuarantineReason::MissingParent { missing_parents } => {
            !missing_parents.is_empty()
                && missing_parents
                    .iter()
                    .all(|parent| admitted.is_admitted(parent))
        }
        _ => false,
    }
}

/// Reports whether a schema hash is now known to this process.
pub trait SchemaAvailability {
    /// Reports whether `schema_hash` is a schema this process can validate against.
    fn is_known(&self, schema_hash: &Hash32) -> bool;
}

/// Reports whether an `UnknownSchema` record's schema is now locally known. Returns `false` for
/// every other reason, including `AuthorEquivocation` (see that variant's doc).
pub fn unknown_schema_promotion_ready(
    reason: &QuarantineReason,
    schemas: &impl SchemaAvailability,
) -> bool {
    match reason {
        QuarantineReason::UnknownSchema { schema_hash } => schemas.is_known(schema_hash),
        _ => false,
    }
}

/// Reports whether an operation kind is one this build can interpret.
pub trait OperationKindAvailability {
    /// Reports whether `operation_kind` has a registered structural rule, schema, and
    /// deterministic reduction in THIS build. An arrival-order guess or a similarly numbered
    /// kind is insufficient, exactly as for [`SchemaAvailability`].
    fn is_known(&self, operation_kind: u16) -> bool;
}

/// Reports whether an `UnknownKind` record's kind is now interpretable here. Returns `false` for
/// every other reason, including `AuthorEquivocation` (see that variant's doc).
pub fn unknown_kind_promotion_ready(
    reason: &QuarantineReason,
    kinds: &impl OperationKindAvailability,
) -> bool {
    match reason {
        QuarantineReason::UnknownKind { operation_kind } => kinds.is_known(*operation_kind),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::envelope::EquivocationKey;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn identifier(filler: u8) -> crate::envelope::Identifier32 {
        crate::envelope::Identifier32([filler; 32])
    }

    fn scope() -> Scope {
        Scope::verse_wide(identifier(0x11))
    }

    fn missing_parent_record(claimed: u8, missing: Vec<Hash32>, bytes: usize) -> QuarantineRecord {
        QuarantineRecord {
            claimed_op_id: hash(claimed),
            derived_op_id: hash(claimed),
            candidate_bytes: vec![0u8; bytes],
            scope: scope(),
            reason: QuarantineReason::MissingParent {
                missing_parents: missing,
            },
            first_seen: Hlc::new(1_000, 0),
            last_validation: Hlc::new(1_000, 0),
            provenance: "test".to_owned(),
        }
    }

    /// Per-class budgets wide enough that a test exercises only the pool-wide bound it names.
    fn generous_per_reason() -> PerReasonBudgets {
        let wide = ReasonBudget {
            max_entries: usize::MAX,
            max_total_bytes: usize::MAX,
        };
        PerReasonBudgets {
            missing_parent: wide,
            unknown_schema: wide,
            unknown_kind: wide,
            other: wide,
        }
    }

    fn unknown_kind_record(claimed: u8, operation_kind: u16, bytes: usize) -> QuarantineRecord {
        QuarantineRecord {
            claimed_op_id: hash(claimed),
            derived_op_id: hash(claimed),
            candidate_bytes: vec![0u8; bytes],
            scope: scope(),
            reason: QuarantineReason::UnknownKind { operation_kind },
            first_seen: Hlc::new(1_000, 0),
            last_validation: Hlc::new(1_000, 0),
            provenance: "test".to_owned(),
        }
    }

    fn generous_bounds() -> QuarantineBounds {
        QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1_000_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 100,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        }
    }

    struct AlwaysAdmitted;
    impl AdmittedOperationView for AlwaysAdmitted {
        fn is_admitted(&self, _op_id: &Hash32) -> bool {
            true
        }
    }

    struct NeverAdmitted;
    impl AdmittedOperationView for NeverAdmitted {
        fn is_admitted(&self, _op_id: &Hash32) -> bool {
            false
        }
    }

    struct AlwaysKnown;
    impl SchemaAvailability for AlwaysKnown {
        fn is_known(&self, _schema_hash: &Hash32) -> bool {
            true
        }
    }

    struct NeverKnown;
    impl SchemaAvailability for NeverKnown {
        fn is_known(&self, _schema_hash: &Hash32) -> bool {
            false
        }
    }

    #[test]
    fn missing_parent_and_unknown_schema_stay_pre_admission() {
        let mut store = InMemoryQuarantineStore::default();
        let bounds = generous_bounds();

        let missing = missing_parent_record(1, vec![hash(9)], 4);
        admit_candidate(&mut store, &bounds, missing.clone()).expect("admit missing-parent");

        let unknown = QuarantineRecord {
            claimed_op_id: hash(2),
            derived_op_id: hash(2),
            candidate_bytes: vec![0u8; 4],
            scope: scope(),
            reason: QuarantineReason::UnknownSchema {
                schema_hash: hash(0xaa),
            },
            first_seen: Hlc::new(1_000, 0),
            last_validation: Hlc::new(1_000, 0),
            provenance: "test".to_owned(),
        };
        admit_candidate(&mut store, &bounds, unknown.clone()).expect("admit unknown-schema");

        // Both candidates stay retrievable from the QUARANTINE store only: this module exposes
        // no "admitted" accessor for either, so nothing here can be mistaken for materialized.
        assert_eq!(store.get(&hash(1)), Some(&missing));
        assert_eq!(store.get(&hash(2)), Some(&unknown));

        // Neither promotes until its specific condition is satisfied.
        assert!(!missing_parent_promotion_ready(
            &missing.reason,
            &NeverAdmitted
        ));
        assert!(!unknown_schema_promotion_ready(
            &unknown.reason,
            &NeverKnown
        ));

        // Each promotes only through its own re-evaluation helper, never the other's.
        assert!(missing_parent_promotion_ready(
            &missing.reason,
            &AlwaysAdmitted
        ));
        assert!(!unknown_schema_promotion_ready(
            &missing.reason,
            &AlwaysKnown
        ));
        assert!(unknown_schema_promotion_ready(
            &unknown.reason,
            &AlwaysKnown
        ));
        assert!(!missing_parent_promotion_ready(
            &unknown.reason,
            &AlwaysAdmitted
        ));
    }

    #[test]
    fn author_equivocation_has_no_promotion_path_in_this_module() {
        let reason = QuarantineReason::AuthorEquivocation {
            equivocation_key: EquivocationKey {
                author_public_key: [0x22; 32],
                wall_ms: 1_000,
                counter: 0,
            },
            first_op_id: hash(0xed),
            second_op_id: hash(0xee),
        };
        assert!(!missing_parent_promotion_ready(&reason, &AlwaysAdmitted));
        assert!(!unknown_schema_promotion_ready(&reason, &AlwaysKnown));
    }

    #[test]
    fn quarantine_bounds_fail_closed_without_history_mutation() {
        let mut store = InMemoryQuarantineStore::default();
        let bounds = QuarantineBounds {
            max_entries: 1,
            max_total_bytes: 1_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        };

        let first = missing_parent_record(1, vec![hash(9)], 4);
        admit_candidate(&mut store, &bounds, first.clone()).expect("first admits");

        let second = missing_parent_record(2, vec![hash(9)], 4);
        assert_eq!(
            admit_candidate(&mut store, &bounds, second),
            Err(QuarantineAdmitError::EntryCapacityExceeded {
                limit: 1,
                current: 1,
            })
        );
        // The failed admission neither evicted nor mutated the existing record.
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(&hash(1)), Some(&first));
        assert_eq!(store.get(&hash(2)), None);

        let byte_bounds = QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1,
            max_age_ms: 1_000_000,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        };
        let mut byte_store = InMemoryQuarantineStore::default();
        let oversized = missing_parent_record(3, vec![hash(9)], 2);
        assert_eq!(
            admit_candidate(&mut byte_store, &byte_bounds, oversized),
            Err(QuarantineAdmitError::ByteBudgetExceeded {
                limit: 1,
                projected: 2,
            })
        );
        assert_eq!(byte_store.len(), 0);

        let too_deep = missing_parent_record(4, vec![hash(1), hash(2), hash(3)], 1);
        let shallow_bounds = QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 2,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        };
        let mut depth_store = InMemoryQuarantineStore::default();
        assert_eq!(
            admit_candidate(&mut depth_store, &shallow_bounds, too_deep),
            Err(QuarantineAdmitError::ParentDepthExceeded {
                declared: 3,
                limit: 2,
            })
        );
        assert_eq!(depth_store.len(), 0);

        // Age exhaustion is the fourth named bound (§4 rule 3). Unlike count/bytes/depth it is
        // a property of elapsed time rather than of the record being admitted, so it surfaces
        // through `evict_expired_or_over_capacity` rather than `admit_candidate`: §4 rule 5
        // explicitly permits removing only the local quarantine copy on expiry. It still never
        // touches any record that has not itself aged out.
        let mut age_store = InMemoryQuarantineStore::default();
        let age_bounds = QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1_000,
            max_age_ms: 500,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        };
        let mut aged_out = missing_parent_record(5, vec![hash(9)], 4);
        aged_out.first_seen = Hlc::new(0, 0);
        age_store.insert(aged_out);
        let mut still_fresh = missing_parent_record(6, vec![hash(9)], 4);
        still_fresh.first_seen = Hlc::new(900, 0);
        age_store.insert(still_fresh.clone());

        let evicted =
            evict_expired_or_over_capacity(&mut age_store, &age_bounds, Hlc::new(1_000, 0));
        assert_eq!(evicted, vec![hash(5)]);
        assert_eq!(age_store.len(), 1);
        assert_eq!(age_store.get(&hash(6)), Some(&still_fresh));
    }

    #[test]
    fn eviction_removes_expired_then_oldest_over_capacity_and_nothing_else() {
        let mut store = InMemoryQuarantineStore::default();
        let bounds = QuarantineBounds {
            max_entries: 2,
            max_total_bytes: 1_000,
            max_age_ms: 500,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: generous_per_reason(),
        };

        let mut ancient = missing_parent_record(1, vec![hash(9)], 4);
        ancient.first_seen = Hlc::new(0, 0);
        store.insert(ancient);

        let mut middle = missing_parent_record(2, vec![hash(9)], 4);
        middle.first_seen = Hlc::new(600, 0);
        store.insert(middle);

        let mut recent = missing_parent_record(3, vec![hash(9)], 4);
        recent.first_seen = Hlc::new(650, 0);
        store.insert(recent);

        let evicted = evict_expired_or_over_capacity(&mut store, &bounds, Hlc::new(700, 0));
        // op 1 is 700ms old (> 500ms max_age) and is evicted as expired.
        assert!(evicted.contains(&hash(1)));
        assert_eq!(store.len(), 2);
        assert!(store.get(&hash(2)).is_some());
        assert!(store.get(&hash(3)).is_some());
    }

    struct KnownKinds(&'static [u16]);
    impl OperationKindAvailability for KnownKinds {
        fn is_known(&self, operation_kind: u16) -> bool {
            self.0.contains(&operation_kind)
        }
    }

    /// Gate G1: the availability property the whole partition exists for. An un-upgraded peer
    /// flooded with a future operation kind must still be able to quarantine its own legitimate
    /// missing-parent backlog.
    #[test]
    fn an_unknown_kind_flood_cannot_starve_the_missing_parent_backlog() {
        let bounds = QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1_000_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: PerReasonBudgets {
                missing_parent: ReasonBudget {
                    max_entries: 4,
                    max_total_bytes: 4_096,
                },
                unknown_schema: ReasonBudget {
                    max_entries: 4,
                    max_total_bytes: 4_096,
                },
                unknown_kind: ReasonBudget {
                    max_entries: 2,
                    max_total_bytes: 4_096,
                },
                other: ReasonBudget {
                    max_entries: 4,
                    max_total_bytes: 4_096,
                },
            },
        };
        let mut store = InMemoryQuarantineStore::default();

        // Fill the unknown-kind class to its own budget.
        admit_candidate(&mut store, &bounds, unknown_kind_record(0x81, 4_242, 8))
            .expect("first unknown kind");
        admit_candidate(&mut store, &bounds, unknown_kind_record(0x82, 4_242, 8))
            .expect("second unknown kind");

        // Further unknown-kind traffic is declined against its OWN budget, naming its own class.
        assert_eq!(
            admit_candidate(&mut store, &bounds, unknown_kind_record(0x83, 4_242, 8)),
            Err(QuarantineAdmitError::ReasonEntryCapacityExceeded {
                class: QuarantineReasonClass::UnknownKind,
                limit: 2,
                current: 2,
            })
        );

        // The pool is nowhere near its own limit, and missing-parent work still admits.
        assert!(store.len() < bounds.max_entries);
        admit_candidate(
            &mut store,
            &bounds,
            missing_parent_record(1, vec![hash(9)], 8),
        )
        .expect("missing-parent backlog is unaffected by the unknown-kind flood");
        assert!(store.get(&hash(1)).is_some());
    }

    /// The byte budget partitions the same way, and eviction sheds only the offending class even
    /// when that class holds the NEWEST records in the pool.
    #[test]
    fn per_class_eviction_sheds_only_the_over_budget_class() {
        let bounds = QuarantineBounds {
            max_entries: 100,
            max_total_bytes: 1_000_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 8,
            retry_cadence_ms: 0,
            per_reason: PerReasonBudgets {
                missing_parent: ReasonBudget {
                    max_entries: 10,
                    max_total_bytes: 1_000,
                },
                unknown_schema: ReasonBudget {
                    max_entries: 10,
                    max_total_bytes: 1_000,
                },
                unknown_kind: ReasonBudget {
                    max_entries: 1,
                    max_total_bytes: 1_000,
                },
                other: ReasonBudget {
                    max_entries: 10,
                    max_total_bytes: 1_000,
                },
            },
        };
        let mut store = InMemoryQuarantineStore::default();

        // The missing-parent record is the OLDEST in the pool: a reason-blind oldest-first
        // eviction would take it first, which is exactly the starvation G1 forecloses.
        let mut backlog = missing_parent_record(1, vec![hash(9)], 8);
        backlog.first_seen = Hlc::new(10, 0);
        store.insert(backlog);

        let mut older_flood = unknown_kind_record(0x81, 4_242, 8);
        older_flood.first_seen = Hlc::new(500, 0);
        store.insert(older_flood);
        let mut newer_flood = unknown_kind_record(0x82, 4_242, 8);
        newer_flood.first_seen = Hlc::new(600, 0);
        store.insert(newer_flood);

        let evicted = evict_expired_or_over_capacity(&mut store, &bounds, Hlc::new(700, 0));
        assert_eq!(evicted, vec![hash(0x81)]);
        assert!(store.get(&hash(1)).is_some(), "backlog must survive");
        assert!(store.get(&hash(0x82)).is_some());

        // Byte pressure partitions identically.
        let byte_bounds = QuarantineBounds {
            per_reason: PerReasonBudgets {
                unknown_kind: ReasonBudget {
                    max_entries: 10,
                    max_total_bytes: 1,
                },
                ..bounds.per_reason
            },
            ..bounds
        };
        let mut byte_store = InMemoryQuarantineStore::default();
        assert_eq!(
            admit_candidate(
                &mut byte_store,
                &byte_bounds,
                unknown_kind_record(0x84, 4_242, 2)
            ),
            Err(QuarantineAdmitError::ReasonByteBudgetExceeded {
                class: QuarantineReasonClass::UnknownKind,
                limit: 1,
                projected: 2,
            })
        );
        assert_eq!(byte_store.len(), 0);
    }

    #[test]
    fn unknown_kind_promotes_only_when_this_build_learns_the_kind() {
        let reason = QuarantineReason::UnknownKind {
            operation_kind: 4_242,
        };
        assert!(!unknown_kind_promotion_ready(&reason, &KnownKinds(&[1, 2])));
        assert!(unknown_kind_promotion_ready(
            &reason,
            &KnownKinds(&[1, 2, 4_242])
        ));

        // Never promotes through another reason's helper, and no other reason promotes through
        // this one -- the same isolation the other two retry classes have.
        assert!(!missing_parent_promotion_ready(&reason, &AlwaysAdmitted));
        assert!(!unknown_schema_promotion_ready(&reason, &AlwaysKnown));
        assert!(!unknown_kind_promotion_ready(
            &QuarantineReason::UnknownSchema {
                schema_hash: hash(0xaa)
            },
            &KnownKinds(&[1, 2, 4_242])
        ));
        assert!(!unknown_kind_promotion_ready(
            &QuarantineReason::AuthorEquivocation {
                equivocation_key: EquivocationKey {
                    author_public_key: [0x22; 32],
                    wall_ms: 1_000,
                    counter: 0,
                },
                first_op_id: hash(0xed),
                second_op_id: hash(0xee),
            },
            &KnownKinds(&[1, 2, 4_242])
        ));
    }

    /// Every reason maps to exactly one budget, and the three independently driven retry classes
    /// are the ones that are separated.
    #[test]
    fn every_reason_class_maps_to_its_own_budget() {
        let budgets = PerReasonBudgets {
            missing_parent: ReasonBudget {
                max_entries: 1,
                max_total_bytes: 10,
            },
            unknown_schema: ReasonBudget {
                max_entries: 2,
                max_total_bytes: 20,
            },
            unknown_kind: ReasonBudget {
                max_entries: 3,
                max_total_bytes: 30,
            },
            other: ReasonBudget {
                max_entries: 4,
                max_total_bytes: 40,
            },
        };
        for class in QuarantineReasonClass::ALL {
            assert!(budgets.for_class(class).max_entries > 0);
        }
        assert_eq!(
            QuarantineReason::MissingParent {
                missing_parents: vec![hash(9)]
            }
            .class(),
            QuarantineReasonClass::MissingParent
        );
        assert_eq!(
            QuarantineReason::UnknownSchema {
                schema_hash: hash(0xaa)
            }
            .class(),
            QuarantineReasonClass::UnknownSchema
        );
        assert_eq!(
            QuarantineReason::UnknownKind {
                operation_kind: 4_242
            }
            .class(),
            QuarantineReasonClass::UnknownKind
        );
        assert_eq!(
            QuarantineReason::PayloadKeyUnavailable.class(),
            QuarantineReasonClass::Other
        );
        assert_eq!(
            QuarantineReason::Unauthorized.class(),
            QuarantineReasonClass::Other
        );
    }

    #[test]
    fn ready_for_reevaluation_honors_the_retry_cadence() {
        let record = missing_parent_record(1, vec![hash(9)], 4);
        let bounds = QuarantineBounds {
            max_entries: 10,
            max_total_bytes: 1_000,
            max_age_ms: 1_000_000,
            max_parent_depth: 8,
            retry_cadence_ms: 100,
            per_reason: generous_per_reason(),
        };
        assert!(!ready_for_reevaluation(
            &record,
            &bounds,
            Hlc::new(1_050, 0)
        ));
        assert!(ready_for_reevaluation(&record, &bounds, Hlc::new(1_100, 0)));
    }
}
