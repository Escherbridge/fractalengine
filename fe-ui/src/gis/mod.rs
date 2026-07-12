//! GIS Query panel state + pure query/layer logic. See
//! `fe-ui/src/AGENTS.md` §gis-query-ui for the end-to-end design and
//! `crate::actions::gis` for the I/O (DbCommand submission) that drives this
//! state from `UiAction`.

mod layers;
mod query;

pub(crate) use layers::{
    layer_entries_from_terrain_json, set_layer_field, set_view_mode_field,
    view_mode_from_terrain_json, LayerUiEntry, ViewMode,
};
pub(crate) use query::{
    annotation_query, bbox_contains, parse_bbox_fields, parse_filter_value, parse_gis_rows,
    property_filter_query,
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
}
