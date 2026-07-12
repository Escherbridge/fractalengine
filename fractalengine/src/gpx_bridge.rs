//! Bridges fe-ui's queued GPX import ops into parse → project → persist.
//! See src/AGENTS.md §gpx for the full import→persist→render→serve map and
//! the rationale behind the correlation/projection/hierarchy design below.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use bevy::prelude::*;

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_terrain::gpx::{compute_stats, parse_gpx_bytes, scene_nodes_to_gpx, GpxData, TrackStats};
use fe_terrain::iot::animation::{TimestampedRoutePoint, TrackRoute};
use fe_terrain::iot::TrackRouteMap;
use fe_terrain::petal_binding::ActivePetalTerrain;
use fe_terrain::projection::Projection;
use fe_terrain::terrain_plugin::GpxTrackLine;
use fe_terrain::ExportNode;
use fe_ui::gpx_ops::{GpxImportStatus, GpxOp, PendingGpxOps};
use fe_ui::path_ops::{PathEditStatus, PathOp, PendingPathOps};
use fe_ui::verse_manager::VerseManager;

/// Reserved flat property key marking a node as GPX-derived (`"track"` or `"waypoint"`).
const GPX_TYPE_KEY: &str = "gpx_type";
/// Approximates a "child of track" relation — see AGENTS.md §gpx (no `parent_node_id` column exists).
const GPX_TRACK_ID_KEY: &str = "gpx_track_id";
/// Reserved annotation key shared with `fe-database/src/AGENTS.md` §gis.
const ANNOTATION_TITLE_KEY: &str = "gis.annotation.title";
const ANNOTATION_BODY_KEY: &str = "gis.annotation.body";
const ANNOTATION_COLOR_KEY: &str = "gis.annotation.color";
/// Track display name, read by fe-ui's Paths tab query to list track nodes
/// (see `fe-ui::gis::query::track_query`). Set alongside `gpx_type = "track"`
/// on every track-creating path (authored `CreateTrack`, GPX import).
const TRACK_NAME_KEY: &str = "gis.track.name";
/// FR-3: flat node property holding a track's authored/imported points as a
/// JSON array of `[x, y, z, time_seconds]` in petal-local meters. See
/// `src/AGENTS.md` §path-editor.
const GPX_POINTS_KEY: &str = "gpx_points";

// ---------------------------------------------------------------------------
// Pure mapping: GpxData -> node drafts (no Bevy/DB — unit tested directly)
// ---------------------------------------------------------------------------

/// A node about to be persisted: name, projected local position, and the
/// flat property key/value pairs to set on it (one `SetNodeProperty` call
/// per pair — `CreateNode` itself has no `properties` field).
#[derive(Debug, Clone, PartialEq)]
pub struct GpxNodeDraft {
    pub name: String,
    pub position: [f32; 3],
    pub properties: Vec<(String, serde_json::Value)>,
}

/// Prepared persistence plan for one imported GPX file.
#[derive(Debug)]
struct PreparedImport {
    /// `None` when the file has zero trackpoints — see AGENTS.md §gpx.
    track: Option<PreparedTrack>,
    /// Only populated when `track` is `None` (waypoints otherwise become the track's children).
    standalone_waypoints: Vec<GpxNodeDraft>,
}

#[derive(Debug)]
struct PreparedTrack {
    draft: GpxNodeDraft,
    waypoints: Vec<GpxNodeDraft>,
    /// Full projected trackpoint list, kept for the render half (TrackRouteMap).
    route_points: Vec<TimestampedRoutePoint>,
}

/// Resolve the `Projection` to use for a GPX import into `petal_id`.
///
/// Prefers the already-resident `ActivePetalTerrain` state when it matches
/// the target petal (no new DB round trip, no side effect on the viewport's
/// active terrain); falls back to the GPX bounding-box center, same as
/// `fe-api/src/gpx.rs`'s HTTP import endpoint.
fn resolve_projection(petal_id: &str, active: &ActivePetalTerrain, stats: &TrackStats) -> Projection {
    if active.petal_id.as_deref() == Some(petal_id) {
        if let Some(cfg) = &active.config {
            return cfg.origin.clone();
        }
    }
    let bb = &stats.bounding_box;
    Projection::new(
        (bb.min_lat + bb.max_lat) / 2.0,
        (bb.min_lon + bb.max_lon) / 2.0,
        bb.min_ele.unwrap_or(0.0),
    )
}

