---
type: Implementation Plan
title: "Implementation Plan: OSS Release — phased pre-push checklist"
tags: [oss_release_20260717]
resource: ./spec.md
---

# Implementation Plan: OSS Release Checklist

## Overview

Phases 1–4 are agent-executable now. Phase 5 is the ratification wall — every item is
**BLOCKED-ON-USER** and nothing in Phase 6 may run until Phase 5 is fully resolved.
Ordering follows the audit's blocking analysis: legal scaffolding verification →
truth alignment → community/CI → metadata → ratification → preflight + push.

## Phase 1 — Verify the 2026-07-17 scaffolding (agent)

- [x] 1.1 `LICENSE-APACHE` present at root with correct canonical text
      (D-69 ratified 2026-07-17: Apache-2.0 single; LICENSE-MIT removed).
- [x] 1.2 Root `[workspace.package]` carries `license = "Apache-2.0"`,
      `repository`, `edition`, `rust-version`; all member crates inherit via
      `license.workspace = true` (spec FR-6).
- [ ] 1.3 `deny.toml` allowlist correct (MIT/Apache-2.0/BSD/ISC/MPL-2.0 + BUSL
      exception scoped to `surrealdb*` only); `cargo deny check licenses` wired into
      `build-artifacts.yml` (spec FR-3).
      — AUDIT 2026-10-10: PARTIAL — allowlist + `surrealdb`/`surrealdb-core`-only BUSL exception are correct (`deny.toml:4-32`), but no `cargo deny` step exists in `.github/workflows/build-artifacts.yml` (only the clippy step at :73).
- [x] 1.4 `THIRD-PARTY-LICENSES.md` names SurrealDB BUSL-1.1 + the Additional Use
      Grant terms explicitly; note the regeneration command used.
      — AUDIT 2026-10-10: DONE d5cc361/b555082 — `THIRD-PARTY-LICENSES.md:8-34` (BUSL-1.1, Additional Use Grant DBaaS carve-out, Change Date) + `:36-47` regeneration (`cargo deny check licenses`, `cargo about generate`); surrealdb 3.0.5 still matches `Cargo.lock`.
- [ ] 1.5 Re-grep `"00".repeat(64)` and reconcile against the spec FR-2 register
      (13 sites, all fe-database, as of 2026-07-17).
      — AUDIT 2026-10-10: OPEN, DRIFTED — the re-grep now finds 20 sites, still all fe-database (`handlers/crud.rs` 6, `space_manager.rs` 5, `queries.rs` 3, `handlers/entity_property.rs` 2, `role_manager.rs` 2, `handlers/transform.rs` 1, `session_cache.rs` 1); the FR-2 register needs updating. D-CL24(5) fixes these legacy rows as permanently unsigned.

## Phase 2 — Truth alignment in shipped docs (agent)

- [x] 2.1 README claims audit (spec FR-5.3): spatial-analytics positioning per
      roadmap, MSRV unified with BUILDING.md (1.83+), thread topology corrected to 7
      (add plugin host to the diagram), honest status section (REL-06).
      — AUDIT 2026-10-10: DONE as written — `README.md:5` spatial analytics, `:24` "Status: alpha", `:63` Rust 1.83+, `:111-134` seven-thread diagram incl. T7 Plugin Host. NEW TRUTH GAP: no shipped binary depends on `fe-plugin` (only fe-plugin-test and extensions/iot-bridge do), so T7 does not run in `fractalengine` or `fe-relay`; the diagram must say so (see 6.3).
- [x] 2.2 README security claims amended per spec FR-2 path (b): op-log signing
      stated as not-yet-implemented — unless Phase 5.3 resolves to path (a).
      — AUDIT 2026-10-10: DONE — `README.md:51-53` known limitation (placeholder signatures) + `:253` "per-op ed25519 signing is designed but not yet implemented".
- [x] 2.3 SECURITY.md known-limitations section lists the unsigned-op-log register
      and any open RBAC gaps (spec FR-7).
      — AUDIT 2026-10-10: DONE — `SECURITY.md:28-44`. STALE BULLET: "The sync write gate is permissive" is false since F3 (inbound rows need Editor+ at the DB thread, `fe-database/src/handlers/replicated_row.rs:261`); fix in the 6.3 claims re-run.
- [ ] 2.4 BUILDING.md: add RUST_MIN_STACK=134217728 / surrealdb-core deep-recursion
      gotcha (+ poisoned-rmeta `cargo clean -p` note); fix the GHCR org pull URL
      (REL-09).
      — AUDIT 2026-10-10: PARTIAL — RUST_MIN_STACK + `cargo clean -p surrealdb-core` note present (`BUILDING.md:182-185`); GHCR URL still wrong (`BUILDING.md:128` `ghcr.io/jadezaher/...` vs `release.yml:208` `ghcr.io/${{ github.repository_owner }}`, i.e. Escherbridge).
- [x] 2.5 README license section: Apache-2.0 text + contribution-licensing sentence
      (landed 2026-07-17 with D-69 ratification).

## Phase 3 — Community health + CI gates (agent)

