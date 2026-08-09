---
type: spec
track: ui_semantics_unification_20260808
created: 2026-08-08
---

# UI Semantics Unification — Audit + Design Spec

Provenance: 7-agent fan-out 2026-08-08 (4 sonnet audit lanes: toggle/pen/terrain/surface; sonnet crash scout + adversarial verifier; opus synthesis). Crash lane verdict: the terrain-editing emath smart_aim NaN abort is CLOSED on the current tree — root cause was unbounded `.range(..=f32::MAX)` / range-less DragValues in `terrain_tools_panel.rs` (span arithmetic overflows to Inf/NaN inside egui) plus no per-frame NaN repair; fixed in 631f389 + uncommitted WIP via `sanitize_numeric_state()` at every render entry + finite bounds (`MAX_SCULPT_DISTANCE = 1_000_000.0`); adversarial verify: refuted=false, zero residual paths. Regression guard to consider: forbid `f32::MAX` inside `.range(` in fe-ui via lint/CI grep.
# Editing-Surface Synthesis: Toggle Semantics + Active-Tool Options Handle

Scope: current dirty tree (~2,100 lines uncommitted WIP), `fe-ui` crate. All four lane findings merged, de-duplicated, and re-verified against source. **One lane claim was found wrong and is corrected in Â§2 (note C-1).**

---

## 1. CONSISTENCY MATRIX

`(D)` marks a cell that diverges from the pattern the other columns share. Terrain-proposals is not a `Tool` at all â€” it is a panel surface â€” which is itself the structural divergence.