/// Build the single merged track node draft, positioned at the first
/// trackpoint in document order. Returns `None` when the file has no
/// trackpoints at all (waypoint-only GPX files are handled as standalone).
fn build_track_draft(data: &GpxData, projection: &Projection, stats: &TrackStats) -> Option<GpxNodeDraft> {
    let first_point = data
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| s.points.iter())
        .next()?;

    let local = projection
        .wgs84_to_local(first_point.lat, first_point.lon, first_point.ele.unwrap_or(0.0))
        .ok()?;

    let name = data
        .tracks
        .first()
        .and_then(|t| t.name.clone())
        .or_else(|| data.metadata.as_ref().and_then(|m| m.name.clone()))
        .unwrap_or_else(|| "GPX Track".to_string());

    Some(GpxNodeDraft {
        name: name.clone(),
        position: [local[0] as f32, local[1] as f32, local[2] as f32],
        properties: vec![
            (GPX_TYPE_KEY.to_string(), serde_json::json!("track")),
            (TRACK_NAME_KEY.to_string(), serde_json::json!(name)),
            ("total_distance_m".to_string(), serde_json::json!(stats.total_distance_m)),
            ("elevation_gain_m".to_string(), serde_json::json!(stats.elevation_gain_m)),
            ("elevation_loss_m".to_string(), serde_json::json!(stats.elevation_loss_m)),
            ("duration_s".to_string(), serde_json::json!(stats.duration)),
            ("avg_speed_kmh".to_string(), serde_json::json!(stats.avg_speed_kmh)),
            ("max_speed_kmh".to_string(), serde_json::json!(stats.max_speed_kmh)),
            (
                "bounding_box".to_string(),
                serde_json::json!({
                    "min_lat": stats.bounding_box.min_lat,
                    "max_lat": stats.bounding_box.max_lat,
                    "min_lon": stats.bounding_box.min_lon,
                    "max_lon": stats.bounding_box.max_lon,
                }),
            ),
        ],
    })
}

/// Project every trackpoint (document order) into a renderable route.
/// `time_seconds` is relative to the first timestamp; untimestamped points
/// fall back to their index (uniform 1 s spacing keeps animation monotonic).
fn build_route_points(data: &GpxData, projection: &Projection) -> Vec<TimestampedRoutePoint> {
    let first_time = data
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| s.points.iter())
        .find_map(|p| p.time);
    data.tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| s.points.iter())
        .enumerate()
        .filter_map(|(i, p)| {
            let local = projection.wgs84_to_local(p.lat, p.lon, p.ele.unwrap_or(0.0)).ok()?;
            let time_seconds = match (first_time, p.time) {
                (Some(t0), Some(t)) => (t - t0).num_milliseconds() as f64 / 1000.0,
                _ => i as f64,
            };
            Some(TimestampedRoutePoint { position: [local[0], local[1], local[2]], time_seconds })
        })
        .collect()
}

/// Encode route points as the `gpx_points` JSON array: `[x, y, z, time_seconds]`
/// per point, petal-local meters — see `GPX_POINTS_KEY`.
fn route_points_to_json(points: &[TimestampedRoutePoint]) -> serde_json::Value {
    serde_json::Value::Array(
        points
            .iter()
            .map(|p| serde_json::json!([p.position[0], p.position[1], p.position[2], p.time_seconds]))
            .collect(),
    )
}

/// Decode a `gpx_points` JSON value back into route points. Malformed/short
/// entries are skipped rather than failing the whole track (best-effort —
/// mirrors the writer's own tolerance for missing fields).
fn json_to_route_points(value: &serde_json::Value) -> Vec<TimestampedRoutePoint> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let a = entry.as_array()?;
                    let x = a.first()?.as_f64()?;
                    let y = a.get(1)?.as_f64()?;
                    let z = a.get(2)?.as_f64()?;
                    let t = a.get(3).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    Some(TimestampedRoutePoint { position: [x, y, z], time_seconds: t })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Shared render half: (re)populate `TrackRouteMap` for `track_node_id` and
/// ensure a `GpxTrackLine` entity exists for it. Reused by import, live path
/// edits (FR-3), and petal-load materialization (FR-4) so the spawn/update
/// logic isn't duplicated three times.
///
/// `existing_entity` is the track's current `GpxTrackLine` entity, if any —
/// `render_gpx_tracks` (fe-terrain, not owned here) only (re)builds the mesh
/// for entities `Without<Mesh3d>`, so a live edit must despawn + respawn the
/// entity to force a redraw rather than just mutating `TrackRouteMap`.
fn spawn_track_route(
    route_map: &mut TrackRouteMap,
    commands: &mut Commands,
    track_node_id: &str,
    points: Vec<TimestampedRoutePoint>,
    existing_entity: Option<Entity>,
) {
    if points.len() < 2 {
        route_map.routes.remove(track_node_id);
        if let Some(entity) = existing_entity {
            commands.entity(entity).despawn();
        }
        return;
    }
    let total = points.last().map(|p| p.time_seconds).unwrap_or(0.0);
    route_map
        .routes
        .insert(track_node_id.to_string(), TrackRoute { points, total_duration_secs: total });
    if let Some(entity) = existing_entity {
        commands.entity(entity).despawn();
    }
    commands.spawn(GpxTrackLine { track_node_id: track_node_id.to_string() });
}

/// Build one draft per `<wpt>` in the file. Out-of-range coordinates are
/// silently skipped (mirrors `fe_terrain::gpx::convert`'s precedent).
/// `gpx_track_id` is NOT included here — it's only known once the track's
/// `CreateNode` result resolves, so the bridge system appends it later.
fn build_waypoint_drafts(data: &GpxData, projection: &Projection) -> Vec<GpxNodeDraft> {
    data.waypoints
        .iter()
        .enumerate()
        .filter_map(|(i, wp)| {
            let local = projection.wgs84_to_local(wp.lat, wp.lon, wp.ele.unwrap_or(0.0)).ok()?;
            let name = wp.name.clone().unwrap_or_else(|| format!("Waypoint {}", i + 1));
            Some(GpxNodeDraft {
                name: name.clone(),
                position: [local[0] as f32, local[1] as f32, local[2] as f32],
                properties: vec![
                    (GPX_TYPE_KEY.to_string(), serde_json::json!("waypoint")),
                    (ANNOTATION_TITLE_KEY.to_string(), serde_json::json!(name)),
                ],
            })
        })
        .collect()
}

