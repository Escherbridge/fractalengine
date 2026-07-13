//! GIS Query panel state + pure query/layer logic. See
//! `fe-ui/src/AGENTS.md` §gis-query-ui for the end-to-end design and
//! `crate::actions::gis` for the I/O (DbCommand submission) that drives this
//! state from `UiAction`.

mod layers;
mod query;

pub(crate) use layers::{
    layer_entries_from_terrain_json, set_layer_field, set_view_mode_field,
    view_mode_from_terrain_json, ViewMode,
};
pub(crate) use query::{
    annotation_query, bbox_contains, parse_bbox_fields, parse_filter_value, parse_gis_rows,
    property_filter_query, track_query,
};

use bevy::prelude::*;

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

/// A single point in the track currently being edited. `time_seconds` is
/// `None` for points authored via "Append from cursor" (no timestamp).
/// Local-only editing buffer — fe-ui never persists this itself, see
/// `crate::path_ops`.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPointRow {
    pub position: [f32; 3],
    pub time_seconds: Option<f64>,
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
    /// Local point-list buffer for `editing_track_id`. fe-ui builds this up
    /// purely from queued `PathOp`s (Append/Remove) — there is no read-back
    /// from the DB in v1, since the bridge is the only reader/writer of
    /// `gpx_points` (see FR-2/FR-3 in the track spec).
    pub points: Vec<PathPointRow>,
    pub last_error: Option<String>,
    /// Point index whose annotation form is currently open, if any. Set by a
    /// modifier-click on a point marker or the list "Annotate" flow; drives the
    /// inline title/body/color form in `path_editor_card`.
    pub annotating_index: Option<usize>,
    /// Inline annotation form buffers for `annotating_index`.
    pub annotate_title_buf: String,
    pub annotate_body_buf: String,
    pub annotate_color_buf: String,
}

impl PathEditorState {
    /// Starts editing `track_node_id` with a fresh (empty) point buffer —
    /// v1 has no read-back of persisted `gpx_points`, so re-selecting an
    /// existing track always starts from an empty local list.
    pub(crate) fn start_editing(&mut self, track_node_id: String) {
        self.editing_track_id = Some(track_node_id);
        self.points.clear();
    }

    pub(crate) fn stop_editing(&mut self) {
        self.editing_track_id = None;
        self.points.clear();
        self.close_annotate_form();
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
}

#[cfg(test)]
mod tests {
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
        s.points.push(PathPointRow { position: [1.0, 0.0, 1.0], time_seconds: None });
        s.start_editing("track-1".to_string());
        assert_eq!(s.editing_track_id.as_deref(), Some("track-1"));
        assert!(s.points.is_empty());
    }

    #[test]
    fn path_editor_stop_editing_clears_state() {
        let mut s = PathEditorState::default();
        s.start_editing("track-1".to_string());
        s.points.push(PathPointRow { position: [1.0, 0.0, 1.0], time_seconds: None });
        s.stop_editing();
        assert!(s.editing_track_id.is_none());
        assert!(s.points.is_empty());
    }
}
