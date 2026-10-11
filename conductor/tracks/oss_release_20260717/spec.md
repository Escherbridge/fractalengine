---
type: Track Spec
title: OSS Release — open-source release checklist
description: Everything that must be true before the first public push — license ratification gate, placeholder-signature register disclosure, BUSL third-party notice maintenance, conductor/ publicity decision, pre-push preflight, crates.io metadata, SECURITY.md contact, CI badge + lint gate
tags: [chore, oss_release_20260717, pending]
timestamp: 2026-07-17T00:00:00Z
resource: ./metadata.json
---

# Specification: OSS Release Checklist

**Track ID:** `oss_release_20260717`
**Sources:** 2026-07-17 OSS-release audit (findings REL-01..REL-11) + crate-consolidation
audit (F1–F9; executed portions in
[../../decisions/crate-consolidation-20260717.md](../../decisions/crate-consolidation-20260717.md))
**Hard rule:** several items below reference UNRATIFIED register entries
([outstanding_decisions](../outstanding_decisions_20260715/spec.md) D-69, D-70, D5-1
lineage, D-53, D-66). Nothing gated on them may be treated as settled; the plan marks
each BLOCKED-ON-USER.

## Current state (as of 2026-07-17)

Scaffolding landed 2026-07-17 and **D-69 was ratified the same day: Apache-2.0
single** (user overrode the dual default). Current: `LICENSE-APACHE` at root
(LICENSE-MIT removed), `[workspace.package] license = "Apache-2.0"` (root
`Cargo.toml`), `THIRD-PARTY-LICENSES.md`, `deny.toml`, `SECURITY.md` draft. The
repo has never been publicly pushed.

## FR-1 — License ratification gate (REL-01, REL-02 → D-69) — RESOLVED

- Dual MIT OR Apache-2.0 was scaffolded 2026-07-17 as the Rust-convention default.
- **RATIFIED 2026-07-17: Apache-2.0 single.** LICENSE-MIT removed; workspace
  metadata, README license section, CONTRIBUTING licensing clause, and
  THIRD-PARTY-LICENSES.md all state Apache-2.0; the stale "All rights reserved.
  See LICENSE for details." text is gone.
- On override: swap license files + workspace metadata before push; everything
  downstream (THIRD-PARTY notice, deny.toml allowlist, README) re-checks.

## FR-2 — Placeholder ed25519 signature register (REL-03)

