---
type: Track Index
title: FractalEngine Project Tracks
timestamp: 2026-07-17T00:00:00Z
---

# Project Tracks

Live board for open work, ordered by the [roadmap](./roadmap.md) go-forward slate
(2026-07-14 alignment pass), not by historical waves.

> **Conventions:** each open track lives at `./tracks/<id>/`; its `metadata.json` is the
> machine source of truth for status (canon: `pending` | `in_progress` | `spec_only` |
> `done` | `superseded`), dependencies (`depends_on`/`blocks`), and the 2026-07-14
> `alignment` verdict. Archived tracks live under [`./tracks/_archive/`](./tracks/_archive/)
> — 28 folders moved there 2026-07-15 in the session close-out; see the
> [consolidated session retro](./tracks/_archive/session_retro_20260715/retro.md) for
> per-track outcomes and verification evidence (commits `c32e873` → `626f307`).
> A `depends_on`/`blocks` entry naming an archived track is a **satisfied dependency**
> (delivered and archived), not a dangling reference — no cross-check needed.

**Open tracks: 24.**

---

## P0 — UX test findings (2026-07-16, user-driven UX pillar)

Bugs and asks from the user's live UX testing session, 2026-07-16 (logged in
[ux_qa_review findings](./tracks/ux_qa_review_20260714/findings.md)). The four
fix tracks (path_interaction, gpx_stamp_persistence, inspector_units_width,
camera_focus_clip) landed same-day — full sweep green (1517 tests, clippy
-D warnings, fmt), in-app verification user-gated — and were archived
2026-07-17; see the [batch retro](./tracks/_archive/ux_retro_20260716/retro.md).
Remaining:

### [ ] road_builder_ux — C:S-inspired road/path builder input layer

_Link: [./tracks/road_builder_ux_20260716/](./tracks/road_builder_ux_20260716/) · pending · P0 UX (user-driven, 2026-07-16)_

Straight/curved(quadratic-bezier)/freeform drag-to-place segments with C:S-style
chaining, 45°/90° angle snap, guidelines + ghost preview, snap-to-existing-path
(coordinate-share only — no topology graph, feeds a future `procedural_roads`
track), live metric length via a `metric_scale()` seam. New `path_kind`
designed|recorded property (absence ⇒ recorded, zero migration) so analytics can
filter designed roads from GPS traces. Input layer ONLY — ribbon meshes,
intersections, upgrade tool, zoning all deferred (user-ratified hybrid, 2026-07-16).

---

## P0 — Analytics engine critical path

The primary roadmap goal: spatial ANALYTICS engine with BI egress (copy a SQL string /
API URL into PowerBI or a spreadsheet). Ordered per roadmap slate.

### [~] analytics_egress — BI Last-Mile Egress (the killer feature)

_Link: [./tracks/analytics_egress_20260714/](./tracks/analytics_egress_20260714/) · in_progress · P0 CORE-ANALYTICS_

Phases 1–5 landed 2026-07-15: real GeoParquet writer/reader in fe-query (arrow/parquet
54.x, WKB Point Z, CRS-honest `geo` metadata) behind a `parquet` feature; fe-api
`export.parquet`/`.csv` endpoints with DuckDB-httpfs headers; shared `query_guard`
pipeline + D4 cost/row/timeout limits (row-cap = error-not-truncate); ed25519 signed
share URLs (authed mint + public redemption with scope ceiling); three-branch CRS
resolution stamped on every egress path; GIS-panel Export tab "Copy for BI" card
(SQL / URLs / DuckDB snippet / curl, 12 tested builders). ~20 new tests. Remaining:
Phase 6 (e2e + docs + workspace sweep); follow-ups — persistent share-signer key wiring
in main.rs, in-app HTTP mint call (curl display shipped instead).

### [~] hexon_scale_orchestration — Real-World Scale + Rulers (CRS/GSD spine)

