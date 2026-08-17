//! Guard test: every migration mode keeps the iroh-docs engine unavailable.
//!
//! Verifies HARD CONSTRAINT 1 from `migration-plan.md` §6.1 (network prohibition) across
//! every reachable `MigrationMode`, per §6.2.8. See `docs/spec/canonical-log/migration-plan.md`
//! §2.2 for why the mode is a local process env var and never a replicated value.

use fe_database::migration::mode::{MigrationFlags, MigrationMode};
use fe_sync::replicator::IrohDocsEngineHolder;

const MIGRATION_MODE_VAR: &str = "FE_MIGRATION_MODE";

/// Runs `case` with `FE_MIGRATION_MODE` set to `value`, restoring the prior value afterwards.
///
/// Env vars are process-global, so every case in this file runs inside one `#[test]` body and
/// restores through this helper; separate `#[test]` functions would race in Rust's threaded
/// test harness and make the outcome order-dependent.
fn with_migration_mode(value: &str, case: impl FnOnce()) {
    let previous = std::env::var(MIGRATION_MODE_VAR).ok();
    std::env::set_var(MIGRATION_MODE_VAR, value);

    case();

    match previous {
        Some(previous) => std::env::set_var(MIGRATION_MODE_VAR, previous),
        None => std::env::remove_var(MIGRATION_MODE_VAR),
    }
}

/// No reachable migration mode makes the iroh-docs engine available, and the two refused
/// mode strings never parse at all.
#[test]
fn all_migration_modes_keep_iroh_docs_engine_unavailable() {
    // `CanonicalAuthoritativeLocal` is absent deliberately: it is owner-gated and
    // `MigrationFlags::from_env` hard-refuses it, so it is unreachable through this path.
    let reachable_modes = [
        ("legacy_only", MigrationMode::LegacyOnly),
        ("seam_ready", MigrationMode::SeamReady),
        ("dual_emit_shadow", MigrationMode::DualEmitShadow),
        ("shadow_rebuild", MigrationMode::ShadowRebuild),
    ];

    for (mode_string, expected_mode) in reachable_modes {
        with_migration_mode(mode_string, || {
            let flags = MigrationFlags::from_env()
                .unwrap_or_else(|_| panic!("'{mode_string}' must parse as a reachable mode"));

            assert_eq!(
                flags.mode(),
                expected_mode,
                "'{mode_string}' must parse to its matching MigrationMode"
            );
            assert!(
                !IrohDocsEngineHolder::new().is_available(),
                "iroh-docs engine must stay unavailable in mode '{mode_string}'"
            );
        });
    }

    for refused_mode in ["canonical_authoritative_local", "unknown_mode_xyz"] {
        with_migration_mode(refused_mode, || {
            assert!(
                MigrationFlags::from_env().is_err(),
                "from_env must return a typed error for '{refused_mode}', never a silent fallback"
            );
        });
    }
}
