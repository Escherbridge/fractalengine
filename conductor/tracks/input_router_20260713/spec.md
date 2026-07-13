---
type: Track Spec
title: Input Router — Unified Pointer/Click Arbitration for fe-ui
tags: [feature, ui, input, router, node_manager, input_router_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Input Router

**Track ID:** `input_router_20260713`
**Crates:** `fe-ui`

## Vision / Why

Three `fe-ui` `node_manager` systems currently race the same left-click,
coordinated only by ad-hoc boolean flags:

- `viewport_pick.rs` (`handle_viewport_click`) guards on `manager.is_dragging()`
  and `manager.path_edit_capturing` before doing entity pick/deselect.
- `gimbal_interaction.rs` (`handle_gimbal_interaction`) sets the drag state
  (`NodeSelection.drag`) that `is_dragging()` reads.
- `path_point_interaction.rs` (`handle_path_point_interaction`) sets
  `manager.path_edit_capturing` to claim the click for path-point
  place/drag/annotate.

Every new click-consumer forces every *existing* system to remember another
flag to check. This already caused a real regression: adding the pen tool
initially left `path_edit_capturing` un-gated, so pen-mode clicks stole
model-selection clicks in `Select` mode. It was fixed reactively at commit
`ab9c53c` by gating `path_edit_capturing` on `Tool::Pen` inside
`path_point_interaction.rs` (see line ~145-146 and the `Tool::Pen` check at
~222-224). That fix is correct but narrow — it patches one collision, not the
pattern. The next click-consumer (glb mesh picking, already queued as
`glb_mesh_picking_20260713`) will hit the same class of bug unless ownership
of "who gets this frame's left-click" is centralized.

The durable fix is a router: one arbitration system that resolves click
ownership per-frame using a declared priority list, so consumers ask
"am I the owner this frame?" instead of each maintaining its own guard
against every other system.

## Functional Requirements

- **FR-1:** Introduce a `ClickArbiter` (or `PointerRouter`) Bevy `Resource`
  where systems register a click-consumer identity with a priority. Priority
  order, highest first: `Gimbal` > `PathMarker` (drag/annotate an existing
  path point) > `PathPlace` (pen tool append) > `NodePick` (glTF/node
  selection). This mirrors the current implicit ordering encoded by the
  `.chain()` system order in `node_manager/mod.rs` (`gimbal_interaction` →
  `path_point_interaction` → `viewport_pick`), but makes it an explicit,
  inspectable priority table instead of an emergent property of system
  ordering.
- **FR-2:** One arbiter system runs **before** the consumer systems each
  frame. It resolves who owns the current left-click — respecting the
  egui pointer-capture guard (`egui_ctx.ctx_mut().is_using_pointer()`,
  currently duplicated per-system, e.g. `viewport_pick.rs` ~32-39) and the
  `ViewportRect` containment gating (currently duplicated per-system, e.g.
  `viewport_pick.rs` ~44-46, `gimbal_interaction.rs` ~43-45) — and exposes
  the decision (an owner enum + the resolved cursor position / camera ray)
  so consumer systems check "am I the owner this frame?" rather than
  re-deriving guard conditions or racing booleans.
- **FR-3:** Dispatch the full pointer lifecycle: press / drag / release /
  hover. The current code only coordinates press plus one ad-hoc drag flag
  (`NodeManager.is_dragging()`, backed by `NodeSelection.drag: Option<AxisDrag>`)
  and one ad-hoc capture flag (`NodeManager.path_edit_capturing`). Hover is
  currently handled ambiently by `gimbal_interaction::update_hovered_axis`
  with no router involvement at all.
- **FR-4:** Migrate the 3 existing systems off `is_dragging()` /
  `path_edit_capturing` onto the router's per-frame ownership decision;
  remove those flags from `NodeManager` (`node_manager/mod.rs` ~42-46,
  `impl NodeManager::is_dragging` ~79-80), or reduce them to
  router-internal state if some derived form is still needed.
- **FR-5:** Consumers register their priority once at plugin build
  (`NodeManagerPlugin::build` in `node_manager/mod.rs` ~110-135), so a
  *new* consumer — e.g. the queued `glb_mesh_picking_20260713` unit — plugs
  in by registering a priority and reading the router's decision, without
  editing `viewport_pick.rs`, `gimbal_interaction.rs`, or
  `path_point_interaction.rs`.

## Relevant Files

- `fe-ui/src/node_manager/viewport_pick.rs` — `handle_viewport_click`;
  `is_dragging()` + `path_edit_capturing` guards at ~21-27; egui-pointer
  guard ~32-39; viewport-rect gating ~44-46.
- `fe-ui/src/node_manager/gimbal_interaction.rs` — `handle_gimbal_interaction`
  sets drag state; `update_hovered_axis` (hover, no router today) ~19-53;
  viewport-rect gating ~43-45.
- `fe-ui/src/node_manager/path_point_interaction.rs` — `handle_path_point_interaction`;
  sets `path_edit_capturing` ~119, ~137, ~145-146; `Tool::Pen` gating fix
  (commit `ab9c53c`) ~222-224.
- `fe-ui/src/node_manager/mod.rs` — `NodeManager.path_edit_capturing` field
  ~42-46; `NodeManager::is_dragging` ~79-80; system registration + `.chain()`
  order encoding the current implicit priority ~116-133.
- `fe-ui/src/node_manager/AGENTS.md` — `§path-points` (~line 66) and
  `§pen-tool` (~line 26) document the current flag scheme; update to
  describe the router once implemented.

**Note:** this track **blocks** `glb_mesh_picking_20260713` — that unit
registers as a router consumer rather than adding a 4th ad-hoc flag.

## Constraints

- Bevy 0.18, `default-features = false`: `VertexAttributeValues` is private —
  read mesh attribute data via `.as_float3()`, not direct variant matching.
- **Never** run `rustfmt` on this repo.
- Do **not** touch quarantine files: `fe-api/*`, `fe-database/src/lib.rs`,
  `conductor/.conductor_session_log`, `.codex/`.
- Machine no-concurrent-cargo rule: only one `cargo build`/`cargo test`
  invocation at a time across the workspace.