- [x] 3.1 CONTRIBUTING.md (build via BUILDING.md, pre-commit trio
      `cargo fmt && cargo clippy -- -D warnings && cargo test`, licensing-of-
      contributions note) (REL-07). (done 2026-07-17)
- [x] 3.2 CODE_OF_CONDUCT.md (Contributor Covenant 2.1) (REL-07). (done 2026-07-17)
- [x] 3.3 `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml` + PR template
      referencing the test policy (REL-07). (done 2026-07-17)
- [x] 3.4 Fast lint job (fmt --check + clippy -D warnings, Linux-only) in
      `build-artifacts.yml` (spec FR-8). (done 2026-07-17)
- [x] 3.5 `release.yml` build-job timeouts 120→150 (parity with 43655f9).
      (done 2026-07-17)
- [x] 3.6 build-artifacts badge in README; document the scoped 3-crate test sweep in
      the workflow (widening is D-66 — do not silently change scope).
      (done 2026-07-17)
- [x] 3.7a Docs polish, done parts: `#![warn(missing_docs)]` on fe-sdk;
      docs/unwrap-audit.md marked STALE (REL-11). (done 2026-07-17)
- [ ] 3.7b Docs polish, remaining: fix root AGENTS.md SQLite→SurrealDB line
      (currently AGENTS.md:20); fill fe-sdk missing-docs gaps (REL-11).
      — AUDIT 2026-10-10: OPEN — `AGENTS.md:20` still says "SQLite (via fe-database)".
      — CONSOLIDATION 2026-10-10: AGENTS.md:20 fixed (SurrealDB/SurrealKV). Remaining: fe-sdk missing-docs gaps (needs a cargo doc/clippy run).

## Phase 4 — crates.io metadata verification (agent)

- [x] 4.1 Per-crate `description` for fe-sdk + fe-plugin-test at minimum (spec FR-6).
      — AUDIT 2026-10-10: DONE — `fe-sdk/Cargo.toml:8`, `fe-plugin-test/Cargo.toml:8`.
- [ ] 4.2 `cargo publish --dry-run` green for fe-sdk and fe-plugin-test.
      (Granted-command exception must be explicit in the executing session; otherwise
      defer to the end-of-session sweep.)
- [ ] 4.3 Root .glb strays (`camera.glb` 14-byte placeholder, `duck.glb`,
      `info_panel.glb`) relocated under assets/ or examples/; camera.glb replaced.
      — AUDIT 2026-10-10: OPEN — all three still at the repo root.

## Phase 5 — Ratification wall (every item BLOCKED-ON-USER)

- [x] 5.1 **RESOLVED 2026-07-17 — D-69:** user ratified **Apache-2.0 single**
      (overrode the dual default; LICENSE-MIT removed, metadata + docs updated).
- [x] 5.2 **RESOLVED 2026-07-17 — D-70:** conductor/ **stays public**;
      conductor/README.md framing added, register marked draft where unratified.
- [ ] 5.3 **BLOCKED-ON-USER — REL-03 path choice:** implement real op-log signing
      (D5-1-adjacent, its own track if chosen) vs ship with disclosure (Phase 2.2/2.3
      text stands). Default posture staged: disclosure.
- [ ] 5.4 **BLOCKED-ON-USER — SECURITY.md contact:** the real private reporting
      channel (GitHub private vulnerability reporting and/or security@ address).
      — AUDIT 2026-10-10: text names GitHub private vulnerability reporting (`SECURITY.md:5-8`); the repo-setting toggle remains the user's action.
- [ ] 5.5 **BLOCKED-ON-USER — D-66:** ratify the scoped CI test sweep or order the
      widening before the badge goes public.
      — AUDIT 2026-10-10: still scoped — `build-artifacts.yml:118` `cargo test --release -p fe-ui -p fe-terrain -p fractalengine`; the RUNBOOK's "full-workspace run is left to CI" assumption does not hold.
- [ ] 5.6 **BLOCKED-ON-USER — D-53:** publishing gate (repo settings, tokens, live
      tag) — required for the eventual push/release but executed by the user.

## Phase 6 — Pre-push preflight + push (agent prepares, user pushes)

- [ ] 6.1 Secrets re-scan across full history; record the tool + date in this plan.
- [ ] 6.2 Decision-register review: no public doc states an unratified default as
      settled; Phase 5 items all carry dated resolutions.
- [ ] 6.3 Final README claims audit re-run (spec FR-5.3) against the tree as it will
      push.
      — CONSOLIDATION 2026-10-10: partial re-run done — README Status section corrected (BI egress + IoT export moved to works-today, MCP vocabulary dropped from Planned, release matrix post-macOS, plugin host marked library-only incl. T7 diagram note, 24 crates); SECURITY.md stale "sync write gate is permissive" bullet replaced with the DB-thread Editor+ gate and its residuals. Keep open for the pre-push re-run.
- [ ] 6.4 Final `"00".repeat(64)` re-grep matches the disclosed register.
- [ ] 6.5 **BLOCKED-ON-USER:** the push itself (first public push is a user action).
