---
type: Decision Record
title: Spatial Builder Program — Product Thesis, Decision Register, and Concurrent-Track Partition
timestamp: 2026-07-25T00:00:00Z
status: ratified
---

# Spatial Builder Program (2026-07-25)

Anchor decision record for the six-track program spun out of the 2026-07-25
planning grilling (12 decision-forcing questions across three rounds). Every
track spec under `conductor/tracks/*_20260725/` cross-links this file for the
thesis, the shared NFR pool, and the conflict-free file partition. This file is
the single source of truth for those; specs do not repeat them.

## Product thesis

> **FractalEngine is a Cities:Skylines-simple spatial *builder* where every
> artifact — every stamped duck, every earthwork cut, every path — is a
> persistent, addressable, real-world-scale data endpoint with a full
> read/write API.**
>
> The builder UX stays simple and tactile. The **analyst** is served not by a
> separate mode but by the API/query layer; the **civil engineer** by the
> real-world to-scale authority. One shell, tool- and selection-driven context.
> Nobody gets a mode — everybody gets the data layer.

## Decision register (ratified 2026-07-25)

| # | Decision | Origin |
|---|---|---|
| **D-A1** | **No modes.** One shell; the selected *tool* and *object* drive which controls appear. Reverses the earlier "unified/mode-switched" framing. | Grill R2Q2 |
| **D-A2** | **Design north star = Cities:Skylines-tactile builder** — viewport-first, contextual controls, minimal chrome. "Simple but powerful." | Grill R2Q1 |
| **D-A3** | **Persona reconciliation:** builder-first UX; the analyst is served by the API/query layer, the civil engineer by the real-world to-scale rule. No persona gets its own mode. | Grill R1Q2 + R3Q2 |
| **D-A4** | Every object is an addressable endpoint with a **full read + write API** (wires into the existing MCP tools → external/agent drive). | Grill R3Q1 |
| **D-A5** | **Stamped assets = full persistent nodes**; position is locked to the path and **follows the curve** (not the flattened polyline); scale + rotation are per-node overridable. | Grill R1Q3 + R3Q2 |
| **D-A6** | **Scale target: tens of thousands+** individually addressable nodes must stay smooth → GPU instancing + LOD + **lazy node promotion** (full node data materialized only when the instance is addressed/edited). | Grill R3Q3 |
| **D-A7** | **Delete = tombstone** (sync-safe under P2P/HLC merge, never a raw row drop) **+ cascade** to children (with confirm) **+ path re-flow** on stamp delete. Fixes the "empty husk" bug: clearing properties is NOT delete. | Grill R2Q3 |
| **D-A8** | **Earthwork edit = a persistent, addressable "modification region" node** (shape + volume + material), editable as an object, baked into the surface, reportable/queryable like any node. Non-destructive-adjacent but baked, not a live adjustment stack. | Grill R3Q4 |
| **D-A9** | **Right-click context menu first** (delete / duplicate / promote-to-node / copy-API-string / report / query); a game-style radial menu is deferred to a later polish track. | Grill R3Q2 |
| **D-A10** | **Settings + Maps are not modes and not modals** — they become tool-contextual panels in the one right-sidebar rail, like every other tool surface. | User ask #4 |
| **D-A11** | **Sticky sidebar** — the left sidebar stays open until the user explicitly closes it; kill the per-frame auto-collapse. | User ask #5 |

## Shared NFR pool (every track inherits unless it overrides)

- **N-1 — Real-world scale authority is preserved (the civil-engineer rule).**
  All geometry math is in raw petal-local **meters**; `world_scale` appears only
  in display formatting via the one conversion seam (`fe-ui panels/widgets.rs`).
  Volumes/quantities are computed in real units through the scale authority
  (`fe-terrain/src/scale.rs`). (Precedent: the 2026-07-19 ribbon regression.)
- **N-2 — No modes (D-A1).** New surfaces are tool/selection-contextual panels
  in the one shell; no global mode switch is introduced.
