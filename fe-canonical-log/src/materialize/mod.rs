//! Pure SPEC-4 identity, admission error taxonomy, checkpoint binding, and
//! store/verifier/materializer trait contracts. No SurrealDB, no I/O -- the SurrealDB-backed
//! half lives in `fe-database/src/canon_log/` against these traits. See `src/AGENTS.md`
//! §module-ownership and `materialize/AGENTS.md`.

pub mod checkpoint_binding;
pub mod errors;
pub mod identity;
pub mod ordering;
pub mod traits;

pub use checkpoint_binding::{CheckpointBinding, CheckpointBindingMismatch};
pub use errors::{
    AdmissionDisposition, AdmissionOutcome, AppendError, AppendOutcome, AuthorizationEvidenceGap,
};
pub use identity::{ApplyMarkerKey, MaterializerVersion, ProjectionIdentity};
pub use ordering::{deterministic_causal_order, ordering_key, OrderingError, OrderingKey};
pub use traits::{
    admit_candidate, admit_candidate_with_facts, check_author_equivocation,
    validate_same_verse_parents, verified_envelope_meta_from, verify_and_project_envelope,
    verify_candidate, verify_envelope_meta, AdmittedCandidate, CandidateVerifier,
    CausalMaterializer, EquivocationIndex, ParentVerseLookup, PayloadFacts, ProjectionMutation,
    VerifiedCandidate, VerifiedEnvelopeMeta, VerifiedLogStore,
};
