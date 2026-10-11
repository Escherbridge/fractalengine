---
type: Track Index
title: FractalEngine Project Tracks
timestamp: 2026-10-10T00:00:00Z
---

# Project Tracks

Live board for open work, ordered by the [roadmap](./roadmap.md) go-forward slate
(2026-07-14 alignment pass), not by historical waves. The classified remaining-work
queue (what is agent-doable, what needs a ratification, what is user-gated) lives in
the **[doneness ledger](./doneness-ledger.md)**; this board says *where each track
stands*, the ledger says *what to do next*.

> **Conventions:** each track lives at `./tracks/<id>/`; its `metadata.json` is the
> machine source of truth for status (canon: `pending` | `in_progress` | `spec_only` |
> `done` | `superseded`, plus three audit statuses introduced 2026-10-10:
> `implemented_pending_inapp_verify` — code landed and tested, only the user's in-app
> check remains; `blocked_on_ratification` — remaining work waits on a user decision;
> `off_strategy` — 2026-07-14 alignment verdict DEFER, no work scheduled), dependencies
> (`depends_on`/`blocks`), and the 2026-07-14 `alignment` verdict. Archived tracks live
> under [`./tracks/_archive/`](./tracks/_archive/); a `depends_on`/`blocks` entry naming
> an archived track is a **satisfied dependency**.

**Tracks: 41 non-archive folders — 9 done (awaiting archival), 32 open.**
Open = 7 `implemented_pending_inapp_verify` · 11 `in_progress` · 5 `pending` ·
4 `spec_only` · 2 `blocked_on_ratification` · 2 `off_strategy` · 1 meta (`ux_qa_review`).

