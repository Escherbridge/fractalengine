---
type: Track Spec
title: fe-ui Decomposition — God-File Breakup into Domain Modules
tags: [chore, feui_decomposition_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: fe-ui Decomposition

**Track ID:** `feui_decomposition_20260710`
**Type:** Chore (refactor)
**Status:** In progress
**Successor to:** `ui_manager_refactor_20260419` (which fixed the *logical*
architecture — UiSet ordering, UiAction queue, ActiveDialog enum, selection
dedup — but left the *physical* file layout untouched).

## Overview

`ui_manager_refactor_20260419` fixed fe-ui's state-management architecture.
It did not address file size. Verified 2026-07-10, `fe-ui/src/` line counts:

| File | Lines |
|---|---|
| `panels.rs` | 1991 |
| `plugin.rs` | 1397 |
| `dialogs.rs` | 1183 |
| `verse_manager.rs` | 914 |
| `node_manager.rs` | 834 |
| `hexon_manager.rs` | 502 |
| `petal_manifest.rs` | 233 |
| `navigation_manager.rs` | 227 |
| `gimbal.rs` | 194 |
| `atlas/petal_wizard.rs` | 165 |

Five files exceed the project's soft cap of ~300 lines by 2-7x. This matches
the user's standing directive: "small domain-specific files (soft cap ~300
lines; god-files get decomposed)". This track decomposes the five largest
files into domain modules without changing behavior — a pure physical split,
building on the logical types (`UiAction`, `ActiveDialog`, `UiSet`,
`UiManager`) that `ui_manager_refactor_20260419` already established.

## Functional Requirements

### FR-1: Split `panels.rs` (1991 lines) by panel domain

**Description:** `panels.rs` currently holds `gardener_console()` plus every
panel-rendering function (toolbar, sidebar, inspector, status bar, dialogs
dispatch). Split into `panels/` with one file per panel domain: `toolbar.rs`,
`sidebar.rs`, `inspector.rs`, `status_bar.rs`, `mod.rs` (re-exports +
`gardener_console()` as a thin dispatcher calling into the domain files).

**Acceptance Criteria:**
- No file in `panels/` exceeds ~300 lines (large panels may need one more
  split level, e.g. `inspector/properties.rs` + `inspector/api_access.rs` +
  `inspector/query.rs` matching the existing `InspectorTab` variants).
- `gardener_console()`'s public signature is unchanged (same call sites in
  `plugin.rs` keep working).
- `Tool` enum (currently in `panels.rs`) moves to whichever file makes
  semantic sense (likely `mod.rs` or a new `tool.rs`) with a one-line
  doc-comment, not a multi-paragraph block — module-level rationale goes in
  `fe-ui/src/AGENTS.md`, not inline.

### FR-2: Split `plugin.rs` (1397 lines) by resource domain

**Description:** `plugin.rs` currently defines `GardenerConsolePlugin` plus
every UI-only resource (`UiAction`, `ActiveDialog`, `InspectorFormState`,
`UiManager`, `UiSet`, `PetalMapState`, etc.) and the `process_ui_actions`
dispatcher. Split resources into `resources/` (one file per resource cluster:
`actions.rs` for `UiAction`/`UiManager`, `dialogs.rs` for `ActiveDialog`,
`inspector_state.rs` for `InspectorFormState`, `petal_map.rs` for
`PetalMapState`), keep `plugin.rs` as the thin `Plugin` impl + `UiSet`
registration.

**Acceptance Criteria:**
- `plugin.rs` itself drops under ~300 lines (just `GardenerConsolePlugin`,
  `UiSet`, system registration).
- `process_ui_actions` (currently ~230 lines within `plugin.rs`, lines
  977-1200+) becomes its own file, `resources/actions_dispatch.rs`, since it
  is a single large match — the natural first target for further internal
  breakup if it grows.
- All existing tests in `plugin.rs` move with the code they test (unit tests
  live next to what they test, per `conductor/code_styleguides/rust.md`
  Testing convention).