| Dimension | Select | Move / Rotate / Scale | Pen | Brush | Terrain-proposals (palette) |
|---|---|---|---|---|---|
| **Activation** | Topbar btn or `S`; `Tool::default()` = Select (`toolbar.rs:13-23`, `TOOL_DEFS` `toolbar.rs:38-45`) | Topbar btn or `G`/`R`/`X` (`toolbar.rs:46-66`); both entry points write `tool.active_tool` (`topbar.rs:68`, `shortcuts.rs:40`) | Topbar btn or `P` (`toolbar.rs:67-73`) | Topbar btn or bare `B`; `Ctrl/Cmd+B` suppressed so it stays the left-sidebar toggle (`shortcuts.rs:14-16,39`). **Also force-opens `RightSidebarSection::Tool`** (`topbar.rs:69-71`, `shortcuts.rs:41-43`) `(D)` | **No tool, no shortcut.** Rail glyph only â†’ `RightSidebarSection::TerrainTools` (`right_sidebar.rs:578`); usable under any active tool `(D)` |
| **Deactivation / toggle-off** | n/a (rest state) | Switch-only; re-press = no-op re-assign (`topbar.rs:68`) | Switch-only; no toggle-off | Switch-only; no toggle-off | Section toggle-off exists (`RightSidebarState::toggle`, `right_sidebar.rs:68-74`) â€” panels toggle, tools do not `(D)` |
| **Esc behavior** | Rung: `deselect()` (`shortcuts.rs:52-56`) | Same as Select (`shortcuts.rs:48-57`); does **not** cancel an in-flight gimbal drag | Rung 1 `stop_editing()` (`gis/mod.rs:426-435`) â€” kills the whole editing session, not the in-flight gesture; live drag dies indirectly at Release via press-time-context validation `(D)` | **Esc fully swallowed**: `if tool.active_tool == Tool::Brush { return; }` before the ladder (`shortcuts.rs:49-51`); Esc instead only drops `gesture.active` (`brush_interaction.rs:134-140`) `(D)` | Esc does nothing to proposals; no draft state to cancel |
| **Never resets tool** | â€” | â€” | Tool stays Pen after `stop_editing` â†’ next empty click auto-creates a new track `(D)` | â€” | â€” |
| **Operates-on / selection target** | `NodeManager.selected` (Authority A) | Authority A entity gimbal, **plus** a lone path vertex/segment as a Move handle in every tool (`dispatch.rs:183-196`, ratified 2026-07-19) | `PathEditorState.editing_track_id/points/selected_point/selected_segment` (Authority B) **only** (`gis/mod.rs:231,262,284,289`); handler files carry zero `NodeManager` refs `(D)` | Neither authority â€” ground-plane ray hit â†’ `PetalMapState.terrain_json` (`brush_interaction.rs:156-217`) `(D)` | Neither â€” client mirror `ProposalEditState.proposals` (`terrain_proposal_state.rs:46-47`) `(D)` |
| **Options surface location** | Right sidebar `Tool` section, static "(soon)" text (`right_sidebar.rs:305-319`, `tool_inspector.rs:38`) | Same, static "(soon)" (`tool_inspector.rs:47,53,59`) | **Three hosts, two topbar buttons**: curve mode/tension/shapes â†’ `PathTools` section via "Tools" btn (`topbar.rs:105-117` â†’ `right_sidebar.rs:328-343` â†’ `tool_panel::render_pen_section`); per-anchor corner edit â†’ Data window Paths tab (`path_editor_card.rs` ~406-425, `gis_panel.rs:104`); read-only anchor mirror â†’ `Tool` section (`right_sidebar.rs:283-289`). Self-documented in `tool_inspector.rs:70-73` `(D)` | **Two hosts, same `SculptToolState`**: `Tool` section (`right_sidebar.rs:300-304` â†’ `render_brush_controls`) and a permanent near-duplicate in `TerrainTools` (`terrain_tools_panel.rs:239-380`) `(D)` | `TerrainTools` section only; no tool ever reveals it `(D)` |
| **Commit / cancel** | Instant on click (`viewport_pick.rs`) | Release-phase commit; `sweep_stranded_drag` safety net | Release-phase commit per gesture (`path_point_interaction.rs:146`, `PEN_DRAG_THRESHOLD_M=0.15`); cancel only via press-time-context invalidation | Whole drag batched into ONE `SculptBrushStroke` at Release (`brush_interaction.rs:203-217`); in-system cancel on Esc / right-click / tool-change / petal-change (`brush_interaction.rs:133-149`) `(D)` | Instant on button click; no draft/preview/bake state machine `(D)` |
| **Right-click** | Object-aware menu (`viewport.rs:171-183` â†’ `context_pick.rs:80-168`) | Same | Same â€” **not** Pen-specific, cannot cancel a Pen gesture; classifier resolves only Node/Stamp/Empty so an anchor reads as the whole track `(D, shared)` | **Menu suppressed**: `active_tool != Some(Tool::Brush)` gate (`viewport.rs:171`); right-click is a stroke cancel instead (`brush_interaction.rs:135`) `(D)` | No proposal branch in the classifier; `HitTarget::TerrainProposal` menu rows (`context_menu.rs:72-100`) unreachable `(D)` |
| **Viewport affordance** | Generic 2-line hint only when nothing selected (`viewport.rs:128-142`) | Same generic hint + gimbal (`gimbal.rs:109-111,129-131`) | Dedicated hint line always (`viewport.rs:112-119`); path-point labels keyed on `editing_track_id`, tool-independent (`viewport_labels.rs:23-64`) | Dedicated hint always (`viewport.rs:120-127`) + cursor ring (`sculpt_cursor.rs:28-58`) | Ghost overlay only (`fe-terrain render_terrain_proposals`); no viewport affordance, no picking |
| **Click-priority claim** | `NodePick`, last (`router.rs:31-32`, chain `mod.rs:169`) | `Gimbal` (`router.rs:20`, chain `mod.rs:164-165`) | `PathHandle` > `PathMarker`/`PathPlace` > `PathSegment` (`router.rs:18,22-28`, chain `mod.rs:163,167-168`) | Declared 6th of 7 (`router.rs:29-32`) but registered **2nd** in the chain (`mod.rs:160`) â€” claims before every "higher" consumer `(D)` | n/a |

---

## 2. TOP INCONSISTENCIES (merged, ranked by user pain)

**C-1 â€” Correction to the PEN lane.** The lane states "Nothing propagates the other directionâ€¦ opening a track never sets `NodeManager.selected`." That is false on this tree. `node_manager/pointer/mod.rs` implements **three** bridge arms: Aâ†’B on select (`:53-57` â†’ `PathSelectTrack`), A-deselectâ†’B (`:59-64` â†’ `stop_editing()`), and **Bâ†’A** (`:70-78` â†’ `manager.pending_sidebar_select`, gated on the track entity being spawned in the active petal). The genuine gap is narrower: `stop_editing()` (Esc rung 1, `gis/mod.rs:426-435`) does **not** clear `NodeManager.selected`, so the Bâ†’None transition has no A-side arm. Design Â§3 is built on the corrected model.

