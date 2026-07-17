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

Scaffolding landed 2026-07-17 but is DEFAULTED, not ratified: `LICENSE-MIT` +
`LICENSE-APACHE` at root, `[workspace.package] license = "MIT OR Apache-2.0"` (root
`Cargo.toml`), `THIRD-PARTY-LICENSES.md`, `deny.toml`, `SECURITY.md` draft. The repo has
never been publicly pushed; there is no license the owner has actually chosen yet.

## FR-1 — License ratification gate (REL-01, REL-02 → D-69)

- Dual MIT OR Apache-2.0 was scaffolded 2026-07-17 as the Rust-convention default.
- **Gate:** the user must ratify D-69 before any public push, tag, or crates.io publish
  — a license choice is effectively irreversible once code ships under it.
- On ratification: README license section states "Licensed under either of Apache
  License 2.0 or MIT license at your option" + the standard contribution-licensing
  sentence; the stale "All rights reserved. See LICENSE for details." text is gone.
- On override: swap license files + workspace metadata before push; everything
  downstream (THIRD-PARTY notice, deny.toml allowlist, README) re-checks.

## FR-2 — Placeholder ed25519 signature register (REL-03)

README currently claims signed mutations ("immutable op-log with Lamport clock +
ed25519 signature"; "all gossip payloads signed") while production write paths stamp
`sig: "00".repeat(64)`. Fresh workspace grep 2026-07-17 (post-crate-consolidation —
the former fe-auth revocation site now lives in fe-database): **13 sites, all in
fe-database**:

| # | Site |
| --- | --- |
| 1 | `fe-database/src/space_manager.rs:48` |
| 2 | `fe-database/src/space_manager.rs:84` |
| 3 | `fe-database/src/space_manager.rs:174` |
| 4 | `fe-database/src/space_manager.rs:234` |
| 5 | `fe-database/src/session_cache.rs:60` (`revoke_session`; moved verbatim from `fe-auth/src/revocation.rs` in the 2026-07-17 fe-auth absorption) |
| 6 | `fe-database/src/role_manager.rs:111` |
| 7 | `fe-database/src/role_manager.rs:147` |
| 8 | `fe-database/src/queries.rs:20` |
| 9 | `fe-database/src/queries.rs:56` |
| 10 | `fe-database/src/queries.rs:91` |
| 11 | `fe-database/src/handlers/transform.rs:44` |
| 12 | `fe-database/src/handlers/entity_property.rs:68` |
| 13 | `fe-database/src/handlers/entity_property.rs:158` |

(A 14th cosmetic placeholder, `signature_placeholder_here` at
`fe-hexon/src/manifest.rs:27`, is test-fixture-only — not in the register.)

- **Requirement:** before public push, EITHER (a) real op-log signing via fe-identity
  keys at all 13 sites — but per-op signing is unratified D5-1-adjacent scope, so this
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
