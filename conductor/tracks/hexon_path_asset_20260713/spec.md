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
- **FR-2 (W4, as-built 2026-07-15):** The shipped `PathAssetDescriptor`
  (`fe-sdk/src/path_asset.rs`) rides `PropertyValue::Json` under the
  `path_asset` property key **on the track node itself** (no separate
  `source_path` reference): `{ asset_path, spacing_mode, spacing_value,
  count, tangent_align }`. Objects round-trip losslessly (tested). The
  spec's original `{ hexon_ref, source_path, count_value }` shape never
  landed — see Open Questions.
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

## As-built notes (reconciled 2026-07-15)

- **FR-1..FR-4 shipped** (commits 35956dd, aad5542, 0be9628). The shipped
  render path loads scenes via `asset_server.load("{path}#Scene0")` in
  `fe-ui/src/verse_manager/spawn.rs` — it does **not** go through
  `fe-renderer::load_to_bevy`, which remains the placeholder stub
  (`fe-renderer/src/loader.rs:6-12`), and P2P blob fetch remains unwired.
  The Reality-gap claim "nothing renders until load_to_bevy exists" is
  therefore obsolete.
- **DECISION (2026-07-15): the `load_to_bevy` loader finish + P2P blob
  fetch are formally HANDED OFF to `hexon_p2p_bucket_20260710`** (its FR-3
  placeholder-rendering contract and FR-5 pull-through-fetch cover exactly
  this residual). This track will not touch `fe-renderer/src/loader.rs` or
  `fe-network/src/iroh_blobs.rs` further.
- **FR-3 as-built:** reconcile is `reconcile_path_asset`
  (`fe-ui/src/verse_manager/path_asset_reconcile.rs`), a per-frame
  change-gated system keyed on `PathEditorState.editing_track_id` + a
  points fingerprint — NOT event-driven on
  `NodePropertiesLoaded`/`NodeDeleted` as originally specced. This is the
  intended as-built behavior, not missing wiring.
- **Remaining scope:** FR-5 (PetalHexon bake) + FR-6 (multi-asset) — see
  `plan.md`.

## Open Questions

1. **`hexon_ref` semantics (from the original FR-2 shape):** should stamping
   be *stamp-from-hexon* (descriptor references a hexon that carries the
   model asset, per the Vision's "hexon carries asset-model data") or the
   shipped *stamp-from-asset-node* (`asset_path` points straight at a
   `blob://{hash}.glb`)? The shipped shape skips the hexon indirection.
   Decide when FR-5/FR-6 give hexons a real asset-carrying story — if
   stamp-from-hexon is still wanted, it is a follow-up descriptor field,
   not a rewrite.
