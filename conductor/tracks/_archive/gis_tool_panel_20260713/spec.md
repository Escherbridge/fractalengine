---
type: Track Spec
title: GIS Tool Panel — Host for Path-Asset + Future Terrain Tools
tags: [feature, ui, gis, panel, gis_tool_panel_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: GIS Tool Panel

**Track ID:** `gis_tool_panel_20260713`
**Crates:** `fe-ui`
**Work unit:** W6 (ultrapilot, branch `up/w6-toolpanel-20260713`) — SHELL landed

## Vision (user, 2026-07-13)

A dedicated tool panel to host the hexon-path-asset controls (hexon picker +
repetition/pattern sliders) with a placeholder section for future terrain
tools. Moves the path-asset UX out of `path_editor_card.rs`.

## Functional Requirements

- **FR-1:** New floating egui panel `tool_panel.rs`, independent like the GIS
  panel (not in the mutual-exclusion `ActiveDialog` set).
- **FR-2:** `ToolPanelState` resource holds panel-open + the path-asset control
  state: `selected_hexon_ref`, `spacing_mode` (FixedSpacing|FixedCount),
  `spacing_value`, `count_value`, `tangent_align`.
- **FR-3:** "Path Asset" section: hexon picker (mirrors `hexon_manager.rs`
  list/select UX — deferred to the stamp unit), spacing-mode radio, spacing/
  count DragValue, tangent-align checkbox.
- **FR-4:** Stubbed "Terrain Tools" section (placeholder for future work).
- **FR-5:** Registered in `panels/mod.rs`; `ToolPanelState` resource wired in
  `plugin.rs` (coordinator integration — plugin.rs is shared with W7a).

## Status

SHELL shipped W6: panel renders, controls hold state. Hexon picker + action
wiring is deferred to `hexon_path_asset_20260713` (the stamp unit).