/// Read + parse a GPX file and build its persistence plan. Pure aside from
/// the filesystem read; `active` supplies the petal-terrain-origin lookup.
fn prepare_import(petal_id: &str, path: &Path, active: &ActivePetalTerrain) -> Result<PreparedImport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read GPX file {}: {e}", path.display()))?;
    let data = parse_gpx_bytes(&bytes).map_err(|e| format!("invalid GPX: {e}"))?;
    let stats = compute_stats(&data);
    let projection = resolve_projection(petal_id, active, &stats);
    let waypoint_drafts = build_waypoint_drafts(&data, &projection);

    match build_track_draft(&data, &projection, &stats) {
        Some(draft) => Ok(PreparedImport {
            track: Some(PreparedTrack {
                draft,
                waypoints: waypoint_drafts,
                route_points: build_route_points(&data, &projection),
            }),
            standalone_waypoints: vec![],
        }),
        None => Ok(PreparedImport { track: None, standalone_waypoints: waypoint_drafts }),
    }
}

// ---------------------------------------------------------------------------
// Bevy resources + systems (DB-command correlation — see AGENTS.md §gpx)
// ---------------------------------------------------------------------------

/// A track node's `CreateNode` was sent; once its `DbResult::NodeCreated`
/// arrives, set these stat properties and create these waypoint children.
struct PendingTrackImport {
    petal_id: String,
    stats_props: Vec<(String, serde_json::Value)>,
    waypoints: Vec<GpxNodeDraft>,
    route_points: Vec<TimestampedRoutePoint>,
}

/// A waypoint node's `CreateNode` was sent; once its `DbResult::NodeCreated`
/// arrives, set its type/title/(optional) track-link properties.
struct PendingWaypointImport {
    title: String,
    track_node_id: Option<String>,
}

/// Correlates fire-and-forget `CreateNode` commands with their eventual
/// `DbResult::NodeCreated` by `(petal_id, name)` — see AGENTS.md §gpx for why
/// this content-based FIFO match (rather than a request ID) is the best
/// available option given the existing command/result channel shape.
#[derive(Resource, Default)]
pub struct PendingGpxImports {
    tracks: HashMap<(String, String), VecDeque<PendingTrackImport>>,
    waypoints: HashMap<(String, String), VecDeque<PendingWaypointImport>>,
}

/// Drains `PendingGpxOps` each frame: parse the file, resolve the petal's
/// terrain projection, and send the first `CreateNode` command(s) — either
/// the merged track node (waypoints become its children once it resolves)
/// or, for waypoint-only files, the waypoints directly as standalones.
pub fn drain_gpx_ops(
    mut ops: ResMut<PendingGpxOps>,
    db_tx: Res<DbCommandSender>,
    active_terrain: Res<ActivePetalTerrain>,
    mut pending: ResMut<PendingGpxImports>,
    mut status: ResMut<GpxImportStatus>,
) {
    if ops.0.is_empty() {
        return;
    }

    for op in ops.0.drain(..) {
        let GpxOp::ImportFile { petal_id, path } = op;
        match prepare_import(&petal_id, &path, &active_terrain) {
            Ok(prepared) => {
                let track_count = prepared.track.is_some() as u32;
                let waypoint_count = (prepared.standalone_waypoints.len()
                    + prepared.track.as_ref().map(|t| t.waypoints.len()).unwrap_or(0))
                    as u32;

                if let Some(track) = prepared.track {
                    let key = (petal_id.clone(), track.draft.name.clone());
                    db_tx
                        .0
                        .send(DbCommand::CreateNode {
                            petal_id: petal_id.clone(),
                            name: track.draft.name.clone(),
                            position: track.draft.position,
                        })
                        .ok();
                    pending.tracks.entry(key).or_default().push_back(PendingTrackImport {
                        petal_id: petal_id.clone(),
                        stats_props: track.draft.properties,
                        waypoints: track.waypoints,
                        route_points: track.route_points,
                    });
                }

                for wp in prepared.standalone_waypoints {
                    let key = (petal_id.clone(), wp.name.clone());
                    db_tx
                        .0
                        .send(DbCommand::CreateNode {
                            petal_id: petal_id.clone(),
                            name: wp.name.clone(),
                            position: wp.position,
                        })
                        .ok();
                    pending.waypoints.entry(key).or_default().push_back(PendingWaypointImport {
                        title: wp.name,
                        track_node_id: None,
                    });
                }

                tracing::info!(petal_id = %petal_id, track_count, waypoint_count, "GPX import queued");
                *status = GpxImportStatus {
                    petal_id: Some(petal_id),
                    track_count,
                    waypoint_count,
                    error: None,
                };
            }
            Err(e) => {
                tracing::warn!(petal_id = %petal_id, path = %path.display(), "GPX import failed: {e}");
                *status = GpxImportStatus {
                    petal_id: Some(petal_id),
                    track_count: 0,
                    waypoint_count: 0,
                    error: Some(e),
                };
            }
        }
    }
}

