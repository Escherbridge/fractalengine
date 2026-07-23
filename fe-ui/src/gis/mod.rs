//! GIS Query panel state + pure query/layer logic. See
//! `fe-ui/src/AGENTS.md` §gis-query-ui for the end-to-end design and
//! `crate::actions::gis` for the I/O (DbCommand submission) that drives this
//! state from `UiAction`.

pub(crate) mod egress_strings;
mod layers;
mod query;

pub(crate) use layers::{
    layer_entries_from_terrain_json, set_layer_field, set_view_mode_field,
    view_mode_from_terrain_json, ViewMode,
};
pub(crate) use query::{
    annotation_query, bbox_contains, decode_gpx_points, parse_bbox_fields, parse_filter_value,
    parse_gis_rows, property_filter_query, track_query,
};

use bevy::prelude::*;
use fe_sdk::path_asset::PathAssetDescriptor;

/// Which query mode the GIS panel is currently building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GisQueryMode {
    #[default]
    Annotated,
    PropertyFilter,
    Bbox,
}

/// Which top-level tab the GIS panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GisPanelTab {
    #[default]
    Query,
    Annotations,
    Layers,
    Paths,
    Export,
}

/// A single query result row: enough to display and to select+focus the node.
#[derive(Debug, Clone, PartialEq)]
pub struct GisResultRow {
    pub node_id: String,
    pub name: String,
    pub position: [f32; 3],
    pub annotation_title: Option<String>,
    /// `gis.annotation.color` hex string, when the row came from the
    /// annotated-nodes query — used for the Annotations tab's swatch.
    pub annotation_color: Option<String>,
}

/// GIS Query & Layer Manager panel state — an independent floating window,
/// not part of the mutual-exclusion `ActiveDialog` set (see
/// `fe-ui/src/dialogs/AGENTS.md`), so it can stay open alongside the
/// inspector.
#[derive(Resource)]
pub struct GisPanelState {
    pub open: bool,
    pub active_tab: GisPanelTab,
    pub mode: GisQueryMode,
    // Property-filter mode
    pub filter_key_buf: String,
    pub filter_value_buf: String,
    pub filter_value_type_buf: String,
    // Bbox mode (local XZ plane, see `query::bbox_contains`)
    pub bbox_min: [String; 2],
    pub bbox_max: [String; 2],
    // Shared result state
    pub results: Vec<GisResultRow>,
    /// `true` while a `RawQuery` round-trip issued by this panel is in
    /// flight — lets `verse_manager::db_results` route the (untagged)
    /// `DbResult::QueryResult` here instead of the inspector's ad-hoc Query
    /// tab buffer. See AGENTS.md for why `DbResult::QueryResult` has no
    /// request-id to correlate against instead.
    pub query_pending: bool,
    pub last_error: Option<String>,
    /// Export-tab "Copy for BI" card state (analytics_egress_20260714).
    pub(crate) egress: egress_strings::EgressCardState,
}

impl Default for GisPanelState {
    fn default() -> Self {
        Self {
            open: false,
            active_tab: GisPanelTab::default(),
            mode: GisQueryMode::default(),
            filter_key_buf: String::new(),
            filter_value_buf: String::new(),
            filter_value_type_buf: "string".into(),
            bbox_min: ["-50".into(), "-50".into()],
            bbox_max: ["50".into(), "50".into()],
            results: Vec::new(),
            query_pending: false,
            last_error: None,
            egress: egress_strings::EgressCardState::default(),
        }
    }
}

impl GisPanelState {
    /// Resets the bbox fields to a fixed-size box centered on `center` (XZ).
    pub(crate) fn center_bbox_on(&mut self, center: [f32; 2], half_extent: f32) {
        self.bbox_min = [
            format!("{:.1}", center[0] - half_extent),
            format!("{:.1}", center[1] - half_extent),
        ];
        self.bbox_max = [
            format!("{:.1}", center[0] + half_extent),
            format!("{:.1}", center[1] + half_extent),
        ];
    }
}