- **N-3 — Two-authority selection split is SACRED (`ui_ux.md` §5).**
  `NodeManager.selected` (viewport) and `PathEditorState.editing_track_id`
  (Paths tab) remain distinct storage; surfaces coordinate via queued
  `UiAction`s, never by merging.
- **N-4 — Sync-safe mutation.** Every destructive op (delete, cascade) is a
  tombstone/op-log entry that survives P2P/HLC merge (auth never LWW; log-first
  WAL). No raw row drops. (Ratified hexon-p2p-commons D1-D6.)
- **N-5 — No authorization in UI; no `block_on` in Bevy systems.** UI receives
  pre-authorized data; async stays on the channel seams; policy is enforced in
  `fe-policy`/`fe-database`.
- **N-6 — Quality gates.** `cargo clippy -- -D warnings` on latest stable
  (CI's floating `rust-toolchain@stable` — 2026-07-23 drift playbook),
  `cargo fmt --check`, workspace tests; **single integrated sweep at the end**,
  not per-fix loops.
- **N-7 — Docs convention.** Terse one-line doc comments; rationale and module
  topology live in directory `AGENTS.md` files, not inline blocks.
- **N-8 — `ui_ux.md` pre-merge checklist** applies to every touched UI surface
  (calm chrome §1, real units §2, no silent failure §6, mode-gated overlays §7,
  path/map terminology §9).
- **N-9 — Data/render split for scale (D-A6).** The data model may be full-node
  per stamp, but rendering MUST instance and picking MUST be spatially indexed
  at the tens-of-thousands ceiling; node data is lazily materialized.
- **N-10 — Report-on-everything (D-A4).** Every new object type (promoted stamp
  node, earthwork region node) must be queryable and egress-able through
  `fe-query`/`fe-api` — a new object that cannot be reported on is incomplete.

## Program → tracks and wave DAG

| Track ID | Title | Wave | depends_on |
|---|---|---|---|
| `node_lifecycle_addressing_20260725` | Node Lifecycle & Addressing (spine) | 0 | — |
| `shell_ux_sidebar_20260725` | Shell UX — Modals→Sidebar + Sticky Sidebar | 0 | — |
| `stamped_asset_nodes_20260725` | Stamped-Asset Nodes + Curve-Follow + Instancing | 1 | node_lifecycle_addressing |
| `sculpt_earthwork_regions_20260725` | Sculpt & Earthwork Region Nodes | 1 | node_lifecycle_addressing |
| `contextual_controls_20260725` | Contextual Right-Click Controls | 1 | node_lifecycle_addressing |
| `endpoint_api_surface_20260725` | Per-Endpoint Read/Write API Surface | 1 | node_lifecycle_addressing |

- **Wave 0 (parallel, zero file overlap):** `node_lifecycle_addressing` (data
  core) ‖ `shell_ux_sidebar` (fe-ui shell seam). Different crates entirely.
- **Wave 1 (parallel, after the spine):** `stamped_asset_nodes` ‖
  `sculpt_earthwork_regions` ‖ `contextual_controls` ‖ `endpoint_api_surface`.

## Conflict-free file partition (the pre-slice homework)

Grounded in the real 2026-07-25 module layout **and re-validated by the
2026-07-25 `/slice` re-grill (fresh greps against the tree).** No two tracks own
the same file in the same wave. This table is authoritative; the corrections
subsection below records the provenance of the additions.

| Owner | Owned files (writes) |
|---|---|
| **T1** (Wave 0) | `fe-entity-store`, `fe-database`, `fe-policy`, `fe-sync`, **`fe-runtime/src/messages.rs`** — data core + tombstone op-log + delete authz + the whole-program `DbCommand`/lifecycle-event **vocabulary** (Wave-1 tracks only construct/match these). *(The `DbCommand` **struct** in `fe-terrain/src/lib.rs` is unrelated — not this.)* |
| **T6** (Wave 0) | `fe-ui/src/ui_shell/{mod,left_sidebar,right_sidebar,topbar}.rs`, `panels/mod.rs`, `dialogs/{settings,hexon_manager}.rs`, **`actions/mod.rs`** (`UiAction` enum + dispatch), **`plugin.rs`** (resource/system reg + `gardener_console` signature). Shell seam owner **+ the Wave-0 fe-ui registration scaffold**; also creates the empty Wave-1 handler stubs the tracks below fill. |
| **T2** (Wave 1) | `fe-terrain/src/mesh/{curve,track,marker}.rs`; `fe-renderer/src/{loader,ingester,viewport}.rs` + new `instancing.rs` + the **`fe-renderer/src/lib.rs` `mod instancing;`** line (the only new fe-renderer module in Wave 1); `fe-ui/src/panels/tool_panel.rs` (PathTools content); fills `fe-ui/src/actions/{asset,path}.rs` stubs. |
| **T3** (Wave 1) | `fe-terrain/src/{terrain_proposal.rs, mesh/{terrain,interp,skirt}.rs, layers/*}` + new `sculpt` module + the **`fe-terrain/src/lib.rs` `mod sculpt;`** line; `fe-renderer/src/{terrain_overlay,terrain_height}.rs` (**brush overlay folded IN — no new fe-renderer module**); `fe-ui/src/panels/{terrain_tools_panel,proposal_report_panel}.rs` (**sculpt UI folded into TerrainTools — no new `RightSidebarSection`**); fills `fe-ui/src/actions/terrain_proposal.rs` stub. |
| **T4** (Wave 1) | `fe-ui/src/ui_shell/modal.rs`, `dialogs/{context_menu,node_options}.rs`; fills `fe-ui/src/actions/{node,node_props}.rs` handler bodies. **Does NOT touch** `actions/mod.rs`/`plugin.rs`/`right_sidebar.rs`/`panels/mod.rs`. |
| **T5** (Wave 1) | `fe-api/*`, `fe-query/*`, `fe-renderer/src/addressing.rs` (already declared → no `lib.rs` edit). Consumes T1's op vocabulary + `fe://` address via channel seams; never edits T1/T2/T3 files. |

**Single-owner central files (the collision-proof seams).** Each of these is
edited by exactly one owner per wave, by construction:
`right_sidebar.rs` (`RightSidebarSection` registry) → **T6**;
`actions/mod.rs` (`UiAction` + dispatch) → **T6 scaffold**;
`plugin.rs` (resource/system reg + console signature) → **T6 scaffold**;
`panels/mod.rs` (module reg + signature) → **T6**;
`fe-runtime/src/messages.rs` (`DbCommand`) → **T1**;
`fe-renderer/src/lib.rs` (`mod instancing;`) → **T2**;
`fe-terrain/src/lib.rs` (`mod sculpt;`) → **T3**.
The fold rules (T3 brush → `terrain_overlay.rs`, T3 sculpt → TerrainTools) keep
the fe-renderer and section registries single-owner. Any *brand-new* fe-ui
section a Wave-1 track later needs is registered through **T6** (owner), never
edited concurrently. This is the single rule the `/slice` run enforces.

## Slice-time partition corrections (2026-07-25, grep-validated)

The `/slice` re-grill (fresh greps, not the once-over module scan) found the
table above partitioned by *feature file* and **omitted the files that
physically hold the mutations** — the classic boundary gap. Five additions,
each resolved to a single owner per wave so no two same-wave slices collide:

| Shared file | Why it's shared | Owner (resolution) |
|---|---|---|
| `fe-runtime/src/messages.rs` (`DbCommand` enum) | every new op = a new variant | **T1** — defines *all* program op/event variants in Wave 0; T4/T5/T2 only construct/match them |
| `fe-ui/src/actions/mod.rs` (`UiAction` enum + drain-dispatch) | T2/T3/T4 each add verbs | **T6 scaffold** (Wave 0) pre-adds variants + dispatch arms calling per-track handler stubs |
| `fe-ui/src/plugin.rs` (`init_resource`/`add_systems` + `gardener_console` call site) | every fe-ui track adds a resource/system/state param | **T6 scaffold** (Wave 0) pre-registers all resources/systems + threads the console signature |
| `fe-ui/src/panels/mod.rs` (`gardener_console` signature + module reg) | new state params + T3 sculpt panel | **T6** owns; scaffold lands signature + module regs in Wave 0 |
| `fe-ui/src/ui_shell/topbar.rs` | T6 FR-3 sticky-toggle button lives here | **T6** (was orphaned by the table) |

**Two fold rules keep the enum registries single-owner (no new variant races):**
- **T3 brush overlay folds into `fe-renderer/src/terrain_overlay.rs`** (which T3
  already owns) — it adds **no** new fe-renderer module, so only **T2** edits
  `fe-renderer/src/lib.rs` (`pub mod instancing;`).
- **T3 sculpt UI folds into the existing TerrainTools section**
  (`terrain_tools_panel.rs`) — it adds **no** new `RightSidebarSection` variant,
  so `right_sidebar.rs` stays T6-exclusive.
- T3's new `fe-terrain/src/sculpt` module means **T3 owns the
  `fe-terrain/src/lib.rs` `mod sculpt;` line** (T2 adds no fe-terrain module).

**T6's real Wave-0 role is shell UX *plus* the fe-ui registration scaffold**
(`actions/mod.rs` + `plugin.rs` + `panels/mod.rs` signature + handler stubs in
`actions/{asset,path,node,node_props,terrain_proposal}.rs`). Wave-1 fe-ui tracks
fill only their leaf handler bodies + owned panel/dialog files; they never touch
the four central files. This converts a would-be-serial fe-ui chain into
parallel leaf work — the enabling investment for Wave-1 fan-out.

## Open-question resolutions (ratified 2026-07-25)

All six tracks' open questions ratified via AskUserQuestion. Per-Q rationale lives
in each spec's `## Ratified decisions (2026-07-25)` section; the cross-cutting
resolutions that touch more than one track are recorded here.

- **Program scope:** plan all six tracks in a **single `/slice` pass** (one
  wave-ordered DAG: Wave 0 spine ‖ shell, Wave 1 the four). Specs committed
  first as a conductor bookkeeping commit.
- **Address form (T1 Q-1) — OVERRIDE of the spec default.** The stable node
  address is the **public `fe://verse/fractal/petal/node` URI, defined at the
  data layer (T1) now** — not an opaque-internal key with a later T5 projection.
  This also settles **T5 Q-1**. **Boundary shift:** T1 *defines* the URI; T5
  *exposes* it over REST/MCP and reconciles `fe-renderer/src/addressing.rs`. The
  file partition is unchanged (`addressing.rs` stays T5's); only the semantic
  contract moves — T5 FR-1 becomes *expose + reconcile*, not *project*.
- **API write auth (T5 Q-2):** external/MCP callers reuse the existing
  session/relay auth to derive a `RoleLevel`; `fe-policy` enforces Editor+ per
  scope — **one auth model for UI and API** (N-5 preserved).
- **Cascade confirm:** always confirm (T1 Q-3 = T4 Q-2), no threshold.
- **Promotion:** on first individual select/edit (T1 Q-2, T2 Q-2 aligned).
- **Earthwork delete:** reverts the baked contribution (T3 Q-2) — region is the
  source of truth.

## Deferred / out-of-program (noted so they aren't silently dropped)

- Game-style **radial menu** (D-A9 defers it) — a polish follow-up after the
  context menu ships.
- **Live non-destructive adjustment stack** for earthwork (D-A8 chose baked
  region nodes) — a future track if the baked model proves limiting.
- **GeoParquet / DataFusion** egress (Phase 6.2, intentionally deferred) — T5
  extends the existing SQL/API egress backbone, not this.
