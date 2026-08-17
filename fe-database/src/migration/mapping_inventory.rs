//! SPEC-8 §4.2 mutation-mapping inventory: every currently-dispatched
//! `DbCommand` mutation kind starts `UnmappedDeferred`; nothing may become
//! `Mapped` until a reviewed, per-kind canonical schema exists. See
//! `fe-database/src/migration/AGENTS.md` §mapping-inventory and §inventory-scope.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Registered status of one mutation kind's canonical mapping (§4.2.1-§4.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingStatus {
    /// A reviewed, complete mapping exists: approved schema hash + materializer
    /// version. Nothing in this codebase is Mapped yet (§4.2.5).
    Mapped {
        schema_hash_hex: String,
        materializer_version: String,
    },
    /// No complete mapping exists yet; the kind stays `legacy_only` for now
    /// (§4.2.4). `reason` is a human-readable citation, not machine-parsed.
    UnmappedDeferred { reason: String },
}

/// The permanent D-CL20 deferral reason cited by every D-CL20 mutation kind
/// (§4.2.5, §7 gate 3).
pub const D_CL20_DEFERRAL_REASON: &str = "D-CL20: no approved atomic intent schema, \
    candidate-set, materializer, or replay/failure contract yet (SPEC-4/SPEC-8)";

/// The six mutation kinds gated by D-CL20 (§7 gate 3). [`MappingInventory::register_mapped`]
/// refuses all of them unconditionally — inventing a per-kind schema for one of them
/// before its approved contract exists is forbidden, not merely undesirable (§4.2.5).
pub const D_CL20_MUTATION_KINDS: [&str; 6] = [
    "CreateVerse",
    "CreateFractal",
    "CreatePetal",
    "ImportGltf",
    "RenameNode",
    "DuplicateNode",
];

const UNMAPPED_NO_SCHEMA_REASON: &str =
    "no reviewed canonical intent schema/materializer mapping exists yet (SPEC-8 §4.2.1)";

/// Every other currently-dispatched `DbCommand` mutation kind
/// (`fe_runtime::messages::DbCommand`), enumerated by hand because no per-kind
/// canonical schema exists yet to derive this list from automatically. Read-only
/// or query variants (`Ping`, `Shutdown`, `LoadHierarchy`, every `Resolve*`,
/// `List*`, `Get*`, `RawQuery`, `CountNodeDescendants`) are intentionally excluded
/// — see AGENTS.md §inventory-scope for the exact inclusion rule.
const OTHER_DISPATCHED_MUTATION_KINDS: &[&str] = &[
    "Seed",
    "CreateNode",
    "GenerateVerseInvite",
    "JoinVerseByInvite",
    "ResetDatabase",
    "UpdateNodeTransform",
    "UpdateNodeUrl",
    "RenameEntity",
    "SetVerseDefaultAccess",
    "UpdateFractalDescription",
    "DeleteEntity",
    "AssignRole",
    "RevokeRole",
    "GenerateScopedInvite",
    "MintApiToken",
    "RevokeApiToken",
    "SetNodeProperty",
    "DeleteNodeProperty",
    "DeleteNode",
    "TombstoneNode",
    "CascadeTombstoneNode",
    "PromoteInstance",
    "CreateFieldDef",
    "UpdateFieldDef",
    "DeleteFieldDef",
    "InstallCrate",
    "InstallCrateEntry",
    "UninstallCrate",
    "SetPetalTerrain",
];

/// Registry of every currently-dispatched mutation kind's canonical-mapping
/// status (§4.2).
#[derive(Debug, Clone)]
pub struct MappingInventory {
    statuses: BTreeMap<String, MappingStatus>,
}

impl Default for MappingInventory {
    fn default() -> Self {
        Self::seeded()
    }
}

