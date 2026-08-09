---
type: Code Styleguide
title: UI/UX & Human-Machine Interface Styleguide
tags: [enforcement, hmi, ux, 2026-07-17]
timestamp: 2026-07-17T00:00:00Z
---

# UI/UX & Human-Machine Interface Styleguide — FractalEngine

Provenance: distilled 2026-07-17 from ISA-101 / High-Performance HMI (grey-chrome
situational awareness, alarm color discipline, display hierarchy), AVEVA PI System /
PI Vision (asset-framework scoping, trend-first displays, copyable tag egress), and
Neara (engineering-grade units, single real-world scale authority, measurement-first
interaction), each mapped to a concrete FractalEngine surface.

Rule tags: **[codified-now]** = matches shipped code or fixes landing in the current
batch; **[adopt-when-touched]** = apply whenever you edit the surface; **[future-track]**
= needs a dedicated track before it can be enforced.

## 1. Design language: calm chrome, color = state

- **[adopt-when-touched]** Chrome is neutral and low-saturation. Normal/healthy states
  render in greys with luminance contrast only; a normal app looks calm so an abnormal
  condition is the only thing that pops. Saturated hue is reserved EXCLUSIVELY for
  (a) abnormal-state tiers below and (b) user-authored data colors (annotation
  swatches, data icons `ICON_TRACK/POINT/WAYPOINT`, `fe-ui/src/theme.rs:55-57`).
- **[adopt-when-touched]** Active/emphasis states use luminance, not hue. Known
  violations to migrate on touch: `BG_BUTTON_ACTIVE` saturated blue (`theme.rs:9`),
  `BG_SAVE` green chrome on an ordinary button (`theme.rs:11`, used for Run Query at
  `gis_panel.rs:326`), `STATUS_ONLINE_DOT = Color32::GREEN` for a NORMAL condition
  (`theme.rs:24`, rendered every frame at `status_bar.rs:33-39`).
- **[adopt-when-touched]** Fixed status vocabulary — four tiers, each owning exactly
  one meaning. Never reuse a tier constant outside its tier.

| Tier | `theme.rs` constant | Only meaning | Current state |
|---|---|---|---|
| Normal | `STATUS_NORMAL` (grey, `TEXT_DIM`-class) | healthy / connected / idle | missing — `STATUS_ONLINE_DOT` is saturated green (`theme.rs:24`); demote |
| Advisory | `STATUS_ADVISORY` (amber) | validation feedback ("bad input, nothing broke") | missing — validation errors reuse `STATUS_OFFLINE` (`gis_panel.rs:200-207,332-339`, `annotation_card.rs:73`); migrate |
| Warning | `STATUS_WARNING` (orange) | connectivity loss / degraded service | `STATUS_OFFLINE` (`theme.rs:26`) currently doubles as this and as advisory |
| Error | `STATUS_ERROR` (red) | data loss / operation failed / denied | `BG_DANGER` (`theme.rs:10`) covers destructive chrome only |

## 2. Units & scale: real units, one authority

- **[codified-now]** Every displayed number carries its unit: lengths/positions `m`,
  angles `°`, areas `m²`. Reference implementation: inspector Position/Rotation/Size
  (`fe-ui/src/panels/inspector.rs:216-275`).
- **[codified-now]** One scale authority: the map sets the scale (binding 2026-07-16
  directive, `conductor/tracks/map_scale_authority_20260716/spec.md:17-24`). All
  conversions route through the canonical accessor (`real_m = world / world_scale`);
  per-asset scale metadata is banned. The `world_to_meters`/`meters_to_world` pair
  (`fe-ui/src/panels/widgets.rs`) is the ONE conversion seam — inspector, gis_panel,
  and egress_card all import it; do not hand-roll a conversion.
- **[adopt-when-touched]** No unlabeled world units. Known split to close: GIS result
  coords and bbox fields are raw world tuples (`gis_panel.rs:472-479`, `:283-300`),
  Paths point rows and `Thickness`/`Radius X/Z` are unlabeled (`path_editor_card.rs:401-404`,
  `tool_panel.rs:520-542`), while the inspector speaks meters — same position, two
  readings when `world_scale != 1.0`.
- **[adopt-when-touched]** Raw world units may appear ONLY in explicitly-labeled
  diagnostic contexts: suffix `wu` and say so (e.g. MCP `update_transform` is
  world-units-on-the-wire — document it in the inputSchema, `fe-api/src/mcp.rs:118-155`).
- **[adopt-when-touched]** The two runtime authorities must agree: ruler HUD uses
  `effective_world_scale()` (`fe-terrain/src/ruler_plugin.rs:72`) while every fe-ui
  readout uses `PetalMapState.world_scale` (`path_segment_interaction.rs:186`).
  Unification is map_scale_authority_20260716 scope; until it lands, new surfaces use
  `PetalMapState.world_scale` and note the seam.

## 3. WYSIWYE egress: what you see is what you can egress