> **Doneness reconciliation 2026-10-10** (three parallel auditors + a consolidation
> pass; code-reading and git evidence only, no cargo). Every plan checkbox was checked
> against the tree. **Status corrections:** `node_lifecycle_addressing` pending→done;
> `endpoint_api_surface` pending→in_progress; `contextual_controls`, `shell_ux_sidebar`,
> `stamped_asset_nodes`, `sculpt_earthwork_regions` pending→implemented_pending_inapp_verify
> (Spatial Builder Wave 0/1 landed c6fdf0d→631f389, the board still said "NOT started");
> `pen_curve_tool`, `ui_shell_architecture`, `ui_semantics_unification`
> →implemented_pending_inapp_verify; `inspector_settings`, `outstanding_decisions`
> →blocked_on_ratification; `light_box`, `profile_manager` →off_strategy;
> `canonical_data_log` Wave 3 recorded as landed (b820b67 + 3R fa0d9d9/d01b098).
> **Consolidator un-ticks** (task's own acceptance test absent): `mcp_scene_primitives`
> 2.2 + 5.2 (track done→in_progress), `sculpt_earthwork_regions` determinism test,
> `stamped_asset_nodes` viewport-gimbal translate lock. **JSON repaired:**
> `hexon_unification` (trailing comma), `p2p_mycelium_completion` (BOM + mojibake).
> Earlier board history (2026-07-19 validation, 2026-07-24 pruning, 2026-10-09 F18
> recount) is in `git log -- conductor/tracks.md`.

---

## Done — awaiting archival

Closed with evidence; move to `_archive/` with a retro when convenient.

- [x] **mission_prodready_continuation** — factory mission F9–F19, A20–A33 validated;
  pushed at `1e9be3a`; deferred register ledgered —
  [./tracks/mission_prodready_continuation_20261008/](./tracks/mission_prodready_continuation_20261008/)
- [x] **node_lifecycle_addressing** — Spatial Builder SPINE: tombstone/cascade/reflow,
  `fe://` addresses, lazy promotion, `CallerAuth::Local` (c6fdf0d, aeba832; live P2P
  tombstone merge via `two_peer_tombstone_sync`). Residuals: cascade not one txn
  (T1-TXN), tombstone GC (T1-GC) —
  [./tracks/node_lifecycle_addressing_20260725/](./tracks/node_lifecycle_addressing_20260725/)
- [x] **analytics_egress** — BI egress: GeoParquet/CSV export, signed share URLs, CRS
  stamping, "Copy for BI" card; live DuckDB e2e 49/49 (`scripts/bi-egress-verify.ps1`),
  [docs/bi-egress.md](../docs/bi-egress.md) —
  [./tracks/analytics_egress_20260714/](./tracks/analytics_egress_20260714/)
- [x] **iot_spatial_reporting** — `iot_reading` rows, ingest endpoint, timeseries builder,
  readings export (F11 2023809). Poll-connector catalog stays future work (D-6) —
  [./tracks/iot_spatial_reporting_20260714/](./tracks/iot_spatial_reporting_20260714/)
- [x] **api_mcp_integration_tests** — ApiHarness + api/mcp suites, strict `wart_*`
  denials (635b8c5). One non-blocking item: endpoint inventory artifact (API-INV-1) —
  [./tracks/api_mcp_integration_tests_20260717/](./tracks/api_mcp_integration_tests_20260717/)
- [x] **p2p_mycelium_completion** — real iroh-docs 0.35 replication (F1–F4, F20–F23);
  phases 2/3/5 superseded, per-petal topics superseded by SPEC-3. Residual: relay
  REST-created verse secrets are process-lifetime (MYC-SECRETS) —
  [./tracks/p2p_mycelium_completion_20260701/](./tracks/p2p_mycelium_completion_20260701/)
- [x] **timeseries_fabric**, **simulation_lab**, **bi_verification** — record-only
  tracks for mission M2/M3/M4 —
  [timeseries_fabric_20261009](./tracks/timeseries_fabric_20261009/) ·
  [simulation_lab_20261009](./tracks/simulation_lab_20261009/) ·
  [bi_verification_20261009](./tracks/bi_verification_20261009/)

---

## P0 — Spatial Builder Program (user planning grill 2026-07-25)

Anchor decision record:
[./decisions/spatial-builder-program-20260725.md](./decisions/spatial-builder-program-20260725.md)
(thesis, D-A1..A11, NFR pool N-1..N-10, file partition). **Wave 0 + Wave 1 landed**
(leaves c6fdf0d→cdd3291, serial integration 623145f, tool restore 631f389). The
2026-07-26 in-app pass was PARTIAL FAIL; the regressions were fixed (631f389, 4d0c12d)
but **no in-app re-verify has happened** (SB-INAPP-VERIFY, user-gated). No track has a
retro yet (SB-RETROS). The spine (`node_lifecycle_addressing`) is done — see above.

### [~] shell_ux_sidebar — Settings & Maps → sidebar sections, user-sticky left sidebar

_Link: [./tracks/shell_ux_sidebar_20260725/](./tracks/shell_ux_sidebar_20260725/) · implemented_pending_inapp_verify · 7 done / 2 open_

FR-1/2/3 landed in b3f4df4: sticky `left_visibility` + Ctrl+B, Settings and Maps are
right-sidebar sections, the floating Settings window is deleted. Open: the fe-ui
directory docs still describe AutoCollapse and the deleted window (SHELL-STALE-DOCS);
retro; in-app verify.

### [~] stamped_asset_nodes — curve-following addressable stamps with overrides + instancing

_Link: [./tracks/stamped_asset_nodes_20260725/](./tracks/stamped_asset_nodes_20260725/) · implemented_pending_inapp_verify · 9 done / 3 open_

Arc-length curve sampling + true tangent, lazy promotion on select, sparse scale/rotate
overrides, instancing data layer + 10k pick bench, reflow (56ccc3b, 623145f, 631f389).
Open: N-9 row-count test; the **viewport Move gimbal still drags a bridged stamp**
(un-ticked 2026-10-10 — `gimbal_interaction.rs:267` treats Stamp as Node;
STAMP-GIMBAL-TRANSLATE-LOCK); retro/in-app.

### [~] sculpt_earthwork_regions — brush/shape sculpt + reportable cut/fill region nodes

_Link: [./tracks/sculpt_earthwork_regions_20260725/](./tracks/sculpt_earthwork_regions_20260725/) · implemented_pending_inapp_verify · 9 done / 2 open_

Footprints, raise/lower/level/smooth, region nodes with real-unit cut/fill, report +
REST summary (09e8e2e, 623145f, 631f389). Open: determinism test (un-ticked
2026-10-10); retro/in-app. Gap: `SculptDeleteRegion` has a handler but **no UI
producer**, so a region cannot be deleted from the app (SB-EARTHWORK-DELETE).

### [~] contextual_controls — object-aware right-click menu with real Delete

_Link: [./tracks/contextual_controls_20260725/](./tracks/contextual_controls_20260725/) · implemented_pending_inapp_verify · 8 done / 1 open_

`menu_for(HitTarget)` table, Delete/cascade, rename/duplicate/promote, copy-API/report
via the T5 seam (4f765fe, 623145f, D16 classifier 4d0c12d). Open: retro + in-app.

### [~] endpoint_api_surface — every object a read/write API endpoint (+ MCP)

_Link: [./tracks/endpoint_api_surface_20260725/](./tracks/endpoint_api_surface_20260725/) · in_progress · 5 done / 4 open_

Addresses (`/nodes/{id}/address`, `/address?uri=`), per-object GET, DELETE/cascade,
promote, MCP `read_node`/`node_address`, egress strings (cdd3291, 623145f). Open:
stamp position-lock not enforced on REST/MCP `update_transform` (T5-LOCK); UI-vs-API
op-log parity + earthwork summary on real rows (T5-PARITY); trigger-op vocabulary
needs ratification (T5-TRIGGER); retro.

---

## P0 — UI shell + UI semantics

### [~] ui_shell_architecture — pointer/tab/modal managers + right-sidebar tool inspector

_Link: [./tracks/ui_shell_architecture_20260724/](./tracks/ui_shell_architecture_20260724/) · implemented_pending_inapp_verify · 23 done / 6 open_

Phases 0–6 landed and pushed (a731369..dbe2e39, efe2126). The FR-1 terrain crash was
closed out of band (190188f proposals-only doc shape + 631f389 smart_aim NaN from
`f32::MAX` DragValue ranges). All 6 open boxes are user-gated in-app checkpoints.
Small doable follow-up: debounced warning on a silent path-pick no-op
(UISHELL-SILENT-PICK-WARN). `panic = "abort"` kept by ratification.

### [~] ui_semantics_unification — unified tool toggles + Options surface

_Link: [./tracks/ui_semantics_unification_20260808/](./tracks/ui_semantics_unification_20260808/) · implemented_pending_inapp_verify_

All phases committed in 4d0c12d (mislabelled as the visibility-resolver commit; 62
files). Open: in-app verify; record the D-1..D-18 rulings in spec §4
(UISEM-RULINGS-RECORD); optional `f32::MAX`-in-`.range(` lint (needs a yes).

### [~] hierarchy_visibility_groups — sidebar eyes at all 4 levels + named Groups

_Link: [./tracks/hierarchy_visibility_groups_20260808/](./tracks/hierarchy_visibility_groups_20260808/) · in_progress_

All 18 decisions ratified 2026-08-08. Phases 0–1 + part of Phase 2 landed in 4d0c12d
(resolver, eyes, chip, viewport-pick + camera filters, force-deselect). Phases 2–6 are
agent-doable: remaining pickers + H/Shift+H/Alt+H + Isolate menu (VIS-PH2), persisted
`view.state`/`view.groups` (VIS-PH3), petal group registry + persisted fractal/verse
eyes (VIS-PH4), Groups section + unified layer registry (VIS-PH5), REST/MCP (VIS-PH6).

---

## P0 — UX test findings (2026-07-16, user-driven UX pillar)

The four 2026-07-16 fix tracks were archived 2026-07-17
([batch retro](./tracks/_archive/ux_retro_20260716/retro.md)). Remaining:

### [ ] road_builder_ux — C:S-inspired road/path builder input layer

_Link: [./tracks/road_builder_ux_20260716/](./tracks/road_builder_ux_20260716/) · pending · 0 / 30_

Not started. Design user-ratified 2026-07-16 (input layer only); its prerequisite
(ui_shell pointer-ops manager) has landed, so it is buildable: `path_kind` provenance
(ROAD-PATH-KIND, independent) then snap core, `Tool::Road`, ghost/guides/readout
(ROAD-CORE). Curved mode should reuse pen_curve's bezier authoring.

### [~] pen_curve_tool — Illustrator-style Pen curve tool

_Link: [./tracks/pen_curve_tool_20260722/](./tracks/pen_curve_tool_20260722/) · implemented_pending_inapp_verify · 31 done_

Phases 1–7 landed and pushed (34a5160 Ph1-2, Ph3-6 + retro 2026-07-24). The 2026-07-24
point-selection blocker was fixed by ui_shell FR-2. Only the user's in-app re-verify
and archival remain.

---

## P0 — Analytics engine critical path

BI egress (the headline) is **done** — see above. Remaining analytics work:

### [~] hexon_scale_orchestration — real-world scale + rulers

_Link: [./tracks/hexon_scale_orchestration_20260712/](./tracks/hexon_scale_orchestration_20260712/) · in_progress · 17 done / 7 open_

Phases 1–4 done (scale plumbing, `RulerPlugin` scale bar); GPX path-length readout done
(906d9cc). Open: tape/bearing + area tools (HS-P5), adaptive metric grid (HS-P6-GRID),
dimension callouts (blocked on D-11), user in-app check. The D-10 gate (egress GA) is met.

### [ ] map_scale_authority — map-authoritative scale for placement + UI numbers

_Link: [./tracks/map_scale_authority_20260716/](./tracks/map_scale_authority_20260716/) · pending · 0 / 21_

Not started (no `fe-format/src/scale.rs`; `crud.rs`/`lib.rs` still hardcode scale
`[1,1,1]`). Design binding since 2026-07-16; Phases 1–3 + 5 agent-doable (MSA-P1..P3,
P5 — P3 also closes the ux_qa scale-authority split); Phase 4 snap-on-place waits on D-38.

### [~] auth_policy_pattern — policy engine (RBAC-on-results backbone)

_Link: [./tracks/auth_policy_pattern_20260710/](./tracks/auth_policy_pattern_20260710/) · in_progress · 13 done / 3 open_

fe-policy crate, fe-database/fe-hexon routing, hexon HTTP routes through the engine
(916a3ae), inbound peer admission at the DB thread (F3, supersedes the sync-thread
`PolicyHandle` flip). Open: fe-api `auth.rs` still bypasses the engine (AP-3.1);
`TokenScopePolicy` + `OwnershipPolicy` absent (AP-3.4). Task 3.5 (causal-DAG
membership) is stale — superseded by the canonical data log (D-CL24(5)).

---

## P1 — Platform

### [~] mcp_scene_primitives — MCP primitive vocabulary for AI scene construction

_Link: [./tracks/mcp_scene_primitives_20260716/](./tracks/mcp_scene_primitives_20260716/) · in_progress (was done) · 23 done / 3 open_

All functionality shipped: CreateAsset/CreateNodeWithAsset, multipart upload, ToolSpec
dispatcher, 29 tools with exact inventory test, RBAC negatives (F13 635b8c5, F14,
5b06795, 9cbdea6). Reopened 2026-10-10 for owed tests: REST upload 201/403/503
(MCP-TEST-UPLOAD), one chained FR-8 loop (MCP-TEST-LOOP), plus REST `create_node`
with `asset_id`/rotation/scale (MCP-2.4).

### [~] canonical_data_log — signed canonical log program (SPEC-1..10, Workstream G)

_Link: [./tracks/canonical_data_log_20260808/](./tracks/canonical_data_log_20260808/) · in_progress · local implementation GO, network rollout NO-GO_

SPEC-1..8 owner-approved; HARD-1..6, Waves 1–3 (+3R remediation) landed. Open and
agent-doable: Wave 4 serial integration incl. the six D-CL20 bypass counters
(CDL-W4), seal `VerifiedEnvelopeMeta` (CDL-R1), production enrolment/issuer views
(CDL-R2), scope-sensitive WS test (CDL-R6), SPEC-10 draft (CDL-SPEC10). Needs a
ratification: SPEC-9 availability floor, provisional CBOR numbering, rotation-fork
resolver, WIT s64, dual-emit cut-over. Network rollout, relay seeding, inbound P2P and
mounting `/ws/canonical` stay owner-gated. The old "is_available() must stay false"
constraint now applies to the canonical path only (F1/F2 made the legacy row path real).

### [ ] hexon_unification — one canonical .hexon + portable petal snapshot

_Link: [./tracks/hexon_unification_20260716/](./tracks/hexon_unification_20260716/) · pending · 0 / 37_

Not started: fe-format is still only a dev-dependency of fe-hexon, `crate_id` and
`.fecrate` remain, no `PetalSnapshot`, `install_crate` records no petal association.
Fully ratified (user directives 2026-07-16) and agent-doable in order HU-P1→P6.

### [~] p2p_asset_streaming — fine-grained hexon transfer + scene-driven residency

_Link: [./tracks/p2p_asset_streaming_20260718/](./tracks/p2p_asset_streaming_20260718/) · in_progress · 8 done / 24 open · D-73…D-78 ratified_

Done: Phase-1 `RelayConfig` + relay health with the EOL warning (320ebfe). Partial:
soft residency allowance (no ledger), settings surface (no persistence, two knobs).
Open: evidence pack (PAS-P0), headless relay verification (PAS-P1V), chunk transport
(`handle_request_chunk` still emits `ChunkFailed`, PAS-P3), residency ledger + lazy
tiles (PAS-P4), settings persistence (PAS-P4B), `PartialHexonFetch` (PAS-P5), close-out.

### [~] iroh_1_0_upgrade — 0.35 → 1.x behind the VerseReplicator seam

_Link: [./tracks/iroh_1_0_upgrade_20260711/](./tracks/iroh_1_0_upgrade_20260711/) · in_progress · **HARD DEADLINE 2026-12-31** (n0 hosted-relay 0.35 EOL)_

Port landed in the working tree 2026-10-11 (uncommitted): iroh 1.3 / iroh-docs 0.101 /
iroh-gossip 0.101 / iroh-blobs 0.103.1 + noq-proto 1.3 (BBRv3), 0.35 store migration,
legacy bootstrap-JSON compat, and the home-relay health watcher (SYNC-RELAYWATCH).
`cargo check --workspace --tests` and workspace clippy `-D warnings` are clean. Open: the
orchestrator's test sweep (incl. the 0.35-fixture migration test and the harness
loopback scenarios), the relay-assisted connection against a self-hosted 1.x relay, and
the flag-day rollout. Timing/posture decisions D-22/D-23 remain open in the register.

