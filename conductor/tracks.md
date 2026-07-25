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

**Open tracks: 33.**

> **Validation pass 2026-07-19** (5-worker swarm, code-reading + git evidence only —
> build env memory-blocked, no test execution). Every track's `metadata.json` re-validated
> against committed git + a dated `VALIDATED 2026-07-19` note appended. **Status corrections:**
> `terrain_editor_overhaul` `pending`→`in_progress` (FR-1..6 landed @ `320ebfe`; 4 in-app
> regression fixes uncommitted), `p2p_asset_streaming` `pending`→`in_progress` (settings/
> ledger/relay @ `320ebfe`; FR-3 chunk transfer still stubbed), `oss_release` `pending`→
> `in_progress` (D-69/D-70 ratified @ `b555082`), `thorns_shields` `pending`→`in_progress`
> (fuzz targets + threat-model docs @ `d5cc361`). New track `tool_inspector_ux_20260719`
> added (Phase 1 local-only). **0 archived** — no track met the `done` bar (all carry
> remaining acceptance items or uncommitted/unverified work). Nearest-to-done:
> `runtime_instance_guardrails` (crash fix committed + verified @ `46e19de`; needs retro +
> FR-6 live-run sign-off to archive).

> **Pruning pass 2026-07-24** (evidence-based board assessment + user live in-app test).
> **5 folders archived** to [`./tracks/_archive/`](./tracks/_archive/), each with a retro:
> `runtime_instance_guardrails` (done — GPU-OOM root cause fixed @ `46e19de`, FR-6 render
> horizon delivered cross-track @ `320ebfe`, no recurrence over two live runs),
> `terrain_editor_overhaul` (done — FR-1..6 @ `320ebfe` + the four regression fixes @
> `f223bfa`; failed 2026-07-24 viewport path-point-selection acceptance TRANSFERRED to
> `ui_shell_architecture_20260724`), `tool_inspector_ux` (superseded — Phase 1 + FR-7
> delivered @ `f223bfa`; always-open sidebar replaced by a tooltip model, Phases 2-6
> subsumed by `ui_shell_architecture_20260724`), `ux_interaction_hardening` (superseded —
> FR-5 camera hardening delivered @ `320ebfe`; FR-1..4 absorbed into
> `ui_shell_architecture_20260724`), `drag_drop_placement` (superseded — spec-only on a
> void base, re-spec routes through the shell track). **Stale CI-failure notes struck**:
> the release_ci + cross_platform_desktop "Lint failing" validation notes are superseded —
> CI fully green @ `f5d9673` (run 30055712210; lint fixes `b75ad2a`/`3dae0ee`/`f5d9673`);
> release_ci remaining = task 2.7 (live `v*` tag e2e) only. **pen_curve_tool status
> corrected** pending→in_progress (Phases 1-6 implemented; see its entry). **2026-07-24
> in-app findings logged** to
> [ux_qa_review findings](./tracks/ux_qa_review_20260714/findings.md) (viewport
> point-selection gap, gardener_ui_system terrain crash, sidebar→tooltip).

---

## P0 — Spatial Builder Program (user planning grill 2026-07-25)

Six tracks from the 2026-07-25 planning grilling (12 decision-forcing questions,
3 rounds). Anchor decision record:
[./decisions/spatial-builder-program-20260725.md](./decisions/spatial-builder-program-20260725.md)
(product thesis, register D-A1..A11, shared NFR pool N-1..N-10, and the
**conflict-free file partition** for `/slice`). **Thesis:** a Cities:Skylines-
simple spatial *builder* where every artifact is a persistent, addressable,
real-world-scale data endpoint with a full read/write API — no modes, one shell,
tool/selection-driven context; the analyst is served by the API/query layer, the
civil engineer by the to-scale rule. **Wave DAG:** Wave 0 = `node_lifecycle_
addressing` ‖ `shell_ux_sidebar` (different crates); Wave 1 = `stamped_asset_
nodes` ‖ `sculpt_earthwork_regions` ‖ `contextual_controls` ‖
`endpoint_api_surface` (all depend on the spine). **`/slice` deferred by user
2026-07-25** ("write specs, don't slice yet"); every track is `pending`, NOT
started.