- **[adopt-when-touched]** Any panel that renders query results MUST expose a
  copy-for-BI affordance (SQL string + export URL) for exactly the query shown. This
  is the primary product feature (`conductor/roadmap.md` §1). Current gap: the GIS
  Query tab's three modes (`gis_panel.rs:229-245`) have no egress path, and the Export
  tab rebuilds only Petal/Node/Bbox scopes (`egress_card.rs:82-86`) — Query-tab and
  Export-tab scopes must be the same enum, not parallel ones.
- **[codified-now]** Machine-consumable strings (IDs, SQL, URLs, connection strings,
  curl commands) always render via `copy_value_box`/`copy_row`
  (`fe-ui/src/panels/widgets.rs:9-30`, `egress_card.rs:59-68`): monospace,
  width-capped, one-click copy + toast. Never a plain selectable label.
- **[codified-now]** Command snippets are portable: emit `curl`, never
  `curl.exe` (both emitters conform: `inspector.rs:783-784`, `gis/egress_strings.rs`).
- **[codified-now]** Egress must be discoverable: surfaces that lead to
  copy-for-BI say so in their labels/tooltips (window is "Data — Query, Layers &
  Export", `gis_panel.rs:46`; toolbar "Data" button hover names export).

## 4. Hierarchy & navigation

- **[adopt-when-touched]** Every data-bearing panel declares which
  Verse>Fractal>Petal>Node scope its data comes from. Shipped precedent: the Query
  tab's "Scope:" chip via `build_nav_scope` (`fe-ui/src/panels/query_tab.rs:32-42`).
  Anti-pattern: GIS panel silently binds `nav.active_petal_id` (`gis_panel.rs:56-63`)
  and the Export tab mints petal-scoped SQL without naming the petal.
- **[future-track]** A standard scope-breadcrumb widget (in `panels/widgets.rs`,
  reusing `build_nav_scope`) becomes mandatory on GIS Query, Annotations, Paths, and
  Export tabs.
- **[adopt-when-touched]** Four-level display hierarchy, ≤2 interactions between
  adjacent levels, every level links down (click-through) and up (breadcrumb):
  L1 verse overview → L2 petal map/viewport + layers → L3 node inspector → L4 raw
  query display (`query_tab.rs`).
- **[future-track]** L1 does not exist yet — the closest surface is the status-bar
  count string (`status_bar.rs:84-91`). Seed: extend `atlas/dashboard.rs` into a grid
  of petal cards (sync state, abnormal counts), rendered grey-normal per §1.

## 5. Interaction semantics

- **[codified-now]** Selection authority: viewport selection (`NodeManager.selected`)
  and tab-local editing targets (`PathEditorState.editing_track_id`) are DISTINCT
  concepts. A panel never silently reads another context's selection; cross-context
  flow is an explicit pull the user performs. Reference implementation: the egress
  card's "Use viewport selection" button copying into panel-local state
  (`egress_card.rs:100-115`). Paths-tab-class features key ONLY on their tab-local
  target.
- **[codified-now]** Staged Escape: first press exits the active editing mode
  (`stop_editing()`), second press clears node selection; toolbar Deselect does both
  (`node_manager/shortcuts.rs:34-36`, `toolbar.rs:71-77`). Editing state never
  survives a petal switch (`gis/mod.rs:301-308` call sites).
- **[codified-now]** ONE destructive-action convention — mandatory: inline
  two-step confirm ("Delete" → "Confirm Delete"), canonical implementation
  `dialogs/entity_settings.rs:242-284`. No single-click destruction. Conforming:
  sidebar "Reset Database" (`sidebar.rs:47-100`), path-list delete
  (`path_editor_card.rs:205-263`), token revoke (`inspector.rs:908-946`). Sole
  remaining variant to migrate on touch: hexon_manager's "Yes"/"No" remove
  (`hexon_manager.rs:352-388`).
- **[adopt-when-touched]** Modifier-key table — one meaning per modifier, all
  surfaces; hint text and tooltips generate from one table so they cannot drift
  (`viewport.rs:111-116`):

| Input | Meaning everywhere |
|---|---|
| `Alt`+click | options / annotate for the clicked thing (`sidebar.rs:351-355`, `path_editor_card.rs:358`) |
| `Ctrl`+drag | constrain to vertical height |
| `Ctrl`+`Enter` | run/submit the panel's query (`query_tab.rs:79-80`; GIS Run Query must match) |
| `Esc` | staged cancel per above |

- **[adopt-when-touched]** Measurement-first: any tool that places or moves geometry
  shows live real-meter dimensions DURING the gesture, using `ruler.rs` math +
  `nice_number` formatting (`fe-terrain/src/ruler.rs:1-45`). The road_builder_ux live
  length readout is the mandated pattern, not an exception.
- **[future-track]** Tape/area/bearing measurement tools reachable from the default
  viewport in one action (hexon_scale_orchestration Phase 5).

## 6. Feedback

- **[adopt-when-touched]** Notification tiers by persistence: completed action →
  transient toast (shipped, `panels/mod.rs:155-183`); abnormal condition → persistent
  status-bar segment that clears on resolution (extend `status_bar.rs`); never a
  vanishing toast for a failure the user must act on. Already mandated by
  `conductor/product-guidelines.md` §Error Handling; only the toast tier is
  implemented today (`plugin.rs:347-351` bridges toast unconditionally).
- **[codified-now]** Every silent failure is a bug. No log-only aborts on user input
  (reference: `actions/transform.rs` `apply` returns `Err` → dispatcher toasts), no
  clickable no-ops (`dialogs/context_menu.rs` conforms), no UI that fakes persistence
  (`inspector.rs:1356-1385`, `sidebar.rs:399-415`). If an edit auto-saves, say so; if
  it needs a Save button, show one; if it failed, toast why.
- **[adopt-when-touched]** No dev jargon in user-facing copy: crate names, `AGENTS.md`
  pointers, "residual", repo paths stay in code comments
  (`layer_manager_card.rs:59,67` is the standing violation).
- **[future-track]** Queryable event history: last-N outcomes with timestamps (seed of
  PI-style event frames), so a missed failure leaves a trace.

## 7. Layers & calm defaults

- **[codified-now]** Default viewport = terrain + assets + scale bar + selection
  highlight, nothing else. Every additional overlay is either (a) gated to the
  tool/editing mode that needs it — reference pattern `viewport_labels.rs:1-7` — or
  (b) an opt-in layer. Screen-space HUD elements are fixed-size and edge-anchored
  (`status_bar.rs:18-19`).
- **[future-track]** Unified overlay-layer contract: every renderable overlay category
  (terrain, paths, annotations, IoT markers, measurements, graticule) registers a
  `(name, visible, opacity)` entry in ONE registry surfaced by the Layer Manager card.
  Today `LayerUiEntry` is terrain-only (`gis/layers.rs:9-14`); scale-bar and
  path-overlay booleans migrate first. New overlay features MUST register rather than
  invent a bespoke toggle.
- **[adopt-when-touched]** No invisible empty states for scale-bearing HUD: when no
  map scale exists, show a dimmed "unscaled (world units)" state, not nothing
  (`ruler_plugin.rs:56-58`).

## 8. Presets over bare numbers

- **[adopt-when-touched]** Any physical parameter with conventional engineering values
  gets a named preset row (chips or combo) with the raw metric value still editable
  underneath. Shipped reference: hexon manager scale presets + bounded log slider
  (`dialogs/hexon_manager.rs` `render_scale_controls`). Next targets: path width
  (road classes) and stamp spacing when road_builder_ux lands; snap angles (45°/90°).

## 9. Terminology (baked decisions — do not re-litigate)

| Concept | User-facing word | Never in UI copy | Notes |
|---|---|---|---|
| Authored/imported linework | **path** | "track" in any label | internal identifiers (`editing_track_id`, `gpx_type=track`) unchanged; "track" allowed only in GPX-format contexts (e.g. Export GPX tooltip) |
| Terrain artifact | **map** / **Map Manager** | "tileset"; "hexon" as the artifact | "hexon" reserved for the package format in publish/import contexts ("Install map from .hexon file") |
| Node webpage field | **Portal URL** | "Webpage URL", "External URL" | one label in Node Options and inspector (`node_options.rs:49,73`, `inspector.rs:535-599`); both save paths trim identically |
| Petal contents | (per existing labels) | "room" | "room" removed from status bar, viewport, and sidebar Space overview (2026-07-17) |
| Named visibility set of nodes | **Group** | — | left sidebar lists + toggles; management lives in right-sidebar Options surface; distinct from terrain "Layer" |
| Dev internals | — | crate names, `AGENTS.md`, "residual", repo paths | see §6 |

The verse/fractal/petal vocabulary is **canon** (D-72 RATIFIED 2026-07-17: keep the
jargon in all user-facing UI). Rules: never rename hierarchy tiers to domain terms,
don't mix hierarchy terms in one sentence, and first-run comprehension is served by
plain-language framing (e.g. the empty verse browser's one-line hierarchy explainer),
never by renaming.

## Pre-merge checklist for any fe-ui change

- [ ] New colors come from `theme.rs`; no saturated hue on a normal state (§1)
- [ ] Every displayed number has `m` / `°` / `m²` (or a labeled `wu` diagnostic) (§2)
- [ ] Query-result panels expose copy-for-BI; pasteable strings use `copy_value_box`/`copy_row` (§3)
- [ ] Panel declares its hierarchy scope (§4)
- [ ] No implicit read of `NodeManager.selected`; destructive actions use inline two-step confirm (§5)
- [ ] Failures surface per the tier table; no dev jargon in copy (§6)
- [ ] New overlays are mode-gated or layer-registered; default viewport stays calm (§7)
- [ ] Labels use path / map / Portal URL vocabulary (§9)