_Link: [./tracks/hexon_scale_orchestration_20260712/](./tracks/hexon_scale_orchestration_20260712/) · in_progress · P0 CORE-ANALYTICS_

Hexon-authoritative scale in `TilesetMeta` (+ Web-Mercator backfill), mixed-GSD
reconciliation in `CompositeTileSource`, then a measurement layer. Phases 1–4 done
(scale plumbing + `RulerPlugin` scale-bar HUD landed 2026-07-15). Remaining: Phase 5
(measurement tools — tape/area/bearing + GPX path length) and Phase 6
(graticule/annotations). Metrically-correct reporting gates egress GA; rulers trail.

### [ ] map_scale_authority — map-authoritative scale for placement + UI numbers

_Link: [./tracks/map_scale_authority_20260716/](./tracks/map_scale_authority_20260716/) · pending · P1 CORE-ANALYTICS · complements hexon_scale_orchestration (owns scale CORRECTNESS; that track owns measurement TOOLS)_

User directive 2026-07-16: the map always sets the scale — no per-asset scale
metadata. One canonical world_scale accessor (new `fe-format/src/scale.rs`;
fe-terrain delegates via re-export), placement handlers default node scale from
petal terrain (kills the hardcoded `[1,1,1]` in ImportGltf/create_node), 6+
duplicated fe-ui conversion formulas become shims, API unit contract documented
(world units on the wire, meters at the UI edge), terrain-height snap-on-place
in scope as an independently de-scopable phase (fixes the y=0 placement issue).

### [~] auth_policy_pattern — Policy Engine (RBAC-on-results backbone)

_Link: [./tracks/auth_policy_pattern_20260710/](./tracks/auth_policy_pattern_20260710/) · in_progress (promoted spec→impl 2026-07-15) · P1 ENABLING_

Implementation slice landed 2026-07-15: new `fe-policy` crate (deny-by-default
`Policy::evaluate`, `RoleLevel` moved here canonically), fe-database `require_write_role`
delegates to the engine, fe-hexon Phase 8.4 RBAC gap closed (`install_as`/`uninstall_as`
gated Editor+), fe-sync §D1 write gate via `PolicyHandle.allow_write` (permissive
warn-logging until peer roles are plumbed), fe-plugin/fe-webview thin adapters.
Remaining: fe-api adapter, TokenScopePolicy/OwnershipPolicy, causal-DAG membership +
strong-removal resolver (blocked on per-op ed25519 signing, decisions D5-1), flipping
fe-sync to strict.

### [~] iot_spatial_reporting — IoT as Queryable Spatial Rows

_Link: [./tracks/iot_spatial_reporting_20260714/](./tracks/iot_spatial_reporting_20260714/) · in_progress · P1 CORE-ANALYTICS · depends on analytics_egress_

Landed 2026-07-15: `iot_reading` table + `insert_readings` handler with read-back tests
(FR-1), fe-query `builder/timeseries.rs` (FR-3), batch ingestion endpoint
`POST /petals/:id/iot/readings` + query_guard whitelist seam (FR-4); FR-2 (node_log cap)
was pre-satisfied by p2p_unblock_now (`9ef76a1`). Remaining: FR-5 — reading-shaped
parquet/CSV export (plan Phase 4) + the inherited bim_primitives FR-8
statistical-analysis seam (2026-07-17, spec §Inherited seam).

---

## P1 — Platform

### [~] release_ci — Cross-Compilation Pipeline + Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/) · in_progress · P2 ENABLING_

Phase 1 done (`build-artifacts.yml` 3-OS PR check, rust-cache blessed over sccache,
Linux glib apt deps fixed after run 29390918844). Phase 2 tasks 2.1–2.6 done
(`release.yml` 8-target matrix, lipo universal binary, SHA256SUMS, gh-release publish,
GHCR docker job, `Cross.toml`). Remaining: task 2.7 end-to-end tag verification
(requires a live GitHub run). Docker relay image = shipping vehicle for the analytics
backend.

### [ ] oss_release — open-source release checklist