### [ ] node_lifecycle_addressing — SPINE: tombstone delete + cascade + re-flow, stable addresses, lazy promotion

_Link: [./tracks/node_lifecycle_addressing_20260725/](./tracks/node_lifecycle_addressing_20260725/) · pending · P0 PROGRAM SPINE (Wave 0) · blocks all four Wave-1 tracks_

The data-layer foundation (D-A4/A5/A6/A7). Sync-safe delete (tombstone + cascade
+ path-reflow event, never a raw drop — fixes the "empty husk" bug), a stable
per-node addressing scheme (substrate for the read/write API + stamp-as-node),
and a lazy node-promotion primitive so tens-of-thousands of addressable stamp
nodes stay smooth. Owns `fe-entity-store` / `fe-database` / `fe-policy` /
`fe-sync` exclusively — zero file overlap, Wave 0 parallel with the shell track.

### [ ] shell_ux_sidebar — Settings & Maps → sidebar sections, user-sticky left sidebar

_Link: [./tracks/shell_ux_sidebar_20260725/](./tracks/shell_ux_sidebar_20260725/) · pending · P1 UX (Wave 0) · continuation of ui_shell_architecture_20260724_

Finishes "no modes, one shell" (D-A1/A10/A11): migrate the Settings + Maps
(hexon manager) modals into tool-contextual right-sidebar sections like the
other tools, and make the left sidebar user-driven sticky (open until closed).
**Owns the fe-ui shell seam** (`ui_shell/{mod,left_sidebar,right_sidebar}.rs`,
`panels/mod.rs`, `dialogs/{settings,hexon_manager}.rs`) — the section-registry
owner; Wave-1 tracks route new-section registration through it. The program's
cheapest, highest-daily-satisfaction track.

### [ ] stamped_asset_nodes — stamps become curve-following addressable nodes with scale/rotate overrides + instancing

_Link: [./tracks/stamped_asset_nodes_20260725/](./tracks/stamped_asset_nodes_20260725/) · pending · P1 BUILDER UX (Wave 1) · depends_on node_lifecycle_addressing_

In-app QA asks #1/#2 (D-A5/A6). Stamps follow the actual curve (not the
flattened polyline), each is an individually addressable node with per-stamp
scale + rotation overrides and free-translate disabled (position path-derived),
backed by GPU instancing + a spatial pick index for the tens-of-thousands
ceiling. Owns `fe-terrain mesh/{curve,track,marker}` + stamp materializer,
`fe-renderer {loader,ingester,viewport}` + new `instancing.rs`, and the fe-ui
path-tools section content (not `right_sidebar.rs`).

### [ ] sculpt_earthwork_regions — brush/shape sculpt tool + reportable cut/fill volume region nodes

_Link: [./tracks/sculpt_earthwork_regions_20260725/](./tracks/sculpt_earthwork_regions_20260725/) · pending · P1 BIM/BUILDER (Wave 1) · depends_on node_lifecycle_addressing_

In-app QA ask #3 (D-A8 + R1Q4). A tactile sculpt tool (brush + defined shapes)
to select the affected area, and earthwork edits that become persistent
addressable "modification region" nodes (footprint + material + real-unit
cut/fill volume via the scale authority) baked into the surface and reportable/
queryable like any node. First BIM-grade terraforming primitive; layered strata
is a later phase. Owns `fe-terrain terrain_proposal + mesh/{terrain,interp,skirt}
+ layers` + new sculpt module, `fe-renderer {terrain_overlay,terrain_height}` +
brush overlay, fe-ui terrain-tools section content + new sculpt panel.

### [ ] contextual_controls — object-aware right-click menu with real Delete + comprehensive verbs

_Link: [./tracks/contextual_controls_20260725/](./tracks/contextual_controls_20260725/) · pending · P1 BUILDER UX (Wave 1) · depends_on node_lifecycle_addressing · coordinates endpoint_api_surface_

