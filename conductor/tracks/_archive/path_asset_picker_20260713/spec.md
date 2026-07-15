---
type: Track Spec
title: Path Asset Picker — Real glb/Hexon Picker for the Path-Asset Stamp
tags: [feature, ui, gis, hexon, path-asset, path_asset_picker_20260713]
timestamp: 2026-07-13T00:00:00Z
resource: ./metadata.json
---

# Specification: Path Asset Picker

**Track ID:** `path_asset_picker_20260713`
**Crates:** `fe-ui`

## Vision / Why (user, 2026-07-13)

The hexon-as-path-asset stamp pipeline shipped (commit `35956dd`): a track's
`path_asset` property drives `reconcile_path_asset`, which stamps a model
repeatedly along a GPX path. But the assignment UX is a v1 stub —
`fe-ui/src/panels/tool_panel.rs::render_path_asset_section` is just a raw
`blob://<hash>.glb` **text field** (`ToolPanelState.selected_hexon_ref`
doubles as the buffer, per its own doc comment: "doubles as the v1
asset-path text buffer... until a real hexon picker lands"). The user
reports: **"no way to assign a glb or hexon as a pattern to a gpx path to
trigger that build workflow."**

The feature is technically wired end-to-end — Stamp button →
`UiAction::PathAssetApply` → `SetNodeProperty(path_asset)` →
`reconcile_path_asset` stamps — but unusable because there is no real
picker; a user has to already know a content-hash blob URL to type in.
This track replaces the text field with an actual picker UI. No new
platform capability is required for the v1 scope (see Reality Gap below);
this is a UI-only track.

## Reality gap / quarantine constraint (recon 2026-07-13)

Two paths exist to get an `asset_path` (`blob://{hash}.glb`) into the
picker:

1. **Pick an already-installed asset/hexon.** No live wiring exists yet
   for listing "nodes/assets with a glb model" as a picker source (unlike
   `InstalledTilesetDto`, which is terrain-tileset-shaped, not
   asset-shaped — see `fe-ui/src/terrain_map/dto.rs:16-25`). This is new
   UI-side plumbing but touches no quarantined file.
2. **Browse a brand-new `.glb` from disk.** `fe-ui/src/dialogs/gltf_import.rs`
   already has the `rfd::FileDialog` browse flow, but ingesting a browsed
   file into the blob store so it gets a `blob://{hash}.glb` path goes
   through `DbCommand::ImportGltf`, dispatched in
   **`fe-database/src/lib.rs` (QUARANTINE)** — confirmed at
   `fe-database/src/lib.rs:354` (`Ok(DbCommand::ImportGltf { .. }) =>
   handlers::crud::import_gltf_handler(...)`). `fe-api/src/assets.rs` is
   also quarantine and is the read-side blob-serving surface consumed at
   render time (`spawn_stamped_entity` resolves `blob://{hash}.glb`
   through it).

**Decision (recorded per user's own scoping in the parent-track
handoff):** v1 picker lists **already-installed** assets/hexons only — no
quarantine contact. Browsing-and-ingesting-a-new-glb is a **follow-on**,
explicitly gated on the quarantine lift for `fe-database/src/lib.rs`. Do
not plan edits to `fe-database/src/lib.rs` or `fe-api/src/assets.rs` in
this track.

## Functional Requirements

- **FR-1 — Real picker, not a text field.** Replace
  `render_path_asset_section`'s raw `TextEdit::singleline` buffer
  (`fe-ui/src/panels/tool_panel.rs:130-139`) with an actual picker:
  - **(a) Browse a `.glb` from disk** — mirror the `rfd::FileDialog`
    flow already used in `fe-ui/src/dialogs/gltf_import.rs:59-64`
    (`.add_filter("GLTF Models", &["glb", "gltf"])`) and
    `fe-ui/src/dialogs/hexon_manager.rs:90-96`
    (`.add_filter("Hexon archive", &["hexon"]).pick_file()`) for the
    file-dialog shape/pattern.
  - **(b) Pick from a list of already-installed assets/hexons** — mirror
    the list/select UX in `fe-ui/src/dialogs/hexon_manager.rs`
    (`render_installed_tab`, ~lines 194-344: scrollable `egui::Grid`,
    filter text field, row-click-to-select pattern) and the action-dispatch
    shape in `fe-ui/src/actions/hexon.rs` (`set_petal_map`, ~lines
    146-174: build a DTO reference from the selection, no file I/O).
  - Whichever path is chosen, the picker must resolve to the
    `asset_path` string (`blob://{hash}.glb`) the `PathAssetDescriptor`
    needs — the *only* contract `build_descriptor`
    (`fe-ui/src/panels/tool_panel.rs:198-206`) and `reconcile_path_asset`
    (`fe-ui/src/verse_manager/path_asset_reconcile.rs`) care about.
  - **v1 scope note (see quarantine constraint above): implement (b)
    first** (list installed assets/hexons — no quarantine contact).
    (a) is a stretch/follow-on: browsing selects a local file path, but
    turning that into a `blob://` asset_path requires `ImportGltf`,
    which is quarantined. If (a) is attempted in this track, the file
    dialog step itself is fine (no quarantine contact), but the
    ingestion step must be either deferred (store the raw file path and
    surface a "not yet ingested" state) or explicitly flagged as blocked
    until the quarantine lifts. Do not silently stub a fake
    `blob://` hash.

- **FR-2 — Ingestion boundary (design constraint, not an implementation
  task in this track).** A *newly browsed* `.glb` only becomes a valid
  `asset_path` after content-addressed ingestion into the blob store,
  which happens via `DbCommand::ImportGltf` → `fe-database/src/lib.rs`
  (quarantine) → `handlers::crud::import_gltf_handler`. This track must
  **not** touch that dispatch. Picking an **already-installed**
  hexon/asset sidesteps ingestion entirely (the `blob://{hash}.glb` path
  already exists) — this is why FR-1(b) is the v1-safe path and FR-1(a)
  is explicitly deferred/flagged.

- **FR-3 — Descriptor emission unchanged.** On pick, build the
  `PathAssetDescriptor` (`fe-sdk::path_asset::PathAssetDescriptor`:
  `asset_path` + `spacing_mode`/`spacing_value`/`count`/`tangent_align`
  from the existing panel controls) and emit
  `UiAction::PathAssetApply { track_node_id, descriptor }` for the
  currently-edited track. This wiring already exists and is correct
  (`build_descriptor` at `fe-ui/src/panels/tool_panel.rs:198-206`, the
  Stamp button at `fe-ui/src/panels/tool_panel.rs:172-178`, the action
  variant in `fe-ui/src/actions/mod.rs:116` and its handler at
  `fe-ui/src/actions/mod.rs:426`). The picker's only job is to populate
  `state.selected_hexon_ref` (or its replacement field) with a correct
  `asset_path` before Stamp is clicked — no changes to the emit path
  itself are anticipated.

- **FR-4 — Discoverability / end-to-end reachability.** Verify (and note
  any gaps found) that the full flow is reachable and legible to a user
  with no prior knowledge of the wiring:
  1. Select a track in the Paths tab (`PathEditorState.editing_track_id`
     must be `Some`).
  2. Open the Tools panel (`ToolPanelState.open`).
  3. Pick an asset (this track's new picker).
  4. Set spacing/count/tangent pattern controls (already implemented).
  5. Click "Stamp along path".
  - Known existing gap: `render_path_asset_section` shows a generic
    "Select a track in the Paths tab to stamp along." hint
    (`fe-ui/src/panels/tool_panel.rs:180-186`) but never names *which*
    track is currently targeted once one is selected — the panel should
    surface the target track's identity/name (not just a Some/None
    hint) so a user editing multiple tracks isn't guessing which one
    Stamp will affect. Record this as an explicit FR-4 follow-up if not
    fixed inline while wiring the picker.
  - Also confirm the Tools panel is actually reachable from wherever a
    user opens panels (i.e. `ToolPanelState.open` has a toggle
    somewhere reachable in the UI) — if it's currently only flippable
    via code/default state, that is itself a discoverability gap worth
    flagging.

## Relevant Files

- `fe-ui/src/panels/tool_panel.rs` — `render_path_asset_section` (the
  blob:// text field, ~lines 119-195), `build_descriptor` (~197-206),
  `ToolPanelState` (~39-77, `selected_hexon_ref` field + doc comment
  admitting the v1-stub status), Stamp button (~172-178). Primary edit
  site for FR-1/FR-3/FR-4.
- `fe-ui/src/dialogs/hexon_manager.rs` — `render_hexon_manager` (~24),
  `render_installed_tab` (~194, list/filter/select UX to mirror),
  "Install from file..." `rfd::FileDialog` pattern (~90-96).
- `fe-ui/src/actions/hexon.rs` — `install_from_file` (~16),
  `set_petal_map` (~146-174, DTO-selection-to-action pattern to mirror
  for FR-1(b)).
- `fe-ui/src/dialogs/gltf_import.rs` — `render_gltf_import_dialog`
  (whole file): `rfd::FileDialog` browse flow (~59-64) feeding
  `DbCommand::ImportGltf` (~113-124) — reference for FR-1(a)'s dialog
  shape; do **not** wire its `ImportGltf` dispatch path into this
  track's picker without flagging the quarantine dependency.
- `fe-sdk/src/path_asset.rs` — `PathAssetDescriptor`,
  `PATH_ASSET_PROPERTY_KEY` (the property-bag key), `SpacingMode`. No
  changes anticipated; this is the contract the picker must satisfy.
- `fe-ui/src/verse_manager/path_asset_reconcile.rs` —
  `reconcile_path_asset` (the consumer that reads `path_asset` +
  `gpx_points` and stamps `spawn_stamped_entity` calls). Confirms the
  only requirement on `asset_path` is that it resolve to a real
  `blob://{hash}.glb` at render time; no changes anticipated.
- `fe-ui/src/terrain_map/dto.rs` — `InstalledTilesetDto` (~16-25):
  confirms the existing installed-tileset list is terrain-shaped, not
  asset-shaped; a new DTO/query surface for "installed
  assets/hexons-with-models" is likely needed for FR-1(b) and should be
  scoped/named during implementation planning, not assumed to reuse
  this type as-is.

## Constraints

- **NEVER run `rustfmt`** on this repository.
- **DO NOT touch quarantine files**: `fe-api/*`, `fe-database/src/lib.rs`,
  `.conductor_session_log`, `.codex/`. Design the v1 picker to **avoid
  needing them** — list installed assets/hexons (FR-1(b)), not
  import-new (FR-1(a), deferred). If FR-1(a) is attempted, the
  file-dialog step is fine but its ingestion step must not touch
  `fe-database/src/lib.rs`'s `ImportGltf` dispatch; flag it as blocked
  instead.
- **No concurrent `cargo build`/`cargo check`/`cargo test`** invocations
  across tracks — coordinate before running builds.
- This is a spec/planning track only: no code changes, no build/test
  runs are part of authoring this document.
