//! GIS query + layer-manager action handling: submits the SurrealQL built by
//! `crate::gis::query` over `DbCommand::RawQuery`, and round-trips terrain
//! layer edits via the existing `SetPetalTerrain` flow (mirrors
//! `actions::hexon::set_petal_map_scale`'s "mutate one field, round-trip"
//! idiom — see `fe-ui/src/AGENTS.md` §gis-query-ui).

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::gis::{self, GisPanelState};
use crate::terrain_map::PetalMapState;

fn submit_query(
    db_sender: &DbCommandSender,
    gis_state: &mut GisPanelState,
    sql: String,
    vars: std::collections::HashMap<String, serde_json::Value>,
) {
    gis_state.query_pending = true;
    gis_state.last_error = None;
    if db_sender.0.send(DbCommand::RawQuery { sql, vars }).is_err() {
        bevy::log::warn!("db_sender channel closed — GIS RawQuery not dispatched");
        gis_state.query_pending = false;
        gis_state.last_error = Some("DB channel closed".to_string());
    }
}

/// Runs the "nodes with annotations" query for `petal_id`.
pub(crate) fn query_annotated(db_sender: &DbCommandSender, gis_state: &mut GisPanelState, petal_id: String) {
    let (sql, vars) = gis::annotation_query(&petal_id);
    submit_query(db_sender, gis_state, sql, vars);
}

/// Runs the property key/value filter query for `petal_id`.
pub(crate) fn query_property_filter(
    db_sender: &DbCommandSender,
    gis_state: &mut GisPanelState,
    petal_id: String,
    key: String,
    value: serde_json::Value,
) {
    let (sql, vars) = gis::property_filter_query(&petal_id, &key, value);
    submit_query(db_sender, gis_state, sql, vars);
}

/// Toggles a single terrain layer's visibility/opacity on the active petal's
/// stored terrain JSON and persists via `SetPetalTerrain`. No-op (with a
/// warn) when no terrain JSON has been loaded for `petal_id` yet — the Layer
/// Manager card only shows this control when `petal_map.terrain_json` is
/// `Some`, so this should be unreachable in practice.
pub(crate) fn set_layer(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    petal_id: String,
    layer_name: String,
    visible: Option<bool>,
    opacity: Option<f32>,
) {
    let Some(current) = petal_map.terrain_json.clone() else {
        bevy::log::warn!(
            "GIS: no terrain JSON loaded for petal {petal_id}; cannot toggle layer '{layer_name}'"
        );
        return;
    };
    let updated = gis::set_layer_field(&current, &layer_name, visible, opacity);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(updated.clone()),
    }) {
        Ok(()) => {
            // Optimistic local update; DbResult::PetalTerrainLoaded confirms
            // (mirrors actions::hexon::set_petal_map_scale).
            petal_map.terrain_json = Some(updated);
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain (layer '{layer_name}') not dispatched"
            );
        }
    }
}

/// Sets the active petal's splat view mode (`"mesh"|"splats"|"hybrid"`) on
/// the stored terrain JSON and persists via `SetPetalTerrain` — same
/// mutate-then-persist idiom as `set_layer`. Unrecognized `view_mode`
/// strings are dropped (with a warn) rather than written, since the field
/// contract is fixed by the renderer track consuming it.
pub(crate) fn set_view_mode(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    petal_id: String,
    view_mode: String,
) {
    let Some(mode) = gis::ViewMode::from_str(&view_mode) else {
        bevy::log::warn!("GIS: unrecognized view_mode '{view_mode}' — not sent");
        return;
    };
    let Some(current) = petal_map.terrain_json.clone() else {
        bevy::log::warn!("GIS: no terrain JSON loaded for petal {petal_id}; cannot set view_mode");
        return;
    };
    let updated = gis::set_view_mode_field(&current, mode);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(updated.clone()),
    }) {
        Ok(()) => {
            petal_map.terrain_json = Some(updated);
        }
        Err(_) => {
            bevy::log::warn!("db_sender channel closed — SetPetalTerrain (view_mode) not dispatched");
        }
    }
}