In-app QA ask #2 (D-A9). An object-aware context menu (verbs depend on the hit
target) replacing the thin create-node/place-asset menu; ships the missing
Delete wired to the spine's tombstone+cascade (fixes "no way to remove a node")
plus duplicate/rename/edit-props/promote-to-node/copy-API-string/report. Menu-
first; radial deferred. Owns `fe-ui ui_shell/modal.rs` + `dialogs/{context_menu,
node_options}.rs`.

### [ ] endpoint_api_surface — every object a read/write API endpoint (+ MCP drive, egress seam)

_Link: [./tracks/endpoint_api_surface_20260725/](./tracks/endpoint_api_surface_20260725/) · pending · P1 DATA/ANALYTICS (Wave 1) · depends_on node_lifecycle_addressing · coordinates contextual_controls_

The "API on every endpoint" idea (D-A4/A3). Every object is a stable public
endpoint with full read + write (GET data / mutate via the spine's sync-safe
ops, authorized by fe-policy), exposed over REST + the existing MCP tools so an
agent/external tool can drive the scene; also emits the copy-API-string / report
seam the analyst persona and the context menu (T4) consume, and makes stamp +
region nodes first-class in fe-query. Owns `fe-api/*` + `fe-query/*` +
`fe-renderer/addressing.rs` — the cleanest-isolated Wave-1 track.

---

## P0 — UI shell architecture (user-directed 2026-07-24)

### [ ] ui_shell_architecture — pointer/tab/modal manager decoupling + right-sidebar tool inspector + P0 crash/selection fixes

_Link: [./tracks/ui_shell_architecture_20260724/](./tracks/ui_shell_architecture_20260724/) · pending · P0 UX + P0 STABILITY (user live-test findings 2026-07-24) · supersedes tool_inspector_ux + ux_interaction_hardening (both archived 2026-07-24)_

User-directed re-coupling of the UI rendering system: **1 pointer/cursor-ops
manager** (consolidating the click-claim/drag surfaces), **1 tab-interaction
manager per core area** (topbar / left sidebar / right sidebar), **1 modal
manager** (tooltips + transient overlays, panic-guarded panels); floating tool
windows (terrain/path/tools) migrate into the **right-sidebar inspector**
revealed on toggle; the always-open tool-descriptions sidebar becomes
**tooltips**. **Phase 0 (P0, ratification-independent, lands first):**
(a) gardener_ui_system terrain crash (panic poisons the whole egui pass, exit
101 — leading hypothesis: proposals-only `terrain_json` on a config-less
petal); (b) viewport path-point selection — ROOT CAUSE CONFIRMED: the
`viewport_pick.rs` track-open gate needs `path_state.tracks`, which only loads
while the Data window's Paths tab renders; fix = eager-load on petal change.
Absorbs ux_interaction_hardening FR-1..4 (incl. FR-3 gimbal damping); carries
terrain_editor_overhaul's transferred viewport-selection acceptance; unblocks
pen_curve_tool in-app verification. 5 open questions await ratification
(spec §Open questions); road_builder_ux + inspector_settings coordinate.

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

### [~] pen_curve_tool — Illustrator-style Pen curve tool (bezier anchors + corner settings)

_Link: [./tracks/pen_curve_tool_20260722/](./tracks/pen_curve_tool_20260722/) · in_progress (Phases 1-6 implemented; Phase 7 + in-app re-verify pending) · P1 UX (user-directed 2026-07-22) · depends_on terrain_editor_overhaul, tool_inspector_ux (both archived 2026-07-24 — satisfied deps)_

Illustrator-style cubic-bezier Pen tool: per-anchor in/out handles +
Corner/Smooth/Symmetric classification, click=corner / press-drag=smooth /
Alt-drag=corner-break, and a per-anchor "corner settings" smoothness slider
that auto-derives collinear handles. Reuses the existing de Casteljau
tessellation (`node_manager/curve.rs`); geometry stays in raw petal-local
meters (no `world_scale`); legacy straight polylines render byte-identically.
**Phases 1-6 implemented**: Phases 1-2 pushed @ `01c175d` (CI green); Phases
3-6 landed 2026-07-24 with a 14-finding review-fix pass. **In-app test
2026-07-24**: cannot select/manipulate existing points — suspected app-wide
viewport selection routing (owned by `ui_shell_architecture_20260724`, not
pen-specific) — plus an unrelated `gardener_ui_system` terrain crash (same
track owns it). Remaining: Phase 7 close-out + in-app re-verify.

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

