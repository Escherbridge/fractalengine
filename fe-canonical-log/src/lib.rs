//! Canonical Fractal Data Log: deterministic CBOR envelopes, signing, admission, and
//! materialization for Workstream G. Rationale, dependency rules, and the module
//! ownership map live in `src/AGENTS.md`.
//!
//! Dependency rule (`src/AGENTS.md` "Forbidden dependencies"): this crate depends on no other
//! workspace crate except `fe-sdk`, one-directionally. Nothing here may gain an `fe-runtime`,
//! `fe-identity`, `fe-policy`, `fe-database`, or `surrealdb` edge.

pub mod author_key;
pub mod branch;
pub mod capability;
pub mod cbor;
pub mod checkpoint;
pub mod compose;
pub mod crypto;
pub mod did_key;
pub mod envelope;
pub mod frontier;
pub mod kind;
pub mod materialize;
pub mod payload_aad;
pub mod retention;
pub mod segment;
pub mod signing;
pub mod units_codec;
pub mod wire;

/// A curated, flat import surface for the crate's stable, most-reached-for types.
///
/// `lib.rs`'s own `pub mod` list above is the complete, authoritative module tree
/// (`src/AGENTS.md` §module-ownership), and every full module path it names keeps working
/// unchanged -- this module adds nothing new, it only re-exports. See `src/AGENTS.md`
/// §crate-surface.
pub mod prelude {
    pub use crate::compose::{
        ByteStoreSealedArtifactStore, ChainCapabilityVerifier, ComposedCandidateVerifier,
        ContentByteStore, QuarantineReason, SealedProofEnvironment, VerifyingCommitPipeline,
    };
    pub use crate::envelope::{Hash32, Identifier32, Scope};
    pub use crate::materialize::errors::{AdmissionOutcome, AppendError, AppendOutcome};
    pub use crate::materialize::identity::{MaterializerVersion, ProjectionIdentity};
    pub use crate::materialize::traits::{
        admit_candidate, admit_candidate_with_facts, AdmittedCandidate, CandidateVerifier,
        CausalMaterializer, EquivocationIndex, ParentVerseLookup, PayloadFacts, VerifiedCandidate,
        VerifiedEnvelopeMeta, VerifiedLogStore,
    };
    pub use crate::wire::commit::{CanonicalCommitPipeline, PipelineResult};
}