_Link: [./tracks/oss_release_20260717/](./tracks/oss_release_20260717/) · pending · P1 ENABLING · gated on D-69 (license) + D-70 (conductor/ publicity)_

The full pre-public-push gate, sourced from the 2026-07-17 OSS-release audit
(REL-01..11) + crate-consolidation audit ([decision record](./decisions/crate-consolidation-20260717.md)).
License scaffolding landed 2026-07-17 (LICENSE-MIT/APACHE, workspace
`license = "MIT OR Apache-2.0"`, THIRD-PARTY-LICENSES + deny.toml, SECURITY.md
draft) but is DEFAULTED, not ratified. Checklist: placeholder ed25519 signature
register disclosure (13 sites, all fe-database post-consolidation), SurrealDB
BUSL-1.1 notice maintenance, crates.io metadata verification, SECURITY.md contact
finalization, CI lint gate + badge, community health files, and the pre-push
preflight (secrets re-scan, decision-register review, README claims audit).
Ratification-gated items are marked BLOCKED-ON-USER in the plan.

### [~] cross_platform_desktop — Linux/macOS/Windows-ARM64 Builds

_Link: [./tracks/cross_platform_desktop_20260429/](./tracks/cross_platform_desktop_20260429/) · in_progress · P2 PLATFORM-KEEP · blocks release_ci_

Phases 1–2 done. Phase 3 awaits green CI evidence: Linux (deps fixed 2026-07-15),
macOS aarch64 (new PR job), Windows ARM64 (release.yml, next `v*` tag). Local
Win-ARM64 compile deferred-to-CI (memory-constrained dev machine). Still unverified:
macOS launch smoke test, macOS x86_64 compile.

### [ ] iroh_1_0_upgrade — 0.35 → 1.0 behind the VerseReplicator seam

_Link: [./tracks/iroh_1_0_upgrade_20260711/](./tracks/iroh_1_0_upgrade_20260711/) · pending · **HARD DEADLINE 2026-12-31** (n0 hosted-relay 0.35 wire-protocol EOL)_

P2 now, rises P1 by Q4 — calendar-driven, independent of the wave chain. Coordinate
with p2p_mycelium_completion (wire real iroh-docs against 1.x directly if still in
flight). Gotcha recorded 2026-07-15: fe-sync pins `iroh-quinn-proto` 0.13 for BBR —
bump in lockstep and re-verify the congestion-controller default
(fe-sync/src/AGENTS.md §congestion-control).

### [ ] hexon_unification — one canonical .hexon + portable petal snapshot

_Link: [./tracks/hexon_unification_20260716/](./tracks/hexon_unification_20260716/) · pending · P1 PLATFORM · subsumes hexon_path_asset FR-5 + inherits its FR-6 (archived 2026-07-17)_

Collapse the fe-hexon `.fecrate` parallel stack onto fe-format v1.0.0 (fe-format
stays the lean format layer; fe-hexon rebuilt as the runtime layer — registry,
publisher, remote client, authz — on fe-format types; dead `.fecrate` code deleted,
fe-api `/crates/*` re-pointed) and add a `PetalSnapshot` hexon type packaging a
petal's full SurrealDB state + op-log (HLC-ordered, sigs verbatim per D5-1),
Manager+ gated, with export→import→export round-trip determinism as the acceptance
gate. User directives 2026-07-16: v1.0.0 upheld; registry compat may break freely.

### [ ] mcp_scene_primitives — MCP primitive vocabulary for AI scene construction

_Link: [./tracks/mcp_scene_primitives_20260716/](./tracks/mcp_scene_primitives_20260716/) · pending · P1 PLATFORM · primitives only (user-ratified 2026-07-16)_