/// pen_curve_tool_20260722: anchor classification. Corner = independent/absent
/// handles; Smooth = collinear, free lengths; Symmetric = collinear + equal length.
/// fe-ui-local twin of `fe_terrain::iot::animation::CornerKind` (no cross-crate import).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CornerKind {
    #[default]
    Corner,
    Smooth,
    Symmetric,
}

impl CornerKind {
    /// Wire code for the 12-slot `gpx_points` encoding.
    pub fn to_code(self) -> f64 {
        match self {
            Self::Corner => 0.0,
            Self::Smooth => 1.0,
            Self::Symmetric => 2.0,
        }
    }
    /// Inverse of `to_code`; unknown codes decode to `Corner`.
    pub fn from_code(c: f64) -> Self {
        match c as i64 {
            1 => Self::Smooth,
            2 => Self::Symmetric,
            _ => Self::Corner,
        }
    }
}

/// A single point in the track currently being edited. `time_seconds` is
/// `None` for points authored via "Append from cursor" (no timestamp).
/// Local-only editing buffer — fe-ui never persists this itself, see
/// `crate::path_ops`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathPointRow {
    pub position: [f32; 3],
    pub time_seconds: Option<f64>,
    /// bezier in-handle: RELATIVE meter offset from position; None = no handle.
    pub handle_in: Option<[f32; 3]>,
    /// bezier out-handle: RELATIVE meter offset from position; None = no handle.
    pub handle_out: Option<[f32; 3]>,
    /// anchor corner classification (Corner/Smooth/Symmetric).
    pub corner: CornerKind,
    /// "corner settings" knob 0=sharp..1=round.
    pub smoothness: f32,
}