### [ ] crate_consolidation_r2 — user-directed crate merges

_Link: [./tracks/crate_consolidation_r2_20260718/](./tracks/crate_consolidation_r2_20260718/) · pending · 0 / 15_

Nothing executed. Phase 1 evidence (consumer maps, closure sizes) is agent-doable
(CC-P1); merges G-1/G-2/G-3 wait on gate decisions (CC-GATES). Spot evidence:
fe-plugin-test has zero consumers; fe-query is consumed by fe-api AND fe-database.

### [~] oss_release — open-source release checklist

_Link: [./tracks/oss_release_20260717/](./tracks/oss_release_20260717/) · in_progress · 17 done / 15 open · Apache-2.0 ratified_

Licence files, THIRD-PARTY-LICENSES (BUSL-1.1), README/SECURITY claims, crate
descriptions done. 2026-10-10: README Status corrected, SECURITY.md stale "permissive
sync gate" bullet replaced, root AGENTS.md SQLite line fixed. Open: cargo-deny CI step
(OSS-1.3), placeholder-signature register now 20 sites (OSS-1.5), GHCR URL in
BUILDING.md (OSS-2.4), fe-sdk docs, publish dry-runs, root `.glb` strays, pre-push
preflight; user-gated: vulnerability reporting toggle, publishing gate, visibility.

