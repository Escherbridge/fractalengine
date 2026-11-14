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
- `layers.rs` — the unified overlay registry (VIS-PH5, ui_ux §7):
  `OverlayKey`/`OverlayEntry` rows built by `overlay_registry` (terrain
  layers, then visibility Groups, then the scale bar / metric grid),
  `apply_overlay_toggle` (row → `UiAction`) and `apply_app_overlay`; see
  `fe-ui/src/AGENTS.md` §gis-query-ui. Plus `set_layer_field` (pure mutate-and-return-clone of the stored
  terrain JSON — find-or-insert a layer by `name`, update only the
  `Some(..)` fields). Also owns `ViewMode` (Mesh/Splats/Hybrid) +
  `view_mode_from_terrain_json`/`set_view_mode_field` — same mutate-clone
  idiom, applied to the additive `"view_mode"` field instead of the
  `"layers"` array; see root `AGENTS.md` §gis-query-ui.

Only `annotation_query`/`property_filter_query`/`parse_gis_rows`/
`bbox_contains`/`parse_bbox_fields` (from `query.rs`) and
`overlay_registry`/`apply_overlay_toggle`/`apply_app_overlay`/`set_layer_field`/
`set_view_mode_field`/`view_mode_from_terrain_json`/`OverlayEntry`/`OverlayKey`/`ViewMode`
(from `layers.rs`)
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

## Paths/Data continuation

Annotations has its own correlated read model, independent of Query mode, rows, errors and request.
The panel synchronizes the active Petal even while closed, clears both buffers on a Petal change,
and auto-loads Annotations once per Petal; failed refreshes require an explicit retry. A successful
empty reply renders `0 results`. Unknown and older-Petal request IDs cannot claim this buffer.
Share TTL accepts only integer seconds 1..=86400, matching fe-api; invalid drafts produce no curl.

## Pending Pen continuation context

PendingPenCreate separates the unsent first-anchor continuation from the host's
accepted CreateNode outcome. The production producer captures navigation intent
revision, actual hierarchy IDs, VerseManager incarnation and access generation
when it stashes that anchor. The result consumer checks that original context
at use before starting an edit session or enqueuing any append. A matching reply
with a missing or obsolete context consumes only the unsent intent. Foreign IDs
leave the intended continuation alone; duplicate matching replies cannot append
twice. Accepted host creation ownership remains independent and drains normally.

The raw matching/take helper exists only in state-only tests. Production callers
must use the context-checked consume seam. The existing result-handler journey
covers same-Petal navigation intent changes, fresh access after invalidation,
manager replacement, ordinary matching success once and duplicate birth refusal.

## Copied and pasted node reads (QA-0127)

Every node read built by `gis/egress_strings.rs` (petal, node and bbox builders, `escape_sql_str`
at :79 and `petal_scope_sql`/`node_selection_sql`/`node_by_id_sql`/`bbox_sql` at :84-130) and by the
Query tab templates in `panels/query_tab.rs` (:44, :54) ends in `AND tombstone = NONE`, so a
soft-deleted node vanishes exactly like the REST read. String literals are escaped with backslashes,
as `query_tab::sql_literal` does, because SurrealQL does not accept doubled quotes (R-DATA-1 D1).
The "This node's device data" template (`query_tab.rs:48`) stays unfiltered: `iot_reading` defines no
tombstone field in `fe-database/src/schema.rs`.
The pasted-string round trip (real DB thread, ScopedQuery, GetNodeProperties parity, then tombstone
and zero rows) is `pasted_string_reads_same_node_then_nothing_after_tombstone`
(`egress_strings.rs:420`), called from `api_string_for_returns_node_scoped_sql` (:404-417); it runs under
batch R3 filter `gis::`.