README currently claims signed mutations ("immutable op-log with Lamport clock +
ed25519 signature"; "all gossip payloads signed") while production write paths stamp
`sig: "00".repeat(64)`.

**Ratified status — D-CL24(5)** (`conductor/decisions/canonical-data-log-20260808.md:69`):
"all pre-canonical `op_log` rows are inherently unsigned and untrusted — no signature
backfill and no dual-read". Legacy op_log rows are permanently unsigned; real signing
lives in the canonical log (fe-canonical-log), not in these sites. This register is the
disclosure artifact for SECURITY.md and README.

**Re-grep (2026-10-11, working tree on 7d40a4e):**
`git grep -n '"00"\.repeat(64)' -- '*.rs'` → **22 hits in 9 files, all fe-database**:
21 production write paths + 1 `#[cfg(test)]` fixture. (Equivalent:
`rg -n '"00"\.repeat\(64\)' --type rust -g '!target'`.) Task 6.4 is a re-run of this
command against the table below.

| # | Site | Purpose (op written with the placeholder sig) |
| --- | --- | --- |
| 1 | [fe-database/src/handlers/crud.rs:219](../../../fe-database/src/handlers/crud.rs#L219) | `create_node_handler` — NodeCreated |
| 2 | [fe-database/src/handlers/crud.rs:318](../../../fe-database/src/handlers/crud.rs#L318) | `delete_node_handler` — NodeDeleted |
| 3 | [fe-database/src/handlers/crud.rs:604](../../../fe-database/src/handlers/crud.rs#L604) | `create_node_with_asset_handler` — NodeCreated |
| 4 | [fe-database/src/handlers/crud.rs:1320](../../../fe-database/src/handlers/crud.rs#L1320) | `tombstone_node_handler` — NodeTombstoned |
| 5 | [fe-database/src/handlers/crud.rs:1444](../../../fe-database/src/handlers/crud.rs#L1444) | `cascade_tombstone_node_handler` — NodeTombstoned |
| 6 | [fe-database/src/handlers/crud.rs:1561](../../../fe-database/src/handlers/crud.rs#L1561) | `promote_instance_handler` — NodePromoted |
| 7 | [fe-database/src/handlers/elevated_query.rs:137](../../../fe-database/src/handlers/elevated_query.rs#L137) | `run_write` — ElevatedQuery audit row |
| 8 | [fe-database/src/handlers/entity_property.rs:34](../../../fe-database/src/handlers/entity_property.rs#L34) | `set_entity_property_handler` |
| 9 | [fe-database/src/handlers/entity_property.rs:128](../../../fe-database/src/handlers/entity_property.rs#L128) | `delete_entity_property_handler` |
| 10 | [fe-database/src/handlers/transform.rs:36](../../../fe-database/src/handlers/transform.rs#L36) | `update_node_transform_handler` |
| 11 | [fe-database/src/queries.rs:21](../../../fe-database/src/queries.rs#L21) | `create_petal` |
| 12 | [fe-database/src/queries.rs:58](../../../fe-database/src/queries.rs#L58) | `create_room` |
| 13 | [fe-database/src/queries.rs:95](../../../fe-database/src/queries.rs#L95) | `place_model` |
| 14 | [fe-database/src/role_manager.rs:110](../../../fe-database/src/role_manager.rs#L110) | `assign_role_checked` |
| 15 | [fe-database/src/role_manager.rs:149](../../../fe-database/src/role_manager.rs#L149) | `revoke_role_checked` |
| 16 | [fe-database/src/session_cache.rs:60](../../../fe-database/src/session_cache.rs#L60) | `revoke_session` (moved from fe-auth 2026-07-17) |
| 17 | [fe-database/src/space_manager.rs:44](../../../fe-database/src/space_manager.rs#L44) | `update_petal_metadata` |
| 18 | [fe-database/src/space_manager.rs:103](../../../fe-database/src/space_manager.rs#L103) | `set_petal_visibility` |
| 19 | [fe-database/src/space_manager.rs:204](../../../fe-database/src/space_manager.rs#L204) | `update_room_metadata` |
| 20 | [fe-database/src/space_manager.rs:270](../../../fe-database/src/space_manager.rs#L270) | `update_model_metadata` |
| 21 | [fe-database/src/space_manager.rs:341](../../../fe-database/src/space_manager.rs#L341) | `upsert_model_kv` |
| T | [fe-database/src/op_log.rs:420](../../../fe-database/src/op_log.rs#L420) | `probe_entry` — `#[cfg(test)]` fixture, not a production path |

Drift since the 2026-07-17 register (13 sites): crud.rs 0→6 (node-lifecycle spine
c6fdf0d and later handler work), space_manager.rs 4→5, elevated_query.rs 0→1 (7d40a4e),
op_log.rs test fixture +1; the original sites' line numbers shifted. (A cosmetic `signature_placeholder_here` in
`fe-hexon/src/manifest.rs` is test-fixture-only — not in the register.)

- **Requirement:** before public push, EITHER (a) real op-log signing via fe-identity
  keys at all 21 production sites — but per-op signing is unratified D5-1-adjacent scope, so this
  path is BLOCKED-ON-USER — OR (b) amend the README Key Design Invariants to state
  op-log signing is NOT yet implemented and list it in SECURITY.md known limitations.
  Shipping the current README text with these sites intact is a false security claim.
- The register above is the maintenance artifact: re-grep `"00".repeat(64)` before
  push and reconcile.

## FR-3 — SurrealDB BUSL-1.1 third-party notice maintenance (REL-04)

surrealdb / surrealdb-core 3.0.5 are Business Source License 1.1 (not OSI), embedded
(SurrealKV) in both shipped binaries. `THIRD-PARTY-LICENSES.md` + `deny.toml` exist.

- Verify `deny.toml` allowlists MIT/Apache-2.0/BSD/ISC/MPL-2.0 with an explicit BUSL
  exception scoped to `surrealdb*` only.
- Verify `THIRD-PARTY-LICENSES.md` explicitly calls out the BUSL Additional Use Grant
  (no competing DBaaS) as binding downstream users.
- Wire `cargo deny check licenses` into `build-artifacts.yml` so future copyleft
  additions fail CI; regenerate the notice file when deps change.

## FR-4 — conductor/ public vs private (REL-10 → D-70)

The full conductor/ PM bundle (including the unratified decision register), research/,
and stray root-level dev assets (`camera.glb` — 14-byte truncated placeholder,
`duck.glb`, `info_panel.glb`) currently ship with the repo. **BLOCKED-ON-USER (D-70):**
keep conductor/ public as open project management (then: add a conductor/README.md
framing + mark the register draft/unratified) or move conductor/ + research/ to a
private sibling before first push. Either way, relocate the root .glb files under
assets/ or examples/ and replace the truncated camera.glb.

## FR-5 — Pre-push preflight

Run in order, immediately before the first public push (and repeat per force-push-free
history rewrite, if any):

1. **Secrets re-scan** — full-history scan (docker/, configs, .env patterns, key
   material). 2026-07-17 audit found none; re-verify at push time.
2. **Decision-register review** — no public doc states an unratified default as
   settled; D-69/D-70 resolved; the register's own publication status follows D-70.
3. **README claims audit** — positioning matches the 2026-07-14 spatial-analytics
   roadmap (REL-06), MSRV unified with BUILDING.md, thread topology says 7 (plugin
   host included), signing claims match FR-2's resolution, status section honest.

## FR-6 — crates.io metadata verification (REL-02)

- Every member crate inherits `license.workspace = true` (plus `repository`,
  `edition`, `version` via `[workspace.package]`).
- Per-crate `description` present at minimum for the author-facing crates: `fe-sdk`,
  `fe-plugin-test`.
- `cargo publish --dry-run` passes for fe-sdk and fe-plugin-test (the two crates
  plugin authors consume). Actual publishing is out of scope (D-53 gate).

## FR-7 — SECURITY.md contact finalization (REL-05)

Draft exists; finalize: supported-versions table, private reporting channel (GitHub
private vulnerability reporting vs a security@ address — the real contact is a USER
input), response SLA, and an honest known-limitations section covering the FR-2
unsigned-op-log register and any open RBAC gaps. Attack surface justifying it: P2P
(libp2p+iroh), embedded WebView, JWT auth, plugin sandbox, HTTP/WS gateway :8765.

## FR-8 — CI badge + lint-gate verification (REL-08)

- Fast lint job (fmt --check + clippy -D warnings, Linux-only) in
  `build-artifacts.yml` — README:79 declares the standard; CI must enforce it.
- `release.yml` build-job timeouts bumped 120→150 to match commit 43655f9's
  cold-cache headroom (tag builds are the coldest).
- build-artifacts workflow badge in README.
- The intentionally-scoped 3-crate PR test sweep is either documented in the workflow
  or widened — scoping choice is register entry D-66 (unratified; do not silently bless).

## Related findings folded into the checklist (plan phases)

- REL-06 README front-door rewrite (positioning, MSRV, 7 threads, screenshots, badge).
- REL-07 community health files (CONTRIBUTING.md, CODE_OF_CONDUCT.md, issue/PR templates).
- REL-09 BUILDING.md accuracy (RUST_MIN_STACK/surrealdb-core ICE gotcha; GHCR org URL).
- REL-11 docs polish (`#![warn(missing_docs)]` on fe-sdk, AGENTS.md SQLite→SurrealDB fix,
  refresh-or-delete docs/unwrap-audit.md).

## Non-goals

- Implementing real op-log signing (D5-1 lineage; foundry-candidate specs presuppose it).
- Actual crates.io publishing / GHCR push / release tagging (D-53 publishing gate).
- The fe-network stage-2 retirement (D-71) — tracked in the consolidation decision record.

## Acceptance

All plan phases checked; every BLOCKED-ON-USER item carries an explicit user
resolution (ratified/overridden) recorded in the decision register; a final preflight
(FR-5) run dated and clean. Only then is a public push authorized.