### [~] release_ci — cross-compilation pipeline + Docker image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/) · in_progress · 9 done / 1 open_

Only task 2.7 remains: the first `v*` tag run (user-gated). Its artifact checklist must
be re-derived after macOS removal (6aaeabd): Linux x64, Windows x64, Windows ARM64,
musl relay ×2, GHCR image. Latest main CI 38095577669 is fully green.

### [~] cross_platform_desktop — Linux / Windows-ARM64 builds

_Link: [./tracks/cross_platform_desktop_20260429/](./tracks/cross_platform_desktop_20260429/) · in_progress · 12 done / 2 open_

Linux compile green (29687337676, 38095577669); macOS compiled once then descoped by the
owner (6aaeabd). Open: Windows ARM64 (same tag run as release_ci 2.7), Linux
`--workspace` tests (tied to D-66 CI scope), manual launch smoke test.

---

## Foundry-candidate specs (separate hexon-foundry project)

2026-07-14 FOUNDRY-CANDIDATE verdict: reference only, do **not** implement in the
analytics core; gated on **[D-12 (open/closed line)](./tracks/outstanding_decisions_20260715/spec.md)**.
Their signing / HashSeq / log-first premises are now realized in-core by
`canonical_data_log`; legacy op-log rows stay permanently unsigned (D-CL24(5)).