| # | Finding | Severity | Pain |
|---|---|---|---|
| **1** | **`ProposalEditState` is never rehydrated â†’ first palette Add/Delete destroys persisted proposals and un-bakes earthwork.** `replace_all` (`terrain_proposal_state.rs:87`) has exactly one caller â€” its own test (`:211`). Neither `verse_manager/db_results/terrain.rs::handle_petal_terrain_loaded` nor `terrain_map/mod.rs::load_petal_terrain_on_nav_change` touch it. `actions/terrain_proposal.rs::embed_proposals` (`:22-32`) does a wholesale `doc["proposals"] = to_json(&proposal_state.proposals)` from that empty mirror. Brush's own path is safe (`embed_region`/`remove_region`, `:551-583`, read-modify-write). | HIGH | Silent, unrecoverable data loss on a first click |
| **2** | **Palette proposals can never bake or become addressable nodes â€” no UI discloses it.** `fe-terrain/src/terrain_proposal.rs:75-109` requires non-empty `material` AND hard-returns `valid_params=false` for `Flatten\|Ramp\|Slope\|Pad\|Cut\|Fill`. `ProposalRecord` has no `material` field. Only `region_json()` (`actions/terrain_proposal.rs:525-546`, Brush-only) qualifies. Button copy is identical across all 8 modes (`terrain_tools_panel.rs:111`). Directly contradicts "every artifact is an addressable endpoint." | HIGH | User builds 8 modes' worth of work that can never render or be queried |
| **3** | **`TerrainTools` + `ProposalReport` have no entry point from the resting app.** No topbar button, no shortcut, no auto-nav (`topbar.rs:105-150` only requests PathTools/Settings/Maps). Sole path is `section_rail` (`right_sidebar.rs:571-595`), rendered inside `section_chrome` (`:528-567`) which the **Inspector never calls** â€” Inspector uses its own `SidePanel::right("inspector")` (`right_sidebar.rs:221-243` â†’ `panels/inspector.rs:23-47`). Resting state = collapsed sidebar, zero affordance. `Operation::SelectProposal` (`dispatch.rs:167`) has zero consumers. | HIGH | Whole feature is undiscoverable |
| **4** | **Right-click menu: 4 of 9 `HitTarget` kinds unreachable; 3 verbs dead on both ends.** `context_menu.rs::menu_for` (`:72-100`) fully defines and tests PathSegment / PathVertex / PathHandle / TerrainProposal verb sets; `context_pick.rs::classify_context_menu` (`:80-168`) + `resolve_context_target` (`:40-72`) resolve only Node/Stamp/Empty (only `SpawnedNodeMarker` entities are queried; `PathPointMarker`/`PathHandleMarker` carry none). `SetCornerSmooth`/`DeletePoint`/`EditRegionParams` additionally hit a "Not reachable from the context menu yet" toast stub (`context_menu.rs:541-548`). No "delete this anchor" exists outside the Paths-tab list. | HIGH | The T4 object-aware-menu promise is unmet exactly where editing happens |
| **5** | **FR-6 "Earthwork totals" panel is blind to every Brush-created region.** `proposal_report_panel::render_report_body`/`cut_fill_totals` source only `ProposalEditState` (`right_sidebar.rs:381-390`); Brush writes only `petal_map.terrain_json`. Real baked volumes exist (`persist_earthwork_volumes`, `fe-terrain/src/terrain_plugin.rs:1223-1268`; `actions/terrain_proposal.rs:472-520`) but surface only as node properties / fe-query. Coupled to #1 and #2: the one reporting panel can only report records that never bake. | HIGH | The reporting surface is structurally empty |
| **6** | **Brush is a de facto hidden MODE.** Escape short-circuits the staged ladder (`shortcuts.rs:49-51`); right-click is suppressed at the viewport level (`viewport.rs:171`) and consumed as cancel (`brush_interaction.rs:135`). No other tool alters what Esc or right-click *mean*. Directly against the ratified no-modes thesis. | HIGH | Two global keys change meaning with no visible indicator |
| **7** | **Brush controls duplicated across two hosts against one resource.** `render_brush_controls` (`terrain_tools_panel.rs:383-458`) in the `Tool` section (`right_sidebar.rs:300-303`) vs. `render_sculpt_placeholder` (`terrain_tools_panel.rs:239-380`) permanently in `TerrainTools` (`right_sidebar.rs:372`) â€” same `SculptToolState`, different widget sets (the second adds shape-mode + footprint readout), no UI signal that they are linked. | MED | Edits teleport between panels |
| **8** | **Options-surface fragmentation + asymmetric auto-reveal.** Pen's mutable settings span 3 hosts behind 2 differently-labeled topbar buttons (matrix row above; self-documented at `tool_inspector.rs:70-73`). Brush auto-opens its section on activation (`topbar.rs:69-71`, `shortcuts.rs:41-43`); Pen â€” with a comparable surface â€” does not. | MED | "Where are my tool's settings" has 3 different answers |
| **9** | **Adjacent controls, unreconciled units.** `render_controls_and_emit` (`terrain_tools_panel.rs:65-139`) labels footprint radius `" wu"` and feeds raw world units into `curve::circle`/`TerrainProposalAdd`; one scroll below in the SAME section `render_sculpt_placeholder`/`render_brush_controls` (`:239-380`, `:383-458`) label the same fields metres and convert via `meters_to_world` (`brush_interaction.rs:29-52`, `geometry.rs:8-29`). Neither surfaces `world_scale`. | MED | Same number, different real size, no way to reconcile |
| **10** | **Brush ring preview skips the map-loaded gate the commit path enforces.** `sculpt_cursor.rs:28-58` checks only `active_tool == Brush`; `brush_interaction.rs:171-178` gates commits on `petal_map_is_loading`. `world_scale` resets to 1.0 on every petal switch (`terrain_map/mod.rs:64-88`), so during the load window the ring can render orders of magnitude off. | MED | Ring lies about the stroke it would make |
| **11** | **`ClickPriority` ordinals contradict runtime pre-emption.** Enum ranks Brush 6th (`router.rs:14-33`) but `claim()` is pure first-claim-wins by execution order (`:73-79`) and Brush is registered 2nd (`mod.rs:160`). Masked today only because every other consumer independently guards `Tool::Brush` (`gimbal_interaction.rs:171-173`, `path_handle_interaction.rs:318`). | MED | Trap for the next click consumer |
| **12** | **Esc never resets the active tool; Pen is sticky after closing a track.** `stop_editing()` clears Authority B but not `ToolState` (`shortcuts.rs:48-57`), so the next empty click auto-creates a fresh track. | MED | "I pressed Esc and it drew another path" |
| **13** | Dead superseded design in `dispatch.rs`: `Tool::Brush â†’ Operation::TerrainCellEdit` (`:156,171`) plus `HitTarget::TerrainCell`/`TerrainBrush`/`TerrainProposalEdit`/`terrain_cell_proposal` (`:102-137`), all `pub` from `mod.rs:59-62`. Unreachable â€” Brush claims before `viewport_pick` runs, and `viewport_pick.rs:92-101` drops `TerrainCellEdit` into `_ => {}`. | LOW | Misleads readers |
| **14** | `SculptShapeMode::{Circle,Rect,Polygon}` selectable but inert â€” zero `push_action(UiAction::SculptShapeRegion/SculptDeleteRegion)` call sites despite full handler wiring (`actions/mod.rs:994-1024`). Self-disclosed in panel copy (`terrain_tools_panel.rs:373-378`). | LOW | Dead radio buttons |
| **15** | Brush's `settings_zone` string is unreachable: `render_tool_section` returns at `right_sidebar.rs:303` before the SETTINGS block (`:305-319`), and `tool_tooltip_text` (`toolbar.rs:131-141`) joins only title/subtitle/use_zone. | LOW | Dead content |
| **16** | `update_hovered_axis` skips only `Tool::Select` (`gimbal_interaction.rs:36-38`) while drag-start and draw skip `{Select,Pen,Brush}` (`:171-173`, `gimbal.rs:109-111,129-131`) â€” wasted per-frame raycast, guard-set mismatch. | LOW | Latent |
| **17** | Stale docs: `ui_shell/AGENTS.md:49-58` documents a removed `LeftSidebarPolicy::AutoCollapse` "(DEFAULT)" and a 3-arg `left_visibility` (actual: `left_sidebar.rs:20-55`, 2 args, `Manual` only). `panels/AGENTS.md` Â§tool-inspector calls `tool_tooltip_text` "not yet called" â€” it is (`topbar.rs:67`). | LOW | Contributors design against fiction |
| **18** | Three uncoordinated "what is showing" state machines: `RightSidebarState.requested` (7 exclusive sections), `ActiveDialog`/`resolve_exclusive` (`panels/mod.rs:203-288`), and `GisPanelState.open` (`gis_panel.rs:26-58`, deliberately outside both). Data window can co-exist with any section and any modal. | LOW | Documented carve-out, not an accident |