/// Paths tab state: the list of track nodes (from `track_query`) plus the
/// currently-selected-for-edit track's local point-list buffer. See
/// `fe-ui/src/AGENTS.md` §path-editor.
#[derive(Resource, Default)]
pub struct PathEditorState {
    /// Track nodes in the active petal, refreshed via `track_query`.
    pub tracks: Vec<GisResultRow>,
    /// `true` while a `RawQuery` round-trip for the track list is in
    /// flight — same routing flag idiom as `GisPanelState::query_pending`,
    /// kept separate since Paths and Query/Annotations can be open on
    /// different tabs whose `RawQuery` replies would otherwise collide.
    pub tracks_pending: bool,
    /// Buffer for the "New Path" name field.
    pub new_track_name_buf: String,
    /// The track node currently selected for editing, if any.
    pub editing_track_id: Option<String>,
    /// Local point-list buffer for `editing_track_id`, seeded by a
    /// `PathSelectTrack` read-back (`GetNodeProperties` → `NodePropertiesLoaded`)
    /// and then built up further purely from queued `PathOp`s (Append/Remove).
    /// See `fe-ui/src/AGENTS.md` §path-editor.
    pub points: Vec<PathPointRow>,
    /// `true` while the `GetNodeProperties` round-trip issued by
    /// `PathSelectTrack` is in flight — gates `verse_manager::db_results`'
    /// `NodePropertiesLoaded` handling so it repopulates `points` instead of
    /// being swallowed by (or stomping) the inspector's own property-load
    /// guard. Same idiom as `tracks_pending`.
    pub points_pending: bool,
    pub last_error: Option<String>,
    /// Track row whose Delete awaits the inline two-step confirm (Paths list).
    pub pending_track_delete: Option<String>,
    /// Point index whose annotation form is currently open, if any. Set by a
    /// modifier-click on a point marker or the list "Annotate" flow; drives the
    /// inline title/body/color form in `path_editor_card`.
    pub annotating_index: Option<usize>,
    /// Inline annotation form buffers for `annotating_index`.
    pub annotate_title_buf: String,
    pub annotate_body_buf: String,
    pub annotate_color_buf: String,
    /// The in-flight Pen auto-create, stashed while the new track's `node_id`
    /// is in flight. `Some` only between a no-track Pen click (queues
    /// `PathCreateTrack` carrying `correlation_id`) and the matching
    /// `DbResult::NodeCreated` whose echoed `correlation_id` flushes the first
    /// point as a `PathAppendPoint`. The flush matches on `correlation_id`
    /// (NOT a content heuristic), so a concurrent foreign create — a GPX import
    /// or the create-entity dialog — can never hijack it. See
    /// `pen_autocreate_track_20260713` + `fe-ui/src/AGENTS.md` §path-editor.
    pub pending_pen_create: Option<PendingPenCreate>,
    /// track_styling_20260713: the edited track's current per-track style,
    /// seeded from its loaded `gis.track.*` node props when a track is selected
    /// (see `verse_manager::db_results`) and bound to the Paths-tab controls.
    /// Stored as raw fields (not `fe_terrain::TrackStyle`) since fe-ui must not
    /// depend on fe-terrain. Default reproduces the historic cyan look.
    pub edited_track_style: TrackStyleFields,
    /// The edited track's `path_asset` stamp descriptor, seeded from its loaded
    /// `path_asset` node prop when a track is selected (see
    /// `verse_manager::db_results`) and refreshed after a `PathAssetApply`
    /// write round-trips. Feeds `verse_manager::PathAssetCache` via
    /// `reconcile_path_asset` for live edits; petal-wide stamping is done by
    /// `materialize_path_assets` (see verse_manager/AGENTS.md
    /// §path-asset-materialization). `None` when the track has no stamp. Same
    /// load/reset seam as `edited_track_style`.
    pub edited_track_path_asset: Option<PathAssetDescriptor>,
    /// path_interaction_20260716 (FR-2): the selected path VERTEX, if any —
    /// index into `points`. Mutually exclusive with `selected_segment`. Set by a
    /// marker click (`node_manager::path_point_interaction`), highlighted in the
    /// viewport + the Paths-tab point list. Cleared on stop/count-change.
    pub selected_point: Option<usize>,
    /// path_interaction_20260716 (FR-3): the selected path SEGMENT, if any —
    /// segment `i` spans `points[i] → points[i+1]`. Mutually exclusive with
    /// `selected_point`. Set by a ribbon click between vertices
    /// (`node_manager::path_segment_interaction`).
    pub selected_segment: Option<usize>,
    /// path_interaction_20260716 (FR-3): total path length in real meters, kept
    /// live by `node_manager::sync_path_measurements` (world distance /
    /// `PetalMapState.world_scale`). `None` when not editing. The Paths tab
    /// formats it — fe-ui can't reach `world_scale` at the panel call site.
    pub total_length_m: Option<f64>,
    /// path_interaction_20260716 (FR-3): the selected segment's length in real
    /// meters, or `None` when no segment is selected. Same producer/consumer as
    /// `total_length_m`.
    pub selected_segment_length_m: Option<f64>,
}

/// track_styling_20260713: the Paths-tab style-control state — a fe-ui-local
/// mirror of `fe_terrain::iot::TrackStyle` (fe-ui must not depend on
/// fe-terrain). `color` is sRGB `0.0..=1.0` RGBA. Default = the historic cyan
/// look so a track with no persisted style shows the picker at its real value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackStyleFields {
    pub color: [f32; 4],
    pub width: f32,
    pub visible: bool,
}

impl Default for TrackStyleFields {
    fn default() -> Self {
        // Width 0.1 petal-local meters, mirroring
        // `fe_terrain::iot::TrackStyle::default` (fe-ui must not depend on it).
        Self {
            color: [0.0, 0.8, 1.0, 1.0],
            width: 0.1,
            visible: true,
        }
    }
}

