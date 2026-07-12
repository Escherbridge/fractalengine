# fe-ui/src/gis — GIS Query panel state + pure logic

Mirrors `terrain_map`'s split: this module owns the **state resource** and
all **pure** query/layer logic (no `db_sender`/Bevy system access); the I/O
side (submitting `DbCommand`s) lives in `actions::gis`, which calls into
this module — same relationship as `terrain_map::tileset_to_terrain_json`
vs `actions::hexon`. See root `AGENTS.md` §gis-query-ui for the full
end-to-end design.

- `mod.rs` — `GisPanelState` (`open`, `active_tab`, `mode`, per-mode input
  buffers, `results: Vec<GisResultRow>`, `query_pending`/`last_error`),
  `GisQueryMode`, `GisPanelTab` (Query/Annotations/Layers — see root
  `AGENTS.md` §gis-query-ui "Round 2 additions"), `GisResultRow` (now also
  carries `annotation_color` for the Annotations tab's swatch), and
  `center_bbox_on` (recomputes the bbox min/max text buffers around a given
  XZ center — used by the panel's "Center on Selection"/"Center on Origin"
  buttons).
- `query.rs` — pure SurrealQL query builders (`annotation_query` — also
  selects `gis.annotation.color` as `annotation_color`,
  `property_filter_query` — both bind user-controlled values via `vars`,
  never string-format them), the property-filter value-type coercion
  (`parse_filter_value`, mirrors `panels::inspector`'s Add Property row),
  `DbResult::QueryResult` row parsing (`parse_gis_rows`, defensive about
  whether SurrealDB's raw-query serialization of a `geometry<point>` field
  is the GeoJSON shape or a bare `[x, z]` array — no existing fe-ui
  precedent pinned this down), and the bbox predicate/parser
  (`bbox_contains`, `parse_bbox_fields`) used by the panel's client-side
  bbox mode.
- `layers.rs` — `LayerUiEntry` (display row) + `layer_entries_from_terrain_json`
  (read) + `set_layer_field` (pure mutate-and-return-clone of the stored
  terrain JSON — find-or-insert a layer by `name`, update only the
  `Some(..)` fields). Also owns `ViewMode` (Mesh/Splats/Hybrid) +
  `view_mode_from_terrain_json`/`set_view_mode_field` — same mutate-clone
  idiom, applied to the additive `"view_mode"` field instead of the
  `"layers"` array; see root `AGENTS.md` §gis-query-ui.

Only `annotation_query`/`property_filter_query`/`parse_gis_rows`/
`bbox_contains`/`parse_bbox_fields` (from `query.rs`) and
`layer_entries_from_terrain_json`/`set_layer_field`/`set_view_mode_field`/
`view_mode_from_terrain_json`/`LayerUiEntry`/`ViewMode` (from `layers.rs`)
are re-exported at `crate::gis::*`; `GisPanelState`/`GisPanelTab`/
`GisQueryMode`/`GisResultRow` live directly in `mod.rs`. `layers`/`query`
submodules themselves stay private — go through the `crate::gis::*`
re-exports rather than `crate::gis::query::*`/`crate::gis::layers::*`.

**Naming note:** there are two "gis" modules by design —
`crate::gis` (this one: state + pure logic, a "domain manager" like
`terrain_map`) and `crate::actions::gis` (the I/O dispatcher, like
`actions::hexon`). Rust resolves the unqualified `gis::` used inside
`actions/mod.rs`'s `process_ui_actions` to the latter (the sibling
submodule declared in the same file); `actions::gis`'s own body
disambiguates by writing `crate::gis::{self, ...}` for the former. Don't
merge them — the split keeps pure logic unit-testable without any
`DbCommandSender`/Bevy `Res`/`ResMut` in scope.
