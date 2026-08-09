---
type: spec
track: hierarchy_visibility_groups_20260808
created: 2026-08-08
---

# Left-Sidebar Hierarchy Visibility — Design Insight Report

Provenance: opus design lane, 2026-08-08, read-only over the live tree (ui_semantics_unification waves in flight). Line numbers cite the tree as of that run.

**Bottom line up front:** there is **no node/petal/fractal/verse visibility concept in this codebase at all.** The left sidebar is a pure navigator with zero per-row state. Exactly one per-node visibility flag exists (`gis.track.visible`, GPX ribbons only), and it is already a live demonstration of the trap this feature must avoid: a hidden ribbon is invisible but still fully clickable, selectable, gimbal-draggable and camera-focusable. Everything below is built on that.

---

## 1. CURRENT STATE MAP

### 1.1 What the left sidebar actually renders

`fe-ui/src/ui_shell/left_sidebar.rs` is **policy only** — it owns nothing but the open/closed decision (`LeftSidebarState { policy, user_intent }` at `left_sidebar.rs:34-37`, pure `left_visibility` at `:51-55`, `render_left_sidebar` at `:60-84`). All content lives in `fe-ui/src/panels/sidebar.rs`.

`panels/sidebar.rs:14-100` renders, top to bottom:

| Region | Fn | Lines | Affordances today |
|---|---|---|---|
| Verse header | `sidebar_verse_header` | `sidebar.rs:102-151` | active-verse name, `+` create verse, `Join` |
| **4-level tree** | `render_verse_tree` → `render_fractals` → `render_petals` → `render_nodes` | `:153-205`, `:208-250`, `:253-310`, `:312-400` | see below |
| Space overview | `sidebar_section_space_overview` | `:424-449` | petal/model/peer counts |
| Reset DB | inline | `:49-98` | two-step destructive confirm |

The tree is a genuine nested `egui::CollapsingHeader` tree, **not** a flat list, and it renders **every verse/fractal/petal in the DB**, not just the active branch, all `default_open(true)`.

Per-level affordances that exist **right now**:
- **Verse / Fractal / Petal**: header click = navigate (`nav.navigate_to_verse/fractal/petal`, `sidebar.rs:192-194`, `:246-248`, `:306-308`); inline `+` add-child (`add_button_inline`, `:402-422`); petal only also gets a `Manifest` button → `UiAction::PetalManifestOpen` (`:289-302`).
- **Node** (`render_nodes`, `:312-400`): glyph `◆` (has asset) / `●` + name; selected row filled with `theme::TREE_SELECTED_BG`; click → `camera_focus.target` + `node_mgr.pending_sidebar_select` (`:383-388`); **Alt+click** → `ActiveDialog::NodeOptions` (`:391-399`).
- Nodes in a **non-active petal** render `theme::TEXT_MUTED` and get `Sense::hover()` instead of `Sense::click()` (`sidebar.rs:351-363`). **This is the only existing "greyed out / inert row" precedent in the tree** and it is exactly the visual language a hidden row should reuse.

No eye toggles. No drag. No reorder. No multi-select. `render_nodes` takes `nodes: &[NodeEntry]` — **immutable**; the node level of the tree is read-only by construction.

**Correction to a premise in the brief:** there is *no* sidebar drag index. `grep drag fe-ui/src/panels/sidebar.rs` → **zero hits**. The NodeManager thing is `NodeManager.pending_sidebar_select` (`node_manager/sidebar_sync.rs:14`), a one-shot "the sidebar clicked this node id, go resolve it to an Entity" hand-off resolved by `sync_sidebar_to_manager` (`sidebar_sync.rs:9-31`). There is no drag idiom to reuse. (The archived `drag_drop_placement_20260402` track was about dragging *assets into the viewport*, not tree rows.)

### 1.2 Hierarchy navigation

Two separate resources, deliberately:
- **`VerseManager`** (`fe-ui/src/verse_manager/mod.rs:88-92`) — the in-memory tree `Vec<VerseEntry>` + `node_index`. Filled from `DbResult::HierarchyLoaded` via `db_results/hierarchy.rs`.
- **`NavigationManager`** (`fe-ui/src/navigation_manager.rs:18-24`) — `{active_verse_id/name, active_fractal_id/name, active_petal_id}`; `navigate_to_*` clears descendants (`:28-45`).

Tree row types: `VerseEntry` (`mod.rs:44-51`), `FractalEntry` (`:54-59`), `PetalEntry` (`:62-67`), `NodeEntry` (`:72-80` = `{id, name, has_asset, position, webpage_url, asset_path}`).

