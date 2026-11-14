---
type: spec
title: Utility node contract and assets-in-volume takeoff
track: utility_node_contract_20261113
tags: [deferred, terrain, bim, fe-terrain, fe-renderer, fe-ui, utility]
created: 2026-11-13
status: deferred
resource: ./metadata.json
plan: ./plan.md
origin: terrain_proposal_delta_viz_20261025 FR-11 and OD-5
---

# Utility node contract and assets-in-volume takeoff

Status: **deferred**. Trigger: a user asks for utility takeoff, or OD-5 (true 3D volume) lands.
This spec only fixes the contract; it commits no code.

## 1. Scope

Enumerate utility assets (pipes, conduits, cables) inside a selected volume, with a simple quantity
takeoff and an excavation clash warning against the cut prism. Point utilities add a count.

## 2. Node classification

- A scene node is a utility when `node_kind: utility` is set.
- `utility_type` names the class (for example pipe, conduit, cable, point asset). It is a closed
  set in the contract, extended by a reviewed change, never by free text.
- Non-utility nodes keep their current kinds (earthwork, stamp, path, asset). Nothing reads
  `node_kind` today, so the field is additive.

## 3. Property contract

| Property | Meaning | Required for |
| --- | --- | --- |
| `depth` | Ground-to-crown depth in metres at the node | clash test, cover check |
| `invert` | Elevation of the lowest internal point in metres | clash test, flow direction |
| `diameter` | Internal diameter in metres | clipped volume (later), labelling |

Values are stored in world units and reported through the map-scale authority (see
`map_scale_authority_20260716`). A utility missing `depth` or `invert` is listed as unclassified
in the report rather than dropped.

## 4. Takeoff rules

- Clipped length per `utility_type` = sum of the segment lengths inside the selection footprint.
- Point utilities contribute a count, not a length.
- Utilities whose `invert` or `depth` falls inside the cut prism of the selection are flagged as
  an excavation clash. The flag is a warning, not a total change.

## 5. Prerequisite: true 3D volume (OD-5)

The 2.5D footprint prism has unbounded vertical extent, so it cannot say whether a utility at a
given depth is inside the cut. The clash test needs an elevation band. The band is already
reserved on the request type, see `VolumeReportRequest.elevation_band` at
`fe-renderer/src/terrain_overlay.rs` (field marked "Ignored today"). Until OD-5 lands, the
clash warning is not computed and the takeoff is length only.

## 6. Extension seam

- The per-item table in the volume report is kind-extensible, so utility rows reuse it.
- `elevation_band: Option<[f32; 2]>` carries the vertical extent once the 3D volume exists.
  Consumers must treat `None` as the 2.5D prism.

## 7. Open questions

- Whether utility segments are stored as paths or as node chains (default: node chains, since
  nodes already carry transforms).
- Whether `utility_type` is per node or per imported layer (default: per node).