impl TrackStyleFields {
    /// Seed the fields from a loaded node-property bag. Missing/invalid values
    /// fall back to that field's default (never panics) — mirrors the bridge's
    /// `style_from_properties`. `#rrggbb`/`#rrggbbaa` (with or without `#`).
    pub(crate) fn from_properties(props: &serde_json::Value) -> Self {
        let mut s = Self::default();
        if let Some(color) = props
            .get("gis.track.color")
            .and_then(|v| v.as_str())
            .and_then(parse_hex_color)
        {
            s.color = color;
        }
        if let Some(width) = props.get("gis.track.width").and_then(|v| v.as_f64()) {
            if width.is_finite() && width > 0.0 {
                s.width = width as f32;
            }
        }
        if let Some(visible) = props.get("gis.track.visible").and_then(|v| v.as_bool()) {
            s.visible = visible;
        }
        s
    }
}

/// Parse a `#rrggbb` / `#rrggbbaa` (optional `#`) hex color into sRGB
/// `[f32; 4]` in `0.0..=1.0`. `None` on malformed input (caller keeps the
/// default). fe-ui-local twin of `fe_terrain::iot::parse_track_color_hex`.
pub(crate) fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    let (r, g, b, a) = match s.len() {
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            255u8,
        ),
        8 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            u8::from_str_radix(&s[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

impl PathEditorState {
    /// Starts editing `track_node_id` with an empty point buffer. Callers
    /// that want the persisted `gpx_points` read back (the normal
    /// track-selection path) should route through `UiAction::PathSelectTrack`
    /// instead of calling this directly — it also sets `points_pending` and
    /// issues the `GetNodeProperties` round-trip that repopulates `points`.
    /// See `fe-ui/src/AGENTS.md` §path-editor.
    pub(crate) fn start_editing(&mut self, track_node_id: String) {
        self.editing_track_id = Some(track_node_id);
        self.points.clear();
        // Reset to defaults; the real style is seeded when the track's props
        // load (verse_manager::db_results), same seam as `points`.
        self.edited_track_style = TrackStyleFields::default();
        // Clear the stamp descriptor too; re-seeded from the loaded props.
        self.edited_track_path_asset = None;
        self.clear_path_selection();
    }

    pub(crate) fn stop_editing(&mut self) {
        self.editing_track_id = None;
        self.points.clear();
        self.points_pending = false;
        self.pending_pen_create = None;
        self.edited_track_style = TrackStyleFields::default();
        self.edited_track_path_asset = None;
        self.clear_path_selection();
        self.close_annotate_form();
    }

    /// Full reset when the active petal changes: ends any edit session and
    /// drops the previous petal's track list so Pen clicks can't append to a
    /// foreign-petal track. Watched by `node_manager::open_track_on_select`.
    pub(crate) fn reset_for_petal_change(&mut self) {
        self.stop_editing();
        self.tracks.clear();
        self.tracks_pending = false;
        self.last_error = None;
        self.pending_track_delete = None;
    }

    /// path_interaction_20260716 (FR-2/FR-3): drop the vertex/segment selection
    /// and its cached measurements. Called on stop-editing, start-editing, and
    /// whenever the point count changes (a stale index would mis-highlight).
    pub(crate) fn clear_path_selection(&mut self) {
        self.selected_point = None;
        self.selected_segment = None;
        self.total_length_m = None;
        self.selected_segment_length_m = None;
    }

    /// Opens the inline annotation form for point `index`, seeding the title
    /// buffer with the v1 placeholder (`"Waypoint {index}"`).
    pub(crate) fn open_annotate_form(&mut self, index: usize) {
        self.annotating_index = Some(index);
        self.annotate_title_buf = format!("Waypoint {index}");
        self.annotate_body_buf.clear();
        self.annotate_color_buf.clear();
    }

    pub(crate) fn close_annotate_form(&mut self) {
        self.annotating_index = None;
        self.annotate_title_buf.clear();
        self.annotate_body_buf.clear();
        self.annotate_color_buf.clear();
    }

    /// `true` while a Pen auto-create is in flight awaiting the new track's
    /// `NodeCreated`. Gates the pen system so a second click before the create
    /// round-trips can't queue a second track.
    pub(crate) fn has_pending_pen_create(&self) -> bool {
        self.pending_pen_create.is_some()
    }

    /// If a Pen auto-create is pending AND `correlation_id` matches it, consume
    /// it (clearing the flag) and return its stashed first point. Returns `None`
    /// on a mismatch or when nothing is pending — so a foreign `NodeCreated`
    /// (GPX import, create-entity dialog) can never flush the pen point onto the
    /// wrong node. See `pen_autocreate_track_20260713` + AGENTS.md §path-editor.
    pub(crate) fn take_pending_pen_create_if(&mut self, correlation_id: &str) -> Option<[f32; 3]> {
        match &self.pending_pen_create {
            Some(p) if p.correlation_id == correlation_id => {
                self.pending_pen_create.take().map(|p| p.first_point)
            }
            _ => None,
        }
    }

    /// Clears the pending Pen auto-create unconditionally, returning `true` if
    /// one was in flight. Called on the pen's own `PathCreateTrack` failure so a
    /// dead create can't leave the pen permanently stuck (HIGH-2).
    pub(crate) fn clear_pending_pen_create(&mut self) -> bool {
        self.pending_pen_create.take().is_some()
    }
}

/// An in-flight Pen auto-create: the fe-ui-generated `correlation_id` the
/// `PathCreateTrack` carries end-to-end (fe-ui → `PathOp` → gpx bridge →
/// `CreateNode` → echoed on `NodeCreated`) plus the first click's world
/// position to flush once the track's `node_id` arrives. Matching the flush on
/// `correlation_id` — not on node content — is what makes the deferred flush
/// hijack-proof. See `fe-ui/src/AGENTS.md` §path-editor.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPenCreate {
    pub correlation_id: String,
    pub first_point: [f32; 3],
}

/// Process-unique correlation id for a Pen auto-created track. fe-ui-side twin
/// of the bridge's `next_authored_track_correlation_id` (which fe-ui never sees,
/// so it can't match on it); the `pen-track:` prefix keeps the two id spaces
/// disjoint. Monotonic `AtomicU64`, no `rand`/`uuid` dep needed.
pub(crate) fn next_pen_correlation_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("pen-track:{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;

    #[test]
    fn default_state_is_closed_with_origin_bbox() {
        let gis = GisPanelState::default();
        assert!(!gis.open);
        assert_eq!(gis.active_tab, GisPanelTab::Query);
        assert_eq!(gis.mode, GisQueryMode::Annotated);
        assert_eq!(parse_bbox_fields(&gis.bbox_min), Some([-50.0, -50.0]));
        assert_eq!(parse_bbox_fields(&gis.bbox_max), Some([50.0, 50.0]));
    }

    #[test]
    fn center_bbox_on_recomputes_min_max() {
        let mut gis = GisPanelState::default();
        gis.center_bbox_on([10.0, 20.0], 5.0);
        assert_eq!(parse_bbox_fields(&gis.bbox_min), Some([5.0, 15.0]));
        assert_eq!(parse_bbox_fields(&gis.bbox_max), Some([15.0, 25.0]));
    }

    #[test]
    fn path_editor_default_has_no_active_edit() {
        let s = PathEditorState::default();
        assert!(s.editing_track_id.is_none());
        assert!(s.points.is_empty());
        assert!(s.tracks.is_empty());
    }

    #[test]
    fn path_editor_start_editing_clears_points() {
        let mut s = PathEditorState::default();
        s.points.push(PathPointRow {
            position: [1.0, 0.0, 1.0],
            time_seconds: None,
            ..Default::default()
        });
        s.start_editing("track-1".to_string());
        assert_eq!(s.editing_track_id.as_deref(), Some("track-1"));
        assert!(s.points.is_empty());
    }

    #[test]
    fn path_editor_stop_editing_clears_state() {
        let mut s = PathEditorState::default();
        s.start_editing("track-1".to_string());
        s.points.push(PathPointRow {
            position: [1.0, 0.0, 1.0],
            time_seconds: None,
            ..Default::default()
        });
        s.stop_editing();
        assert!(s.editing_track_id.is_none());
        assert!(s.points.is_empty());
    }

    #[test]
    fn reset_for_petal_change_clears_list_and_session() {
        let mut s = PathEditorState::default();
        s.tracks.push(GisResultRow {
            node_id: "track-1".to_string(),
            name: "track-1".to_string(),
            position: [0.0, 0.0, 0.0],
            annotation_title: None,
            annotation_color: None,
        });
        s.tracks_pending = true;
        s.last_error = Some("boom".to_string());
        s.pending_track_delete = Some("track-1".to_string());
        s.start_editing("track-1".to_string());
        s.reset_for_petal_change();
        assert!(s.editing_track_id.is_none());
        assert!(s.tracks.is_empty());
        assert!(!s.tracks_pending);
        assert!(s.last_error.is_none());
        assert!(s.pending_track_delete.is_none());
    }

    #[test]
    fn pending_pen_create_defaults_none() {
        let s = PathEditorState::default();
        assert!(s.pending_pen_create.is_none());
        assert!(!s.has_pending_pen_create());
    }

    #[test]
    fn track_style_fields_default_is_cyan_visible() {
        let s = TrackStyleFields::default();
        assert_eq!(s.color, [0.0, 0.8, 1.0, 1.0]);
        assert_eq!(s.width, 0.1);
        assert!(s.visible);
    }

    #[test]
    fn track_style_fields_from_properties_reads_all_and_defaults_missing() {
        let props = serde_json::json!({
            "gis.track.color": "#ff0000",
            "gis.track.width": 6.0,
            // visible absent → default true
        });
        let s = TrackStyleFields::from_properties(&props);
        assert_eq!(s.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.width, 6.0);
        assert!(s.visible);
    }

    #[test]
    fn track_style_fields_from_properties_falls_back_on_invalid() {
        let props = serde_json::json!({
            "gis.track.color": "bad",
            "gis.track.width": 0.0, // non-positive → default
            "gis.track.visible": false,
        });
        let s = TrackStyleFields::from_properties(&props);
        assert_eq!(s.color, TrackStyleFields::default().color);
        assert_eq!(s.width, TrackStyleFields::default().width);
        assert!(!s.visible);
    }

    #[test]
    fn parse_hex_color_valid_and_invalid() {
        assert_eq!(
            parse_hex_color("#00ccff"),
            Some([0.0, 204.0 / 255.0, 1.0, 1.0])
        );
        assert_eq!(
            parse_hex_color("00ccffff"),
            Some([0.0, 204.0 / 255.0, 1.0, 1.0])
        );
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn start_editing_resets_style_to_default() {
        let mut s = PathEditorState::default();
        s.edited_track_style = TrackStyleFields {
            color: [1.0, 0.0, 0.0, 1.0],
            width: 9.0,
            visible: false,
        };
        s.start_editing("track-1".to_string());
        assert_eq!(s.edited_track_style, TrackStyleFields::default());
    }

    fn sample_descriptor() -> PathAssetDescriptor {
        PathAssetDescriptor {
            asset_path: "blob://x.glb".to_string(),
            spacing_mode: fe_sdk::path_asset::SpacingMode::FixedCount,
            spacing_value: 0.0,
            count: 3,
            tangent_align: true,
        }
    }

    #[test]
    fn start_editing_clears_path_asset_descriptor() {
        let mut s = PathEditorState::default();
        s.edited_track_path_asset = Some(sample_descriptor());
        s.start_editing("track-1".to_string());
        assert!(
            s.edited_track_path_asset.is_none(),
            "start_editing must clear the stale descriptor"
        );
    }

    #[test]
    fn stop_editing_clears_path_asset_descriptor() {
        let mut s = PathEditorState::default();
        s.start_editing("track-1".to_string());
        s.edited_track_path_asset = Some(sample_descriptor());
        s.stop_editing();
        assert!(
            s.edited_track_path_asset.is_none(),
            "stop_editing must clear the descriptor"
        );
    }

    #[test]
    fn pending_pen_create_ids_are_unique_and_prefixed() {
        let a = next_pen_correlation_id();
        let b = next_pen_correlation_id();
        assert!(a.starts_with("pen-track:"), "got {a}");
        assert_ne!(a, b, "each call yields a distinct id");
    }

    #[test]
    fn take_pending_pen_create_if_matching_flushes_and_clears() {
        // Mirrors the NodeCreated flush: the echoed correlation_id matches the
        // pending create → take the stashed first point and clear the flag.
        let mut s = PathEditorState::default();
        s.pending_pen_create = Some(PendingPenCreate {
            correlation_id: "pen-track:7".to_string(),
            first_point: [3.0, 0.0, 4.0],
        });
        assert!(s.has_pending_pen_create());
        assert_eq!(
            s.take_pending_pen_create_if("pen-track:7"),
            Some([3.0, 0.0, 4.0])
        );
        assert!(!s.has_pending_pen_create(), "match clears the flag");
        assert_eq!(
            s.take_pending_pen_create_if("pen-track:7"),
            None,
            "second take yields None"
        );
    }

    #[test]
    fn take_pending_pen_create_if_mismatch_does_not_clear() {
        // HIGH-1: a foreign create (GPX import / dialog) carries a different
        // correlation_id (or None → the caller never calls this), so the pen
        // point must NOT be flushed and the pending create must remain intact.
        let mut s = PathEditorState::default();
        s.pending_pen_create = Some(PendingPenCreate {
            correlation_id: "pen-track:2".to_string(),
            first_point: [1.0, 0.0, 1.0],
        });
        assert_eq!(
            s.take_pending_pen_create_if("authored-track:0"),
            None,
            "mismatch never flushes"
        );
        assert!(
            s.has_pending_pen_create(),
            "mismatch leaves the pending create untouched"
        );
        // The correct id still flushes it afterwards.
        assert_eq!(
            s.take_pending_pen_create_if("pen-track:2"),
            Some([1.0, 0.0, 1.0])
        );
    }

    #[test]
    fn start_editing_then_flush_leaves_track_and_clears_pending() {
        // The NodeCreated flush sequence: match+take the pending point, then
        // start_editing(new_id). After it, the track is active, no pending left.
        let mut s = PathEditorState::default();
        s.pending_pen_create = Some(PendingPenCreate {
            correlation_id: "pen-track:0".to_string(),
            first_point: [1.0, 0.0, 1.0],
        });
        let flushed = s.take_pending_pen_create_if("pen-track:0");
        s.start_editing("auto-track".to_string());
        assert_eq!(flushed, Some([1.0, 0.0, 1.0]));
        assert_eq!(s.editing_track_id.as_deref(), Some("auto-track"));
        assert!(!s.has_pending_pen_create());
        assert!(
            s.points.is_empty(),
            "start_editing seeds an empty buffer for the flushed append"
        );
    }

    #[test]
    fn clear_pending_pen_create_reports_and_clears() {
        // HIGH-2: a failed PathCreateTrack (DbResult::Error) must clear the
        // pending create so the pen isn't left permanently stuck.
        let mut s = PathEditorState::default();
        assert!(!s.clear_pending_pen_create(), "nothing pending → false");
        s.pending_pen_create = Some(PendingPenCreate {
            correlation_id: "pen-track:1".to_string(),
            first_point: [2.0, 0.0, 2.0],
        });
        assert!(
            s.clear_pending_pen_create(),
            "pending → cleared, reports true"
        );
        assert!(!s.has_pending_pen_create());
    }

    #[test]
    fn stop_editing_clears_pending_pen_create() {
        // Leaving the editor mid-flight must not strand a stale pending create.
        let mut s = PathEditorState::default();
        s.start_editing("track-1".to_string());
        s.pending_pen_create = Some(PendingPenCreate {
            correlation_id: "pen-track:3".to_string(),
            first_point: [2.0, 0.0, 2.0],
        });
        s.stop_editing();
        assert!(!s.has_pending_pen_create());
    }
}