/// Advances pending GPX imports as their `CreateNode` results arrive: sets
/// the track's cached stats + spawns its waypoint children, then sets each
/// waypoint's type/title/track-link properties once its own node exists.
pub fn advance_gpx_imports(
    mut db_results: MessageReader<DbResult>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingGpxImports>,
    mut route_map: ResMut<TrackRouteMap>,
    mut commands: Commands,
) {
    if pending.tracks.is_empty() && pending.waypoints.is_empty() {
        return;
    }

    for result in db_results.read() {
        let DbResult::NodeCreated { id, petal_id, name, .. } = result else {
            continue;
        };
        let key = (petal_id.clone(), name.clone());

        if let Some(queue) = pending.tracks.get_mut(&key) {
            if let Some(track) = queue.pop_front() {
                if queue.is_empty() {
                    pending.tracks.remove(&key);
                }
                for (prop_key, value) in track.stats_props {
                    db_tx
                        .0
                        .send(DbCommand::SetNodeProperty { node_id: id.clone(), key: prop_key, value })
                        .ok();
                }
                for wp in track.waypoints {
                    let wp_key = (track.petal_id.clone(), wp.name.clone());
                    db_tx
                        .0
                        .send(DbCommand::CreateNode {
                            petal_id: track.petal_id.clone(),
                            name: wp.name.clone(),
                            position: wp.position,
                        })
                        .ok();
                    pending.waypoints.entry(wp_key).or_default().push_back(PendingWaypointImport {
                        title: wp.name,
                        track_node_id: Some(id.clone()),
                    });
                }
                // FR-4: persist trackpoints as `gpx_points` so the render half
                // survives a session reload, not just this frame's TrackRouteMap.
                if track.route_points.len() >= 2 {
                    db_tx
                        .0
                        .send(DbCommand::SetNodeProperty {
                            node_id: id.clone(),
                            key: GPX_POINTS_KEY.to_string(),
                            value: route_points_to_json(&track.route_points),
                        })
                        .ok();
                    spawn_track_route(&mut route_map, &mut commands, &id, track.route_points, None);
                }
                continue;
            }
        }

        if let Some(queue) = pending.waypoints.get_mut(&key) {
            if let Some(wp) = queue.pop_front() {
                if queue.is_empty() {
                    pending.waypoints.remove(&key);
                }
                db_tx
                    .0
                    .send(DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key: GPX_TYPE_KEY.to_string(),
                        value: serde_json::json!("waypoint"),
                    })
                    .ok();
                db_tx
                    .0
                    .send(DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key: ANNOTATION_TITLE_KEY.to_string(),
                        value: serde_json::json!(wp.title),
                    })
                    .ok();
                if let Some(track_id) = wp.track_node_id {
                    db_tx
                        .0
                        .send(DbCommand::SetNodeProperty {
                            node_id: id.clone(),
                            key: GPX_TRACK_ID_KEY.to_string(),
                            value: serde_json::json!(track_id),
                        })
                        .ok();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FR-2/FR-3/FR-4/FR-5: Path editor (PathOp) — see fe-ui/src/path_ops.rs for
// the op enum and src/AGENTS.md §path-editor for the design.
// ---------------------------------------------------------------------------

/// What to do once a `GetNodeProperties` result for a track node arrives, for
/// ops that need the current `gpx_points` before they can mutate it.
/// Correlated by `node_id` (unique — no FIFO/name-collision risk like the
/// import path's `(petal_id, name)` correlation).
enum PendingPathRead {
    AppendPoint { position: [f32; 3], time_seconds: Option<f64> },
    RemovePoint { index: usize },
    AnnotatePoint { index: usize, title: String, body: String, color: String },
    ExportGpx { save_path: std::path::PathBuf },
}

/// A `CreateTrack` node's `CreateNode` was sent; once `NodeCreated` arrives,
/// mark it as an (empty) track. No extra fields needed — the properties to
/// set are fixed (`gpx_type = "track"`, `gpx_points = []`).
struct PendingTrackCreate;

/// An `AnnotatePoint` waypoint's `CreateNode` was sent; once `NodeCreated`
/// arrives, set its type/title/body/color annotation properties.
struct PendingAnnotateCreate {
    title: String,
    body: String,
    color: String,
}

/// Correlates in-flight path-editor DB round trips. Separate maps because
/// `GetNodeProperties`/`CreateNode` results are disambiguated differently
/// (existing node_id vs. content-based petal_id+name, per AGENTS.md §gpx).
#[derive(Resource, Default)]
pub struct PendingPathEdits {
    reads: HashMap<String, VecDeque<PendingPathRead>>,
    creates: HashMap<(String, String), VecDeque<PendingTrackCreate>>,
    annotate_creates: HashMap<(String, String), VecDeque<PendingAnnotateCreate>>,
}

/// Drains `PendingPathOps` each frame. Ops needing the current point list
/// (`AppendPoint`, `RemovePoint`, `AnnotatePoint`, `ExportGpx`) issue a
/// `GetNodeProperties` and defer their mutation to `advance_path_edits`;
/// `CreateTrack` issues `CreateNode` and defers to the same system;
/// `DeleteTrack` (no `DbCommand::DeleteNode` exists — see AGENTS.md §gpx) is
/// handled inline as a best-effort clear of `gpx_points`/`gpx_type`.
pub fn drain_path_ops(
    mut ops: ResMut<PendingPathOps>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingPathEdits>,
    mut route_map: ResMut<TrackRouteMap>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    mut status: ResMut<PathEditStatus>,
    mut commands: Commands,
) {
    if ops.0.is_empty() {
        return;
    }

    for op in ops.0.drain(..) {
        match op {
            PathOp::CreateTrack { petal_id, name } => {
                let key = (petal_id.clone(), name.clone());
                db_tx
                    .0
                    .send(DbCommand::CreateNode { petal_id: petal_id.clone(), name: name.clone(), position: [0.0; 3] })
                    .ok();
                pending.creates.entry(key).or_default().push_back(PendingTrackCreate);
            }
            PathOp::DeleteTrack { track_node_id } => {
                db_tx
                    .0
                    .send(DbCommand::DeleteNodeProperty { node_id: track_node_id.clone(), key: GPX_POINTS_KEY.to_string() })
                    .ok();
                db_tx
                    .0
                    .send(DbCommand::DeleteNodeProperty { node_id: track_node_id.clone(), key: GPX_TYPE_KEY.to_string() })
                    .ok();
                route_map.routes.remove(&track_node_id);
                if let Some((entity, _)) = track_lines.iter().find(|(_, t)| t.track_node_id == track_node_id) {
                    commands.entity(entity).despawn();
                }
                *status = PathEditStatus {
                    track_node_id: Some(track_node_id),
                    message: Some("Track deleted".to_string()),
                    error: None,
                };
            }
            PathOp::AppendPoint { track_node_id, position, time_seconds } => {
                db_tx.0.send(DbCommand::GetNodeProperties { node_id: track_node_id.clone() }).ok();
                pending
                    .reads
                    .entry(track_node_id)
                    .or_default()
                    .push_back(PendingPathRead::AppendPoint { position, time_seconds });
            }
            PathOp::RemovePoint { track_node_id, index } => {
                db_tx.0.send(DbCommand::GetNodeProperties { node_id: track_node_id.clone() }).ok();
                pending.reads.entry(track_node_id).or_default().push_back(PendingPathRead::RemovePoint { index });
            }
            PathOp::AnnotatePoint { track_node_id, index, title, body, color } => {
                db_tx.0.send(DbCommand::GetNodeProperties { node_id: track_node_id.clone() }).ok();
                pending
                    .reads
                    .entry(track_node_id)
                    .or_default()
                    .push_back(PendingPathRead::AnnotatePoint { index, title, body, color });
            }
            PathOp::ExportGpx { track_node_id } => {
                let Some(dialog_path) = prompt_gpx_save_path(&track_node_id) else {
                    tracing::debug!(track_node_id = %track_node_id, "GPX export cancelled by user");
                    continue;
                };
                db_tx.0.send(DbCommand::GetNodeProperties { node_id: track_node_id.clone() }).ok();
                pending
                    .reads
                    .entry(track_node_id)
                    .or_default()
                    .push_back(PendingPathRead::ExportGpx { save_path: dialog_path });
            }
        }
    }
}

/// Open a native save dialog for a GPX export, suggesting `{track_node_id}.gpx`
/// (the bridge has no node-name lookup here without a DB round trip; the
/// suggested name is a placeholder the user can rename in the dialog).
/// `None` means the user cancelled.
fn prompt_gpx_save_path(track_node_id: &str) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_file_name(format!("{track_node_id}.gpx"))
        .add_filter("GPX", &["gpx"]);
    if let Some(dir) = dirs::download_dir() {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file()
}

/// Advances pending path-editor DB round trips as their results arrive.
pub fn advance_path_edits(
    mut db_results: MessageReader<DbResult>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingPathEdits>,
    mut route_map: ResMut<TrackRouteMap>,
    active_terrain: Res<ActivePetalTerrain>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    mut status: ResMut<PathEditStatus>,
    mut commands: Commands,
) {
    if pending.reads.is_empty() && pending.creates.is_empty() && pending.annotate_creates.is_empty() {
        return;
    }

    for result in db_results.read() {
        match result {
            DbResult::NodeCreated { id, petal_id, name, .. } => {
                let annotate_key = (petal_id.clone(), name.clone());
                if let Some(queue) = pending.annotate_creates.get_mut(&annotate_key) {
                    if let Some(annotate) = queue.pop_front() {
                        if queue.is_empty() {
                            pending.annotate_creates.remove(&annotate_key);
                        }
                        db_tx
                            .0
                            .send(DbCommand::SetNodeProperty {
                                node_id: id.clone(),
                                key: GPX_TYPE_KEY.to_string(),
                                value: serde_json::json!("waypoint"),
                            })
                            .ok();
                        db_tx
                            .0
                            .send(DbCommand::SetNodeProperty {
                                node_id: id.clone(),
                                key: ANNOTATION_TITLE_KEY.to_string(),
                                value: serde_json::json!(annotate.title),
                            })
                            .ok();
                        db_tx
                            .0
                            .send(DbCommand::SetNodeProperty {
                                node_id: id.clone(),
                                key: ANNOTATION_BODY_KEY.to_string(),
                                value: serde_json::json!(annotate.body),
                            })
                            .ok();
                        db_tx
                            .0
                            .send(DbCommand::SetNodeProperty {
                                node_id: id.clone(),
                                key: ANNOTATION_COLOR_KEY.to_string(),
                                value: serde_json::json!(annotate.color),
                            })
                            .ok();
                        *status = PathEditStatus {
                            track_node_id: Some(id.clone()),
                            message: Some("Point annotated".to_string()),
                            error: None,
                        };
                        continue;
                    }
                }

                let key = (petal_id.clone(), name.clone());
                let Some(queue) = pending.creates.get_mut(&key) else { continue };
                let Some(_create) = queue.pop_front() else { continue };
                if queue.is_empty() {
                    pending.creates.remove(&key);
                }
                db_tx
                    .0
                    .send(DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key: GPX_TYPE_KEY.to_string(),
                        value: serde_json::json!("track"),
                    })
                    .ok();
                db_tx
                    .0
                    .send(DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key: GPX_POINTS_KEY.to_string(),
                        value: serde_json::Value::Array(vec![]),
                    })
                    .ok();
                db_tx
                    .0
                    .send(DbCommand::SetNodeProperty {
                        node_id: id.clone(),
                        key: TRACK_NAME_KEY.to_string(),
                        value: serde_json::json!(name.clone()),
                    })
                    .ok();
                *status = PathEditStatus {
                    track_node_id: Some(id.clone()),
                    message: Some("Track created".to_string()),
                    error: None,
                };
            }
            DbResult::NodePropertiesLoaded { node_id, properties } => {
                let Some(queue) = pending.reads.get_mut(node_id) else { continue };
                let Some(action) = queue.pop_front() else { continue };
                if queue.is_empty() {
                    pending.reads.remove(node_id);
                }
                let mut points = properties
                    .get(GPX_POINTS_KEY)
                    .map(json_to_route_points)
                    .unwrap_or_default();

                match action {
                    PendingPathRead::AppendPoint { position, time_seconds } => {
                        points.push(TimestampedRoutePoint {
                            position: [position[0] as f64, position[1] as f64, position[2] as f64],
                            time_seconds: time_seconds.unwrap_or(0.0),
                        });
                        persist_and_render_points(&db_tx, &mut route_map, &track_lines, &mut commands, node_id, points);
                        *status = PathEditStatus {
                            track_node_id: Some(node_id.clone()),
                            message: Some("Point added".to_string()),
                            error: None,
                        };
                    }
                    PendingPathRead::RemovePoint { index } => {
                        if index < points.len() {
                            points.remove(index);
                            persist_and_render_points(&db_tx, &mut route_map, &track_lines, &mut commands, node_id, points);
                            *status = PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: Some("Point removed".to_string()),
                                error: None,
                            };
                        } else {
                            *status = PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            };
                        }
                    }
                    PendingPathRead::AnnotatePoint { index, title, body, color } => {
                        let Some(point) = points.get(index) else {
                            *status = PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            };
                            continue;
                        };
                        // Best-effort petal resolution: the bridge has no
                        // node_id -> petal_id lookup without a hierarchy round
                        // trip, and `PathOp::AnnotatePoint` doesn't carry
                        // petal_id — mirrors `resolve_projection`'s same-frame
                        // active-petal assumption. See INTEGRATION_REQUEST in
                        // the worker report if annotating a non-active petal's
                        // track needs to be supported.
                        let Some(petal_id) = active_terrain.petal_id.clone() else {
                            *status = PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some("cannot annotate: no active petal terrain".to_string()),
                            };
                            continue;
                        };
                        let position = [point.position[0] as f32, point.position[1] as f32, point.position[2] as f32];
                        let key = (petal_id.clone(), title.clone());
                        db_tx
                            .0
                            .send(DbCommand::CreateNode { petal_id, name: title.clone(), position })
                            .ok();
                        pending
                            .annotate_creates
                            .entry(key)
                            .or_default()
                            .push_back(PendingAnnotateCreate { title, body, color });
                    }
                    PendingPathRead::ExportGpx { save_path } => {
                        let export_node = ExportNode {
                            node_id: node_id.clone(),
                            name: node_id.clone(),
                            position: [0.0; 3],
                            properties: Some(serde_json::json!({ "gpx_type": "track" })),
                            children: points_to_export_children(&points),
                        };
                        let projection = active_terrain
                            .config
                            .as_ref()
                            .map(|c| c.origin.clone())
                            .unwrap_or_else(|| Projection::new(0.0, 0.0, 0.0));
                        let xml = scene_nodes_to_gpx(&[export_node], &projection);
                        match std::fs::write(&save_path, xml) {
                            Ok(()) => {
                                tracing::info!(track_node_id = %node_id, path = %save_path.display(), "GPX exported");
                                *status = PathEditStatus {
                                    track_node_id: Some(node_id.clone()),
                                    message: Some(format!("Exported to {}", save_path.display())),
                                    error: None,
                                };
                            }
                            Err(e) => {
                                tracing::warn!(track_node_id = %node_id, "GPX export write failed: {e}");
                                *status = PathEditStatus {
                                    track_node_id: Some(node_id.clone()),
                                    message: None,
                                    error: Some(format!("write failed: {e}")),
                                };
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Persist `points` as `gpx_points` and update the render half in one step —
/// the FR-3 "live edits update immediately" requirement.
fn persist_and_render_points(
    db_tx: &DbCommandSender,
    route_map: &mut TrackRouteMap,
    track_lines: &Query<(Entity, &GpxTrackLine)>,
    commands: &mut Commands,
    track_node_id: &str,
    points: Vec<TimestampedRoutePoint>,
) {
    db_tx
        .0
        .send(DbCommand::SetNodeProperty {
            node_id: track_node_id.to_string(),
            key: GPX_POINTS_KEY.to_string(),
            value: route_points_to_json(&points),
        })
        .ok();
    let existing = track_lines.iter().find(|(_, t)| t.track_node_id.as_str() == track_node_id).map(|(e, _)| e);
    spawn_track_route(route_map, commands, track_node_id, points, existing);
}

/// Wrap route points as the `segment` > `trackpoint` child hierarchy
/// `scene_nodes_to_gpx` expects — synthesized in memory from the flat
/// `gpx_points` representation (there are no real trackpoint child nodes).
fn points_to_export_children(points: &[TimestampedRoutePoint]) -> Vec<ExportNode> {
    let trackpoints = points
        .iter()
        .map(|p| ExportNode {
            node_id: String::new(),
            name: String::new(),
            position: [p.position[0] as f32, p.position[1] as f32, p.position[2] as f32],
            properties: Some(serde_json::json!({ "gpx_type": "trackpoint" })),
            children: vec![],
        })
        .collect();
    vec![ExportNode {
        node_id: String::new(),
        name: String::new(),
        position: [0.0; 3],
        properties: Some(serde_json::json!({ "gpx_type": "segment" })),
        children: trackpoints,
    }]
}

/// FR-4: on petal load/switch, request `GetNodeProperties` for every node in
/// the newly-active petal so any track carrying `gpx_points` can be
/// re-materialized by `advance_path_materialization`. Cheap no-op for petals
/// with no GPX tracks (unmatched results are just ignored downstream).
pub fn request_petal_gpx_materialization(
    active_terrain: Res<ActivePetalTerrain>,
    verse_mgr: Res<VerseManager>,
    db_tx: Res<DbCommandSender>,
    mut last_revision: Local<u64>,
) {
    if !active_terrain.is_changed() || active_terrain.revision == *last_revision {
        return;
    }
    *last_revision = active_terrain.revision;
    let Some(petal_id) = &active_terrain.petal_id else { return };
    let Some(petal) = verse_mgr.find_petal(petal_id) else { return };

    for node in &petal.nodes {
        db_tx.0.send(DbCommand::GetNodeProperties { node_id: node.id.clone() }).ok();
    }
}

/// Consumes `NodePropertiesLoaded` results looking for `gpx_type == "track"`
/// nodes carrying `gpx_points`, and (re)materializes their `TrackRouteMap`
/// entry + `GpxTrackLine` entity. Runs unconditionally (cheap match against
/// whatever `DbResult`s already flowed this frame) rather than maintaining
/// its own pending-set, since `request_petal_gpx_materialization` fires a
/// broad, uncorrelated batch of `GetNodeProperties` — there's nothing to
/// dequeue per-result, just a type-tag check.
pub fn advance_path_materialization(
    mut db_results: MessageReader<DbResult>,
    mut route_map: ResMut<TrackRouteMap>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    mut commands: Commands,
) {
    for result in db_results.read() {
        let DbResult::NodePropertiesLoaded { node_id, properties } = result else { continue };
        let is_track = properties.get(GPX_TYPE_KEY).and_then(|v| v.as_str()) == Some("track");
        if !is_track {
            continue;
        }
        let Some(points_json) = properties.get(GPX_POINTS_KEY) else { continue };
        let points = json_to_route_points(points_json);
        if points.len() < 2 {
            continue;
        }
        let existing = track_lines.iter().find(|(_, t)| t.track_node_id == *node_id).map(|(e, _)| e);
        // Only respawn if this track isn't already rendered (materialization
        // fires once per petal-load batch, not on every unrelated property
        // read) — avoids despawn/respawn churn for tracks already live.
        if existing.is_none() {
            spawn_track_route(&mut route_map, &mut commands, node_id, points, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_terrain::config::TerrainConfig;
    use std::path::PathBuf;

    const SAMPLE_GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test">
  <trk>
    <name>Test Hike</name>
    <trkseg>
      <trkpt lat="45.0" lon="-122.0"><ele>100.0</ele></trkpt>
      <trkpt lat="45.001" lon="-122.001"><ele>110.0</ele></trkpt>
    </trkseg>
  </trk>
  <wpt lat="45.0005" lon="-122.0005">
    <ele>105.0</ele>
    <name>Camp</name>
  </wpt>
  <wpt lat="45.0006" lon="-122.0006">
    <ele>106.0</ele>
  </wpt>
</gpx>"#;

    const WAYPOINT_ONLY_GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test">
  <wpt lat="10.0" lon="20.0">
    <name>Lone Marker</name>
  </wpt>
</gpx>"#;

    fn parse(gpx: &str) -> GpxData {
        parse_gpx_bytes(gpx.as_bytes()).expect("sample GPX must parse")
    }

    fn prop<'a>(draft: &'a GpxNodeDraft, key: &str) -> &'a serde_json::Value {
        draft
            .properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("expected property {key:?} on draft {draft:?}"))
    }

    #[test]
    fn track_draft_positions_at_first_trackpoint_with_matching_origin() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        // Origin == first trackpoint exactly, so the projected position is ~[0,0,0].
        let projection = Projection::new(45.0, -122.0, 100.0);

        let draft = build_track_draft(&data, &projection, &stats).expect("track present");
        assert_eq!(draft.name, "Test Hike");
        assert!(draft.position[0].abs() < 1e-3, "x should be ~0: {:?}", draft.position);
        assert!(draft.position[1].abs() < 1e-3, "y should be ~0: {:?}", draft.position);
        assert!(draft.position[2].abs() < 1e-3, "z should be ~0: {:?}", draft.position);
    }

    #[test]
    fn track_draft_carries_gpx_type_and_all_cached_stat_keys() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let projection = Projection::new(45.0, -122.0, 100.0);
        let draft = build_track_draft(&data, &projection, &stats).unwrap();

        assert_eq!(prop(&draft, "gpx_type"), &serde_json::json!("track"));
        for key in [
            "total_distance_m",
            "elevation_gain_m",
            "elevation_loss_m",
            "duration_s",
            "avg_speed_kmh",
            "max_speed_kmh",
            "bounding_box",
        ] {
            assert!(
                draft.properties.iter().any(|(k, _)| k == key),
                "missing expected stat key {key:?} in {:?}",
                draft.properties
            );
        }
        let bbox = prop(&draft, "bounding_box");
        assert_eq!(bbox["min_lat"], serde_json::json!(45.0));
    }

    #[test]
    fn track_draft_none_when_file_has_no_trackpoints() {
        let data = parse(WAYPOINT_ONLY_GPX);
        let stats = compute_stats(&data);
        let projection = Projection::new(10.0, 20.0, 0.0);
        assert!(build_track_draft(&data, &projection, &stats).is_none());
    }

    #[test]
    fn waypoint_drafts_use_name_and_set_annotation_title() {
        let data = parse(SAMPLE_GPX);
        let projection = Projection::new(45.0, -122.0, 100.0);
        let drafts = build_waypoint_drafts(&data, &projection);

        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].name, "Camp");
        assert_eq!(prop(&drafts[0], "gpx_type"), &serde_json::json!("waypoint"));
        assert_eq!(prop(&drafts[0], "gis.annotation.title"), &serde_json::json!("Camp"));
    }

    #[test]
    fn waypoint_draft_falls_back_to_default_name_when_unnamed() {
        let data = parse(SAMPLE_GPX);
        let projection = Projection::new(45.0, -122.0, 100.0);
        let drafts = build_waypoint_drafts(&data, &projection);

        // Second <wpt> in the fixture has no <name>.
        assert_eq!(drafts[1].name, "Waypoint 2");
        assert_eq!(prop(&drafts[1], "gis.annotation.title"), &serde_json::json!("Waypoint 2"));
    }

    #[test]
    fn resolve_projection_prefers_active_terrain_when_petal_matches() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let expected_origin = Projection::new(1.0, 2.0, 3.0);
        let active = ActivePetalTerrain {
            petal_id: Some("petal-1".to_string()),
            config: Some(TerrainConfig { origin: expected_origin.clone(), ..TerrainConfig::default() }),
            revision: 1,
        };

        let resolved = resolve_projection("petal-1", &active, &stats);
        assert_eq!(resolved.origin_lat, expected_origin.origin_lat);
        assert_eq!(resolved.origin_lon, expected_origin.origin_lon);
        assert_eq!(resolved.origin_ele, expected_origin.origin_ele);
    }

    #[test]
    fn resolve_projection_falls_back_to_bbox_center_when_petal_does_not_match() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let active = ActivePetalTerrain {
            petal_id: Some("some-other-petal".to_string()),
            config: Some(TerrainConfig::default()),
            revision: 1,
        };

        let resolved = resolve_projection("petal-1", &active, &stats);
        let bb = &stats.bounding_box;
        assert_eq!(resolved.origin_lat, (bb.min_lat + bb.max_lat) / 2.0);
        assert_eq!(resolved.origin_lon, (bb.min_lon + bb.max_lon) / 2.0);
    }

    #[test]
    fn resolve_projection_falls_back_when_no_terrain_configured() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let active = ActivePetalTerrain::default();

        let resolved = resolve_projection("petal-1", &active, &stats);
        let bb = &stats.bounding_box;
        assert_eq!(resolved.origin_lat, (bb.min_lat + bb.max_lat) / 2.0);
        assert_eq!(resolved.origin_lon, (bb.min_lon + bb.max_lon) / 2.0);
    }

    #[test]
    fn prepare_import_reads_file_and_produces_track_plus_waypoint_children() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path: PathBuf = tmp.path().join("sample.gpx");
        std::fs::write(&path, SAMPLE_GPX).expect("write fixture");
        let active = ActivePetalTerrain::default();

        let prepared = prepare_import("petal-1", &path, &active).expect("prepare_import should succeed");
        let track = prepared.track.expect("track present");
        assert_eq!(track.draft.name, "Test Hike");
        assert_eq!(track.waypoints.len(), 2);
        assert!(prepared.standalone_waypoints.is_empty());
    }

    #[test]
    fn prepare_import_reports_missing_file() {
        let active = ActivePetalTerrain::default();
        let err = prepare_import("petal-1", Path::new("does-not-exist.gpx"), &active).unwrap_err();
        assert!(err.contains("could not read GPX file"));
    }

    #[test]
    fn prepare_import_waypoint_only_file_has_no_track_and_standalone_waypoints() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path: PathBuf = tmp.path().join("waypoints.gpx");
        std::fs::write(&path, WAYPOINT_ONLY_GPX).expect("write fixture");
        let active = ActivePetalTerrain::default();

        let prepared = prepare_import("petal-1", &path, &active).expect("prepare_import should succeed");
        assert!(prepared.track.is_none());
        assert_eq!(prepared.standalone_waypoints.len(), 1);
        assert_eq!(prepared.standalone_waypoints[0].name, "Lone Marker");
    }
}
