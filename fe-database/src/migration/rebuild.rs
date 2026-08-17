//! SPEC-8 §5.2 shadow rebuild: replay retained candidate bytes into an empty
//! projection, three times, independently. See
//! `fe-database/src/migration/AGENTS.md` §rebuild and §materializer-purity.

use std::collections::BTreeMap;

use crate::migration::candidate::MigrationCandidateMember;
use crate::migration::comparator::{CanonicalExport, QuantizedValue};

/// Deterministically reduces one candidate member's opaque bytes into a
/// projection effect. May read ONLY `member` — never a database handle or any
/// other ambient state — because that is what makes
/// [`replay_admitted_closure_three_times`]'s determinism guarantee hold by
/// construction, not merely by convention (see AGENTS.md §materializer-purity).
pub trait ShadowMaterializer {
    fn reduce(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<ShadowProjectionEffect, ShadowMaterializationError>;
}

/// One deterministic effect on the shadow projection produced by reducing one
/// member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowProjectionEffect {
    pub field_path: String,
    pub value: QuantizedValue,
}

/// Typed failure from [`ShadowMaterializer::reduce`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShadowMaterializationError {
    #[error("member for scope {scope} could not be reduced: {reason}")]
    Unreducible { scope: String, reason: String },
}

/// Result of replaying one admitted candidate closure three independent times
/// (§5.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayResult {
    pub export: CanonicalExport,
    /// Hex BLAKE3 root over the export, identical across all three replays when
    /// deterministic.
    pub root_hex: String,
}

/// Typed failure from [`replay_admitted_closure_three_times`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    #[error("replay {replay_index} failed to reduce a member: {source}")]
    MaterializationFailed {
        replay_index: usize,
        #[source]
        source: ShadowMaterializationError,
    },
    #[error(
        "the three replays diverged: replay 0 root {root_a}, replay {other_index} root {root_b}"
    )]
    NonDeterministic {
        root_a: String,
        other_index: usize,
        root_b: String,
    },
}

/// Replay the same admitted candidate closure three independent times, each
/// from a freshly initialized empty projection (§5.2.1, §5.2.3). Never reads
/// live SurrealDB state, event ordering, local receipt order, or UI buffers —
/// only `members`, the exact retained candidate bytes (§5.2.2). Fails if any
/// replay cannot reduce every member, or if the three exports/roots are not
/// byte-identical.
pub fn replay_admitted_closure_three_times(
    members: &[MigrationCandidateMember],
    materializer: &dyn ShadowMaterializer,
) -> Result<ReplayResult, ReplayError> {
    let mut results: Vec<(CanonicalExport, String)> = Vec::with_capacity(3);
    for replay_index in 0..3 {
        let export = replay_once(members, materializer).map_err(|source| {
            ReplayError::MaterializationFailed {
                replay_index,
                source,
            }
        })?;
        let root_hex = export.root_hex();
        results.push((export, root_hex));
    }

    let (first_export, first_root) = results[0].clone();
    for (other_index, (_, root)) in results.iter().enumerate().skip(1) {
        if root != &first_root {
            return Err(ReplayError::NonDeterministic {
                root_a: first_root,
                other_index,
                root_b: root.clone(),
            });
        }
    }

    Ok(ReplayResult {
        export: first_export,
        root_hex: first_root,
    })
}

/// One replay pass: a fresh empty projection, folding every member's effect in
/// order (§5.2.1).
fn replay_once(
    members: &[MigrationCandidateMember],
    materializer: &dyn ShadowMaterializer,
) -> Result<CanonicalExport, ShadowMaterializationError> {
    let mut fields: BTreeMap<String, QuantizedValue> = BTreeMap::new();
    for member in members {
        let effect = materializer.reduce(member)?;
        fields.insert(effect.field_path, effect.value);
    }
    Ok(CanonicalExport { fields })
}
