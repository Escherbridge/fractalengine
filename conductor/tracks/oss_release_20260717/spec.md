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

**Register rebuilt 2026-11-13 (REL5-04, working tree on 91782e9a).** Command:
`git grep -n '"00"\.repeat(64)' -- 'fe-*' 'fractalengine*' tools extensions` returns **51 hits**:
**36 production** write paths + **14 test** + **0 check** + **1 doc mention**
(`fe-database/src/AGENTS.md:502`, prose, not code) = 51. Every site stamps an `OpLogEntry`
whose `sig` is the all-zero placeholder; none of the hits compares against or rejects it
(no quarantine or admission check reads the placeholder), so the CHECK class is empty.
Classification is mechanical: a hit is TEST when it sits in a `#[cfg(test)]` module or
`#[cfg(test)]` fn, or in a `tests/` file; otherwise it is a production write.

| # | Site (function) | Op written with the placeholder sig |
| --- | --- | --- |
| 1 | [fe-database/src/handlers/checked_mutation.rs:205](../../../fe-database/src/handlers/checked_mutation.rs#L205) `commit` | caller-supplied kind (checked mutation) |
| 2 | [fe-database/src/handlers/crud.rs:1188](../../../fe-database/src/handlers/crud.rs#L1188) `tombstone_node_handler` | NodeTombstoned |
| 3 | [fe-database/src/handlers/crud.rs:1312](../../../fe-database/src/handlers/crud.rs#L1312) `cascade_tombstone_node_handler` | NodeTombstoned |
| 4 | [fe-database/src/handlers/crud.rs:1508](../../../fe-database/src/handlers/crud.rs#L1508) `promote_instance_handler` | NodePromoted |
| 5 | [fe-database/src/handlers/crud.rs:1574](../../../fe-database/src/handlers/crud.rs#L1574) `rename_node_handler` | NodeRenamed |
| 6 | [fe-database/src/handlers/editor_batch/mod.rs:141](../../../fe-database/src/handlers/editor_batch/mod.rs#L141) `compile` | EditorTransaction manifest |
| 7 | [fe-database/src/handlers/elevated_query.rs:169](../../../fe-database/src/handlers/elevated_query.rs#L169) `run_write` | ElevatedQuery audit row |
| 8 | [fe-database/src/handlers/entity_property.rs:42](../../../fe-database/src/handlers/entity_property.rs#L42) `set_entity_property_granted` | PropertySet |
| 9 | [fe-database/src/handlers/entity_property.rs:113](../../../fe-database/src/handlers/entity_property.rs#L113) `delete_entity_property_granted` | PropertyDeleted |
| 10 | [fe-database/src/handlers/entity_property.rs:158](../../../fe-database/src/handlers/entity_property.rs#L158) `delete_entity_properties_as` | PropertyDeleted |
| 11 | [fe-database/src/handlers/extension_binding.rs:635](../../../fe-database/src/handlers/extension_binding.rs#L635) `commit_local` | ExtensionAudit |
| 12 | [fe-database/src/handlers/extension_policy.rs:833](../../../fe-database/src/handlers/extension_policy.rs#L833) `audit_op` | ExtensionAudit |
| 13 | [fe-database/src/handlers/object_creation.rs:324](../../../fe-database/src/handlers/object_creation.rs#L324) `commit` | NodeCreated |
| 14 | [fe-database/src/handlers/petal_render_distance.rs:67](../../../fe-database/src/handlers/petal_render_distance.rs#L67) `set_petal_render_distance_handler` | SetPetalRenderDistance |
| 15 | [fe-database/src/handlers/petal_requirements.rs:103](../../../fe-database/src/handlers/petal_requirements.rs#L103) `set` | SetPetalRequirements |
| 16 | [fe-database/src/handlers/petal_snapshot.rs:63](../../../fe-database/src/handlers/petal_snapshot.rs#L63) `export_petal_snapshot` | ExportPetal |
| 17 | [fe-database/src/handlers/petal_snapshot.rs:328](../../../fe-database/src/handlers/petal_snapshot.rs#L328) `import_petal_snapshot_correlated` | ImportPetal |
| 18 | [fe-database/src/handlers/petal_terrain.rs:129](../../../fe-database/src/handlers/petal_terrain.rs#L129) `commit` | SetPetalTerrain |
| 19 | [fe-database/src/handlers/plugin_host.rs:607](../../../fe-database/src/handlers/plugin_host.rs#L607) `commit` | PluginTransaction |
| 20 | [fe-database/src/handlers/scene_asset_adoption.rs:245](../../../fe-database/src/handlers/scene_asset_adoption.rs#L245) `adopt` | SceneAssetAdopted |
| 21 | [fe-database/src/handlers/stamp_overrides.rs:265](../../../fe-database/src/handlers/stamp_overrides.rs#L265) `prepare` | PropertySet |
| 22 | [fe-database/src/handlers/stamp_promotion.rs:301](../../../fe-database/src/handlers/stamp_promotion.rs#L301) `prepare` | NodePromoted |
| 23 | [fe-database/src/handlers/transform.rs:48](../../../fe-database/src/handlers/transform.rs#L48) `update_node_transform_granted` | TransformUpdate |
| 24 | [fe-database/src/handlers/transform.rs:325](../../../fe-database/src/handlers/transform.rs#L325) `prepare_node_url_write` | UpdateModelMeta |
| 25 | [fe-database/src/migration/role_grants.rs:176](../../../fe-database/src/migration/role_grants.rs#L176) `convert_to_owner_statement` | AssignRole (migrated) |
| 26 | [fe-database/src/migration/transform_mapping.rs:943](../../../fe-database/src/migration/transform_mapping.rs#L943) `dual_emit_update_node_transform` | TransformUpdate |
| 27 | [fe-database/src/queries.rs:20](../../../fe-database/src/queries.rs#L20) `create_petal` | CreatePetal |
| 28 | [fe-database/src/queries.rs:60](../../../fe-database/src/queries.rs#L60) `create_room` | CreateRoom |
| 29 | [fe-database/src/queries.rs:97](../../../fe-database/src/queries.rs#L97) `place_model` | PlaceModel |
| 30 | [fe-database/src/role_manager.rs:194](../../../fe-database/src/role_manager.rs#L194) `issue_local_statement` | AssignRole |
| 31 | [fe-database/src/session_cache.rs:60](../../../fe-database/src/session_cache.rs#L60) `revoke_session` | RevokeSession |
| 32 | [fe-database/src/space_manager.rs:44](../../../fe-database/src/space_manager.rs#L44) `update_petal_metadata` | UpdatePetalMeta |
| 33 | [fe-database/src/space_manager.rs:103](../../../fe-database/src/space_manager.rs#L103) `set_petal_visibility` | UpdatePetalMeta |
| 34 | [fe-database/src/space_manager.rs:204](../../../fe-database/src/space_manager.rs#L204) `update_room_metadata` | UpdateRoomMeta |
| 35 | [fe-database/src/space_manager.rs:270](../../../fe-database/src/space_manager.rs#L270) `update_model_metadata` | UpdateModelMeta |
| 36 | [fe-database/src/space_manager.rs:342](../../../fe-database/src/space_manager.rs#L342) `upsert_model_kv` | UpdateModelMeta |

**TEST hits (14):**
- `fe-api/tests/export_share_test.rs:1875` (`petal_snapshot_export_import_authz_journey` fixture).
- `fe-database/src/handlers/crud.rs:268` (`delete_node_handler` is itself `#[cfg(test)]`-gated, line 224).
- `fe-database/src/handlers/invite.rs:480` (corrupts a fixture row on purpose; inside `mod tests`, line 177).
- `fe-database/src/op_log.rs:477` (`probe_entry`, inside `mod tests`, line 368).
- `fe-database/src/role_manager.rs:491` (`resolve_role_uses_verse_default_access`, inside `mod tests`, line 368).
- `fe-database/tests/petal_snapshot_test.rs:78, 433, 443, 449, 474, 489, 510, 516, 523` (9 fixture rows).

**CHECK hits (0):** no site compares against or rejects the placeholder.

Drift note: the 2026-10-11 register had 22 hits (21 production + 1 test). New production
sites since then come from the checked-mutation spine, object creation, petal terrain,
render distance, requirements, snapshot export/import, plugin host, extension audit,
stamp overrides and promotion, and the grant migration. (A cosmetic
`signature_placeholder_here` in `fe-hexon/src/manifest.rs` is test-fixture-only and not
counted.)

- **Requirement:** before public push, EITHER (a) real op-log signing via fe-identity
  keys at all 36 production sites — but per-op signing is unratified D5-1-adjacent scope, so this
  path is BLOCKED-ON-USER — OR (b) amend the README Key Design Invariants to state
  op-log signing is NOT yet implemented and list it in SECURITY.md known limitations.
  Shipping the current README text with these sites intact is a false security claim.
- The register above is the maintenance artifact: re-grep `"00".repeat(64)` before
  push and reconcile.


The 2026-11-13 re-run is folded into the register table above (51 hits classified: 36 production, 14 test, 0 check, 1 doc mention); re-run the command in its header and reconcile before push.

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
  **Amendment 2026-11-13:** the fast lint job no longer runs clippy (user decision
  2026-11-09: clippy is in no gate; CONTRIBUTING.md:30). The lint job is fmt +
  cargo-deny + fuzz, warn-only (build-artifacts.yml:60-125).
- `release.yml` build-job timeouts bumped 120→150 to match commit f0d4bd3's
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