**Two structural facts that dominate the design:**
1. **`NodeEntry` carries no properties.** The hierarchy payload (`fe_runtime::NodeHierarchyData`) deliberately carries no property bag — documented at `verse_manager/AGENTS.md:104-107`. Every property-driven petal-wide feature therefore runs on a **fe-ui-local cache fed off `NodePropertiesLoaded`**: `PrimitiveDescriptorCache` (`primitive_materialize.rs`) and `PathAssetCache` (`path_asset_materialize.rs`) are the two shipped instances of this idiom. Any property-backed visibility must be a third.
2. **`VerseEntry.expanded` / `FractalEntry.expanded` / `PetalEntry.expanded`** (`mod.rs:49,57,65`) are **dead** — `grep '\.expanded'` across `fe-ui/src` returns zero hits. Per-level UI-state slots already exist on the tree types and are inert (egui's own `id_salt` persistence does the collapsing). Free real estate, or a warning about how the last per-level-state attempt went.

### 1.3 Visibility machinery that exists end to end — the honest inventory

Three unrelated things share the word "visibility". Only one is a real render pipeline, and it is not node-scoped.

**(a) `fe_database::atlas::Visibility` = Public / Private / Unlisted — FALSE FRIEND.** A petal *discovery/listing* flag: column `fe-database/src/schema.rs:75`, enum `fe-database/src/atlas.rs:6-11`, setter `space_manager.rs:61`, UI radio `fe-ui/src/atlas/visibility_control.rs:20-25`, consumed by the petal wizard (`atlas/petal_wizard.rs:38`). Not render visibility, not RBAC, no groups. Do not overload it.

**(b) fe-terrain `LayerStack` — the ONE real end-to-end visibility pipeline, and it is *not* a false friend; it is the closest relative.**
- Data: `MapLayer { id, layer_type, visible, opacity, z_order }` (`fe-terrain/src/layers/stack.rs:56-63`); `LayerStack` Bevy `Resource` (`:90-93`) with `set_visibility` (`:117-124`), `set_opacity` (`:127-134`), `get_visible_layers` (`:147-151`).
- Populated **per petal** from the petal terrain doc's `layers` array: `fe-terrain/src/petal_binding.rs:219-247` (`layer_type_from_config_name`, `stack.rs:39-53`).
- Binding: entities opt in by carrying `LayerEntity { layer_id }` (`terrain_plugin.rs:123-126`), inserted at build time on terrain chunks (`:614`), GPX ribbons (`:809`), waypoints/markers (`:919,947,971`), proposal ghosts (`:1057`).
- Apply: `sync_layer_visibility` (`terrain_plugin.rs:1356-1391`) writes Bevy `Visibility::Inherited/Hidden` **and** folds opacity into material alpha, gated on `layer_stack.is_changed()`.
- UI: `panels/layer_manager_card.rs:119-183` (checkbox + opacity slider) → local preview `preview_layer_edit` (`:178-183`) → `UiAction::GisSetLayerField` (`actions/gis.rs:60-69`) → `gis::set_layer_field` (`gis/layers.rs:42-90`) → `SetPetalTerrain`. Display row type `LayerUiEntry` (`gis/layers.rs:10-14`).
- **Reachability today: only inside the Data window's GIS panel** (`panels/gis_panel.rs:100`). Not in either sidebar.
- **Unused seam:** `LayerType::GpxTrack { node_id, .. }` (`stack.rs:21`) is the only per-node-ish layer type, and `layer_type_from_config_name` (`:39-53`) only mints it if a petal config literally declares a `gpx_track` layer with a `source`. **Nothing in the app ever writes such a config.** Per-node layer binding is real, tested, and dead.

**(c) `gis.track.visible` — the only per-NODE visibility flag in existence.**
Key constant `fe-ui/src/actions/path.rs:248` (and `fractalengine/src/gpx_bridge.rs:49`). Write path: Paths-tab checkbox `panels/path_editor_card.rs:633-638` → `UiAction::PathStyleApply` → `style_property_writes` (`actions/path.rs:272-316`) → `DbCommand::SetNodeProperty`. Read path: `fe-ui/src/gis/mod.rs:363-364` and `gpx_bridge.rs:71-72` → `TrackStyle.visible`. Apply: `fe-terrain/src/terrain_plugin.rs:794-802` inserts `Visibility::Inherited/Hidden` **at ribbon build time only** — a toggle is a full despawn→respawn round-trip, by design (`terrain_plugin.rs:686-693`, `fe-terrain/src/AGENTS.md:141-142`). Persists as a node property. **Applies to GPX path ribbons and nothing else** — no equivalent for GLB nodes, primitives, stamps, or fallback signs.

### 1.4 The defect that pre-figures the whole feature

**A track hidden via `gis.track.visible=false` is invisible but remains fully interactive.** Every picker queries raw markers with **zero** `Visibility`/`ViewVisibility` filter:
- `node_manager/viewport_pick.rs:17-102` — `Query<(Entity, &SpawnedNodeMarker, Option<&TrackPickShape>)>` (`:18`), filtered only by active petal (`:43-48`).
- `node_manager/context_pick.rs:80-168` + `pick_ground_stamp` `:174-209`.
- `node_manager/pointer/mod.rs:27-80`, `spawned_in_petal` `:136-147`.
- `node_manager/sidebar_sync.rs:9-31`.
- `fe-ui/src/plugin.rs:705-724` (`apply_camera_focus`) — flies to any matching node_id.

Bevy `Visibility` is a **render-only** component. It gates nothing else. This is live, shipped proof that `Visibility::Hidden` alone is not a sufficient guard — and it is a pre-existing bug fixable as a side effect of this track.

**Also:** none of fe-ui's four spawn helpers ever inserts a `Visibility` component — `spawn_node_entity` (`verse_manager/spawn.rs:78-105`), `spawn_stamped_entity` (`:128-161`), `spawn_primitive_entity` (`:210-241`), `spawn_fallback_sign` (`:246-284`). fe-ui-spawned node entities have **no visibility state whatsoever** today.

---

## 2. GAP ANALYSIS — per-level eye toggles

### 2.1 Where state could live, per level (schema reality)

| Level | Generic property bag? | Round-trip that exists | Verdict |
|---|---|---|---|
| **Node** | **Yes** — `node.properties`, `TYPE option<object> FLEXIBLE` (`fe-database/src/schema.rs:195`) | `DbCommand::SetNodeProperty` (`fe-runtime/src/messages.rs:319`) / `GetNodeProperties` (`:325`) / `DeleteNodeProperty` (`:329`) → dispatch `fe-database/src/lib.rs:780-841` → `handlers/entity_property.rs:15,70,109` | Ready today |
| **Petal** | **No.** Fixed typed columns only (`schema.rs:69-85`): `visibility`, `tags`, `hexon_manifest`, `terrain` | `terrain` (`schema.rs:83`) is a FLEXIBLE JSON doc with `SetPetalTerrain`/`GetPetalTerrain` (`messages.rs:452,457`; `handlers/petal_terrain.rs:7,36`) and **already hosts the `layers` array** | Use `terrain`, or add a sibling column |
| **Fractal** | **No** property bag, **no** visibility/tags columns (`schema.rs:162-172`) | — | Nowhere to persist without a schema change |
| **Verse** | **No** property bag, **no** visibility/tags columns (`schema.rs:137-145`) | — | Nowhere to persist without a schema change |

**Do not use `petal.hexon_manifest`.** It is a *signed publishing artifact* (`fe-hexon/src/manifest.rs:95-134`, has a `signature` field over the whole manifest), round-tripped through export/import — not a mutable settings store.

### 2.2 What actually hides at each level

- **Node — two mechanisms, and the choice is load-bearing.**
  1. `Visibility::Hidden` on the spawned entity: cheap, instant, reversible. But it does **not** free the mesh-instance budget (`mesh_instance_watchdog` counts `With<Mesh3d>`, `fe-ui/src/plugin.rs:86-145`) and does **not** stop picking (§1.4).
  2. **Don't spawn at all.** All node spawning funnels through three systems — `respawn_on_petal_change` (`verse_manager/petal_respawn.rs:18-192`, spawn loop `:112-180`), `materialize_cached_primitives` (`primitive_materialize.rs:121-243`), `materialize_path_assets` (`path_asset_materialize.rs:374+`) — all already gated by the residency ledger (`spawn::distance_ranked_allowance`, `verse_manager/spawn.rs:26-55`; `ResidencyBudget` `:57-64`). Skipping hidden nodes there **frees real budget** and makes picking-of-hidden structurally impossible.
  **Recommendation: hybrid.** Hidden ⇒ not spawned (primary), plus `Visibility::Hidden` as belt-and-braces for entities fe-ui does not spawn (fe-terrain ribbons via the existing `gis.track.visible`/LayerStack paths).

- **Petal.** Hiding a **non-active** petal is nearly free — its entities are already despawned by `respawn_on_petal_change` (`petal_respawn.rs:63-83`). So petal visibility is mostly a *sidebar filter* + a "don't navigate here" semantic. Hiding the **active** petal is a semantic contradiction that needs an explicit rule (Decision 4).

- **Fractal / Verse.** Nothing renders at those levels. A fractal/verse eye is a **pure rollup**: a bulk cascade over descendant petals/nodes plus a tree-dim. Be honest about this in the UI — it is a tree-filter concept, not a render concept, unless defined as a cascade write.

### 2.3 Cross-cutting systems that must respect hidden state

Everything below queries node entities with **no** visibility awareness today.

**A. Picking / hit-test** (must skip hidden):
- `node_manager/viewport_pick.rs:17-102` (`:18`) — primary click→select raycast.
- `node_manager/context_pick.rs:80-168` (`:83-88`) + `pick_ground_stamp` `:174-209` — right-click classifier.
- `node_manager/pointer/mod.rs:27-80`, `spawned_in_petal` `:136-147` — A↔B selection bridge; second checkpoint that should refuse a hidden path.
- `node_manager/path_segment_interaction.rs:197-282` (`:201`) — ribbon shape of the edited track.
- `node_manager/sidebar_sync.rs:9-31` (`:12`) — defense-in-depth for any non-sidebar setter of `pending_sidebar_select`.

**B. Gizmo / overlay draw:**
- `node_manager/gimbal_interaction.rs:24-67` (`update_hovered_axis`), `:154-293` (drag), `:340-407` (`draw_gimbal_system`); shared `fe-ui/src/gimbal.rs:77-99` (`gimbal_center` — pure geometry, no visibility awareness; guard belongs in callers).
- Path editing markers: `path_point_interaction.rs:511-528, 549-817`; `path_handle_interaction.rs:177-189, 299-464`, stems `:469-483`; `path_gimbal_drag.rs:108-273`. **All gated by `PathEditorState.editing_track_id`** — so the correct guard is one rule ("cannot open, and must close, editing on a hidden path"), not per-marker checks.
- Labels/billboards: `fe-ui/src/viewport_labels.rs:23-64` (reads `PathEditorState.points`, indirect); `node_manager/billboard.rs:12-28` (rotates every `Billboard` unconditionally — perf only, not correctness).
- `fe-ui/src/sculpt_cursor.rs:28-58` — no node query; N/A.

**C. Terrain overlays — mostly *not* node entities:**
- `render_terrain_proposals` (`fe-terrain/src/terrain_plugin.rs:984-1040`) drives off `ActivePetalTerrain.config.proposals`, not `SpawnedNodeMarker`.
- `sync_layer_visibility` (`terrain_plugin.rs:1356-1391`) is the existing layer path — reuse, don't duplicate.
- `fe-terrain/src/splat/render.rs:188-243` is terrain-chunk LOD; unrelated entity class.

**D. Other consumers:**
- `apply_camera_focus` (`fe-ui/src/plugin.rs:705-724`) — will fly to a hidden node. Design decision (D-14).
- `transform_broadcast.rs:16-86` (outbound, selected only) and `:88-136` `apply_inbound_transforms` — **the inbound one should probably NOT be gated**: a node hidden locally should still receive remote/API transform updates.
- `inspector_sync.rs:18-97` — moot if hiding force-deselects.
- `mesh_instance_watchdog` (`plugin.rs:86-145`) — hidden-but-spawned still counts against budget.
- `fe-terrain/src/iot/animation.rs:106-146` (`advance_track_animations`) — **not registered in any `add_systems` anywhere**; treat as unwired dead code today, but it needs the guard when wired.

### 2.4 Non-system gaps

- **No auto-deselect on entity disappearance.** `respawn_on_petal_change` despawns without clearing `NodeManager.selected` (`petal_respawn.rs:63-83`); every consumer silently no-ops via `Query::get(...) else return`. Hiding the selected node needs an **explicit** deselect, or the gimbal draws around nothing.
- **RBAC hole on properties.** `SetNodeProperty`/`GetNodeProperties`/`DeleteNodeProperty` carry **no `CallerAuth`** and get **no `fe_policy` check** on the DB thread (`fe-database/src/lib.rs:780-841`) — contrast the `TombstoneNode`/`RenameNode`/`DuplicateNode` arms at `lib.rs:862-1052`, which all call `lifecycle_auth_context` → `fe_policy::authorize_node_*`. The only gate is REST-layer `require_role(&claims,"editor")` in `fe-api/src/rest.rs` (`set_node_property` ~`:476-493`). **Anything property-backed inherits zero data-layer authz.**
- **Sync/P2P.** Property writes are HLC-stamped into the op-log (`fe-database/src/op_log.rs:93-113`), but **fe-sync reads no `OpType::PropertySet` at all**; replication is whole-row `UPDATE node MERGE $row` (`fe-database/src/merge.rs:90`) with tombstone-dominance only (`:62-84`) and **no per-key LWW**. iroh-docs is mock-backed (`IrohDocsEngineHolder::is_available()` hardcoded `false`, per `fe-sync/src/AGENTS.md`). So P2P is a non-issue *today*, and a whole-row merge would clobber group membership *tomorrow*.
- **The cache tax.** Because the hierarchy payload carries no properties (`verse_manager/AGENTS.md:104-107`), a property-backed node visibility cannot be read from `VerseManager` — it needs a third `*Cache` resource fed off `NodePropertiesLoaded` in `db_results/properties.rs`, mirroring `PrimitiveDescriptorCache`/`PathAssetCache`. **This is the single largest hidden cost of any property-backed option.**

---

## 3. VISIBILITY GROUPS — architecture options

### Option A — UI-only ephemeral groups (fe-ui `Resource`)

| Aspect | |
|---|---|
| **State home** | New `VisibilityState { hidden_nodes: HashSet<String>, hidden_scopes: HashSet<String>, groups: Vec<UiGroup{name, members, visible}>, isolate: Option<GroupRef> }` — a plain `Resource` next to `LeftSidebarState` |
| **Persistence** | None. Dies on app close, and on petal switch unless explicitly retained (matching `LeftSidebarState`'s session-scoped Q-2 rule, `left_sidebar.rs:1-7`) |
| **Sync/P2P** | None. Zero merge risk |
| **RBAC** | None needed — a local view filter is not a security boundary |
| **Effort** | **~350-500 lines**, all in fe-ui, no DB/runtime/api changes |
| **Thesis fit** | **Poor.** A group is not addressable, not an endpoint, invisible to API/MCP. Directly against "every artifact is an addressable read/write endpoint" |

### Option B — Persisted petal-scoped group registry (membership held by the group)

| Aspect | |
|---|---|
| **State home** | A `visibility_groups` array inside the petal `terrain` doc (`fe-database/src/schema.rs:83`), sibling to `layers` — or a new `petal.visibility_groups` FLEXIBLE column with its own `Set/GetPetalVisibilityGroups` `DbCommand` pair modelled on `SetPetalTerrain` (`messages.rs:452,457`) |
| **Persistence** | Survives reload. Petal-scoped, which matches the entity model (nodes belong to petals) |
| **Sync/P2P** | Whole-doc read-modify-write required (the `embed_region` idiom, `actions/terrain_proposal.rs:551-583`) — **do NOT copy the pre-fix `embed_proposals` wholesale-overwrite** (finding #1 of `ui_semantics_unification_20260808/spec.md`). Merge = whole-row replace today, so concurrent group edits from two peers would clobber |
| **RBAC** | Inherits `SetPetalTerrain`'s gate (also unchecked on the DB thread; REST checks at `fe-api/src/server.rs:184-188`). A dedicated `DbCommand` with `auth: CallerAuth` following the `RenameNode` template (`fe-database/src/lib.rs:1005-1052`) would fix this properly |
| **Effort** | **~700-1000 lines** across fe-ui + fe-runtime + fe-database (+ fe-api if endpoints) |
| **Thesis fit** | **Good for the registry** (`GET/PATCH /api/v1/petals/{id}/visibility-groups/{gid}` slots straight into the existing flat-route convention, `fe-api/src/server.rs:136-146`). **Bad for membership**: a 10k-node petal's membership lists live in one JSON blob, every add is a whole-doc write, and node deletion leaves dangling ids (tombstone/cascade at `fe-database/src/lib.rs:862-961` won't clean them) |

### Option C — Property-tag-based (membership on the node; groups are queries)

| Aspect | |
|---|---|
| **State home** | `node.properties["view.groups"] = ["utilities","phase-2"]` and `node.properties["view.hidden"]` — the existing FLEXIBLE bag (`schema.rs:195`), written via `SetNodeProperty` (`messages.rs:319`) |
| **Persistence** | Per node, durable, HLC-stamped in the op-log (`op_log.rs:93-113`) |
| **Sync/P2P** | Rides the existing (dormant) node replication. Whole-row merge would still clobber the whole `properties` object — same caveat as everything property-backed |
| **RBAC** | **Zero data-layer check** (§2.4). If group membership is ever security-relevant, this option is wrong as-is |
| **Effort** | **~500-700 lines** — but **plus** a new petal-wide cache fed off `NodePropertiesLoaded` (`db_results/properties.rs`), the third instance of that idiom; that is where the real cost is |
| **Thesis fit** | **Excellent for membership.** Group membership becomes a queryable predicate (fe-query / `list_nodes_by_kind`-style, `fe-api/src/endpoint.rs:393-434`); node delete removes membership for free; scales to 10k+ stamps. **Bad for the registry**: a group has no identity of its own — no name, no visible flag, no color, no addressable endpoint. "Hide the Utilities group" has nowhere to be stored |

### 3.1 Compose semantics — recommended

Three sources of truth must resolve to one boolean. Recommended precedence, **highest first**:

1. **Explicit per-node override — tri-state `Show` / `Hide` / `Auto` (default `Auto`).** Only a non-`Auto` value participates. `Show` beats a hidden group (the "I know, show it anyway" escape hatch); `Hide` beats everything.
2. **Ancestor chain** (verse → fractal → petal): **any hidden ancestor hides.** Non-negotiable — an eye on a petal that doesn't hide its contents is a lie.
3. **Groups: hidden if ANY containing group is hidden** (equivalently: visible requires **all** containing groups visible).

**Why ANY-hides for groups, not ALL:** CAD convention (AutoCAD/Revit layers) is one-object-one-layer, so the question never arises. With multi-membership, "hide the Utilities group" *must* actually hide everything in Utilities, or the control is useless — ALL-semantics would mean a node in `{Utilities, Phase-2}` stays visible when you hide Utilities, which no user will predict. The `Show` override at level 1 buys back the exception case.

**Isolate / solo is NOT part of this lattice.** It is a transient session overlay: `solo(X)` ⇒ everything not in X renders hidden, nothing is persisted, cleared on petal switch and on Escape. Modelled on `LeftSidebarState.user_intent`'s session scoping (`left_sidebar.rs:1-7`). Keeping solo out of the persisted lattice prevents "I isolated something last Tuesday and lost my model."

---

## 4. INTERACTION DESIGN

### 4.1 Tree row layout

Reuse the row frame at `sidebar.rs:338-367` and the right-aligned gutter idiom already used by the verse header (`sidebar.rs:125-148`, `Layout::right_to_left`):

```
[▾] Fractal name ............................ [eye]
    [▾] Petal name .......................... [eye]
        ◆ Node name ......................... [eye]
```

- **Hidden row styling: luminance only, no hue** (`ui_ux.md:20-27`, §1). Reuse `theme::TEXT_MUTED` (`theme.rs:20`) — literally the same treatment the tree already gives non-active-petal nodes (`sidebar.rs:351-357`). A hidden row therefore reads as "inert" using vocabulary already in the product.
- **Eye glyph:** the tree currently uses BMP geometric shapes only (`\u{25C6}` / `\u{25CF}`, `sidebar.rs:343`). Recommend filled/hollow pair `●`/`○` (`\u{25CF}`/`\u{25CB}`) or `◉`/`◌` rather than `\u{1F441}` — **verify glyph coverage in the bundled egui font before committing to an emoji eye.**
- **Indeterminate state:** a fractal whose petals are mixed needs a third glyph (e.g. `◐`). Do not fake it with a boolean.
- **Persistent status-bar chip when anything is hidden.** Per `ui_ux.md:143-148`, "geometry is hidden" is an *abnormal condition the user must be able to act on*, not a toast. Extend `panels/status_bar.rs` with a dim `"3 hidden"` segment that clears when nothing is hidden. **This is the single most important safety affordance in the feature** — without it, users lose work and blame the app.

### 4.2 Toggle / isolate gestures

- **Click the eye** = toggle that row (with cascade per §3.1 rule 2).
- **Isolate ("solo")**: the obvious binding is `Alt`+click on the eye, but the §5 modifier table already assigns `Alt+click` = "options / annotate for the clicked thing" (`ui_ux.md:127-132`, implemented at `sidebar.rs:371-378`). **Recommended instead: a per-row right-click context menu** (`Isolate`, `Hide others`, `Show all`, `Add selection to group…`). Adds no modifier meaning, matches the object-aware-menu direction, and gives the group verbs a home. (D-8.)
- **Drag membership: do not build it in v1.** There is no sidebar drag idiom to reuse (§1.1) — it would be a brand-new interaction primitive in the same pass as a new data model. Use `Add selection to group…` from the group row plus the node context menu instead.

### 4.3 Where group management lives

**Split by role, matching the in-flight unification thesis:**
- **Left sidebar = navigator.** Hosts the eye toggles on tree rows, plus a collapsed **`Groups`** section (sibling of the existing `Space` section, `sidebar.rs:424-449`) listing each group with its own eye + member count. Read + toggle only.
- **Right sidebar Options = editor.** Group create/rename/delete/recolor and membership editing get a new section reached by `UiAction::RevealSection { slug: "groups" }` — the addressable UI-surface handle introduced by `ui_semantics_unification_20260808`. This is why the two tracks must sequence, not parallelize.

**Register with the §7 overlay contract.** `ui_ux.md:167-172` already mandates a **[future-track]** unified overlay-layer registry where every renderable category registers `(name, visible, opacity)` in ONE registry surfaced by the Layer Manager card ("New overlay features MUST register rather than invent a bespoke toggle"). **Visibility groups are exactly the feature that should generalize `LayerUiEntry` from terrain-only into the shared registry** — group rows and terrain-layer rows should be the same row widget, and the Layer Manager card (`panels/layer_manager_card.rs:119-183`) should become reachable from the left sidebar rather than buried in the Data window's GIS panel (`gis_panel.rs:100`).

### 4.4 Keyboard

Existing bindings are `S/G/R/X/P/B` + `Ctrl+B` (`panels/toolbar.rs:38-73`, `node_manager/shortcuts.rs:14-16, 39-43`). `H` is free.
- `H` — hide selection · `Shift+H` — show all · `Alt+H` — isolate selection · `Esc` — clears isolate (slots in as a new rung above the existing staged ladder; **must be coordinated with the ladder rewrite in `ui_semantics_unification` Phase 2**).

### 4.5 Empty / calm states

Per `ui_ux.md:173-175`: an empty group shows a dim `"no members — select nodes and use Add to group"` line, never nothing. A fully-hidden petal shows a dim `"all contents hidden"` in the tree, not an empty branch.

---

## 5. RECOMMENDATION + DECISION POINTS

### 5.1 Recommended architecture: **B ⊕ C hybrid**

> **Group registry = Option B** (petal-scoped, persisted, addressable: `{id, name, visible, order}`).
> **Group membership = Option C** (node property `view.groups: [group_id]`).
> **Per-node override = Option C** (node property `view.state: "auto"|"show"|"hide"`).
> **Isolate/solo = Option A** (session-only resource, never persisted).

Rationale: B gives each group a real identity and a real endpoint (satisfying the addressability thesis) while staying tiny — a registry is O(groups), not O(nodes). C keeps membership O(1)-per-node, queryable through fe-query/fe-api, automatically cleaned up by the existing tombstone/cascade path (`fe-database/src/lib.rs:862-961`), and free of whole-doc read-modify-write races. A keeps solo out of persistence, where it belongs.

**Non-negotiable engineering rules for this track:**
1. **Hidden ⇒ not spawned** (guard the three spawners: `petal_respawn.rs:112-180`, `primitive_materialize.rs:145-157`, `path_asset_materialize.rs:374+`), so picking-of-hidden is structurally impossible and the residency budget actually benefits. `Visibility::Hidden` is belt-and-braces only.
2. **One pure resolver.** A single `effective_visibility(node_id, groups, overrides, ancestors) -> bool` pure fn, unit-tested, consumed by the spawners, the sidebar renderer, and the pickers. No surface re-derives it. (Same discipline as `left_visibility` / `active_section`.)
3. **Give group membership its own `DbCommand` with `auth: CallerAuth`** following the `RenameNode` template (`fe-database/src/lib.rs:1005-1052`) rather than piggybacking on unauthenticated `SetNodeProperty` — otherwise the feature ships with zero data-layer authz (§2.4).

### 5.2 Decision points to ratify

1. **Fractal/verse rows get eyes?** (A) Yes, persisted (schema migration). (B) Yes, session-only/UI-filter. (C) No — eyes stop at petal.
2. **B ⊕ C hybrid architecture?** Yes / No / counter-proposal.
3. **Group registry home:** (A) inside petal `terrain` doc (reuses `SetPetalTerrain`). (B) new `petal.visibility_groups` column + auth-carrying `DbCommand` pair.
4. **Hiding the ACTIVE petal:** (A) forbidden (eye disabled + tooltip). (B) allowed — navigates away first. (C) allowed — contents hide, petal stays active.
5. **Compose precedence** — override > ancestors > ANY-containing-hidden-group hides? Yes / No (ALL-hides).
6. **Tri-state per-node override** (`Auto`/`Show`/`Hide`)? Yes / No.
7. **Hidden ⇒ not spawned** (guard spawners) rather than spawn-then-hide? Yes / No.
8. **Isolate gesture:** (A) per-row right-click menu (`Isolate`/`Hide others`/`Show all`). (B) Alt+click eye (conflicts with §5 modifier table).
9. **Drag-to-assign deferred out of v1**, use `Add selection to group…`? Yes / No.
10. **Group management in right-sidebar Options** via `RevealSection { slug: "groups" }`, left sidebar read+toggle only? Yes / No.
11. **Generalize `LayerUiEntry` into the unified (name, visible, opacity) registry** mandated by `ui_ux.md:167-172`? Yes / No.
12. **Fix the `gis.track.visible` picking bug in this track** (§1.4)? In-scope / separate.
13. **Hiding the selected node force-deselects?** Yes / No.
14. **`apply_camera_focus` refuses hidden targets?** Yes / No.
15. **Auth-carrying `DbCommand` for visibility-group writes** (not raw `SetNodeProperty`)? Yes / No.
16. **Persistent status-bar "N hidden" chip?** Yes / No.
17. **Keyboard:** `H` hide / `Shift+H` show-all / `Alt+H` isolate, isolate-clear as a new Esc rung? Yes / No / other.
18. **Terminology:** "Group" / "Layer" (collides with terrain layers) / "View set"? (Needs a `ui_ux.md` §9 entry.)

### 5.3 Phase sketch (file-level)

| Phase | Goal | Files | Size |
|---|---|---|---|
| **0** | Pure resolver + state types, no UI. `effective_visibility()` + `VisibilityState` resource + `GroupRegistry` types, fully unit-tested | new `fe-ui/src/visibility/mod.rs` (+ `resolve.rs`) | ~300 lines, ~all tests |
| **1** | **Sidebar read-only rendering.** Eye glyphs + muted hidden rows + indeterminate state, session-only toggling | `fe-ui/src/panels/sidebar.rs` (`:153-400`), `fe-ui/src/theme.rs` | ~350 lines |
| **2** | **Make hiding real.** Guard the three spawners + pickers + camera focus + force-deselect | `verse_manager/petal_respawn.rs:112-180`, `primitive_materialize.rs:145-157`, `path_asset_materialize.rs:374+`, `node_manager/viewport_pick.rs:17-102`, `context_pick.rs:80-168`, `pointer/mod.rs:27-80`, `plugin.rs:705-724` | ~300 lines; **highest regression risk** |
| **3** | **Persistence.** `view.state`/`view.groups` node properties + the `NodePropertiesLoaded`-fed cache (third instance of the idiom) | new `fe-ui/src/visibility/cache.rs`, `verse_manager/db_results/properties.rs`, `fe-ui/src/actions/mod.rs` | ~400 lines |
| **4** | **Group registry + petal persistence + auth-carrying command** | `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs` (+ `handlers/`), `fe-policy` reuse, `fe-ui/src/actions/` | ~450 lines |
| **5** | **Right-sidebar Groups section + `RevealSection { slug: "groups" }` + Layer-registry unification** | `ui_shell/right_sidebar.rs`, `panels/layer_manager_card.rs`, `gis/layers.rs` | ~350 lines |
| **6** | **API/MCP endpoints** — `GET/PATCH /api/v1/petals/{id}/visibility-groups[/{gid}]`, MCP tool | `fe-api/src/endpoint.rs`, `server.rs`, `mcp.rs` | ~250 lines |

Phases 0-2 deliver a genuinely useful feature (session-scoped hide/isolate that actually works) with **zero** schema, DB, runtime or API changes. That is the shippable slice if ratification stalls.

### 5.4 Track sequencing — AFTER, not beside, `ui_semantics_unification_20260808`

**CLEAN (untouched by the in-flight waves):** `panels/sidebar.rs`, `ui_shell/left_sidebar.rs`, `navigation_manager.rs`, `theme.rs`, `panels/layer_manager_card.rs`, `gis/layers.rs`, `verse_manager/{spawn.rs, petal_respawn.rs, primitive_materialize.rs, db_results/properties.rs, db_results/hierarchy.rs}`, `node_manager/{sidebar_sync.rs, gimbal_interaction.rs, pointer/mod.rs}`.

**DIRTY (mid-edit by unification waves or the canonical-data-log session):** `plugin.rs`, `panels/mod.rs`, `panels/toolbar.rs`, `ui_shell/right_sidebar.rs`, `ui_shell/topbar.rs`, `viewport.rs`, `actions/mod.rs`, `actions/path.rs`, `verse_manager/mod.rs`, `verse_manager/path_asset_materialize.rs`, `node_manager/{viewport_pick.rs, context_pick.rs, mod.rs, router.rs, shortcuts.rs, transform_broadcast.rs, dispatch.rs}`, plus `fe-runtime/src/messages.rs`, `fe-database/src/{lib.rs, handlers/entity_property.rs}`, `fe-api/src/{server.rs, rest.rs, endpoint.rs}`.

**Conclusion:** Phases 0-1 are parallel-safe today (disjoint file set). Phases 2-6 must serialize after the unification waves (16-SystemParam ceiling on `gardener_ui_system`, `actions/mod.rs` variants, pickers all mid-edit; Phase 5 depends on `UiAction::RevealSection` from unification Phase 4). Textbook "leaf logic parallelizes, integration doesn't."

---

## RATIFICATION — 2026-08-08 (all 18 decisions, user-grilled in two rounds)

| # | Decision | Ruling |
|---|---|---|
| 1 | Fractal/verse eyes | **PERSISTED — schema migration for fractal + verse tables (OVERRIDE of session-only rec)** |
| 2 | Architecture | **B⊕C hybrid ratified** (registry persisted petal-scoped; membership = node property `view.groups`; tri-state override `view.state`; solo session-only) |
| 3 | Registry home | **New `petal.visibility_groups` FLEXIBLE column + auth-carrying `Set/GetPetalVisibilityGroups` DbCommand pair** (CallerAuth through fe_policy, RenameNode template) |
| 4 | Active petal | **Forbidden — eye disabled + tooltip** |
| 5 | Compose precedence | **override > ancestor chain > ANY-containing-hidden-group hides** |
| 6 | Per-node override | **Tri-state Auto/Show/Hide** |
| 7 | Hide mechanism | **SPAWN-THEN-HIDE (OVERRIDE of hidden⇒not-spawned rec).** Consequences accepted: every picker + camera-focus + gimbal path MUST filter on effective visibility (mandatory core work, not belt-and-braces); hidden nodes still count against the mesh-instance budget; toggling is instant (no respawn round-trip) |
| 8 | Isolate gesture | **Per-row right-click menu** (Isolate / Hide others / Show all / Add selection to group…) |
| 9 | Drag-to-assign | **Deferred out of v1** — `Add selection to group…` instead |
| 10 | Group management home | **Right-sidebar Options via `RevealSection { slug: "groups" }`; left sidebar read+toggle only** |
| 11 | Layer registry | **Unify — generalize `LayerUiEntry` into the shared (name, visible, opacity) registry per ui_ux.md §7** |
| 12 | `gis.track.visible` picking bug | **In-scope — with spawn-then-hide the picker filters ARE the core mechanism** |
| 13 | Hide selected node | **Force-deselects** |
| 14 | Camera focus on hidden | **Refuses** |
| 15 | Authz | **Yes — auth-carrying DbCommand, no raw SetNodeProperty for group writes** (merged into #3) |
| 16 | Status-bar chip | **Yes — persistent "N hidden" chip** |
| 17 | Keyboard | **H hide / Shift+H show all / Alt+H isolate; Esc clears isolate as a new ladder rung (coordinate with unification Phase 2 ladder)** |
| 18 | Terminology | **"Group"** — needs the ui_ux.md §9 baked-terminology entry |

**Phase-plan amendments from the D-7 override:** Phase 2 no longer gates the three spawners; instead it (a) adds a `sync_node_visibility` apply-system writing `Visibility::Hidden/Inherited` from the resolver, (b) adds effective-visibility filters to every picker (`viewport_pick`, `context_pick`, `pointer`, `sidebar_sync`, `path_segment_interaction`), camera focus, and the selection bridge, (c) force-deselect on hide. Mesh-budget relief is forfeited (documented). Phase 4 gains the fractal/verse schema migration (D-1) alongside the petal registry column.

**Execution status:** Phases 0-1 launched 2026-08-08 in parallel with unification Wave 3 (disjoint file sets). Phases 2-6 remain gated on unification completion.