impl MappingInventory {
    /// Seed one entry per currently-dispatched `DbCommand` mutation kind, every
    /// one `UnmappedDeferred` (§4.2.4-§4.2.5). Registering `Mapped` requires an
    /// explicit, later call to [`Self::register_mapped`], which still refuses the
    /// six D-CL20 kinds unconditionally.
    pub fn seeded() -> Self {
        let mut statuses = BTreeMap::new();
        for kind in D_CL20_MUTATION_KINDS {
            statuses.insert(
                kind.to_string(),
                MappingStatus::UnmappedDeferred {
                    reason: D_CL20_DEFERRAL_REASON.to_string(),
                },
            );
        }
        for kind in OTHER_DISPATCHED_MUTATION_KINDS {
            statuses.insert(
                (*kind).to_string(),
                MappingStatus::UnmappedDeferred {
                    reason: UNMAPPED_NO_SCHEMA_REASON.to_string(),
                },
            );
        }
        Self { statuses }
    }

    /// The mapping status of `mutation_kind`, or `None` if it isn't a known
    /// dispatched mutation kind at all.
    pub fn status(&self, mutation_kind: &str) -> Option<&MappingStatus> {
        self.statuses.get(mutation_kind)
    }

    pub fn is_mapped(&self, mutation_kind: &str) -> bool {
        matches!(
            self.status(mutation_kind),
            Some(MappingStatus::Mapped { .. })
        )
    }

    /// Number of currently registered kinds with no complete mapping — feeds the
    /// §6.1 "Ingress coverage" measurement.
    pub fn unmapped_count(&self) -> usize {
        self.statuses
            .values()
            .filter(|status| matches!(status, MappingStatus::UnmappedDeferred { .. }))
            .count()
    }

    /// Register a reviewed, complete mapping for `mutation_kind`. REFUSES every
    /// D-CL20 kind unconditionally (§4.2.5) and refuses any kind this inventory
    /// did not seed (§4.2.1: mappings only exist for currently-dispatched kinds).
    pub fn register_mapped(
        &mut self,
        mutation_kind: &str,
        schema_hash_hex: String,
        materializer_version: String,
    ) -> Result<(), MappingRegistrationError> {
        if D_CL20_MUTATION_KINDS.contains(&mutation_kind) {
            return Err(MappingRegistrationError::DCl20Blocked {
                mutation_kind: mutation_kind.to_string(),
            });
        }
        if !self.statuses.contains_key(mutation_kind) {
            return Err(MappingRegistrationError::UnknownMutationKind {
                mutation_kind: mutation_kind.to_string(),
            });
        }
        self.statuses.insert(
            mutation_kind.to_string(),
            MappingStatus::Mapped {
                schema_hash_hex,
                materializer_version,
            },
        );
        Ok(())
    }
}

/// Typed failure from [`MappingInventory::register_mapped`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingRegistrationError {
    #[error(
        "{mutation_kind} is gated by D-CL20 and cannot be registered as Mapped (SPEC-8 §4.2.5)"
    )]
    DCl20Blocked { mutation_kind: String },
    #[error("{mutation_kind} is not a recognized dispatched mutation kind")]
    UnknownMutationKind { mutation_kind: String },
}

// ---------------------------------------------------------------------------
// Process-global unmapped-bypass counter (mirrors fe-database's existing
// AtomicU64 pattern, e.g. `REPLICATION_DROPS` in `lib.rs` — see AGENTS.md
// §bypass-counter).
// ---------------------------------------------------------------------------

/// Operations that reached the legacy path with no candidate coverage, because
/// their mutation kind is not `Mapped` (§4.2.4, §6.1 "Ingress coverage").
static UNMAPPED_BYPASS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Record that an unmapped mutation kind reached the legacy path without a
/// candidate. Feeds §6.1 "Ingress coverage": `unmapped_bypass_count` must be 0
/// for every enabled mutation surface for promotion to proceed.
pub fn record_bypass() {
    UNMAPPED_BYPASS_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Total unmapped-mutation bypasses recorded by this process since startup.
pub fn bypass_count() -> u64 {
    UNMAPPED_BYPASS_COUNT.load(Ordering::Relaxed)
}
