---
type: Track Spec
title: Asset Download Fix — Save Dialog + E2E Resolution
tags: [bug, assets, ui, asset_download_fix_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: Asset Download Fix

**Track ID:** `asset_download_fix_20260711`
**Files:** `fe-ui/src/panels/asset_card.rs`, `fe-ui/src/actions/asset.rs`,
`fe-ui/src/asset_ops.rs`, `fractalengine/src/asset_bridge.rs`

## Problem

User clicks the inspector Asset card's Download button and nothing visible
happens — no dialog, no file, no error. Current design silently copies to
`Downloads/fractalengine/` + transient toast; even on success this reads as
"button does not work".

## Functional Requirements

- **FR-1 Root cause with evidence:** integration test driving
  `resolve_and_copy` against a real temp `BlobStore` + `VerseManager` node
  fixture (blob present, `asset_path = blob://{hash}.glb`) proving the resolve
  path, plus a test for the `has_asset && asset_path == None` gap.
- **FR-2 Native save dialog:** clicking Download opens an rfd save dialog
  (fe-ui already depends on rfd for GLB import — reuse that idiom), suggested
  filename from node name + asset extension, default dir Downloads. Chosen
  destination travels in `AssetOp::Download { node_id, dest }`; bridge copies
  to it (None → legacy Downloads-dir behavior).
- **FR-3 Unmissable feedback:** the Asset card renders a persistent status
  row from `AssetDownloadStatus` (saved path on success, error text on
  failure) in addition to the toast.
- **FR-4 Graceful degradation:** node with `has_asset` but no cached
  `asset_path` shows why the download can't proceed instead of failing
  silently (DB-backed re-resolution is out of scope this round — the
  `DbCommand` dispatch file is under external-IDE quarantine).
