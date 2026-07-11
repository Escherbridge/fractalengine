---
type: Track Spec
title: GIS Query & Annotation UI — Edit, Query, Orchestrate Geo Data
tags: [feature, ui, gis, gis_query_ui_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: GIS Query & Annotation UI

**Track ID:** `gis_query_ui_20260711`
**Crate:** `fe-ui` (must not depend on fe-terrain)

## Overview

The in-app half of the GIS goal: let the user edit, query, and orchestrate
geo data without leaving the viewport. Uses the shared `gis.annotation.*`
node-property contract (see petal_gis_endpoints spec) and the existing
property-write path over the DB channel.

## Functional Requirements

- **FR-1 Annotation editor:** selected node gains an "Annotation" card
  (title/body/color on `gis.annotation.*` properties) with save through the
  existing node-property update flow. Empty fields remove the property.
- **FR-2 Query panel:** a GIS panel to run petal-scoped queries — by
  annotation presence/text, by property filters, and by local-coords bbox
  around the camera — listing results with name, position, annotation
  summary; clicking a result selects the node and flies the camera to it.
- **FR-3 Layer manager:** UI to toggle visibility and opacity of the
  terrain layer stack (satellite, terrain, GPX tracks, GeoJSON overlays)
  driving the existing `LayerStack` resource semantics through UI state the
  terrain plugin already honors.
- **FR-4 Small-file discipline:** new modules under `fe-ui/src/panels/` /
  `fe-ui/src/actions/` per the decomposition conventions (~300-line soft
  cap), new UiAction variants in `actions/mod.rs` delegating to a new
  `actions/gis.rs`.
