# fe-ui/src/model_library — Models section (browse, install, place, credits)

Track: `conductor/tracks/model_library_20261115/` (spec §4.1, §4.4, §4.6, §6; plan ML-08).

- `mod.rs` — `ModelLibraryState` (UI state Resource), `ModelOp` + `PendingModelOps` (queue),
  `ModelResult` + `ModelLibraryState::apply` (result folding), the row types (`ModelRow`,
  `ModelEntryRow`, `ModelDetail`, `InstalledPackage`, `Credits`, `Licence`) and the thumbnail cache.
- `text.rs` — `display_text`, the single sanitiser for publisher-controlled strings.
- `panel.rs` — `show(ui, state, pending, local_role, active_petal_id)`, the section body.

## §state

`ModelLibraryState` is UI state like `PetalMapState`, not a fourth manager. It owns the view
(Browse / Installed / Credits), search text and tag filter, the listing, the open detail, the
installed list, per-uri install progress and errors, in-flight removals and placements, the active
petal's credits and the thumbnail cache. It never writes `NodeManager` or `NavigationManager`:
the import result router (`verse_manager/db_results/mod.rs` `finish_library_placement`) selects the
placed node through `NodeManager::request_select_after_navigation`, because the node's entity is spawned
by deferred commands after the reply and `NodeManager::select` needs that entity.

The section is `RightSidebarSection::Models` (slug `models`, guard key `right.models`), rendered by
`ui_shell/right_sidebar.rs::render_models_section` inside the shared section chrome. Its two resources
ride `plugin.rs`'s `MiscUiParams` bundle (the console system is at Bevy's 16-parameter limit).

Each view's data is a `Load<T>` (`NotAsked` / `Loading` / `Ready` / `Failed`), rendered by one
helper as spinner / error with a named Retry / content. `prepare` (first thing after the tabs each
frame) issues the first load for the shown view, the installed list once, and reloads credits when
the active petal changes. Install, remove and place invalidate credits so they reload on next view.

## §ops

fe-ui has no registry, filesystem or decoder access. The panel only pushes `ModelOp`s into
`PendingModelOps`; the main binary (`fractalengine/src/model_library_bridge.rs`) drains them on
worker threads and sends `ModelResult`s back, folded by `ModelLibraryState::apply`. This is the
`HexonOp` / `drain_hexon_ops` split (root `AGENTS.md` §petal-map), as a separate queue so the
terrain lane's enum is untouched.

- Stale results are dropped: `Listed` must echo the latest `generation`, `Detail` must match the
  open uri, `Credits` must match the requested petal, `Placed` / `PlaceFailed` must match a request
  this panel minted. Rows are capped at `MAX_REGISTRY_ROWS`.
- `Place` carries a `request_id` prefixed `PLACE_REQUEST_PREFIX`; `is_library_request` lets the
  `GltfImported` / `ImportGltfFailed` arm route the outcome here. The bridge resolves the entry
  (refusing non-placeable entries, U4), fills verse and fractal from navigation, and places at the
  camera focus through the scene-write door (`PlaceStoredGlb`).
- Install is library-wide and needs no role (spec D-3); Place is the Editor-gated step (U7), with
  the DB as the authority.

## §thumbnails

The bridge fetches the preview member through the verified partial-fetch path and decodes it with
`fe_hexon::thumbnail::decode`; fe-ui never decodes image bytes. The UI receives RGBA and uploads it
as a texture on first paint. Rules:

- at most `THUMBNAIL_CACHE_CAPACITY` (64) entries, least recently painted evicted (never an entry
  still fetching);
- at most `MAX_THUMBNAIL_FETCHES` (4) fetches in flight;
- a fetch is requested only when the row's thumbnail slot is on screen (`ui.is_rect_visible`);
- a listed preview over `THUMBNAIL_LIMIT_BYTES`, a failed fetch, or pixels whose length is not
  exactly `width * height * 4` within the side cap show the neutral placeholder (the length check
  guards `ColorImage`'s assertion; the binary is `panic=abort`).

## §strings

Every publisher-controlled string (package name, description, tags, entry labels, attribution,
SPDX id, publisher name and DID, error text) passes through `text::display_text` once, in the row
constructors and `apply`, before it can reach the screen: control characters and bidirectional
marks, embeddings, overrides and isolates (plus zero-width space and BOM) are stripped, whitespace
runs including newlines collapse to one space, and the text is clipped with an ellipsis. Raw
`hexon_id`, `version` and `entry_id` stay unsanitised only to address ops and are never painted.
Publisher text is rendered as plain egui labels (styling only): never markdown, never a link.

## Tests

`text::tests::display_text_table` (sanitiser table) and `panel::tests::model_library_journey`
(real egui frames over `show`: loading, error with Retry and a dropped stale reply, empty, listing
with licence chips and a sanitised bidi name, too-large and paid rows without Install, the
four-fetch thumbnail cap, install to Installed, Viewer Place disabled with the reason, Editor Place
with its request id, no-petal reason, credits with a source not installed here).
Batch: R11 `cargo test -p fe-ui --lib -- model_library::`.
