---
type: Track Spec
title: Hexon as Path Asset — Stamp a Hexon's Model Along a GPX Path
tags: [feature, hexon, gpx, terrain, renderer, platform, hexon_path_asset_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Hexon as Path Asset

**Track ID:** `hexon_path_asset_20260713`
**Crates:** `fe-sdk`, `fe-ui`, `fe-renderer`, `fe-hexon`, `fe-database`,
`fractalengine`
**Work units:** W3 (glb-render), W4 (stamp pipeline), W5 (PetalHexon op-store)
— SEQUENTIAL, built on the green boundary from the parallel wave

## Vision (user, 2026-07-13)

Select a hexon and stamp it repeated along a GPX path with repetition +
pattern settings. A hexon can carry asset-model data (e.g. `duck.glb`)
alongside its other data; stamping instances that asset at computed
transforms along the path. The bigger idea: **all node write-ops are saved
into a petal-level hexon** (hexon = container of node write-ops + assets).
Replaces the ripped-out path→wall extrusion.

## Locked decisions (user, 2026-07-13)

- **Spacing:** BOTH modes, panel-toggled — fixed-spacing OR fixed-count radio.
- **Orientation:** tangent-align toggle (rotate each instance to the local
  path direction).
- **Instance type:** the hexon's model asset (real `.glb`), NOT a terrain
  tileset per instance.
- **Op-store:** full petal-hexon op-store this session (build the net-new
  PetalHexon bake layer + real glb rendering, per user 2026-07-13).

## Reality gap (recon 2026-07-13)

Three net-new platform pieces the feature sits on, none of which exist today:
1. **`.glb` → visible mesh in the Bevy scene.** `fe-renderer` `load_to_bevy`
   is a STUB falling back to `placeholder.glb`; no spawn-into-scene, no
   GltfPlugin wiring, P2P fetch is an unwired TODO. *Nothing renders until
   this exists.* (W3)
2. **`PetalHexon` type + bake.** No type exists. `node_log`/`op_log` tables
   already record every write-op (append-only, HLC-stamped) — the net-new
   part is serializing that log into a `fe-hexon` package and reloading. (W5)
3. **Multi-asset-per-node.** `node.asset_id` is single-asset today; the
   `application/x-fe-directory` seam returns 501. (W5)

## Functional Requirements

- **FR-1 (W3):** A node referencing a `.glb` asset spawns the real glTF mesh
  into the Bevy scene (finish the Sprint-5B loader TODO).
- **FR-2 (W4):** A path-asset descriptor rides `PropertyValue::Json`:
  `{ hexon_ref, source_path (track node), spacing_mode, spacing_value,
  count_value, tangent_align }`. Objects round-trip losslessly.
- **FR-3 (W4):** Reconcile system reads the source path's `gpx_points`, stamps
  the hexon's model at arc-length-spaced transforms (reuse `PathTracker`),
  rotating to tangent when enabled; re-projects on `NodePropertiesLoaded` /
  `NodeDeleted` (mirrors P1/P2 reconcile discipline, NOT the deleted wall one).
- **FR-4 (W4):** Tool-panel controls (from `gis_tool_panel_20260713`) drive the
  descriptor; hexon picker mirrors `hexon_manager.rs`.
- **FR-5 (W5):** `PetalHexon` bake: serialize a petal's node write-ops
  (`node_log`/`op_log`) into a `fe-hexon` package; reload rehydrates nodes.
- **FR-6 (W5):** Multi-asset-per-node via the `application/x-fe-directory`
  seam — a hexon can carry model data alongside other data.

## Sequencing

W3 → W4 → W5, each landing green before the next. Stop at last green boundary
if runway runs out; hand off the remainder. Highest-risk platform work; do NOT
bundle into the parallel wave (depends on not-yet-existing APIs).