---

## 3. UNIFIED DESIGN PROPOSAL

Two deliverables, matching the user's ask: **(a) one tool-toggle semantic**, **(b) one addressable active-tool options surface**.

### (a) One tool-toggle semantic

**Single writer.** Today activation is duplicated in `topbar.rs:68-71` and `shortcuts.rs:40-43` (and they already differ in nothing but luck). Introduce one method on the existing resource in `plugin.rs:160-163`:

```
ToolState::activate(&mut self, tool: Tool, right: &mut RightSidebarState)
```

Rules, in order:
1. `if self.active_tool == tool && tool != Tool::Select { self.active_tool = Tool::Select }` â€” **re-press toggles off to Select**, the neutral rest tool.
2. `else { self.active_tool = tool }`
3. `right.requested = if self.active_tool == Tool::Select { None } else { Some(RightSidebarSection::Tool) }` â€” one rule for all tools; kills the Brush/Pen asymmetry (#8).

Both `topbar.rs:61-73` and `shortcuts.rs:34-46` become one-line calls. `TOOL_DEFS` (`toolbar.rs:38-81`) stays the single source of buttons + bindings.

**Not a mode, because:** there is always exactly one active tool (never a tool-less state); "toggle off" means *return to Select*, not *leave the toolbar*; and no tool changes what a click on a concrete object means â€” `dispatch.rs:149-175` remains the sole truth table, tool-independent for `PathVertex`/`PathHandle`/`PathSegment`/`Stamp`.

**One staged Escape ladder, four rungs, all tools, one place** (`shortcuts.rs`, replacing `:48-57` and deleting the Brush short-circuit at `:49-51`):

| Rung | Condition | Action |
|---|---|---|
| 0 | any in-flight viewport gesture | cancel **only** that gesture, consume Esc |
| 1 | `path_state.editing_track_id.is_some()` | `stop_editing()` |
| 2 | `node_mgr.selected.is_some()` | `deselect()` |
| 3 | `tool.active_tool != Tool::Select` | `activate(Select)` â€” fixes #12 |

Rung 0 needs a gesture aggregate. All five gesture resources already exist and are `init_resource`d together (`mod.rs:147-151`): `BrushGesture`, `PathPointDrag`, `PenHandleDrag`, `PathHandleDrag`, `PathGimbalDrag`. Add a `GestureParams` `SystemParam` bundle with `any_active() -> bool` and `cancel_all()`, consumed by `handle_tool_shortcuts` â€” which already runs **first** in the chain (`mod.rs:157`), ahead of every gesture system, so a rung-0 cancel lands the same frame. `brush_interaction.rs:134-140` drops its private Escape read; Pen gains real gesture cancel (currently it has none â€” cancellation is an accident of press-time-context validation).

**One right-click rule, all tools:** right-click cancels an in-flight gesture if one exists, otherwise opens the object-aware menu. Delete the `active_tool != Some(Tool::Brush)` gate at `viewport.rs:171`. `viewport.rs` is egui-side and cannot read Bevy resources, so widen the existing per-frame stash idiom: replace `stash_active_tool`/`active_tool_hint` (`toolbar.rs:97-111`) with a single `InputContext { active_tool: Tool, gesture_active: bool }` stashed by the topbar (`topbar.rs:74`) and read by the viewport (`viewport.rs:111`). Same frame ordering guarantee as today (topbar always renders before viewport, `panels/mod.rs:120,182`), so no drift path is introduced. `gesture_active` reaches the topbar via the existing `MiscUiParams` bundle â€” see the ceiling note in Â§5.

**Mode risk, called out explicitly:**
- *Auto-revealing the options section on activation* is a persistent UI state change. Mitigation: it is a **one-shot request** written at the activation edge only. Never re-assert `right.requested` per frame â€” that would pin the panel and make tool activation a mode. The rail/topbar toggle must still win until the next explicit activation.
- *Rung 3 (Esc resets tool to Select)* is safe only because Select is a real tool with real behavior, not an "off" state.
- *Brush's Release-batched commit* stays as-is; it is a gesture shape, not a mode, once Esc/right-click no longer have Brush-specific meanings.

### (b) One addressable options surface

**`RightSidebarSection::Tool` becomes "Options" â€” the canonical, always-correct home for the active tool's mutable settings.** `render_tool_section` (`right_sidebar.rs:251-321`) is rewritten to: selection readout (unchanged, from `project_selection`) â†’ separator â†’ `panels::tool_options::render(ui, active_tool, ...)`, a new pure-ish dispatcher with one arm per `Tool`:

| Tool | Body (reuses existing code, moved not rewritten) |
|---|---|
| Select | selection readout + filters (soon) |
| Move/Rotate/Scale | `gimbal_affordance_label` (`tool_inspector.rs:94-110`) + snap/axis-lock (soon) |
| Pen | `tool_panel::render_pen_section` **moved out of `PathTools`** (`right_sidebar.rs:341`) + `render_path_asset_section` + the per-anchor corner editor **extracted from** `path_editor_card.rs` (~406-425) |
| Brush | `terrain_tools_panel::render_brush_controls` (`:383-458`) â€” the **only** copy; `render_sculpt_placeholder` (`:239-380`) loses its duplicated Radius/Strength/Op/Material widgets and keeps only shape-mode + footprint readout, or is deleted entirely (Decision 6) |

Consequences: `PathTools` becomes empty and is **retired** as a section variant; the topbar "Tools" button (`topbar.rs:105-117`) is rebound to `RightSidebarSection::Tool` and relabelled "Options". Fixes #7, #8, and half of #3.

**The "surface handle" the user asked for.** Give sections a stable string identity so they are addressable the same way nodes are (spatial-builder endpoint thesis):

```
impl RightSidebarSection {
    pub fn slug(&self) -> &'static str;              // "inspector" | "options" | "terrain" | "report" | "settings" | "maps"
    pub fn from_slug(s: &str) -> Option<Self>;
}
UiAction::RevealSection { slug: String }             // actions/mod.rs
```

Every reveal path â€” topbar, rail, `activate()`, context-menu cross-links, and later fe-api/MCP â€” routes through this one action. `section_label` (`right_sidebar.rs:83-93`) already exists as the human half; `slug` is the machine half.

**Reachability for TerrainTools / ProposalReport (#3):** make `render_inspector_section` (`right_sidebar.rs:221-243`) render through `section_chrome` like the other six, so the 7-glyph rail (`:571-595`) is visible in the resting state. Add cross-link buttons in Brush's options body ("Terrain Tools â†’", "Earthwork report â†’") emitting `RevealSection`.

**Reconciling `NodeManager.selected` vs `PathEditorState.editing_track_id`.** Do **not** merge them (NFR-2, `ui_ux.md Â§5`, `node_manager/AGENTS.md` Â§pen-tool). Instead formalize what already half-exists:

1. **One projection, consumed by every surface.** `project_selection` + `fresh_path_selection` (`right_sidebar.rs:259-269`, `node_manager/selection.rs`) already unify both authorities into `SelectionKind` for display. Make it the **only** input to the options surface, the gimbal draw, and â€” new â€” `context_pick.rs`'s classifier. No panel reads raw `NodeManager`/`PathEditorState` again.
2. **One bridge module, four arms.** `node_manager/pointer/mod.rs` is already the sole home of the bridge and implements three arms (`:53-57` Aâ†’B select, `:59-64` A-deselectâ†’B, `:70-78` Bâ†’A via `pending_sidebar_select`). The missing fourth is Bâ†’None â†’ A: `stop_editing()` leaves `NodeManager.selected` pointing at the track. Adding it makes Esc rung 1 â†’ rung 2 read as one coherent back-out instead of two presses on the same object. **This is Decision 4** â€” it is a behavior change, not a bug fix, because today rung 1 â†’ rung 2 is arguably the intended two-stage exit.
3. **No third authority.** The options surface holds no selection state of its own; `ToolPanelState`/`SculptToolState` stay pure tool *parameters*, never targets.

---

## 4. DECISION POINTS

1. **Re-press toggles a tool off to Select?** (A) Yes â€” re-pressing `P` while Pen is active returns to Select. (B) No â€” keep switch-only, re-press is a no-op.
2. **Esc rung 3 â€” should Esc eventually return the active tool to Select?** Yes / No. (Fixes the sticky-Pen complaint #12; costs one extra Esc press to reach a fully neutral state.)
3. **Esc rung 0 â€” should Esc cancel only the in-flight gesture for *every* tool (Pen included), before touching path-editing or selection?** Yes / No. (Yes = Brush stops being special; Pen gains a real cancel it does not have today.)
4. **Should `stop_editing()` also clear `NodeManager.selected` (bridge arm 4)?** Yes = one press backs fully out of a track. No = keep today's two-stage exit (Esc1 closes the session, Esc2 deselects the track entity).
5. **Right-click, one rule for all tools (cancel gesture if any, else object menu)?** Yes / No. (Yes removes `viewport.rs:171`'s Brush gate.)
6. **Brush controls: single copy in the Options section, with `render_sculpt_placeholder`'s duplicate widgets deleted?** (A) Delete duplicates, keep shape-mode + footprint readout in TerrainTools. (B) Delete `render_sculpt_placeholder` entirely and move shape-mode into Options too.
7. **Retire `RightSidebarSection::PathTools`, folding pen/curve/shape controls into the Options section, and rebind the topbar "Tools" button to Options?** Yes / No.
8. **Move Pen's per-anchor corner/smoothness editor out of the Data window's Paths tab into the Options section?** Yes / No. (The Paths tab keeps the track list, start/stop editing, and the per-point list.)
9. **Should activating any non-Select tool auto-reveal the Options section (one-shot request, user toggle still wins)?** Yes / No.
10. **Give `TerrainTools` and `ProposalReport` reachability by rendering the Inspector through `section_chrome` so the rail is always visible?** Yes / No. (Alternative: add two more topbar buttons.)
11. **Add `UiAction::RevealSection { slug }` + `RightSidebarSection::slug()/from_slug()` as the single addressable UI-surface handle?** Yes / No.
12. **Is the palette's 8-mode ghost-only behavior intended scope for this cycle?** (A) Intended â€” add explicit UI disclosure ("proposal only â€” does not modify terrain"). (B) Not intended â€” route the palette through the same material-tagged bakeable path Brush uses.
13. **Re-point the Proposal Report at persisted `terrain.proposals` (or a properly rehydrated `ProposalEditState`) so it can see Brush-created earthwork?** Yes / No.
14. **`ClickPriority`: which is normative?** (A) The `.chain()` order is normative â€” correct the enum + comment to rank Brush 2nd. (B) The enum is normative â€” reorder the chain and make `claim()` compare ordinals.
15. **`dispatch.rs`'s `Operation::TerrainCellEdit` / `HitTarget::TerrainCell` / `TerrainBrush` / `terrain_cell_proposal`:** (A) Delete as superseded. (B) Keep, marked `#[doc(hidden)] // reserved`.
16. **Extend `context_pick.rs` to classify PathVertex / PathHandle / PathSegment / TerrainProposal and wire the 3 stub verbs?** Yes / No. (No = mark the four `menu_for` arms explicitly as reserved so the dead-code state is intentional.)
17. **Units: convert the palette's footprint/height/delta to metres via `meters_to_world` and show the petal's `world_scale` chip?** Yes / No.
18. **Are the two `AGENTS.md` corrections (`ui_shell` Â§left, `panels` Â§tool-inspector) in scope for this pass?** Yes / No.

---

## 5. MIGRATION SKETCH

**Hard constraints honored throughout:**
- `gardener_ui_system` (`plugin.rs:608-625`) has **exactly 16 `SystemParam`s** â€” it is *at* Bevy's tuple ceiling. **No phase may add a top-level param.** Every new resource goes into `MiscUiParams` (`plugin.rs:~575-591`) or `UiShellParams` (`plugin.rs:597-606`); the bundles exist for precisely this (`plugin.rs:593-596`).
- `ui_shell` managers are plain fns called from `gardener_console` (`panels/mod.rs:70-115`, call sites `:120,138,160,182`), not Bevy systems â€” new panel data is threaded as a `&mut` arg through `gardener_console`'s (already very wide, `#[allow]`-ed) signature, then sourced from a bundle at the `plugin.rs:628-658` call site.
- Node-manager systems keep their `.chain()` order (`mod.rs:154-180`); `handle_tool_shortcuts` must stay first.
- **One build/lint/test sweep per phase, at the end** â€” never testâ†’fixâ†’test.

| Phase | Goal | Files touched | Rough size |
|---|---|---|---|
| **0 â€” Docs + dead code (no behavior)** | Land the free wins first so later diffs are clean | `ui_shell/AGENTS.md:49-58` (drop `AutoCollapse`, 2-arg `left_visibility`); `panels/AGENTS.md` Â§tool-inspector (tooltip wiring is done); `node_manager/router.rs:14-33` (comment per Decision 14); `panels/tool_inspector.rs:83` (Brush `settings_zone` per #15); `node_manager/dispatch.rs:102-137,156,171` + `node_manager/mod.rs:59-62` (Decision 15); `node_manager/gimbal_interaction.rs:36-38` (widen guard to `{Select,Pen,Brush}`) | ~120 lines, mostly deletions |
| **1 â€” Data-loss fix (independent, ship first)** | Findings #1, #5, #13 | `terrain_proposal_state.rs` (call `replace_all` â€” no new API needed); `verse_manager/db_results/terrain.rs::handle_petal_terrain_loaded` (rehydrate on load); `terrain_map/mod.rs:64-88` (clear on petal switch); `actions/terrain_proposal.rs:22-32` (`embed_proposals` â†’ read-modify-write, mirroring `embed_region` at `:551-583`); optional `proposal_report_panel.rs` re-point per Decision 13 | ~150-250 lines + tests |
| **2 â€” Toggle semantics** | Decisions 1-5; findings #6, #12, #11 | `plugin.rs` (`ToolState::activate`, ~30 lines); `ui_shell/topbar.rs:61-73` + `node_manager/shortcuts.rs:34-57` (call it; delete the Brush Esc short-circuit); new `GestureParams` bundle in `node_manager/mod.rs` or `router.rs`; `node_manager/brush_interaction.rs:133-149` (drop private Esc/right-click reads); `panels/toolbar.rs:97-111` (`InputContext` stash); `fe-ui/src/viewport.rs:111,171` (read it, drop the Brush gate); `MiscUiParams` in `plugin.rs` gains the gesture-active read | ~250-350 lines; **highest regression risk** â€” every Esc/right-click test in `node_manager/` and `panels/` needs review |
| **3 â€” Options surface** | Decisions 6-9; findings #7, #8 | new `panels/tool_options.rs` (dispatcher, ~150 lines); `ui_shell/right_sidebar.rs:251-321` (rewrite `render_tool_section`), `:328-343` (retire `PathTools`), `:41-56` + `:83-93` + `:571-595` (variant + rail), `:105-117` in `topbar.rs` (rebind "Tools"â†’Options); `panels/tool_panel.rs` (pen section moves, not rewritten); `panels/terrain_tools_panel.rs:239-380` (de-duplicate per Decision 6); `panels/path_editor_card.rs` ~406-425 (extract corner editor per Decision 8); `panels/mod.rs:70-115,160-180` if new args are needed | ~400-500 lines, mostly moves |
| **4 â€” Reachability + surface handle** | Decisions 10-11; finding #3 | `ui_shell/right_sidebar.rs:221-243` (Inspector through `section_chrome`) + `slug()/from_slug()`; `actions/mod.rs` (`UiAction::RevealSection`); `ui_shell/topbar.rs` + `dialogs/context_menu.rs` cross-links; `panels/tool_options.rs` (Brush "Terrain Tools â†’" / "Report â†’") | ~150-200 lines |
| **5 â€” Right-click classifier** | Decision 16; finding #4 | `node_manager/context_pick.rs:40-72,80-168` (PathVertex/PathHandle/PathSegment/TerrainProposal branches â€” query `PathPointMarker`/`PathHandleMarker`, consume the Â§3 projection); `dialogs/context_menu.rs:541-548` (wire `SetCornerSmooth`/`DeletePoint`/`EditRegionParams` to real actions); `actions/mod.rs` + `actions/path.rs` for any missing verbs | ~250-350 lines; **do after Phase 3** so the corner editor it links into has settled |
| **6 â€” Units + preview truthfulness** | Decisions 12, 17; findings #9, #10, #2, #14 | `panels/terrain_tools_panel.rs:65-139` (metres + `world_scale` chip); `fe-ui/src/sculpt_cursor.rs:28-58` (add the `petal_map.loaded` gate + a dim/hidden ring during load); `fe-terrain/src/terrain_proposal.rs:75-109` and/or palette copy per Decision 12; `SculptShapeMode` wiring or removal per #14 | ~200-300 lines |

**Ordering rationale.** Phase 1 is fully independent of the UI work and fixes active data loss â€” it should land alone, even if everything else is deferred. Phase 0 removes noise that would otherwise pollute every later diff. Phases 2 and 3 both touch `topbar.rs` and `right_sidebar.rs`; run them serially, never in parallel slices. Phase 5 depends on Phase 3's extracted corner editor. Phase 6 is independent of 2-5 and can run in parallel with Phase 5 by a separate agent (disjoint file sets: `terrain_tools_panel.rs`/`sculpt_cursor.rs`/`fe-terrain` vs. `context_pick.rs`/`context_menu.rs`).

