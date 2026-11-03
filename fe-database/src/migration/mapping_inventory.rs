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
    "CommitCheckedEditorBatch",
    "ComparePetalVisibilityGroups",
    "Seed",
    "CreateNode",
    "GenerateVerseInvite",
    "JoinVerseByInvite",
    "IssueJoinInvite",
    "AdmitJoinInvite",
    "FinalizeJoinProof",
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
    "SetNodeView",
    "SetPetalVisibilityGroups",
    "SetScopeHidden",
    "CreateFieldDef",
    "UpdateFieldDef",
    "DeleteFieldDef",
    "InstallCrate",
    "InstallCrateEntry",
    "UninstallCrate",
    "SetPetalTerrain",
    "SetPetalRequirements",
    "SetPetalRenderDistance",
    "SetVerseTimeseriesSettings",
    "CreateAsset",
    "GcUnreferencedAssets",
    "CreateNodeWithAsset",
    "ElevatedQuery",
    "EnrolLocalDevice",
    "RevokeLocalDevice",
    // CDL-SPEC10 A3 (§2.1 pre-existing-drift fix): `InsertIotReadings` was a
    // dispatched mutation before this slice (`lib.rs` dispatch arm,
    // `fe-runtime/src/messages.rs`) but missing from this inventory —
    // `migration/AGENTS.md` §inventory-scope requires new mutating variants
    // "in the same change", so this fixes the drift this slice found rather
    // than widening it.
    "InsertIotReadings",
    "RegisterDeviceFormat",
    "InsertDevicePayloads",
];

/// The SPEC-10 observation family: the two ingest-door mutation kinds whose accepted rows the
/// shadow sealer commits as kind-5 `observation_batch` operations (CDL-SPEC10 B3). Neither is a
/// D-CL20 kind; `RegisterDeviceFormat` is not in the family (SPEC-10 §8 rule 7: `device_format`
/// is not graduated).
pub const OBSERVATION_FAMILY_MUTATION_KINDS: [&str; 2] =
    ["InsertIotReadings", "InsertDevicePayloads"];

/// The materializer the observation family maps onto, as `{id}@{version}`.
pub fn observation_family_materializer_version() -> String {
    let version = fe_canonical_log::observation::ObservationBatchMaterializer::version();
    format!("{}@{}", version.materializer_id, version.version)
}

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

    /// Registers the observation family as `Mapped` under the approved SPEC-10 schema (kind-5
    /// `observation_batch` schema hash, the reference materializer). `seeded()` stays all
    /// `UnmappedDeferred`; a shadow-mode run charter calls this on its own inventory, exactly as
    /// the equivalence harness does for `UpdateNodeTransform`. D-CL20 kinds stay refused.
    pub fn register_observation_family(&mut self) -> Result<(), MappingRegistrationError> {
        let schema_hash_hex =
            hex::encode(fe_canonical_log::observation::observation_batch_schema_hash().as_bytes());
        for kind in OBSERVATION_FAMILY_MUTATION_KINDS {
            self.register_mapped(
                kind,
                schema_hash_hex.clone(),
                observation_family_materializer_version(),
            )?;
        }
        Ok(())
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

// ---------------------------------------------------------------------------
// D-CL20 per-kind bypass counters (CDL-W4 slice W4-1, AGENTS.md §bypass-counter).
// One fixed slot per `D_CL20_MUTATION_KINDS` entry, index-matched to that array,
// plus the existing global `UNMAPPED_BYPASS_COUNT` above, which keeps recording
// every bypass regardless of kind so `bypass_count()`/`MeasurementReport` keep
// their existing, broader meaning unchanged.
// ---------------------------------------------------------------------------

/// Per-kind counterpart to [`UNMAPPED_BYPASS_COUNT`], index-aligned with
/// [`D_CL20_MUTATION_KINDS`]. Six fixed slots: this inventory never grows the
/// D-CL20 set at runtime (§4.2.5 — the six are closed by spec, not configuration).
static D_CL20_BYPASS_COUNTS: [AtomicU64; 6] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Record a legacy-path bypass for one named D-CL20 mutation kind: increments
/// that kind's own slot in [`D_CL20_BYPASS_COUNTS`] *and* the existing global
/// [`UNMAPPED_BYPASS_COUNT`] (so every existing global-counter consumer keeps
/// seeing every bypass). `kind` must be one of [`D_CL20_MUTATION_KINDS`]; any
/// other value only increments the global counter (`debug_assert!` catches a
/// caller typo in debug builds without changing release behavior).
pub fn record_bypass_for(kind: &'static str) {
    debug_assert!(
        D_CL20_MUTATION_KINDS.contains(&kind),
        "record_bypass_for called with a kind outside D_CL20_MUTATION_KINDS: {kind}"
    );
    if let Some(index) = D_CL20_MUTATION_KINDS.iter().position(|k| *k == kind) {
        D_CL20_BYPASS_COUNTS[index].fetch_add(1, Ordering::Relaxed);
    }
    record_bypass();
}

/// Total bypasses recorded for one named D-CL20 mutation kind since process
/// startup. Returns 0 for a `kind` outside [`D_CL20_MUTATION_KINDS`] (never
/// recorded, so never incremented).
pub fn bypass_count_for(kind: &str) -> u64 {
    match D_CL20_MUTATION_KINDS.iter().position(|k| *k == kind) {
        Some(index) => D_CL20_BYPASS_COUNTS[index].load(Ordering::Relaxed),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CDL-SPEC10 B3: the observation family maps onto the SPEC-10 schema; nothing else moves,
    /// and the six D-CL20 kinds stay refused (SPEC-10 §8 rule 8).
    #[test]
    fn observation_family_maps_under_the_spec10_schema_and_d_cl20_stays_refused() {
        let mut inventory = MappingInventory::seeded();
        for kind in ["IssueJoinInvite", "AdmitJoinInvite", "FinalizeJoinProof"] {
            assert!(matches!(inventory.status(kind), Some(MappingStatus::UnmappedDeferred { .. })));
        }
        for kind in ["PrepareJoinInvite", "CheckJoinReadiness"] {
            assert!(inventory.status(kind).is_none());
        }
        let unmapped_before = inventory.unmapped_count();
        inventory
            .register_observation_family()
            .expect("the observation family is registrable");

        let schema_hash_hex =
            hex::encode(fe_canonical_log::observation::observation_batch_schema_hash().as_bytes());
        for kind in OBSERVATION_FAMILY_MUTATION_KINDS {
            assert_eq!(
                inventory.status(kind),
                Some(&MappingStatus::Mapped {
                    schema_hash_hex: schema_hash_hex.clone(),
                    materializer_version: "observation-batch-reference@1".to_string(),
                }),
                "{kind}"
            );
        }
        assert_eq!(inventory.unmapped_count(), unmapped_before - 2);
        assert!(!inventory.is_mapped("RegisterDeviceFormat"));
        for kind in D_CL20_MUTATION_KINDS {
            assert!(matches!(
                inventory.register_mapped(kind, String::new(), String::new()),
                Err(MappingRegistrationError::DCl20Blocked { .. })
            ));
        }
        assert!(!MappingInventory::seeded().is_mapped("InsertIotReadings"));
    }
}