### [~] api_mcp_integration_tests — reusable API harness + api/mcp integration suites

_Link: [./tracks/api_mcp_integration_tests_20260717/](./tracks/api_mcp_integration_tests_20260717/) · in_progress · P1 ENABLING_

Reusable integration-test harness (in-mem SurrealDB + **real** fe-api router +
tower `oneshot`), consumed by `api_integration.rs` (query-guard limits,
SQL-injection attempts, RBAC negatives, GIS round-trips, egress CSV) and
`mcp_integration.rs` (MCP tool round-trips + KNOWN-WEAK authz markers for the
create_node / create_petal / update_transform gaps, cross-referencing
mcp_scene_primitives). Gives `fractalengine-test-harness` its first downstream
consumer (closes the styleguide SG-08 gap). FR-5 appended 2026-07-18 (user
ask): comprehensive expansion — remaining endpoint families (WS/realtime if
present, hexon/tileset, IoT ingest+export, share-URL mint→redeem, auth token
lifecycle), cross-thread DB↔API↔sync scenarios via the harness, MCP
negative/fuzz coverage. Acceptance: both suites green in the workspace sweep.

### [ ] crate_consolidation_r2 — user-directed merges with audit counter-evidence embedded

_Link: [./tracks/crate_consolidation_r2_20260718/](./tracks/crate_consolidation_r2_20260718/) · pending · P1 PLATFORM (user directive 2026-07-18)_