- [ ] **hexon_delta_format** — replayable op-log hexons — spec_only —
  [./tracks/hexon_delta_format_20260710/](./tracks/hexon_delta_format_20260710/)
- [ ] **hexon_p2p_bucket** — 3D visual IPFS; partial FR-4 exists in core (asset GET by
  hash, GLB upload); `fe-network/src/iroh_blobs.rs` still a stub — spec_only —
  [./tracks/hexon_p2p_bucket_20260710/](./tracks/hexon_p2p_bucket_20260710/)
- [ ] **verse_services** — accelerator-only per-verse services — spec_only —
  [./tracks/verse_services_20260711/](./tracks/verse_services_20260711/)

---

## Backlog / deferred

- [~] **thorns_shields** — security hardening; fuzz targets + threat model landed
  (d5cc361). Open: unwrap/expect census script + rewritten audit (TS-FR1), fuzz CI job
  (TS-FR3); two webview v2 gaps await a v1/v2 call (TS-FR2) —
  [./tracks/thorns_shields_20260321/](./tracks/thorns_shields_20260321/)
- [ ] **inspector_settings** — blocked_on_ratification: FR-1/FR-2 shipped; FR-3
  (hierarchy inspection) and FR-4 (Access tab) have zero code and wait on D-32/D-33 —
  [./tracks/inspector_settings_20260419/](./tracks/inspector_settings_20260419/)
