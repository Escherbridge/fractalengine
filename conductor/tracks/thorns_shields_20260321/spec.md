---
type: Track Spec
---

# Track: Thorns and Shields — Security Hardening + Pre-Launch Documents

**Created:** 2026-03-21 (folder reconstructed 2026-07-14 during board-hygiene pass)
**Status:** Pending

## Scope

Security hardening and pre-launch security documents. The track existed only as a
tracks.md entry until 2026-07-14; this folder was reconstructed from the surviving
artifacts. Current state: docs/webview-threat-model.md, docs/security-checklist.md,
docs/unwrap-audit.md (says Status: PENDING and cites an .expect already removed
2026-04-30), scripts/audit.sh, and fuzz/targets/ are all thin Wave-1 scaffolds; the
audit was never run.

## Functional Requirements

- **FR-1**: Run + update the unwrap audit (docs/unwrap-audit.md is stale — it cites an .expect removed 2026-04-30).
  - **Status 2026-10-11: DONE (commit pending).** `scripts/unwrap-census.sh` and its
    `.ps1` twin are committed. They run lib and bin targets only, so tests are excluded.
    docs/unwrap-audit.md is rewritten with 194 sites in 6 categories, the
    intentional-panic register (the SurrealDB-init expect is marked REMOVED in d3c5fab),
    and follow-ups F-1 to F-4. F-1 is the only site reachable from untrusted input
    (fe-plugin wasm `into_memory().unwrap()`). STALE marker removed.
- **FR-2**: Flesh out the webview threat model and security checklist against the current architecture.
- **FR-3**: Make the fuzz targets build and run in CI.
  - **Status 2026-10-11: WIRED, pending first CI run.** fuzz/ is now a standalone
    cargo-fuzz workspace: `[workspace]`, `cargo-fuzz = true`, `publish = false`,
    ed25519-dalek 2.2, and a lock seeded from the root lock. The root `exclude = ["fuzz"]`
    and .gitignore entries are added. The build-artifacts lint job runs
    `cargo +nightly fuzz build`, then 60 s of each of `jwt_parse` and `ed25519_verify`,
    under a 30-minute step timeout. Local verification on Windows was
    `cargo check --manifest-path fuzz/Cargo.toml` (green). "Green on main" needs the
    user-gated push.