User directive 2026-07-18: the 22-crate workspace is itself an anti-pattern —
crate count is a cost, sparse crates should merge. Re-opens exactly three
candidates against the [2026-07-17 audit's](./decisions/crate-consolidation-20260717.md)
KEEP verdicts, each with the counter-evidence embedded and an accept/defer
gate: G-1 fe-plugin-test→fe-plugin as a `test-utils` feature (F8 cost: OSS
plugin authors pull wasmtime+bevy to test); G-2 fe-hexon-registry→fe-hexon as
a feature-gated bin (F7 cost: docker engine-closure + foundry extraction path;
the fe-format merge question routes as a gate INSIDE hexon_unification's
scope — no parallel workstream); G-3 fe-query→fe-api evidence-first
(preliminary grep: fe-database ALSO consumes fe-query ⇒ naive merge inverts
layering — consumer map is the gate artifact). fe-network untouchable
(D-71 RESOLVED-KEEP: P2P is the differentiator). Merges are structure-only.

### [~] release_ci — Cross-Compilation Pipeline + Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/) · in_progress · P2 ENABLING_

Phase 1 done (`build-artifacts.yml` 3-OS PR check, rust-cache blessed over sccache,
Linux glib apt deps fixed after run 29390918844). Phase 2 tasks 2.1–2.6 done
(`release.yml` 8-target matrix, lipo universal binary, SHA256SUMS, gh-release publish,
GHCR docker job, `Cross.toml`). Remaining: task 2.7 end-to-end tag verification
(requires a live GitHub run). Docker relay image = shipping vehicle for the analytics
backend.

### [~] oss_release — open-source release checklist

_Link: [./tracks/oss_release_20260717/](./tracks/oss_release_20260717/) · in_progress (D-69/D-70 ratified @ b555082; Apache-2.0 SINGLE, scaffolding + CI lint gate landed; many items BLOCKED-ON-USER) · P1 ENABLING · D-69 + D-70 RATIFIED 2026-07-17 (Apache-2.0; conductor/ public)_

The full pre-public-push gate, sourced from the 2026-07-17 OSS-release audit
(REL-01..11) + crate-consolidation audit ([decision record](./decisions/crate-consolidation-20260717.md)).
License **ratified 2026-07-17: Apache-2.0 single** (LICENSE-APACHE, workspace
`license = "Apache-2.0"`, THIRD-PARTY-LICENSES + deny.toml, SECURITY.md draft);
conductor/ ships public with its README framing. Checklist: placeholder ed25519 signature
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

### [~] p2p_asset_streaming — fine-grained hexon transfer + scene-driven residency

_Link: [./tracks/p2p_asset_streaming_20260718/](./tracks/p2p_asset_streaming_20260718/) · in_progress (FR-1 relay + FR-7/D-78 settings + FR-4 soft residency landed @ 320ebfe; **FR-3 chunk transfer still STUBBED** — sync_thread.rs:767-793 emits ChunkFailed; FR-5/FR-6 unbuilt) · P1 PLATFORM (user-driven 2026-07-18; P2P = main differentiator per D-71) · decision round **D-73…D-78 RATIFIED 2026-07-18**_

User ask 2026-07-18: stream parts of multiple hexons based on what's actually
in the scene (render distance + entity caps) instead of forcing full downloads.
Decision round in [decisions/p2p-streaming-20260718.md](./decisions/p2p-streaming-20260718.md)
**RATIFIED 2026-07-18** — user locked the staged defaults ("no new primitive;
leverage pointers to multiple hexons"): A4 HashSeq of size-tuned chunk bundles
reusing the existing `package_chunked`/RequestChunk scaffolding (protocol exists
in types + UI wiring, sync-thread handlers are stubs); D-74 residency ledger
driving an **ephemeral pointer-set** `{(hexon_uri, bundle_hash), …}` recomputed
per frame — **no materialized scene artifact**, so no staleness (content-addressed
blobs make the pointer-set free + cross-hexon-dedup free); C1 registry
member-granular routes behind one `PartialHexonFetch` trait; D2 Merkle-extended
signature root; E2 build-on-0.35 behind a transport trait; **D-78 new: a
dedicated application-settings surface** (FR-7 — no settings surface exists
today; resurrects the archived render_distance_lod `AppSettings`). The
Ratification section answers P2P mechanics, renderer effect, and 2-layer limit
enforcement (hard watchdog backstops + soft distance-ranked ledger; promote
DIAG-15M census to a shared resource for the closed-loop GPU-byte horizon).
Phases 0–1 (evidence pack + relay loud-fail hardening for the 2026-12-31 EOL) +
Phase 4b (settings) are decision-independent and executable now. FR-4 implements
runtime_instance_guardrails' specced-but-unbuilt FR-6 render horizon; integrity
fields route through hexon_unification.

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
- [~] **thorns_shields** — security hardening + pre-launch docs; in_progress: fuzz targets
  (ed25519/jwt) + threat-model/security-checklist/unwrap-audit docs landed @ d5cc361, but
  `scripts/audit.sh` never committed, no fuzz CI job, 2 v2 gaps (DNS-rebinding, oversized-content)
  open — [./tracks/thorns_shields_20260321/](./tracks/thorns_shields_20260321/)
- [ ] **sso_federation** — enterprise OIDC/SSO, spec_only P2 —
  [./tracks/sso_federation_20260429/](./tracks/sso_federation_20260429/)
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
- **2026-07-24 — pruning-pass batch** (5 folders, each with its own
  [`_archive/<id>/retro.md`](./tracks/_archive/)): runtime_instance_guardrails
  (done — GPU-OOM root cause fixed @ 46e19de, FR-6 delivered cross-track @
  320ebfe), terrain_editor_overhaul (done — FR-1..6 @ 320ebfe + regression
  fixes @ f223bfa; viewport point-selection defect TRANSFERRED to
  ui_shell_architecture_20260724), tool_inspector_ux (superseded — Phase 1
  delivered @ f223bfa; sidebar→tooltip, Phases 2-6 subsumed),
  ux_interaction_hardening (superseded — FR-5 shipped @ 320ebfe; FR-1..4
  absorbed), drag_drop_placement (superseded — spec-only on a void base)