- [ ] **sso_federation** — enterprise OIDC/SSO, spec_only P2, never scheduled —
  [./tracks/sso_federation_20260429/](./tracks/sso_federation_20260429/)
- [ ] **light_box** — default lighting rig; off_strategy (zero code) —
  [./tracks/light_box_20260402/](./tracks/light_box_20260402/)
- [ ] **profile_manager** — identity/profile UI + P2P sync; off_strategy (zero code) —
  [./tracks/profile_manager_20260419/](./tracks/profile_manager_20260419/)

---

## Meta tracks

- [~] **ux_qa_review** — review-only harness for the **user-owned** UX review (never
  `done` by design). Batch #2 findings are fixed in code pending the user's re-check;
  open findings (undo, modifier conventions, onboarding framing) wait on D-29 —
  [./tracks/ux_qa_review_20260714/](./tracks/ux_qa_review_20260714/)
- [ ] **outstanding_decisions_20260715** — blocked_on_ratification —
  **[the ratification register](./tracks/outstanding_decisions_20260715/spec.md)**: 72
  entries, 4 resolved (D-69..D-72). The 2026-10-10 audit appendix marks 10 as overtaken
  by later decisions/landed work (close on confirmation), 3 as user actions, and 55
  awaiting ratification. **Time-critical: D-22/D-23** (iroh relay EOL 2026-12-31) —
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
- **2026-07-24 — pruning-pass batch** (5 folders, each with its own
  [`_archive/<id>/retro.md`](./tracks/_archive/)): runtime_instance_guardrails
  (done — GPU-OOM root cause fixed @ 46e19de, FR-6 delivered cross-track @
  320ebfe), terrain_editor_overhaul (done — FR-1..6 @ 320ebfe + regression
  fixes @ f223bfa; viewport point-selection defect TRANSFERRED to
  ui_shell_architecture_20260724), tool_inspector_ux (superseded — Phase 1
  delivered @ f223bfa; sidebar→tooltip, Phases 2-6 subsumed),
  ux_interaction_hardening (superseded — FR-5 shipped @ 320ebfe; FR-1..4
  absorbed), drag_drop_placement (superseded — spec-only on a void base)