Close the asset-ingestion gap — glb upload writes blobs API-side through the
shared BlobStore handle (bytes never cross the bounded crossbeam channel;
metadata-only `DbCommand::CreateAsset`), new `CreateNodeWithAsset` reusing
`DbResult::GltfImported` — and grow `/mcp` from 6 to 20 tools behind a
table-driven ToolSpec/ScopeRule dispatcher that also fixes the 3 weak-authz
warts (create_node / create_petal / update_transform scope checks). Acceptance:
headless e2e upload→place→property→move→read→delete over MCP with RBAC
negatives. No semantic city verbs (tripwire-tested); 256 MiB glb-only limit.

---

## Foundry-candidate specs (separate hexon-foundry project)

All four carry the 2026-07-14 FOUNDRY-CANDIDATE/-ADJACENT verdict: keep specs here as
reference, do **not** implement in the analytics core. Gated on decision
**[D-12 (open/closed line)](./tracks/outstanding_decisions_20260715/spec.md)** — where
open-core FractalEngine ends and the closed foundry/registry/marketplace begins. All
presuppose real per-op ed25519 signing (13 placeholder sites, decisions D5-1).

- [ ] **hexon_delta_format** — replayable op-log hexons, HashSeq container, log-first
  WAL — spec_only — [./tracks/hexon_delta_format_20260710/](./tracks/hexon_delta_format_20260710/)
- [ ] **hexon_p2p_bucket** — 3D visual IPFS, handshake-then-swarm, relay-as-seeder —
  spec_only; inherited fe-renderer loader finish + P2P blob fetch from
  hexon_path_asset (its FR-3/FR-5) — [./tracks/hexon_p2p_bucket_20260710/](./tracks/hexon_p2p_bucket_20260710/)
- [ ] **verse_services** — accelerator-only per-verse services (seeder/presence/
  materializer); reconstruct-without-service invariant — spec_only —
  [./tracks/verse_services_20260711/](./tracks/verse_services_20260711/)
- [~] **p2p_mycelium_completion** — real iroh-docs Engine + gossip RX loop; phases 1–2
  reopened 2026-07-11 with file:line evidence (mock-backed replicators), phase 4
  partially done (verse gossip topics) — in_progress, FOUNDRY-ADJACENT P2; policy-gate
  prerequisite met by auth_policy_pattern's sync write gate —
  [./tracks/p2p_mycelium_completion_20260701/](./tracks/p2p_mycelium_completion_20260701/)

---

## Backlog / deferred

No blocking dependencies; opportunistic or explicitly deferred (see each
`metadata.json` `alignment` field).

- [~] **inspector_settings** — FR-3 (hierarchy inspection) + FR-4 (Access-tab RBAC UI)
  remain; FR-1 SaveUrl `is_url_allowed` landed 2026-07-15, FR-2 folded to shipped tab
  set — [./tracks/inspector_settings_20260419/](./tracks/inspector_settings_20260419/)
- [ ] **thorns_shields** — security hardening + pre-launch docs; Wave-1 scaffolds only,
  unwrap audit never run; folder reconstructed 2026-07-14 —
  [./tracks/thorns_shields_20260321/](./tracks/thorns_shields_20260321/)
- [ ] **sso_federation** — enterprise OIDC/SSO, spec_only P2 —
  [./tracks/sso_federation_20260429/](./tracks/sso_federation_20260429/)
- [ ] **drag_drop_placement** — OS file drop + placement flow, spec_only; hand to the
  user's UX track — [./tracks/drag_drop_placement_20260402/](./tracks/drag_drop_placement_20260402/)
- [ ] **light_box** — default lighting rig; OFF-STRATEGY defer —
  [./tracks/light_box_20260402/](./tracks/light_box_20260402/)
- [ ] **profile_manager** — identity/profile UI + P2P sync; OFF-STRATEGY defer
  (PeerRegistry dep satisfied) — [./tracks/profile_manager_20260419/](./tracks/profile_manager_20260419/)

---

## Meta tracks

