---
type: Handoff
title: W5 — PetalHexon Op-Store + Multi-Asset-Per-Node (deferred)
tags: [handoff, hexon, petal-hexon, deferred, hexon_path_asset_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# W5 Handoff — PetalHexon Op-Store + Multi-Asset

**Status:** DEFERRED at the 2026-07-13 session's last green boundary (commit
`8565ca1` on `wip/ultrapilot-4tracks-20260712`, pushed). Not started.

## Why it was deferred (not a time problem — a constraint problem)

W5 **cannot be built without editing quarantine files** that the session was
forbidden to touch:
- The `application/x-fe-directory` multi-asset seam returns `501` in
  **`fe-api/src/assets.rs`** (`DIRECTORY_ASSET_CONTENT_TYPE`, the 501 at
  `assets.rs:213`, test `directory_placeholder_content_type_returns_501`).
  fe-api/* is quarantined (live Antigravity IDE WIP).
- Node write-op dispatch (the bake source) lives in
  **`fe-database/src/lib.rs`** (the `DbCommand::*` match). Also quarantined.

**Lift the quarantine on those files (or get explicit sign-off to edit them)
before starting W5.**

## What already exists (build on it, don't rebuild)

- **Node write-ops are already logged.** `node_log` table +
  `NodeLogEntry`/`NodeLogOp` (`fe-entity-store/src/lib.rs`); `append_node_log`
  (`fe-database/src/handlers/node_log.rs`) is called on every mutation. Also a
  petal-scoped `op_log` (`fe-database/src/types.rs`, `op_log.rs`). Both
  append-only, HLC-timestamped. **The recording half is done.**
- **Hexon packaging exists.** `fe-hexon` (NOT quarantined): `HexonPackageData`
  (`package.rs`), `HexonManifest`/`HexonKind` (`manifest.rs`),
  `AssetEntry`/`EntryKind` (`Model`, `TerrainTileset`, …). A `.glb` is already
  a first-class `AssetEntry { kind: Model, format: "gltf" }` + bytes in
  `HexonPackageData.assets`.
- **glTF renders.** `spawn_node_entity` / `spawn_stamped_entity`
  (`fe-ui/src/verse_manager/spawn.rs`) spawn `SceneRoot`; `blob://{hash}.glb`
  served by `BlobAssetReader` (`fe-runtime/src/bevy_blob_reader.rs`).

## What is net-new (the actual W5 work)

1. **`PetalHexon` type** — no such type exists. It should serialize a petal's
   node write-ops (`node_log`/`op_log`) into a `fe-hexon` package and reload
   (rehydrate nodes). START STANDALONE IN `fe-hexon` (not quarantined) to
   minimize quarantine contact: define the type + bake/reload logic there,
   then wire the DB read of the op-log (the read may be doable via existing
   query handlers without editing `lib.rs` — investigate before assuming an
   edit is needed).
2. **Multi-asset-per-node** — implement the `x-fe-directory` seam
   (`fe-api/src/assets.rs`, currently 501) so a node/hexon can carry model
   data alongside other data. QUARANTINE — needs sign-off.

## Recommended sequencing

W5a (fe-hexon-only PetalHexon type + bake/reload, no quarantine) →
W5b (op-log DB read wiring — check if quarantine edit is truly required) →
W5c (x-fe-directory multi-asset — quarantine, needs sign-off).
Land W5a green first; it's the most valuable and least constrained.

## Also outstanding (smaller, this track)

- **Real hexon picker** in the Tools panel — currently a v1 `blob://…` text
  field (`selected_hexon_ref`). Mirror `fe-ui/src/dialogs/hexon_manager.rs`'s
  list/select UX. Emit path won't change.
- **Pen-tool live curve preview** — phase-2 is resample-then-replace via a
  "Smooth path" button, not a live preview mesh.
- **Terrain-height-aware placement** — all raycasts hit y=0, not terrain.

## In-app verification still pending

The 4 landed objectives (persistence reload, wall removal, stamp-along-path,
pen tool) were test-green but await in-app confirmation.