### FR-3: Split `dialogs.rs` (1183 lines) by dialog variant

**Description:** One file per `ActiveDialog` variant's render function,
under `dialogs/`: `context_menu.rs`, `create_dialog.rs`, `gltf_import.rs`,
`node_options.rs`, `invite_dialog.rs`, `join_dialog.rs`, `peer_debug.rs`,
`mod.rs` (dispatcher matching on `ActiveDialog`).

**Acceptance Criteria:**
- No file in `dialogs/` exceeds ~300 lines.
- The `ActiveDialog` mutual-exclusion invariant from
  `ui_manager_refactor_20260419` (FR-3) is preserved exactly — this is a
  physical split only, not a logic change.

### FR-4: Split `verse_manager.rs` (914 lines) — dispatcher + per-DbResult handlers

**Description:** `apply_db_results` (~430 lines, `code_review_20260430_mega_function`'s
still-open target) is the natural fault line. Extract each `DbResult` match
arm into its own function in `verse_manager/handlers.rs`, leaving
`apply_db_results` as a thin dispatcher matching on the variant and calling
the extracted function. Tree types (`VerseEntry`, `NodeEntry`, etc.) and
`VerseManager` methods (`update_node_position`, `find_petal`, etc.) move to
`verse_manager/tree.rs`.

**Acceptance Criteria:**
- This FR **also closes** `code_review_20260430_mega_function` — mark that
  track `[x]` once this FR lands (do not duplicate the fix).
- `apply_db_results` itself is under ~50 lines (pure dispatch).
- No handler function exceeds ~100 lines.
- **Do not** fix `code_review_20260430_performance_hotpaths`'s O(n^3) lookup
  as a side effect of this split — that is a separate, already-registered
  track; keep this FR to structure only, unless doing both is trivially
  cheap once the code is already open (if so, note it in both tracks'
  metadata rather than silently doing it).

### FR-5: Split `node_manager.rs` (834 lines) by the 7 chained systems

**Description:** Each of the 7 systems chained by `NodeManagerPlugin`
(`handle_tool_shortcuts`, `sync_sidebar_to_manager`, `handle_gimbal_interaction`,
`handle_viewport_click`, `sync_manager_to_inspector`, `draw_gimbal_system`,
`broadcast_transform`) becomes its own file under `node_manager/systems/`,
keeping `NodeManager` resource + `NodeManagerPlugin` + the `.chain()` wiring
in `node_manager/mod.rs`.

**Acceptance Criteria:**
- The `.chain()` ordering (documented in `AGENTS.md` "NodeManager Pattern")
  is preserved exactly — verify against `AGENTS.md`'s numbered list after
  the split.
- No file exceeds ~300 lines.

## Non-Functional Requirements

- **NFR-1 (No behavior regression):** identical to `ui_manager_refactor_20260419`'s
  NFR-1 — every user-facing interaction must behave identically before/after.
- **NFR-2:** `cargo build`, `cargo test -p fe-ui`, `cargo clippy -p fe-ui -- -D warnings`,
  `cargo fmt --check` all pass after each FR.
- **NFR-3:** Update `AGENTS.md`'s "File Ownership Map" table to reflect the
  new module layout — this is exactly the kind of directory-level doc the
  user's global convention prefers over inline comment blocks.

## Out of Scope

- Any logic change to `UiSet`/`UiAction`/`ActiveDialog`/selection dedup
  (already correct per `ui_manager_refactor_20260419`).
- Fixing `code_review_20260430_performance_hotpaths` (separate track).
- Splitting files under ~300 lines (`hexon_manager.rs` at 502 is a judgment
  call — include only if it falls out naturally from FR-1's inspector split
  since Hexon Manager is inspector-adjacent; do not force it).

## Dependencies

- `ui_manager_refactor_20260419` — must be understood before touching any of
  these files (this track builds directly on its types).