- [~] **ux_qa_review** — QA harness for the **user-owned** UX review of the shipped
  2026-07 GPX/path/analytics surfaces; review-only, its `findings.md` seeds the future
  UX implementation track (roadmap defers UX to the user — do not pre-empt);
  in_progress: live-testing batch #1 (2026-07-16, 9 findings) triaged and closed,
  audit-sourced findings logged 2026-07-17 —
  [./tracks/ux_qa_review_20260714/](./tracks/ux_qa_review_20260714/)
- [ ] **outstanding_decisions_20260715** — **[the ratification register](./tracks/outstanding_decisions_20260715/spec.md)**:
  72 entries (34 USER decisions, 38 DEFAULTED) from the 2026-07-14/15 audit + 3-wave
  session, plus the 2026-07-17 OSS/consolidation appendix (D-69..D-72), awaiting user
  ratify/override/kill — including D-12 (foundry split) and
  D-68 (egress guard bypass fixes) —
  [./tracks/outstanding_decisions_20260715/](./tracks/outstanding_decisions_20260715/)

---

## Archived work

One line per batch; full detail lives in each folder under
[`./tracks/_archive/`](./tracks/_archive/). Archiving = folder moved to
`_archive/<id>/` + `metadata.json` `archived: true` / `archived_at`.

- **2026-07-10 — Waves 1–3 foundations + Tauri migration + plugin system** (25 folders):
  seed_runtime through gardener_console, viewport/scene-graph/selection/gizmos,
  Entity Data Layer phases, terrain_gpx_maps, crate_registry, realtime_api_mcp,
  plugin_host / extension_sdk_ui / plugin_testing_dx, tauri_* trio, 2026-04-30
  code-review trio, wave_retros — [`_archive/`](./tracks/_archive/)
- **2026-07-11 — hexon-p2p-commons + ultrapilot close-out** (12 folders):
  analytics_extension_api, iot_extension_slice, feui_decomposition,
  terrain_scale_controls, p2p_mycelium (original), pears spike, early chore/research
  tracks — decisions ratified in
  [./decisions/hexon-p2p-commons-20260711.md](./decisions/hexon-p2p-commons-20260711.md)
- **2026-07-12 — splat coverage experiment** (1 folder, shelved-reverted): baked-bake
  direction preserved in
  [`_archive/splat_lod_zoom_20260712/`](./tracks/_archive/splat_lod_zoom_20260712/)
  (succeeded by the archived splat_hexon_bake track)
- **2026-07-13 — GPX/path/GIS burst** (17 tracks, done 07-13, archived in the 07-15
  close-out): gpx_pipeline, gpx_path_editor, pen_tool_curves, pen_autocreate_track,
  input_router, data_icons, track_styling, glb_mesh_picking, path_asset_picker,
  path_node_binding_hardening, node_placement_z_axis, gpx_path_persistence_fix,
  gis_tool_panel, gis_query_ui, petal_gis_endpoints, terrain_lod_hardening,
  splat_hexon_bake — all at `_archive/<id>/`
- **2026-07-15 — session close-out** (28 folders incl. the burst above): plus
  p2p_unblock_now, asset_download_fix, headless_relay, build_size_mobile_prep,
  code_review_cleanup + mega_function/perf_hotpaths/clippy trio, terrain_splat_view
  (verified already-done), petal_seed + seedling_onboarding (superseded) — outcomes in
  [session_retro_20260715/retro.md](./tracks/_archive/session_retro_20260715/retro.md)
- **2026-07-17 — board-hygiene batch** (7 folders + batch retro): the four 2026-07-16
  P0 UX tracks path_interaction, gpx_stamp_persistence, inspector_units_width,
  camera_focus_clip (done 07-16, in-app verify user-gated — consolidated
  [ux_retro_20260716/retro.md](./tracks/_archive/ux_retro_20260716/retro.md)); plus
  tauri_host_shell_spike (superseded — shelved OFF-STRATEGY 07-14, exit report
  delivered, CONDITIONAL GO), bim_primitives_on_paths (FR-8 seam folded into
  iot_spatial_reporting), hexon_path_asset (FR-5 subsumed / FR-6 inherited by
  hexon_unification)
