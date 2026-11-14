//! Bridges fe-ui's queued GPX import ops into parse → project → persist.
//! See src/AGENTS.md §gpx for the full import→persist→render→serve map and
//! the rationale behind the correlation/projection/hierarchy design below.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use bevy::prelude::*;

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_terrain::gpx::{compute_stats, parse_gpx_bytes, scene_nodes_to_gpx, GpxData, TrackStats};
use fe_terrain::iot::animation::{CornerKind, TimestampedRoutePoint, TrackRoute};
use fe_terrain::iot::{parse_track_color_hex, TrackRouteMap, TrackStyle, TrackStyleMap};
use fe_terrain::petal_binding::ActivePetalTerrain;
use fe_terrain::projection::Projection;
use fe_terrain::terrain_plugin::{GpxTrackLine, RenderedTrackGeometry};
use fe_terrain::ExportNode;
use fe_ui::gpx_ops::{GpxImportStatus, GpxOp, PendingGpxOps};
use fe_ui::node_manager::TrackPickShape;
use fe_ui::path_ops::{PathEditStatus, PathOp, PendingPathOps};
use fe_ui::verse_manager::{
    invalidate_path_stamp_projection, PathAssetApplied, PathAssetCache, StampRenderIndex,
    VerseManager,
};

/// Reserved flat property key marking a node as GPX-derived (`"track"` or `"waypoint"`).
const GPX_TYPE_KEY: &str = "gpx_type";
/// Provenance property: `"designed"` (authored in-app) or `"recorded"` (GPX import); absent means recorded.
const PATH_KIND_KEY: &str = "path_kind";
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
/// Per-track border, visibility, closed-zone, and fill properties. Read
/// into `TrackStyleMap` by admitted current projections; written by the Paths
/// tab via `SetNodeProperty`. Absent/invalid → `TrackStyle::default()` (FR-4).
const TRACK_COLOR_KEY: &str = "gis.track.color";
const TRACK_WIDTH_KEY: &str = "gis.track.width";
const TRACK_VISIBLE_KEY: &str = "gis.track.visible";
const TRACK_CLOSED_KEY: &str = "gis.track.closed";
const TRACK_FILL_COLOR_KEY: &str = "gis.track.fill.color";

/// Parse a track's `gis.track.*` properties into a [`TrackStyle`]. Any
/// missing/invalid field falls back to that field's default (FR-4) — this
/// never panics. Pure so it's unit-testable without a Bevy App. Takes the
/// same `serde_json::Value` property bag `NodePropertiesLoaded` carries.
fn style_from_properties(properties: &serde_json::Value) -> TrackStyle {
    let mut style = TrackStyle::default();
    if let Some(color) = properties
        .get(TRACK_COLOR_KEY)
        .and_then(|v| v.as_str())
        .and_then(parse_track_color_hex)
    {
        style.color = color;
    }
    if let Some(width) = properties.get(TRACK_WIDTH_KEY).and_then(|v| v.as_f64()) {
        if width.is_finite() && width > 0.0 {
            style.width = width.clamp(0.1, 20.0) as f32;
        }
    }
    if let Some(visible) = properties.get(TRACK_VISIBLE_KEY).and_then(|v| v.as_bool()) {
        style.visible = visible;
    }
    if let Some(closed) = properties.get(TRACK_CLOSED_KEY).and_then(|v| v.as_bool()) {
        style.closed = closed;
    }
    if let Some(fill_color) = properties
        .get(TRACK_FILL_COLOR_KEY)
        .and_then(|v| v.as_str())
        .and_then(parse_track_color_hex)
    {
        style.fill_color = fill_color;
    }
    style
}

/// Private process-unique identity for a host-authored track request.
fn next_authored_track_correlation_id() -> String {
    host_request_id()
}

fn enqueue_track_delete(db_tx: &DbCommandSender, track_node_id: &str) -> Result<(), &'static str> {
    use fe_ui::node_manager::{submit_scene_write, GuiSend};
    let command = DbCommand::DeleteNode {
        node_id: track_node_id.to_string(),
    };
    match submit_scene_write(db_tx, command) {
        GuiSend::Sent | GuiSend::Waiting => Ok(()),
        GuiSend::Refused(_) => Err("Track delete was refused by an extension"),
        GuiSend::Busy | GuiSend::Closed => Err("Database channel closed; track was not deleted"),
    }
}

fn projection_matches_track(candidate_track_id: &str, deleted_track_id: &str) -> bool {
    candidate_track_id == deleted_track_id
}

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
fn resolve_projection(
    petal_id: &str,
    active: &ActivePetalTerrain,
    stats: &TrackStats,
) -> Projection {
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

/// Initial properties written to an authored (designed) track node.
fn authored_track_properties(name: &str) -> [(&'static str, serde_json::Value); 4] {
    [
        (GPX_TYPE_KEY, serde_json::json!("track")),
        (PATH_KIND_KEY, serde_json::json!("designed")),
        (GPX_POINTS_KEY, serde_json::json!([])),
        (TRACK_NAME_KEY, serde_json::json!(name)),
    ]
}

/// Submit the authored-track property writes for a created node; returns the partial-init message on a refused send.
fn submit_authored_track_properties(
    pending: &mut PendingPathEdits,
    db_tx: &DbCommandSender,
    node_id: &str,
    name: &str,
) -> Option<String> {
    let mut confirmations = PendingPropertyConfirms::new();
    let mut failure = None;
    for (key, value) in authored_track_properties(name) {
        match pending.property_outcomes.submit(db_tx, node_id, key, value) {
            Ok(request) => {
                confirmations.insert(request);
            }
            Err(error) => {
                pending.failed_track_properties.insert(node_id.to_string());
                failure = Some(format!("Track node exists; initialization is partial: {error}"));
                break;
            }
        }
    }
    if !confirmations.is_empty() {
        pending.track_property_confirms.insert(node_id.to_string(), confirmations);
    } else {
        pending.failed_track_properties.remove(node_id);
    }
    failure
}

/// Build the single merged track node draft, positioned at the first
/// trackpoint in document order. Returns `None` when the file has no
/// trackpoints at all (waypoint-only GPX files are handled as standalone).
fn build_track_draft(
    data: &GpxData,
    projection: &Projection,
    stats: &TrackStats,
) -> Option<GpxNodeDraft> {
    let first_point = data
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| s.points.iter())
        .next()?;

    let local = projection
        .wgs84_to_local(
            first_point.lat,
            first_point.lon,
            first_point.ele.unwrap_or(0.0),
        )
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
            (PATH_KIND_KEY.to_string(), serde_json::json!("recorded")),
            (TRACK_NAME_KEY.to_string(), serde_json::json!(name)),
            (
                "total_distance_m".to_string(),
                serde_json::json!(stats.total_distance_m),
            ),
            (
                "elevation_gain_m".to_string(),
                serde_json::json!(stats.elevation_gain_m),
            ),
            (
                "elevation_loss_m".to_string(),
                serde_json::json!(stats.elevation_loss_m),
            ),
            ("duration_s".to_string(), serde_json::json!(stats.duration)),
            (
                "avg_speed_kmh".to_string(),
                serde_json::json!(stats.avg_speed_kmh),
            ),
            (
                "max_speed_kmh".to_string(),
                serde_json::json!(stats.max_speed_kmh),
            ),
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
            let local = projection
                .wgs84_to_local(p.lat, p.lon, p.ele.unwrap_or(0.0))
                .ok()?;
            let time_seconds = match (first_time, p.time) {
                (Some(t0), Some(t)) => (t - t0).num_milliseconds() as f64 / 1000.0,
                _ => i as f64,
            };
            Some(TimestampedRoutePoint {
                position: [local[0], local[1], local[2]],
                time_seconds,
                ..Default::default()
            })
        })
        .collect()
}

/// Encode route points as the `gpx_points` JSON array — compact 4-slot
/// `[x,y,z,t]` for plain corners, else 12-slot `[x,y,z,t, in3, out3, corner, smoothness]`
/// (pen_curve_tool_20260722); petal-local meters — see `GPX_POINTS_KEY`.
fn route_points_to_json(points: &[TimestampedRoutePoint]) -> serde_json::Value {
    serde_json::Value::Array(
        points
            .iter()
            .map(|p| {
                if p.is_plain_corner() {
                    serde_json::json!([p.position[0], p.position[1], p.position[2], p.time_seconds])
                } else {
                    let hin = p.handle_in.unwrap_or([0.0, 0.0, 0.0]);
                    let hout = p.handle_out.unwrap_or([0.0, 0.0, 0.0]);
                    serde_json::json!([
                        p.position[0],
                        p.position[1],
                        p.position[2],
                        p.time_seconds,
                        hin[0],
                        hin[1],
                        hin[2],
                        hout[0],
                        hout[1],
                        hout[2],
                        p.corner.to_code(),
                        p.smoothness,
                    ])
                }
            })
            .collect(),
    )
}

/// Decode persisted rows through the shared lossless renderer-source contract.
fn json_to_route_points(value: &serde_json::Value) -> Vec<TimestampedRoutePoint> {
    fe_sdk::path_geometry::decode_route_source(value)
        .into_iter()
        .map(|row| TimestampedRoutePoint {
            position: row.position,
            time_seconds: row.time_seconds,
            handle_in: row.handle_in,
            handle_out: row.handle_out,
            corner: CornerKind::from_code(f64::from(row.corner_code)),
            smoothness: row.smoothness,
        })
        .collect()
}

/// Widen a fe-ui smooth-anchor op payload (f32 + corner code) into the
/// bridge's route point (pen_curve_tool_20260722 Phase 3).
fn smooth_anchor_point(
    position: [f32; 3],
    handle_in: Option<[f32; 3]>,
    handle_out: Option<[f32; 3]>,
    corner_code: f64,
    smoothness: f32,
) -> TimestampedRoutePoint {
    let widen = |h: [f32; 3]| [h[0] as f64, h[1] as f64, h[2] as f64];
    TimestampedRoutePoint {
        position: widen(position),
        time_seconds: 0.0,
        handle_in: handle_in.map(widen),
        handle_out: handle_out.map(widen),
        corner: CornerKind::from_code(corner_code),
        smoothness,
    }
}

/// Apply a `SetAnchorHandles` payload to an existing point in place —
/// position/timestamp/corner untouched (pen_curve_tool_20260722 Phase 3).
fn apply_anchor_handles(
    point: &mut TimestampedRoutePoint,
    handle_in: Option<[f32; 3]>,
    handle_out: Option<[f32; 3]>,
    smoothness: f32,
) {
    let widen = |h: [f32; 3]| [h[0] as f64, h[1] as f64, h[2] as f64];
    point.handle_in = handle_in.map(widen);
    point.handle_out = handle_out.map(widen);
    point.smoothness = smoothness;
}

/// Populate `TrackRouteMap` for `track_node_id` and (re)spawn its
/// `GpxTrackLine` entity. Used only by the import path, which never has a
/// pre-existing render entity (`existing_entity` is always `None` there); live
/// edits and petal-load both go through `reconcile_track_render` instead so the
/// single-point-node reconcile stays in one place. Callers guarantee
/// `points.len() >= 2` — the `< 2` cases (`None`/`Node`) are `reconcile_track_render`'s.
///
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
    let total = points.last().map(|p| p.time_seconds).unwrap_or(0.0);
    route_map.routes.insert(
        track_node_id.to_string(),
        TrackRoute {
            points,
            total_duration_secs: total,
        },
    );
    if let Some(entity) = existing_entity {
        commands.entity(entity).despawn();
    }
    commands.spawn(GpxTrackLine {
        track_node_id: track_node_id.to_string(),
    });
}

/// FR-3 single source of truth: reconcile a track's render entity against its
/// current point count. Shared by BOTH the live-edit path
/// (`persist_and_render_points`, driven by Pen append / remove / move / height
/// edits) and the petal-load path (`advance_path_materialization`) so the
/// `None`/`Node`/`Line` decision never diverges between them — the divergence
/// that let a live edit leak a stale single-point node (1→2) or drop a track to
/// nothing (2→1). Fully idempotent: an already-correct state is left alone.
///
/// `force_line_redraw` = `true` for live edits, which must despawn+respawn an
/// existing `GpxTrackLine` to force `render_gpx_tracks` to rebuild the mesh (it
/// only rebuilds entities `Without<Mesh3d>`). Petal-load passes `false` so a
/// matching line spawned earlier in the same batch is left untouched.
#[allow(clippy::too_many_arguments)]
fn reconcile_track_render(
    route_map: &mut TrackRouteMap,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    track_lines: &Query<(Entity, &GpxTrackLine)>,
    single_nodes: &Query<(Entity, &SinglePointTrackNode)>,
    active_petal_id: &str,
    track_node_id: &str,
    points: Vec<TimestampedRoutePoint>,
    force_line_redraw: bool,
) {
    let line = track_lines
        .iter()
        .find(|(_, t)| t.track_node_id.as_str() == track_node_id)
        .map(|(e, _)| e);
    let node = single_nodes
        .iter()
        .find(|(_, t)| t.track_node_id.as_str() == track_node_id)
        .map(|(e, _)| e);
    match materialization_kind(points.len()) {
        MaterializationKind::Line => {
            // ≥2 points: drop any single-point node from a prior 1-point state,
            // then (re)spawn the polyline. `spawn_track_route` handles the
            // existing-line despawn+respawn so a live edit forces a redraw.
            if let Some(entity) = node {
                commands.entity(entity).despawn();
            }
            if force_line_redraw || line.is_none() {
                spawn_track_route(route_map, commands, track_node_id, points, line);
            }
        }
        MaterializationKind::Node => {
            // Exactly 1 point: drop any line + its route, spawn the node if absent.
            if let Some(entity) = line {
                route_map.routes.remove(track_node_id);
                commands.entity(entity).despawn();
            }
            if force_line_redraw {
                if let Some(entity) = node {
                    commands.entity(entity).try_despawn();
                }
            }
            if node.is_none() || force_line_redraw {
                let p = points[0].position;
                let position = [p[0] as f32, p[1] as f32, p[2] as f32];
                spawn_single_point_node(
                    commands,
                    meshes,
                    materials,
                    track_node_id,
                    active_petal_id,
                    position,
                );
            }
        }
        MaterializationKind::None => {
            // Empty track: tear down whatever render entity remains.
            if let Some(entity) = line {
                route_map.routes.remove(track_node_id);
                commands.entity(entity).despawn();
            }
            if let Some(entity) = node {
                commands.entity(entity).despawn();
            }
        }
    }
}

/// FR-3: marker for a single-point track rendered as a plain, selectable node
/// (spawned when `gpx_points.len() == 1`). Mirrors the `GpxTrackLine` marker so
/// `advance_path_materialization` can reconcile 1↔2-point transitions and
/// tear the node down on delete. See `src/AGENTS.md` §path-editor.
#[derive(Component, Debug)]
pub struct SinglePointTrackNode {
    pub track_node_id: String,
}

/// Marker icon-quad edge length (world units) for a single-point track node —
/// matches the path-point markers so it reads as a placed point. Billboarded
/// flat quad (data_icons_20260713).
const SINGLE_POINT_NODE_SIZE: f32 = 0.8;

/// FR-3: what a track's point count should materialize as. Pure so the
/// point-count → spawn-decision branching is unit-testable without a Bevy App.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationKind {
    /// Zero points: render nothing.
    None,
    /// Exactly one point: a plain, visible, selectable node.
    Node,
    /// Two or more points: the polyline track route.
    Line,
}

/// Map a track's `gpx_points` length to its render kind (FR-3 threshold).
pub fn materialization_kind(len: usize) -> MaterializationKind {
    match len {
        0 => MaterializationKind::None,
        1 => MaterializationKind::Node,
        _ => MaterializationKind::Line,
    }
}

/// FR-3: spawn a visible, selectable node for a single-point track at `position`.
/// Tags it with `SinglePointTrackNode` (reconcile) and `fe_ui::plugin::SpawnedNodeMarker`
/// (so the glb-mesh-picking AABB test selects it; `Mesh3d` supplies the `Aabb`)
/// and `fe_ui::plugin::Billboard` (data_icons_20260713 — fe-ui's
/// `billboard_face_camera` turns the flat icon quad to face the viewport).
/// The mesh stays a `Mesh3d`, so the AABB mesh-pick keeps selecting it.
/// Crate-local because `fe-ui`'s node-spawn helpers are `pub(super)`.
fn spawn_single_point_node(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    track_node_id: &str,
    petal_id: &str,
    position: [f32; 3],
) {
    // Flat icon quad (Rectangle) instead of a solid sphere — still a `Mesh3d`
    // so it yields an `Aabb` for the mesh-pick; double-sided + unlit so it reads
    // at any angle before the first billboard-face frame.
    let mesh = meshes.add(Rectangle::new(
        SINGLE_POINT_NODE_SIZE,
        SINGLE_POINT_NODE_SIZE,
    ));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.7, 1.0),
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_xyz(position[0], position[1], position[2]),
            Name::new(format!("SinglePointTrack {track_node_id}")),
            fe_ui::plugin::SpawnedNodeMarker {
                node_id: track_node_id.to_string(),
                petal_id: petal_id.to_string(),
            },
            fe_ui::plugin::Billboard,
            SinglePointTrackNode {
                track_node_id: track_node_id.to_string(),
            },
        ))
        .id();
    bevy::log::debug!(
        "Spawned single-point track node '{}' entity={:?} (petal={})",
        track_node_id,
        entity,
        petal_id
    );
}

/// Tags only the exact mesh-bound render source; see src/AGENTS.md section closed-path source.
type UntaggedTrackLineQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GpxTrackLine,
        &'static Mesh3d,
        &'static RenderedTrackGeometry,
    ),
    Without<fe_ui::plugin::SpawnedNodeMarker>,
>;

pub fn tag_track_lines_selectable(
    lines: UntaggedTrackLineQuery,
    active_terrain: Res<ActivePetalTerrain>,
    mut commands: Commands,
) {
    let petal_id = active_terrain.petal_id.clone().unwrap_or_default();
    for (entity, line, mesh, geometry) in &lines {
        if !geometry.matches_mesh(mesh) {
            continue;
        }
        let Some(shape) = track_pick_shape(geometry) else {
            continue;
        };
        commands.entity(entity).try_insert((
            fe_ui::plugin::SpawnedNodeMarker {
                node_id: line.track_node_id.clone(),
                petal_id: petal_id.clone(),
            },
            shape,
        ));
    }
}

/// Build precise picking and source proof from the geometry that built the actual mesh.
fn track_pick_shape(geometry: &RenderedTrackGeometry) -> Option<TrackPickShape> {
    let style = geometry.style();
    let (mut pick_points, fill_triangles) = if style.closed {
        let prepared = geometry.zone()?;
        let triangles = prepared
            .triangle_indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|triangle| {
                [0, 1, 2].map(|offset| {
                    let point = prepared.points[triangle[offset] as usize];
                    Vec3::new(point[0], point[1] + 0.48, point[2])
                })
            })
            .collect();
        (
            prepared
                .points
                .iter()
                .copied()
                .map(Vec3::from_array)
                .collect::<Vec<_>>(),
            triangles,
        )
    } else {
        (
            geometry
                .positions()
                .iter()
                .copied()
                .map(Vec3::from_array)
                .collect(),
            Vec::new(),
        )
    };
    if style.closed && pick_points.len() >= 3 {
        pick_points.push(pick_points[0]);
    }
    let anchors = geometry
        .source_points()
        .iter()
        .map(|point| Vec3::from_array(point.position.map(|value| value as f32)))
        .filter(|point| point.is_finite())
        .collect();
    Some(
        TrackPickShape {
            source_witness: None,
            points: pick_points,
            anchors,
            half_width: style.width.max(0.01) / 2.0,
            closed: style.closed,
            fill_triangles,
            centroid: Vec3::from_array(geometry.centroid()),
        }
        .with_render_source_rows(
            geometry
                .source_points()
                .iter()
                .map(|point| fe_sdk::path_geometry::RouteSourceRow {
                    position: point.position,
                    time_seconds: point.time_seconds,
                    handle_in: point.handle_in,
                    handle_out: point.handle_out,
                    corner_code: point.corner.to_code() as u8,
                    smoothness: point.smoothness,
                })
                .collect(),
        ),
    )
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
            let local = projection
                .wgs84_to_local(wp.lat, wp.lon, wp.ele.unwrap_or(0.0))
                .ok()?;
            let name = wp
                .name
                .clone()
                .unwrap_or_else(|| format!("Waypoint {}", i + 1));
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
fn prepare_import(
    petal_id: &str,
    path: &Path,
    active: &ActivePetalTerrain,
) -> Result<PreparedImport, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("could not read GPX file {}: {e}", path.display()))?;
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
        None => Ok(PreparedImport {
            track: None,
            standalone_waypoints: waypoint_drafts,
        }),
    }
}

// ---------------------------------------------------------------------------
// Bevy resources + systems (DB-command correlation — see AGENTS.md §gpx)
// ---------------------------------------------------------------------------

const MAX_HOST_OUTCOMES: usize = 256;

fn host_request_id() -> String {
    format!(
        "{}{}",
        fe_ui::path_ops::HOST_PATH_REQUEST_PREFIX,
        ulid::Ulid::new()
    )
}

struct Accepted<T> {
    payload: T,
    retired: bool,
    started: std::time::Instant,
    unknown_reported: bool,
}

impl<T> Accepted<T> {
    fn new(payload: T) -> Self {
        Self {
            payload,
            retired: false,
            started: std::time::Instant::now(),
            unknown_reported: false,
        }
    }

    fn report_unknown(&mut self) -> bool {
        if self.unknown_reported || self.started.elapsed() < std::time::Duration::from_secs(5) {
            return false;
        }
        self.unknown_reported = true;
        true
    }
}

struct PropertyWrite {
    node_id: String,
    key: String,
    value: serde_json::Value,
}

#[derive(Default)]
struct PropertyOutcomes(HashMap<String, Accepted<PropertyWrite>>);

struct PropertySettlement {
    request_id: String,
    node_id: String,
    retired: bool,
    error: Option<String>,
}

impl PropertyOutcomes {
    fn submit(
        &mut self,
        sender: &DbCommandSender,
        node_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<String, String> {
        if self.0.len() >= MAX_HOST_OUTCOMES {
            return Err(
                "Accepted property outcomes are full; no further write was submitted".into(),
            );
        }
        let request_id = host_request_id();
        through_scene_door(
            sender,
            DbCommand::SetNodeProperty {
                node_id: node_id.into(),
                key: key.into(),
                value: value.clone(),
                correlation_id: Some(request_id.clone()),
            },
            "Database queue unavailable; property write was not submitted",
        )?;
        self.0.insert(
            request_id.clone(),
            Accepted::new(PropertyWrite {
                node_id: node_id.into(),
                key: key.into(),
                value,
            }),
        );
        Ok(request_id)
    }

    fn retire(&mut self) {
        for request in self.0.values_mut() {
            request.retired = true;
        }
    }

    fn route(&mut self, result: &DbResult) -> Option<PropertySettlement> {
        let (request_id, error) = match result {
            DbResult::NodePropertySet {
                node_id,
                key,
                correlation_id: Some(id),
            } if self.0.get(id).is_some_and(|owner| {
                owner.payload.node_id == *node_id && owner.payload.key == *key
            }) =>
            {
                (id, None)
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if self.0.contains_key(correlation_id) => (correlation_id, Some(error.clone())),
            _ => return None,
        };
        let owner = self.0.remove(request_id)?;
        drop(owner.payload.value);
        Some(PropertySettlement {
            request_id: request_id.clone(),
            node_id: owner.payload.node_id,
            retired: owner.retired,
            error,
        })
    }
}

/// A track node's `CreateNode` was sent; once its `DbResult::NodeCreated`
/// arrives, set these stat properties and create these waypoint children.
struct PendingTrackImport {
    petal_id: String,
    name: String,
    position: [f32; 3],
    stats_props: Vec<(String, serde_json::Value)>,
    waypoints: Vec<GpxNodeDraft>,
    route_points: Vec<TimestampedRoutePoint>,
}

/// A waypoint node's `CreateNode` was sent; once its `DbResult::NodeCreated`
/// arrives, set its type/title/(optional) track-link properties.
struct PendingWaypointImport {
    petal_id: String,
    position: [f32; 3],
    title: String,
    track_node_id: Option<String>,
}

/// Exact accepted import outcomes; see src/AGENTS.md ?host-path-outcomes.
#[derive(Resource, Default)]
pub struct PendingGpxImports {
    tracks: HashMap<String, Accepted<PendingTrackImport>>,
    waypoints: HashMap<String, Accepted<PendingWaypointImport>>,
    properties: PropertyOutcomes,
}

impl PendingGpxImports {
    pub fn retire_for_terminal_reset(&mut self) {
        self.retire();
    }
    pub fn pending_outcomes(&self) -> usize {
        self.tracks.len() + self.waypoints.len() + self.properties.0.len()
    }

    fn retire(&mut self) {
        for request in self.tracks.values_mut() {
            request.retired = true;
        }
        for request in self.waypoints.values_mut() {
            request.retired = true;
        }
        self.properties.retire();
    }

    fn submit_track(
        &mut self,
        sender: &DbCommandSender,
        petal_id: &str,
        track: PreparedTrack,
    ) -> Result<(), String> {
        if self.tracks.len() + self.waypoints.len() >= MAX_HOST_OUTCOMES {
            return Err("Accepted import outcomes are full; no track was submitted".into());
        }
        let retained_rows: usize = self
            .tracks
            .values()
            .map(|request| request.payload.route_points.len() + request.payload.waypoints.len())
            .sum();
        if retained_rows
            .saturating_add(track.route_points.len())
            .saturating_add(track.waypoints.len())
            > fe_ui::verse_manager::SCENE_NODE_HARD_LIMIT
        {
            return Err("Import outcome source capacity is full; no track was submitted".into());
        }
        let request_id = host_request_id();
        through_scene_door(
            sender,
            DbCommand::CreateNode {
                petal_id: petal_id.into(),
                name: track.draft.name.clone(),
                position: track.draft.position,
                correlation_id: Some(request_id.clone()),
            },
            "Database queue unavailable; track was not submitted",
        )?;
        self.tracks.insert(
            request_id,
            Accepted::new(PendingTrackImport {
                petal_id: petal_id.into(),
                name: track.draft.name,
                position: track.draft.position,
                stats_props: track.draft.properties,
                waypoints: track.waypoints,
                route_points: track.route_points,
            }),
        );
        Ok(())
    }

    fn submit_waypoint(
        &mut self,
        sender: &DbCommandSender,
        petal_id: &str,
        waypoint: GpxNodeDraft,
        track_node_id: Option<String>,
    ) -> Result<(), String> {
        if self.tracks.len() + self.waypoints.len() >= MAX_HOST_OUTCOMES {
            return Err("Accepted import outcomes are full; no waypoint was submitted".into());
        }
        let request_id = host_request_id();
        through_scene_door(
            sender,
            DbCommand::CreateNode {
                petal_id: petal_id.into(),
                name: waypoint.name.clone(),
                position: waypoint.position,
                correlation_id: Some(request_id.clone()),
            },
            "Database queue unavailable; waypoint was not submitted",
        )?;
        self.waypoints.insert(
            request_id,
            Accepted::new(PendingWaypointImport {
                petal_id: petal_id.into(),
                position: waypoint.position,
                title: waypoint.name,
                track_node_id,
            }),
        );
        Ok(())
    }
}

/// Submit one scene write through the GUI door (caller `Ui`); a refusal or a full queue is the `Err` text.
fn through_scene_door(
    sender: &DbCommandSender,
    command: DbCommand,
    unavailable: &str,
) -> Result<(), String> {
    use fe_ui::node_manager::{try_submit_scene_write, GuiSend};
    match try_submit_scene_write(sender, command) {
        GuiSend::Sent | GuiSend::Waiting => Ok(()),
        GuiSend::Refused(message) => Err(message),
        GuiSend::Busy | GuiSend::Closed => Err(unavailable.to_string()),
    }
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
    for op in ops.0.drain(..) {
        let GpxOp::ImportFile { petal_id, path } = op;
        let mut accepted_tracks = 0;
        let mut accepted_waypoints = 0;
        let submitted = prepare_import(&petal_id, &path, &active_terrain).and_then(|prepared| {
            if let Some(track) = prepared.track {
                pending.submit_track(&db_tx, &petal_id, track)?;
                accepted_tracks += 1;
            }
            for waypoint in prepared.standalone_waypoints {
                pending.submit_waypoint(&db_tx, &petal_id, waypoint, None)?;
                accepted_waypoints += 1;
            }
            Ok(())
        });
        *status = GpxImportStatus {
            petal_id: Some(petal_id), track_count: accepted_tracks,
            waypoint_count: accepted_waypoints,
            error: submitted.err().map(|error| format!("Import submission incomplete: {error}. Accepted requests remain owned; no write was retried.")),
        };
    }
}

/// Continues only current, exact owned import births; retired outcomes drain separately.
pub fn advance_gpx_imports(
    mut db_results: MessageReader<DbResult>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingGpxImports>,
    mut route_map: ResMut<TrackRouteMap>,
    mut commands: Commands,
    mut status: ResMut<GpxImportStatus>,
) {
    for result in db_results.read() {
        let DbResult::NodeCreated {
            id,
            petal_id,
            name,
            position,
            correlation_id: Some(request_id),
            ..
        } = result
        else {
            continue;
        };
        if pending.tracks.get(request_id).is_some_and(|owner| {
            !owner.retired
                && owner.payload.petal_id == *petal_id
                && owner.payload.name == *name
                && owner.payload.position == *position
        }) {
            let track = pending.tracks.remove(request_id).unwrap().payload;
            let mut failure = None;
            for (key, value) in track.stats_props {
                if let Err(error) = pending.properties.submit(&db_tx, id, &key, value) {
                    failure = Some(error);
                    break;
                }
            }
            if failure.is_none() {
                for waypoint in track.waypoints {
                    if let Err(error) =
                        pending.submit_waypoint(&db_tx, &track.petal_id, waypoint, Some(id.clone()))
                    {
                        failure = Some(error);
                        break;
                    }
                }
            }
            if failure.is_none() && track.route_points.len() >= 2 {
                match pending.properties.submit(
                    &db_tx,
                    id,
                    GPX_POINTS_KEY,
                    route_points_to_json(&track.route_points),
                ) {
                    Ok(_) => spawn_track_route(
                        &mut route_map,
                        &mut commands,
                        id,
                        track.route_points,
                        None,
                    ),
                    Err(error) => failure = Some(error),
                }
            }
            if let Some(error) = failure {
                status.error = Some(format!("Import node exists, but initialization is partial: {error}. Accepted requests remain owned."));
            }
            continue;
        }
        if pending.waypoints.get(request_id).is_some_and(|owner| {
            !owner.retired
                && owner.payload.petal_id == *petal_id
                && owner.payload.title == *name
                && owner.payload.position == *position
        }) {
            let waypoint = pending.waypoints.remove(request_id).unwrap().payload;
            let mut properties = vec![
                (GPX_TYPE_KEY, serde_json::json!("waypoint")),
                (ANNOTATION_TITLE_KEY, serde_json::json!(waypoint.title)),
            ];
            if let Some(track) = waypoint.track_node_id {
                properties.push((GPX_TRACK_ID_KEY, serde_json::json!(track)));
            }
            for (key, value) in properties {
                if let Err(error) = pending.properties.submit(&db_tx, id, key, value) {
                    status.error = Some(format!("Waypoint exists, but initialization is partial: {error}. Accepted requests remain owned."));
                    break;
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
    AppendPoint {
        position: [f32; 3],
        time_seconds: Option<f64>,
    },
    AppendSmoothPoint {
        position: [f32; 3],
        handle_in: Option<[f32; 3]>,
        handle_out: Option<[f32; 3]>,
        corner_code: f64,
        smoothness: f32,
    },
    RemovePoint {
        index: usize,
    },
    MovePoint {
        index: usize,
        position: [f32; 3],
    },
    SetAnchorHandles {
        index: usize,
        handle_in: Option<[f32; 3]>,
        handle_out: Option<[f32; 3]>,
        smoothness: f32,
    },
    SetAnchorCorner {
        index: usize,
        corner_code: f64,
    },
    AnnotatePoint {
        index: usize,
        title: String,
        body: String,
        color: String,
    },
    ExportGpx {
        save_path: fe_ui::file_dialogs::DialogPath,
    },
}

/// Apply a queued append read (plain or smooth) onto the working point list —
/// no-op for non-append variants (callers match those separately).
fn push_append_read(points: &mut Vec<TimestampedRoutePoint>, read: PendingPathRead) {
    match read {
        PendingPathRead::AppendPoint {
            position,
            time_seconds,
        } => points.push(TimestampedRoutePoint {
            position: [position[0] as f64, position[1] as f64, position[2] as f64],
            time_seconds: time_seconds.unwrap_or(0.0),
            ..Default::default()
        }),
        PendingPathRead::AppendSmoothPoint {
            position,
            handle_in,
            handle_out,
            corner_code,
            smoothness,
        } => points.push(smooth_anchor_point(
            position,
            handle_in,
            handle_out,
            corner_code,
            smoothness,
        )),
        _ => {}
    }
}

/// Drain queued append reads onto `points` in queue order, hoisting them past
/// in-place entries (Move/SetAnchor*/Annotate/Export — appends only touch the
/// list tail, so existing indices are undisturbed) but stopping at the first
/// `RemovePoint`: the one index-shifting variant, whose own un-deduped read
/// reply resumes the drain. Returns how many appends were applied. Closes the
/// reply-count deficit where seed-deduped appends queued behind a Set/Move had
/// no reply left to pop them (see src/AGENTS.md §gpx-points-wire-format).
fn drain_queued_appends(
    queue: &mut VecDeque<PendingPathRead>,
    points: &mut Vec<TimestampedRoutePoint>,
) -> usize {
    let mut applied = 0;
    let mut kept: VecDeque<PendingPathRead> = VecDeque::with_capacity(queue.len());
    let mut barrier = false;
    for read in queue.drain(..) {
        let is_append = matches!(
            read,
            PendingPathRead::AppendPoint { .. } | PendingPathRead::AppendSmoothPoint { .. }
        );
        if is_append && !barrier {
            push_append_read(points, read);
            applied += 1;
        } else {
            barrier |= matches!(read, PendingPathRead::RemovePoint { .. });
            kept.push_back(read);
        }
    }
    *queue = kept;
    applied
}

/// Accepted CreateTrack payload before initialization writes.
struct PendingTrackCreate {
    petal_id: String,
    name: String,
    position: [f32; 3],
}

/// Exact accepted property request IDs still needed for track initialization.
type PendingPropertyConfirms = std::collections::HashSet<String>;

/// An `AnnotatePoint` waypoint's `CreateNode` was sent; once `NodeCreated`
/// arrives, set its type/title/body/color annotation properties.
struct PendingAnnotateCreate {
    petal_id: String,
    position: [f32; 3],
    title: String,
    body: String,
    color: String,
}

/// Owns accepted host outcomes and transient path read/render state.
#[derive(Resource, Default)]
pub struct PendingPathEdits {
    access_generation: Option<u128>,
    outcome_navigation: Option<(u128, u64)>,
    access_retired: bool,
    checked_reads:HashMap<String,String>,
    materialization_refresh: VecDeque<String>,
    projection_petal_id: Option<String>,
    projected_versions: HashMap<String, i64>,
    deferred_projections: HashMap<String, DeferredProjection>,
    projection_incarnation: Option<u128>,
    reads: HashMap<String, VecDeque<PendingPathRead>>,
    /// FR-4: authored-`CreateTrack` waiters keyed by a process-unique
    /// correlation id (not `(petal_id, name)`) so a same-named GPX import in
    /// the same window can never steal this path's `NodeCreated` — the tuple
    /// was the ambiguity that FR-4 closes. One waiter per id (unique ⇒ no FIFO).
    creates: HashMap<String, Accepted<PendingTrackCreate>>,
    annotate_creates: HashMap<String, Accepted<PendingAnnotateCreate>>,
    /// FR-4: node_id -> still-unconfirmed property keys for a just-created track.
    track_property_confirms: HashMap<String, PendingPropertyConfirms>,
    property_outcomes: PropertyOutcomes,
    failed_track_properties: std::collections::HashSet<String>,
    /// FR-5: authoritative in-flight point list per track, once seeded from
    /// the first `GetNodeProperties` read. Subsequent `AppendPoint` ops for
    /// the same track mutate this directly and flush one `SetNodeProperty`
    /// per settle instead of re-issuing `GetNodeProperties` per op — kills
    /// the read-modify-write race where two overlapping reads both start
    /// from the same stale base and the second write clobbers the first
    /// append.
    in_flight_points: HashMap<String, Vec<TimestampedRoutePoint>>,
    /// FR-5: tracks that have a seed `GetNodeProperties` outstanding but not
    /// yet resolved into `in_flight_points`. A second `AppendPoint` arriving in
    /// this window enqueues onto `reads` without issuing a redundant seed read,
    /// so overlapping appends serialize against one read instead of racing two.
    seed_pending: std::collections::HashSet<String>,
}

struct DeferredProjection {
    projection: fe_ui::undo::PathProjection,
    deadline: std::time::Instant,
}

/// Drains accepted outcomes without a database sender or any write continuation.
pub fn advance_host_outcomes(
    mut results: MessageReader<DbResult>,
    mut paths: ResMut<PendingPathEdits>,
    mut imports: ResMut<PendingGpxImports>,
    mut status: ResMut<PathEditStatus>,
    mut import_status: ResMut<GpxImportStatus>,
) {
    for result in results.read() {
        if let Some(settled) = paths.property_outcomes.route(result) {
            let group = paths
                .track_property_confirms
                .get_mut(&settled.node_id)
                .map(|requests| {
                    requests.remove(&settled.request_id);
                    requests.is_empty()
                });
            if group.is_some() && settled.error.is_some() {
                paths
                    .failed_track_properties
                    .insert(settled.node_id.clone());
            }
            if group == Some(true) {
                paths.track_property_confirms.remove(&settled.node_id);
                let failed = paths.failed_track_properties.remove(&settled.node_id);
                *status = PathEditStatus { track_node_id: Some(settled.node_id),
                    message: (!failed).then(|| if settled.retired { "Previously submitted track initialization writes confirmed after access changed" } else { "Track created" }.into()),
                    error: failed.then(|| "Track exists but one or more initialization writes failed; no write was retried".into()),
                };
            } else if let Some(error) = settled.error {
                *status = PathEditStatus {
                    track_node_id: Some(settled.node_id),
                    message: None,
                    error: Some(format!(
                        "Previously submitted property request {} failed: {error}",
                        settled.request_id
                    )),
                };
            } else if settled.retired {
                *status = PathEditStatus { track_node_id: Some(settled.node_id), message: Some("Previously submitted property write confirmed after access changed; no continuation was submitted".into()), error: None };
            }
        }
        if let Some(settled) = imports.properties.route(result) {
            if settled.retired || settled.error.is_some() {
                import_status.error = Some(match settled.error {
                    Some(error) => format!("Import property request {} failed: {error}; the import may be partial", settled.request_id),
                    None => format!("Previously submitted import property for {} confirmed after access changed; no further import step was submitted", settled.node_id),
                });
            }
        }
        match result {
            DbResult::NodeCreated {
                id,
                petal_id,
                name,
                position,
                correlation_id: Some(request_id),
                ..
            } => {
                if paths.creates.get(request_id).is_some_and(|owner| {
                    owner.retired
                        && owner.payload.petal_id == *petal_id
                        && owner.payload.name == *name
                        && owner.payload.position == *position
                }) {
                    paths.creates.remove(request_id);
                    *status = PathEditStatus { track_node_id: Some(id.clone()), message: Some("Previously submitted node was created after access changed; track initialization was not submitted".into()), error: None };
                }
                if paths.annotate_creates.get(request_id).is_some_and(|owner| {
                    owner.retired
                        && owner.payload.petal_id == *petal_id
                        && owner.payload.title == *name
                        && owner.payload.position == *position
                }) {
                    paths.annotate_creates.remove(request_id);
                    *status = PathEditStatus { track_node_id: Some(id.clone()), message: Some("Previously submitted annotation node was created after access changed; annotation properties were not submitted".into()), error: None };
                }
                let retired_import = imports.tracks.get(request_id).is_some_and(|owner| {
                    owner.retired
                        && owner.payload.petal_id == *petal_id
                        && owner.payload.name == *name
                        && owner.payload.position == *position
                }) || imports.waypoints.get(request_id).is_some_and(|owner| {
                    owner.retired
                        && owner.payload.petal_id == *petal_id
                        && owner.payload.title == *name
                        && owner.payload.position == *position
                });
                if retired_import {
                    imports.tracks.remove(request_id);
                    imports.waypoints.remove(request_id);
                    import_status.petal_id = Some(petal_id.clone());
                    import_status.error = Some(format!("Previously submitted import node {id} was created after access changed; remaining import steps were not submitted"));
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } => {
                if paths.creates.remove(correlation_id).is_some()
                    || paths.annotate_creates.remove(correlation_id).is_some()
                {
                    *status = PathEditStatus { track_node_id: Some(correlation_id.clone()), message: None, error: Some(format!("Previously submitted node creation failed: {error}; no write was retried")) };
                }
                let failed_import = imports
                    .tracks
                    .remove(correlation_id)
                    .map(|owner| owner.payload.petal_id)
                    .or_else(|| {
                        imports
                            .waypoints
                            .remove(correlation_id)
                            .map(|owner| owner.payload.petal_id)
                    });
                if let Some(petal_id) = failed_import {
                    import_status.petal_id = Some(petal_id);
                    import_status.error = Some(format!("Previously submitted import creation failed: {error}; accepted sibling requests remain owned"));
                }
            }
            _ => {}
        }
    }
    let unknown_path = paths.creates.values_mut().any(Accepted::report_unknown)
        || paths
            .annotate_creates
            .values_mut()
            .any(Accepted::report_unknown)
        || paths
            .property_outcomes
            .0
            .values_mut()
            .any(Accepted::report_unknown);
    if unknown_path {
        status.error = Some("Previously submitted path request outcome is unknown; its owner and payload are retained, and no write was retried".into());
    }
    let unknown_import = imports.tracks.values_mut().any(Accepted::report_unknown)
        || imports.waypoints.values_mut().any(Accepted::report_unknown)
        || imports
            .properties
            .0
            .values_mut()
            .any(Accepted::report_unknown);
    if unknown_import {
        import_status.error = Some("Previously submitted import outcome is unknown; its owner and payload are retained, and no write was retried".into());
    }
}

impl PendingPathEdits {
    pub fn retire_for_terminal_reset(&mut self) {
        self.retire_access();
    }
    fn submit_create(
        &mut self,
        sender: &DbCommandSender,
        request_id: String,
        petal_id: String,
        name: String,
    ) -> Result<(), String> {
        if self.creates.len() + self.annotate_creates.len() >= MAX_HOST_OUTCOMES
            || self.creates.contains_key(&request_id)
        {
            return Err("Accepted creation outcomes are full or the request ID is already owned; no create was submitted".into());
        }
        through_scene_door(
            sender,
            DbCommand::CreateNode {
                petal_id: petal_id.clone(),
                name: name.clone(),
                position: [0.0; 3],
                correlation_id: Some(request_id.clone()),
            },
            "Database queue unavailable; track creation was not submitted",
        )?;
        self.creates.insert(
            request_id,
            Accepted::new(PendingTrackCreate {
                petal_id,
                name,
                position: [0.0; 3],
            }),
        );
        Ok(())
    }

    fn retire_outcomes(&mut self) {
        for owner in self.creates.values_mut() {
            owner.retired = true;
        }
        for owner in self.annotate_creates.values_mut() {
            owner.retired = true;
        }
        self.property_outcomes.retire();
        self.reads.clear();
        self.checked_reads.clear();
        self.seed_pending.clear();
    }

    fn retire_access(&mut self) -> bool {
        if self.access_retired {
            return false;
        }
        let mut creates = std::mem::take(&mut self.creates);
        let mut annotate_creates = std::mem::take(&mut self.annotate_creates);
        let mut property_outcomes = std::mem::take(&mut self.property_outcomes);
        for owner in creates.values_mut() {
            owner.retired = true;
        }
        for owner in annotate_creates.values_mut() {
            owner.retired = true;
        }
        property_outcomes.retire();
        let track_property_confirms = std::mem::take(&mut self.track_property_confirms);
        let failed_track_properties = std::mem::take(&mut self.failed_track_properties);
        *self = Self {
            access_retired: true,
            creates,
            annotate_creates,
            property_outcomes,
            track_property_confirms,
            failed_track_properties,
            ..Default::default()
        };
        true
    }

    pub fn pending_current_projections(&self) -> usize {
        self.deferred_projections.len()
            + self.materialization_refresh.len()
            + self.creates.len()
            + self.annotate_creates.len()
            + self.property_outcomes.0.len()
    }
    fn accept_path_projection(&mut self, projection: &fe_ui::undo::PathProjection) -> bool {
        if self.projection_petal_id.as_deref() != Some(projection.petal_id.as_str()) {
            self.projected_versions.clear();
            self.projection_petal_id = Some(projection.petal_id.clone());
        }
        if projection.history_version <= 0
            || self
                .projected_versions
                .get(&projection.node_id)
                .is_some_and(|version| *version > projection.history_version)
        {
            return false;
        }
        self.projected_versions
            .insert(projection.node_id.clone(), projection.history_version);
        true
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct PathUndoContext<'w> {
    ui_mgr:Option<Res<'w,fe_ui::actions::UiManager>>,
    nav:Option<Res<'w,fe_ui::navigation_manager::NavigationManager>>,
    undo:ResMut<'w,fe_ui::undo::UndoState>,
}

/// Drains `PendingPathOps` each frame. Ops needing the current point list
/// (`AppendPoint`, `RemovePoint`, `AnnotatePoint`, `ExportGpx`) issue a
/// `GetNodeProperties` and defer their mutation to `advance_path_edits`;
/// `CreateTrack` issues `CreateNode` and defers to the same system;
/// `DeleteTrack` sends `DbCommand::DeleteNode` (FR-1/FR-2 — see AGENTS.md
/// §gpx) which cascades the node row + waypoints DB-side.
#[allow(clippy::too_many_arguments)]
pub fn drain_path_ops(
    mut ops: ResMut<PendingPathOps>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingPathEdits>,
    mut route_map: ResMut<TrackRouteMap>,
    mut path_asset_cache: ResMut<PathAssetCache>,
    mut path_asset_applied: ResMut<PathAssetApplied>,
    mut stamp_render_index: ResMut<StampRenderIndex>,
    active_terrain: Res<ActivePetalTerrain>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    single_nodes: Query<(Entity, &SinglePointTrackNode)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut status: ResMut<PathEditStatus>,
    mut commands: Commands,
    context: PathUndoContext,
) {
    let PathUndoContext {
        ui_mgr,
        nav,
        mut undo,
    } = context;
    if let Some(nav) = nav.as_deref() {
        undo.synchronize_navigation(nav);
    }
    pending.checked_reads.retain(|id, _| undo.owns_request(id));
    if ops.0.is_empty() {
        return;
    }
    let active_petal_id = active_terrain.petal_id.clone().unwrap_or_default();

    for op in std::mem::take(&mut ops.0) {
        let waiting = undo.pending_path();
        let target = match &op {
            PathOp::DeleteTrack { track_node_id }
            | PathOp::AppendPoint { track_node_id, .. }
            | PathOp::RemovePoint { track_node_id, .. }
            | PathOp::MovePoint { track_node_id, .. }
            | PathOp::AppendSmoothPoint { track_node_id, .. }
            | PathOp::SetAnchorHandles { track_node_id, .. }
            | PathOp::SetAnchorCorner { track_node_id, .. } => Some(track_node_id.as_str()),
            _ => None,
        };
        if target.is_some() && target == waiting {
            ops.0.push(op);
            continue;
        }
        match op {
            PathOp::CheckedMovePoint { track_node_id, request_id }
            | PathOp::CheckedRemovePoint { track_node_id, request_id }
            | PathOp::CheckedAnchorMetadata { track_node_id, request_id } => {
                let unsettled = pending
                    .reads
                    .get(&track_node_id)
                    .is_some_and(|q| !q.is_empty())
                    || pending.seed_pending.contains(&track_node_id)
                    || pending.track_property_confirms.contains_key(&track_node_id);
                let outcome = if unsettled {
                    Err("Wait for the path's queued edits to finish".to_string())
                } else {
                    undo.path_read_command(&request_id).and_then(|command| {
                        db_tx.0.send(command).map_err(|_| {
                            "Database connection closed; point move was not queued".into()
                        })
                    })
                };
                match outcome {
                    Ok(()) => {
                        pending.checked_reads.insert(request_id, track_node_id);
                    }
                    Err(error) => {
                        undo.cancel_path(&request_id);
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(error),
                        };
                    }
                }
            }

            PathOp::CreateTrack {
                petal_id,
                name,
                correlation_id,
            } => {
                // Pen retains its UI request ID; manual creation gets a private host ID.
                let correlation_id =
                    correlation_id.unwrap_or_else(next_authored_track_correlation_id);
                if let Err(error) = pending.submit_create(&db_tx, correlation_id, petal_id, name) {
                    status.error = Some(error);
                }
            }
            PathOp::DeleteTrack { track_node_id } => {
                // FR-2: use the real DeleteNode primitive (FR-1) instead of
                // clearing two properties — that left the node row,
                // gis.track.name (→ reappeared in the Paths tab), and all
                // waypoint child nodes alive. DeleteNode cascades all of
                // that DB-side; the render-side sinks below (TrackRouteMap,
                // GpxTrackLine) are cleared optimistically here, and the
                // left panel + Paths tab sync via DbResult::NodeDeleted
                // (FR-3, fe-ui/src/verse_manager/db_results.rs).
                if let Err(error) = enqueue_track_delete(&db_tx, &track_node_id) {
                    *status = PathEditStatus {
                        track_node_id: Some(track_node_id),
                        message: None,
                        error: Some(error.to_string()),
                    };
                    continue;
                }
                pending.in_flight_points.remove(&track_node_id);
                pending.seed_pending.remove(&track_node_id);
                route_map.routes.remove(&track_node_id);
                invalidate_path_stamp_projection(
                    &track_node_id,
                    &mut path_asset_cache,
                    &mut path_asset_applied,
                    &mut stamp_render_index,
                );
                // Despawn ALL matching lines — a leaked duplicate ribbon must
                // not survive a single-`.find()` teardown.
                for (entity, _) in track_lines.iter().filter(|(_, track)| {
                    projection_matches_track(&track.track_node_id, &track_node_id)
                }) {
                    commands.entity(entity).despawn();
                }
                for (entity, _) in single_nodes.iter().filter(|(_, node)| {
                    projection_matches_track(&node.track_node_id, &track_node_id)
                }) {
                    commands.entity(entity).despawn();
                }
                *status = PathEditStatus {
                    track_node_id: Some(track_node_id),
                    message: Some("Track deleted".to_string()),
                    error: None,
                };
            }
            PathOp::AppendPoint {
                track_node_id,
                position,
                time_seconds,
            } => {
                // FR-5: if we already have an authoritative in-flight list
                // for this track (seeded by an earlier AppendPoint's read),
                // mutate it directly and flush — no GetNodeProperties
                // round-trip, so no read-modify-write race with a
                // still-in-flight sibling append.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    points.push(TimestampedRoutePoint {
                        position: [position[0] as f64, position[1] as f64, position[2] as f64],
                        time_seconds: time_seconds.unwrap_or(0.0),
                        ..Default::default()
                    });
                    let points = points.clone();
                    persist_and_render_points(
                        &db_tx,
                        &mut route_map,
                        &mut meshes,
                        &mut materials,
                        &track_lines,
                        &single_nodes,
                        &active_petal_id,
                        &mut commands,
                        &track_node_id,
                        points,
                    );
                    *status = PathEditStatus {
                        track_node_id: Some(track_node_id),
                        message: Some("Point added".to_string()),
                        error: None,
                    };
                } else {
                    // FR-5: only issue a seed read if one isn't already
                    // outstanding for this track — a second append while the
                    // seed is in flight just queues, so both apply to the one
                    // authoritative buffer the seed produces (no double-read
                    // race that could lose the first append).
                    if pending.seed_pending.insert(track_node_id.clone()) {
                        db_tx
                            .0
                            .send(DbCommand::GetNodeProperties {
                                node_id: track_node_id.clone(),
                                correlation_id: None,
                            })
                            .ok();
                    }
                    pending.reads.entry(track_node_id).or_default().push_back(
                        PendingPathRead::AppendPoint {
                            position,
                            time_seconds,
                        },
                    );
                }
            }
            PathOp::RemovePoint {
                track_node_id,
                index,
            } => {
                // FR-5/HIGH-2: if an authoritative in-flight buffer exists for
                // this track, mutate IT and flush — re-reading the DB here
                // would start from a base missing any not-yet-committed
                // AppendPoint and clobber it. Only fall back to the DB-read
                // path when no in-flight buffer exists.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    if index < points.len() {
                        points.remove(index);
                        let points = points.clone();
                        persist_and_render_points(
                            &db_tx,
                            &mut route_map,
                            &mut meshes,
                            &mut materials,
                            &track_lines,
                            &single_nodes,
                            &active_petal_id,
                            &mut commands,
                            &track_node_id,
                            points,
                        );
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: Some("Point removed".to_string()),
                            error: None,
                        };
                    } else {
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(format!("point index {index} out of range")),
                        };
                    }
                } else {
                    db_tx
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: track_node_id.clone(),
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .reads
                        .entry(track_node_id)
                        .or_default()
                        .push_back(PendingPathRead::RemovePoint { index });
                }
            }
            PathOp::MovePoint {
                track_node_id,
                index,
                position,
            } => {
                // FR-5: like RemovePoint, mutate the authoritative in-flight
                // buffer in place when it exists (a DB re-read would start from
                // a base missing any not-yet-committed AppendPoint and clobber
                // it). Only fall back to the DB-read path when no buffer exists.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    if let Some(point) = points.get_mut(index) {
                        // Reposition only; preserve the existing timestamp (no index churn).
                        point.position =
                            [position[0] as f64, position[1] as f64, position[2] as f64];
                        let points = points.clone();
                        persist_and_render_points(
                            &db_tx,
                            &mut route_map,
                            &mut meshes,
                            &mut materials,
                            &track_lines,
                            &single_nodes,
                            &active_petal_id,
                            &mut commands,
                            &track_node_id,
                            points,
                        );
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: Some("Point moved".to_string()),
                            error: None,
                        };
                    } else {
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(format!("point index {index} out of range")),
                        };
                    }
                } else {
                    db_tx
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: track_node_id.clone(),
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .reads
                        .entry(track_node_id)
                        .or_default()
                        .push_back(PendingPathRead::MovePoint { index, position });
                }
            }
            PathOp::AppendSmoothPoint {
                track_node_id,
                position,
                handle_in,
                handle_out,
                corner_code,
                smoothness,
            } => {
                // Phase 3: same FR-5 direct-mutate vs seed-read split as AppendPoint.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    points.push(smooth_anchor_point(
                        position,
                        handle_in,
                        handle_out,
                        corner_code,
                        smoothness,
                    ));
                    let points = points.clone();
                    persist_and_render_points(
                        &db_tx,
                        &mut route_map,
                        &mut meshes,
                        &mut materials,
                        &track_lines,
                        &single_nodes,
                        &active_petal_id,
                        &mut commands,
                        &track_node_id,
                        points,
                    );
                    *status = PathEditStatus {
                        track_node_id: Some(track_node_id),
                        message: Some("Point added".to_string()),
                        error: None,
                    };
                } else {
                    if pending.seed_pending.insert(track_node_id.clone()) {
                        db_tx
                            .0
                            .send(DbCommand::GetNodeProperties {
                                node_id: track_node_id.clone(),
                                correlation_id: None,
                            })
                            .ok();
                    }
                    pending.reads.entry(track_node_id).or_default().push_back(
                        PendingPathRead::AppendSmoothPoint {
                            position,
                            handle_in,
                            handle_out,
                            corner_code,
                            smoothness,
                        },
                    );
                }
            }
            PathOp::SetAnchorHandles {
                track_node_id,
                index,
                handle_in,
                handle_out,
                smoothness,
            } => {
                // Phase 3: same FR-5 buffer-first split as MovePoint — mutate the
                // anchor's bezier fields in place, position/timestamp untouched.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    if let Some(point) = points.get_mut(index) {
                        apply_anchor_handles(point, handle_in, handle_out, smoothness);
                        let points = points.clone();
                        persist_and_render_points(
                            &db_tx,
                            &mut route_map,
                            &mut meshes,
                            &mut materials,
                            &track_lines,
                            &single_nodes,
                            &active_petal_id,
                            &mut commands,
                            &track_node_id,
                            points,
                        );
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: Some("Handles set".to_string()),
                            error: None,
                        };
                    } else {
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(format!("point index {index} out of range")),
                        };
                    }
                } else {
                    db_tx
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: track_node_id.clone(),
                            correlation_id: None,
                        })
                        .ok();
                    pending.reads.entry(track_node_id).or_default().push_back(
                        PendingPathRead::SetAnchorHandles {
                            index,
                            handle_in,
                            handle_out,
                            smoothness,
                        },
                    );
                }
            }
            PathOp::SetAnchorCorner {
                track_node_id,
                index,
                corner_code,
            } => {
                // Phase 3: same FR-5 buffer-first split as MovePoint.
                if let Some(points) = pending.in_flight_points.get_mut(&track_node_id) {
                    if let Some(point) = points.get_mut(index) {
                        point.corner = CornerKind::from_code(corner_code);
                        let points = points.clone();
                        persist_and_render_points(
                            &db_tx,
                            &mut route_map,
                            &mut meshes,
                            &mut materials,
                            &track_lines,
                            &single_nodes,
                            &active_petal_id,
                            &mut commands,
                            &track_node_id,
                            points,
                        );
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: Some("Corner set".to_string()),
                            error: None,
                        };
                    } else {
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(format!("point index {index} out of range")),
                        };
                    }
                } else {
                    db_tx
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: track_node_id.clone(),
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .reads
                        .entry(track_node_id)
                        .or_default()
                        .push_back(PendingPathRead::SetAnchorCorner { index, corner_code });
                }
            }
            PathOp::AnnotatePoint {
                track_node_id,
                index,
                title,
                body,
                color,
            } => {
                db_tx
                    .0
                    .send(DbCommand::GetNodeProperties {
                        node_id: track_node_id.clone(),
                        correlation_id: None,
                    })
                    .ok();
                pending.reads.entry(track_node_id).or_default().push_back(
                    PendingPathRead::AnnotatePoint {
                        index,
                        title,
                        body,
                        color,
                    },
                );
            }
            PathOp::ExportGpx { track_node_id } => {
                let dialog_path = match prompt_gpx_save_path(
                    &track_node_id,
                    &ui_mgr
                        .as_ref()
                        .map(|u| u.file_dialogs.clone())
                        .unwrap_or_default(),
                    nav.as_ref().and_then(|n| n.active_verse_id.as_deref()),
                ) {
                    Ok(Some(path)) => path,
                    Ok(None) => {
                        tracing::debug!(track_node_id = %track_node_id, "GPX export cancelled by user");
                        continue;
                    }
                    Err(error) => {
                        *status = PathEditStatus {
                            track_node_id: Some(track_node_id),
                            message: None,
                            error: Some(error.to_string()),
                        };
                        continue;
                    }
                };
                db_tx
                    .0
                    .send(DbCommand::GetNodeProperties {
                        node_id: track_node_id.clone(),
                        correlation_id: None,
                    })
                    .ok();
                pending.reads.entry(track_node_id).or_default().push_back(
                    PendingPathRead::ExportGpx {
                        save_path: dialog_path,
                    },
                );
            }
        }
    }
}

/// Open a native save dialog for a GPX export, suggesting `{track_node_id}.gpx`
/// (the bridge has no node-name lookup here without a DB round trip; the
/// suggested name is a placeholder the user can rename in the dialog).
/// `None` means the user cancelled.
fn prompt_gpx_save_path(
    track_node_id: &str,
    dialogs: &fe_ui::file_dialogs::FileDialogs,
    verse_id: Option<&str>,
) -> Result<Option<fe_ui::file_dialogs::DialogPath>, fe_ui::file_dialogs::DialogFailure> {
    dialogs.choose(fe_ui::file_dialogs::FileDialogRequest {
        kind: fe_runtime::qa_debug::DialogKind::Save,
        verse_id,
        filters: &[("GPX", &["gpx"])],
        suggested_name: Some(&format!("{track_node_id}.gpx")),
    })
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct PathProjectionContext<'w> {
    manager: ResMut<'w, VerseManager>,
    clock: Option<Res<'w, fe_ui::notice::NoticeClock>>,
    styles: Option<ResMut<'w, TrackStyleMap>>,
}

/// Advances pending path-editor DB round trips as their results arrive.
#[allow(clippy::too_many_arguments)]
pub fn advance_path_edits(
    mut db_results: MessageReader<DbResult>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingPathEdits>,
    mut route_map: ResMut<TrackRouteMap>,
    active_terrain: Res<ActivePetalTerrain>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    single_nodes: Query<(Entity, &SinglePointTrackNode)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut status: ResMut<PathEditStatus>,
    mut commands: Commands,
    mut undo: ResMut<fe_ui::undo::UndoState>,
    mut path_state: Option<ResMut<fe_ui::gis::PathEditorState>>,
    nav: Res<fe_ui::navigation_manager::NavigationManager>,
    mut ui: Option<ResMut<fe_ui::actions::UiManager>>,
    mut projection_context: PathProjectionContext,
) {
    let manager = &mut projection_context.manager;
    manager.synchronize_scene_navigation(&nav);
    let mut current_projections = HashMap::new();
    while let Some(projection) = manager.take_current_path_projection(nav.active_petal_id.as_deref()) {
        if active_terrain.petal_id.as_deref() == Some(projection.petal_id.as_str()) {
            current_projections.insert(projection.node_id.clone(), projection);
        }
    }
    if pending.projection_incarnation != Some(manager.scene_incarnation()) {
        pending.deferred_projections.clear();
        pending.projected_versions.clear();
        pending.projection_incarnation = Some(manager.scene_incarnation());
    }
    if pending.projection_petal_id != nav.active_petal_id
        || nav
            .active_petal_id
            .as_deref()
            .and_then(|id| manager.find_petal(id))
            .is_none()
    {
        pending.projected_versions.clear();
        pending.deferred_projections.clear();
        pending.projection_petal_id = nav.active_petal_id.clone();
    }
    loop {
        let projection = ui
            .as_mut()
            .and_then(|ui| ui.take_editor_path_projection(&nav))
            .or_else(|| undo.take_path_projection(&nav));
        let Some(projection) = projection else {
            break;
        };
        if active_terrain.petal_id.as_deref() != Some(projection.petal_id.as_str())
            || projection.scene_incarnation != manager.scene_incarnation()
        {
            continue;
        }
        if pending.deferred_projections.len() >= 256
            && !pending.deferred_projections.contains_key(&projection.node_id)
        {
            status.error = Some("Confirmed path projection queue is full; refresh this Petal".into());
            continue;
        }
        if pending.deferred_projections.get(&projection.node_id)
            .is_some_and(|queued| queued.projection.history_version > projection.history_version)
        {
            continue;
        }
        pending.deferred_projections.insert(projection.node_id.clone(), DeferredProjection {
            projection,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
        });
    }
    let deferred = std::mem::take(&mut pending.deferred_projections);
    for (node_id, queued) in deferred {
        let projection = queued.projection;
        if projection.scene_incarnation != manager.scene_incarnation()
            || active_terrain.petal_id.as_deref() != Some(projection.petal_id.as_str())
        {
            continue;
        }
        if !projection.is_current(manager) {
            if std::time::Instant::now() < queued.deadline
                && manager.scene_admission(&projection.node_id, &projection.petal_id).is_none()
            {
                pending.deferred_projections.insert(node_id, DeferredProjection {
                    projection,
                    deadline: queued.deadline,
                });
            }
            continue;
        }
        if let Some(petal) = manager.find_petal(&projection.petal_id) {
            pending
                .projected_versions
                .retain(|id, _| petal.nodes.iter().any(|node| node.id == *id));
        }
        if !pending.accept_path_projection(&projection) {
            continue;
        }
        if let Some(current) = current_projections.remove(&projection.node_id) {
            if let (Some(properties), Some(styles)) = (
                current.properties.as_ref(),
                projection_context.styles.as_deref_mut(),
            ) {
                styles.styles.insert(projection.node_id.clone(), style_from_properties(properties));
            }
        }
        let points = json_to_route_points(&projection.points);
        pending
            .in_flight_points
            .insert(projection.node_id.clone(), points.clone());
        reconcile_track_render(
            &mut route_map,
            &mut commands,
            &mut meshes,
            &mut materials,
            &track_lines,
            &single_nodes,
            &projection.petal_id,
            &projection.node_id,
            points,
            true,
        );
        let retained = path_state
            .as_deref_mut()
            .filter(|state| state.editing_track_id.as_deref() == Some(projection.node_id.as_str()))
            .is_some_and(|state| projection.apply_editor(state));
        if retained {
            if let Some(ui) = ui.as_deref_mut() {
                let now = projection_context
                    .clock
                    .as_ref()
                    .map_or(0.0, |clock| clock.elapsed_secs_f64());
                ui.show_advisory("Stored path state refreshed; your newer local draft was retained. The save outcome is shown separately.", now);
            }
        }
        *status = PathEditStatus {
            track_node_id: Some(projection.node_id),
            message: Some(
                if retained {
                    "Stored path state applied; local draft retained"
                } else {
                    "Point edit confirmed"
                }
                .into(),
            ),
            error: None,
        };
    }
    for projection in current_projections.into_values() {
        if active_terrain.petal_id.as_deref() != Some(projection.petal_id.as_str()) {
            continue;
        }
        pending.deferred_projections.remove(&projection.node_id);
        pending.projected_versions.remove(&projection.node_id);
        pending.in_flight_points.remove(&projection.node_id);
        pending.seed_pending.remove(&projection.node_id);
        let points = projection.properties.as_ref().and_then(|properties| {
            (properties.get(GPX_TYPE_KEY).and_then(|value| value.as_str()) == Some("track"))
                .then(|| properties.get(GPX_POINTS_KEY))
                .flatten()
                .filter(|points| fe_sdk::path_geometry::decode_anchor_rows(points).is_some())
                .map(json_to_route_points)
        });
        if let Some(styles) = projection_context.styles.as_deref_mut() {
            if let Some(properties) = projection.properties.as_ref().filter(|_| points.is_some()) {
                styles.styles.insert(projection.node_id.clone(), style_from_properties(properties));
            } else {
                styles.styles.remove(&projection.node_id);
            }
        }
        if let Some(points) = &points {
            pending.in_flight_points.insert(projection.node_id.clone(), points.clone());
        } else {
            route_map.routes.remove(&projection.node_id);
        }
        reconcile_track_render(
            &mut route_map,
            &mut commands,
            &mut meshes,
            &mut materials,
            &track_lines,
            &single_nodes,
            &projection.petal_id,
            &projection.node_id,
            points.unwrap_or_default(),
            true,
        );
        let retained = path_state.as_deref_mut()
            .is_some_and(|state| projection.apply_editor(state));
        if retained {
            if let Some(ui) = ui.as_deref_mut() {
                let now = projection_context.clock.as_ref()
                    .map_or(0.0, |clock| clock.elapsed_secs_f64());
                ui.show_advisory("Stored path state refreshed; your newer local draft was retained", now);
            }
        }
    }
    if pending.reads.is_empty()
        && pending.creates.is_empty()
        && pending.annotate_creates.is_empty()
        && pending.track_property_confirms.is_empty()
        && pending.checked_reads.is_empty()
    {
        return;
    }
    let active_petal_id = active_terrain.petal_id.clone().unwrap_or_default();

    for result in db_results.read() {
        match result {
            DbResult::CheckedNodeMutationResolved {
                node_id,
                request_id,
                receipt,
                current,
                correlation_id,
            } if request_id == correlation_id
                && pending.checked_reads.get(request_id) == Some(node_id) =>
            {
                pending.checked_reads.remove(request_id);
                let outcome = if receipt.is_some() {
                    Err("This point request already has a persisted result; reconciliation required".into())
                } else {
                    undo.commit_path(&nav, request_id, current, &db_tx.0)
                };
                if let Err(error) = outcome {
                    undo.cancel_path(request_id);
                    *status = PathEditStatus {
                        track_node_id: Some(node_id.clone()),
                        message: None,
                        error: Some(error),
                    };
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if pending.checked_reads.contains_key(correlation_id) => {
                let node_id = pending.checked_reads.remove(correlation_id).unwrap();
                undo.cancel_path(correlation_id);
                *status = PathEditStatus {
                    track_node_id: Some(node_id),
                    message: None,
                    error: Some(error.clone()),
                };
            }
            DbResult::NodeCreated { id, petal_id, name, position, correlation_id: Some(request_id), .. } => {
                if pending.creates.get(request_id).is_some_and(|owner| !owner.retired && owner.payload.petal_id == *petal_id && owner.payload.name == *name && owner.payload.position == *position) {
                    pending.creates.remove(request_id);
                    if let Some(message) = submit_authored_track_properties(&mut pending, &db_tx, id, name) {
                        status.error = Some(message);
                    }
                    continue;
                }
                if pending.annotate_creates.get(request_id).is_some_and(|owner| !owner.retired && owner.payload.petal_id == *petal_id && owner.payload.title == *name && owner.payload.position == *position) {
                    let annotate = pending.annotate_creates.remove(request_id).unwrap().payload;
                    for (key, value) in [(GPX_TYPE_KEY, serde_json::json!("waypoint")), (ANNOTATION_TITLE_KEY, serde_json::json!(annotate.title)), (ANNOTATION_BODY_KEY, serde_json::json!(annotate.body)), (ANNOTATION_COLOR_KEY, serde_json::json!(annotate.color))] {
                        if let Err(error) = pending.property_outcomes.submit(&db_tx, id, key, value) {
                            status.error = Some(format!("Annotation node exists; initialization is partial: {error}"));
                            break;
                        }
                    }
                }
            }
            DbResult::NodePropertiesLoaded {
                node_id,
                properties,
                ..
            } => {
                let Some(queue) = pending.reads.get_mut(node_id) else {
                    continue;
                };
                let Some(action) = queue.pop_front() else {
                    continue;
                };
                // FR-5: the seed read for this track has now resolved. Any
                // further appends queued while it was outstanding will be
                // drained below, so this track is no longer "seed pending".
                pending.seed_pending.remove(node_id);
                // FR-5: honor an already-seeded authoritative buffer. If two
                // appends overlapped one in-flight read, the first
                // NodePropertiesLoaded seeds `in_flight_points`; a second read
                // reply (or the queued sibling appends) must extend THAT buffer
                // rather than rebuild from this (now-stale) read snapshot, which
                // would silently drop the first append.
                let mut points = match pending.in_flight_points.get(node_id) {
                    Some(buf) => buf.clone(),
                    None => properties
                        .get(GPX_POINTS_KEY)
                        .map(json_to_route_points)
                        .unwrap_or_default(),
                };

                match action {
                    append_read @ (PendingPathRead::AppendPoint { .. }
                    | PendingPathRead::AppendSmoothPoint { .. }) => {
                        push_append_read(&mut points, append_read);
                        // FR-5: this is the seed read — hand off authority to
                        // in_flight_points so any append queued while
                        // this read was outstanding (or arriving after) uses
                        // the direct-mutate path instead of another
                        // GetNodeProperties race.
                        // Drain any appends (plain or smooth) that piggy-backed
                        // on this one seed read (dedup in drain suppressed their
                        // own reads) — including ones queued BEHIND an in-place
                        // Set/Move entry, which the old front-contiguous pop
                        // stranded forever (see `drain_queued_appends`).
                        if let Some(queue) = pending.reads.get_mut(node_id) {
                            drain_queued_appends(queue, &mut points);
                        }
                        pending
                            .in_flight_points
                            .insert(node_id.clone(), points.clone());
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        persist_and_render_points(
                            &db_tx,
                            &mut route_map,
                            &mut meshes,
                            &mut materials,
                            &track_lines,
                            &single_nodes,
                            &active_petal_id,
                            &mut commands,
                            node_id,
                            points,
                        );
                        *status = PathEditStatus {
                            track_node_id: Some(node_id.clone()),
                            message: Some("Point added".to_string()),
                            error: None,
                        };
                        continue;
                    }
                    PendingPathRead::RemovePoint { index } => {
                        let in_range = index < points.len();
                        if in_range {
                            points.remove(index);
                        }
                        // Drain seed-deduped appends stranded behind this reply
                        // (even when the remove itself errored — the appends
                        // are valid regardless). See `drain_queued_appends`.
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if in_range || drained > 0 {
                            // Keep FR-5's authoritative buffer in sync so a
                            // subsequent AppendPoint doesn't resurrect the
                            // just-removed point from a stale in_flight_points.
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                        *status = if in_range {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: Some("Point removed".to_string()),
                                error: None,
                            }
                        } else {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            }
                        };
                    }
                    PendingPathRead::MovePoint { index, position } => {
                        let moved = if let Some(point) = points.get_mut(index) {
                            // Reposition only; preserve the existing timestamp (no index churn).
                            point.position =
                                [position[0] as f64, position[1] as f64, position[2] as f64];
                            true
                        } else {
                            false
                        };
                        // Drain seed-deduped appends stranded behind this reply
                        // (see `drain_queued_appends`).
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if moved || drained > 0 {
                            // Keep FR-5's authoritative buffer in sync so a later
                            // AppendPoint doesn't rebuild from a stale in_flight_points.
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                        *status = if moved {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: Some("Point moved".to_string()),
                                error: None,
                            }
                        } else {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            }
                        };
                    }
                    PendingPathRead::SetAnchorHandles {
                        index,
                        handle_in,
                        handle_out,
                        smoothness,
                    } => {
                        // Phase 3: mirror MovePoint's deferred arm — bezier
                        // fields only, buffer kept in sync.
                        let applied = if let Some(point) = points.get_mut(index) {
                            apply_anchor_handles(point, handle_in, handle_out, smoothness);
                            true
                        } else {
                            false
                        };
                        // Drain seed-deduped appends stranded behind this reply
                        // (see `drain_queued_appends`).
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if applied || drained > 0 {
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                        *status = if applied {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: Some("Handles set".to_string()),
                                error: None,
                            }
                        } else {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            }
                        };
                    }
                    PendingPathRead::SetAnchorCorner { index, corner_code } => {
                        // Phase 3: mirror MovePoint's deferred arm.
                        let applied = if let Some(point) = points.get_mut(index) {
                            point.corner = CornerKind::from_code(corner_code);
                            true
                        } else {
                            false
                        };
                        // Drain seed-deduped appends stranded behind this reply
                        // (see `drain_queued_appends`).
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if applied || drained > 0 {
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                        *status = if applied {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: Some("Corner set".to_string()),
                                error: None,
                            }
                        } else {
                            PathEditStatus {
                                track_node_id: Some(node_id.clone()),
                                message: None,
                                error: Some(format!("point index {index} out of range")),
                            }
                        };
                    }
                    PendingPathRead::AnnotatePoint {
                        index,
                        title,
                        body,
                        color,
                    } => {
                        // Snapshot the anchor BEFORE the drain — the annotate's
                        // index carries issue-time semantics (appends queued
                        // after it must not widen its valid range).
                        let anchor = points.get(index).map(|p| {
                            [
                                p.position[0] as f32,
                                p.position[1] as f32,
                                p.position[2] as f32,
                            ]
                        });
                        // Drain seed-deduped appends stranded behind this reply
                        // BEFORE the error-path `continue`s below (see
                        // `drain_queued_appends`).
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if drained > 0 {
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                        let Some(position) = anchor else {
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
                        if pending.creates.len() + pending.annotate_creates.len() >= MAX_HOST_OUTCOMES {
                            status.error = Some("Accepted annotation outcomes are full; no node was submitted".into());
                            continue;
                        }
                        let request_id = host_request_id();
                        match through_scene_door(&db_tx, DbCommand::CreateNode { petal_id: petal_id.clone(), name: title.clone(), position, correlation_id: Some(request_id.clone()) }, "Database queue unavailable; annotation node was not submitted") {
                            Ok(()) => { pending.annotate_creates.insert(request_id, Accepted::new(PendingAnnotateCreate { petal_id, position, title, body, color })); }
                            Err(message) => status.error = Some(message),
                        }
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
                        match save_path.write_bytes(xml.as_bytes()) {
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
                        // Drain seed-deduped appends stranded behind this reply
                        // (the export above snapshots issue-time points; see
                        // `drain_queued_appends`).
                        let drained = pending
                            .reads
                            .get_mut(node_id)
                            .map(|q| drain_queued_appends(q, &mut points))
                            .unwrap_or(0);
                        if pending
                            .reads
                            .get(node_id)
                            .map(|q| q.is_empty())
                            .unwrap_or(true)
                        {
                            pending.reads.remove(node_id);
                        }
                        if drained > 0 {
                            pending
                                .in_flight_points
                                .insert(node_id.clone(), points.clone());
                            persist_and_render_points(
                                &db_tx,
                                &mut route_map,
                                &mut meshes,
                                &mut materials,
                                &track_lines,
                                &single_nodes,
                                &active_petal_id,
                                &mut commands,
                                node_id,
                                points,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Persist `points` as `gpx_points` and update the render half in one step —
/// the FR-3 "live edits update immediately" requirement. Routes through the
/// shared `reconcile_track_render` (with `force_line_redraw = true`) so a live
/// edit reconciles the single-point node the same way petal-load does: 0→1
/// spawns the node, 2→1 tears down the line + spawns the node, 1→2 despawns the
/// stale node before spawning the line (no duplicate leak), 1→0 despawns it.
#[allow(clippy::too_many_arguments)]
fn persist_and_render_points(
    db_tx: &DbCommandSender,
    route_map: &mut TrackRouteMap,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    track_lines: &Query<(Entity, &GpxTrackLine)>,
    single_nodes: &Query<(Entity, &SinglePointTrackNode)>,
    active_petal_id: &str,
    commands: &mut Commands,
    track_node_id: &str,
    points: Vec<TimestampedRoutePoint>,
) {
    if let Err(message) = through_scene_door(
        db_tx,
        DbCommand::SetNodeProperty {
            node_id: track_node_id.to_string(),
            key: GPX_POINTS_KEY.to_string(),
            value: route_points_to_json(&points),
            correlation_id: None,
        },
        "Database queue unavailable; track points were not saved",
    ) {
        warn!(%message, "track point write was not submitted");
    }
    reconcile_track_render(
        route_map,
        commands,
        meshes,
        materials,
        track_lines,
        single_nodes,
        active_petal_id,
        track_node_id,
        points,
        true,
    );
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
            position: [
                p.position[0] as f32,
                p.position[1] as f32,
                p.position[2] as f32,
            ],
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

/// Require a resolved Viewer before host path operations.
pub fn path_projection_access(
    manager: Res<VerseManager>,
    role: Option<Res<fe_ui::plugin::LocalUserRole>>,
) -> bool {
    can_project_paths(&manager, role.as_deref())
}

pub fn path_write_access(
    manager: Res<VerseManager>,
    role: Option<Res<fe_ui::plugin::LocalUserRole>>,
) -> bool {
    !manager.scene_access_blocked() && role.is_some_and(|role| role.can_edit())
}

fn can_project_paths(manager: &VerseManager, role: Option<&fe_ui::plugin::LocalUserRole>) -> bool {
    !manager.scene_access_blocked()
        && role.is_some_and(|role| role.role.is_some_and(|role| role.can_view()))
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct PathAccessRetirement<'w, 's> {
    path_status: Option<ResMut<'w, PathEditStatus>>,
    import_status: Option<ResMut<'w, GpxImportStatus>>,
    role: Option<Res<'w, fe_ui::plugin::LocalUserRole>>,
    routes: ResMut<'w, TrackRouteMap>,
    styles: ResMut<'w, TrackStyleMap>,
    lines: Query<'w, 's, (Entity, &'static GpxTrackLine)>,
    single_nodes: Query<'w, 's, (Entity, &'static SinglePointTrackNode)>,
    commands: Commands<'w, 's>,
    ops: ResMut<'w, PendingPathOps>,
    imports: ResMut<'w, PendingGpxImports>,
    import_ops: ResMut<'w, PendingGpxOps>,
    undo: Option<ResMut<'w, fe_ui::undo::UndoState>>,
    ui: Option<ResMut<'w, fe_ui::actions::UiManager>>,
}

impl PathAccessRetirement<'_, '_> {
    fn retire(
        &mut self,
        pending: &mut PendingPathEdits,
        manager: &mut VerseManager,
        nav: &fe_ui::navigation_manager::NavigationManager,
    ) {
        manager.retire_scene_read_models();
        if pending.retire_access() {
            if pending.creates.len() + pending.annotate_creates.len() + pending.property_outcomes.0.len() > 0 {
                if let Some(status) = self.path_status.as_deref_mut() {
                    status.error = Some("Access changed with accepted path requests outstanding; outcomes remain owned and no continuation will be submitted".into());
                }
            }
            if self.imports.pending_outcomes() > 0 {
                if let Some(status) = self.import_status.as_deref_mut() {
                    status.error = Some("Access changed with accepted import requests outstanding; the import may be partial and outcomes remain owned".into());
                }
            }
            self.routes.routes.clear();
            self.styles.styles.clear();
            self.imports.retire();
            for (entity, _) in &self.lines {
                self.commands.entity(entity).try_despawn();
            }
            for (entity, _) in &self.single_nodes {
                self.commands.entity(entity).try_despawn();
            }
        }
        *self.ops = PendingPathOps::default();
        *self.import_ops = PendingGpxOps::default();
        while self.ui.as_deref_mut()
            .and_then(|ui| ui.take_editor_path_projection(nav))
            .or_else(|| self.undo.as_deref_mut()
                .and_then(|undo| undo.take_path_projection(nav)))
            .is_some() {}
    }
}

/// Refresh a Petal through bounded full-current reads; see src/AGENTS.md.
pub fn request_petal_gpx_materialization(
    active_terrain: Res<ActivePetalTerrain>,
    mut manager: ResMut<VerseManager>,
    nav: Res<fe_ui::navigation_manager::NavigationManager>,
    mut pending: ResMut<PendingPathEdits>,
    mut last: Local<Option<(u128, u64, u64, u64)>>,
    mut access: PathAccessRetirement,
) {
    manager.synchronize_scene_navigation(&nav);
    let generation = manager.scene_access_generation();
    let outcome_navigation = (manager.scene_incarnation(), nav.intent_revision);
    if pending.access_generation != generation
        || pending.outcome_navigation.is_some_and(|owner| owner != outcome_navigation)
    {
        access.retire(&mut pending, &mut manager, &nav);
        pending.access_generation = generation;
        *last = None;
    }
    pending.outcome_navigation = Some(outcome_navigation);
    if !can_project_paths(&manager, access.role.as_deref()) {
        access.retire(&mut pending, &mut manager, &nav);
        *last = None;
        return;
    }
    if !access.role.as_ref().is_some_and(|role| role.can_edit()) {
        pending.retire_outcomes();
        access.imports.retire();
        *access.ops = PendingPathOps::default();
        *access.import_ops = PendingGpxOps::default();
    }
    pending.access_retired = false;
    if active_terrain.petal_id != nav.active_petal_id {
        return;
    }
    let key = (
        manager.scene_incarnation(),
        manager.scene_hierarchy_revision(),
        active_terrain.revision,
        nav.intent_revision,
    );
    if *last != Some(key) {
        *last = Some(key);
        pending.in_flight_points.clear();
        pending.seed_pending.clear();
        pending.projected_versions.clear();
        pending.deferred_projections.clear();
        pending.projection_petal_id = active_terrain.petal_id.clone();
        pending.materialization_refresh.clear();
        if let Some(petal) = nav.active_petal_id.as_deref().and_then(|id| manager.find_petal(id)) {
            let mut seen = std::collections::HashSet::new();
            pending.materialization_refresh.extend(petal.nodes.iter()
                .filter(|node| first_seen(&mut seen, &node.id))
                .take(fe_ui::verse_manager::SCENE_NODE_HARD_LIMIT)
                .map(|node| node.id.clone()));
            if petal.nodes.len() > fe_ui::verse_manager::SCENE_NODE_HARD_LIMIT {
                bevy::log::warn!("Path current-state refresh is limited by the scene node cap");
            }
        }
    }
    for _ in 0..64 {
        let Some(node) = pending.materialization_refresh.front() else {
            break;
        };
        if manager.petal_id_of(node) != nav.active_petal_id.as_deref() {
            pending.materialization_refresh.pop_front();
            continue;
        }
        if !manager.request_scene_refresh(node, &nav) {
            break;
        }
        pending.materialization_refresh.pop_front();
    }
}

/// `true` only the first time `id` is seen this frame — the same-batch
/// duplicate-`NodePropertiesLoaded` guard. Pure so it is unit-testable.
fn first_seen(seen: &mut std::collections::HashSet<String>, id: &str) -> bool {
    seen.insert(id.to_string())
}

/// Removes deleted legacy routes; current geometry uses the admitted projection queue.
pub fn advance_path_materialization(
    mut db_results: MessageReader<DbResult>,
    mut route_map: ResMut<TrackRouteMap>,
    mut style_map: ResMut<TrackStyleMap>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    single_nodes: Query<(Entity, &SinglePointTrackNode)>,
    mut commands: Commands,
) {
    for result in db_results.read() {
        let (DbResult::NodeDeleted { node_id, .. } | DbResult::PluginNodeRemoved { node_id, .. }) = result else {
            continue;
        };
        route_map.routes.remove(node_id);
        style_map.styles.remove(node_id);
        for (entity, _) in track_lines.iter().filter(|(_, line)| line.track_node_id == *node_id) {
            commands.entity(entity).try_despawn();
        }
        for (entity, _) in single_nodes.iter().filter(|(_, node)| node.track_node_id == *node_id) {
            commands.entity(entity).try_despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_terrain::config::TerrainConfig;
    use std::path::PathBuf;

    #[test]
    fn pick_shape_anchors_are_the_unflattened_route_points() {
        // A curved anchor flattens into many ribbon points; the snap targets
        // must stay the three authored anchors, bit-for-bit.
        let mut route_map = TrackRouteMap::default();
        let anchor = |x: f64, z: f64| TimestampedRoutePoint {
            position: [x, 0.25, z],
            ..Default::default()
        };
        let mut curved = anchor(10.0, 0.0);
        curved.handle_in = Some([-2.0, 0.0, 3.0]);
        curved.handle_out = Some([2.0, 0.0, -3.0]);
        curved.corner = CornerKind::Symmetric;
        route_map.routes.insert(
            "t1".into(),
            TrackRoute {
                points: vec![anchor(0.0, 0.0), curved, anchor(20.0, 5.5)],
                total_duration_secs: 0.0,
            },
        );
        let geometry = RenderedTrackGeometry::prepare(&route_map.routes["t1"].points, TrackStyle::default()).unwrap();
        let shape = track_pick_shape(&geometry).unwrap();
        assert!(shape.points.len() > 3, "ribbon is flattened");
        assert_eq!(
            shape.anchors,
            vec![
                Vec3::new(0.0, 0.25, 0.0),
                Vec3::new(10.0, 0.25, 0.0),
                Vec3::new(20.0, 0.25, 5.5)
            ]
        );
        let stored = route_points_to_json(&route_map.routes["t1"].points);
        let closed = RenderedTrackGeometry::prepare(&route_map.routes["t1"].points, TrackStyle { closed: true, width: 1.0, ..Default::default() }).unwrap();
        let closed_shape = track_pick_shape(&closed).unwrap();
        assert!(closed_shape.closed && closed_shape.matches_render_source(&stored));
        assert_eq!(closed_shape.half_width, 0.5);
        assert_eq!(closed_shape.fill_triangles.len(), closed.zone().unwrap().triangle_indices.len() / 3, "picking uses the renderer's actual accepted triangulation");
        assert_eq!(closed_shape.points[..closed.zone().unwrap().points.len()], closed.zone().unwrap().points.iter().copied().map(Vec3::from_array).collect::<Vec<_>>());
        assert!(!closed.matches_mesh(&Mesh3d(Handle::default())), "a prepared input alone is not proof of a built mesh");
        route_map.routes.get_mut("t1").unwrap().points[1].handle_out = Some([9.0, 0.0, -3.0]);
        assert!(!closed_shape.matches_render_source(&route_points_to_json(&route_map.routes["t1"].points)), "late live-map input cannot relabel the actual older mesh source");
        assert!(closed_shape.matches_render_source(&stored));
        let mut changed_geometry = closed_shape.clone();
        changed_geometry.points[0].x += 0.5;
        assert!(!changed_geometry.matches_render_source(&stored), "changed prepared geometry cannot retain a stale source witness");
        let mut changed_metadata = stored.clone();
        changed_metadata[1][10] = serde_json::json!(1.0);
        assert!(!closed_shape.matches_render_source(&changed_metadata), "equal positions with changed corner metadata are a different source");
        for (original, changed) in [(1.0, 1.000000001), (0.0, 1e-8)] {
            let mut precise_source = json_to_route_points(&stored);
            precise_source[0].handle_in = Some([original, 0.0, 0.0]);
            let precise_geometry = RenderedTrackGeometry::prepare(
                &precise_source,
                TrackStyle {
                    closed: true,
                    width: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
            let precise_shape = track_pick_shape(&precise_geometry).unwrap();
            let exact = route_points_to_json(&precise_source);
            assert!(precise_shape.matches_render_source(&exact));
            precise_source[0].handle_in = Some([changed, 0.0, 0.0]);
            let changed = route_points_to_json(&precise_source);
            assert!(
                !precise_shape.matches_render_source(&changed),
                "inactive f64 handles cannot collapse to the editor's lower precision"
            );
        }
        for slot in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
            let mut complete = json_to_route_points(&stored);
            complete[0].position = [1.0, 2.0, 3.0];
            complete[0].time_seconds = 7.25;
            complete[0].handle_in = Some([1.0, 2.0, 3.0]);
            complete[0].handle_out = Some([4.0, 5.0, 6.0]);
            complete[0].corner = CornerKind::Smooth;
            complete[0].smoothness = 0.5;
            let geometry = RenderedTrackGeometry::prepare(
                &complete,
                TrackStyle {
                    closed: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let witness = track_pick_shape(&geometry).unwrap();
            let mut changed = route_points_to_json(&complete);
            let original = changed[0][slot].as_f64().unwrap();
            changed[0][slot] = serde_json::json!(
                original
                    + if slot == 10 {
                        1.0
                    } else if slot == 11 {
                        0.125
                    } else {
                        1e-9
                    }
            );
            assert!(
                !witness.matches_render_source(&changed),
                "all typed source slots retain their renderer precision: {slot}"
            );
        }
        let mut compact = stored.clone();
        compact[0] = serde_json::json!([0.0, 0.25, 0.0]);
        assert!(
            closed_shape.matches_render_source(&compact),
            "missing time and stored zero are equivalent"
        );
        compact[0] = serde_json::json!([0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0]);
        assert!(
            closed_shape.matches_render_source(&compact),
            "zero handles and handle-free corner normalization are shared with the renderer"
        );
        let mut negative_smoothness = json_to_route_points(&stored);
        negative_smoothness[0].smoothness = -0.5;
        let negative_geometry = RenderedTrackGeometry::prepare(
            &negative_smoothness,
            TrackStyle {
                closed: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !track_pick_shape(&negative_geometry)
                .unwrap()
                .matches_render_source(&stored),
            "frozen typed metadata cannot pass through compact persistence encoding"
        );
    }

    // pen_curve_tool_20260722 Phase 1: bezier wire-format (4/12-slot) round-trips.
    #[test]
    fn plain_corner_encodes_compact_4slot_and_round_trips() {
        let pts = vec![TimestampedRoutePoint {
            position: [1.0, 2.0, 3.0],
            time_seconds: 5.0,
            ..Default::default()
        }];
        let json = route_points_to_json(&pts);
        let row = json.as_array().unwrap()[0].as_array().unwrap();
        assert_eq!(row.len(), 4, "plain corner must encode as compact 4-slot");
        let back = json_to_route_points(&json);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].position, [1.0, 2.0, 3.0]);
        assert!((back[0].time_seconds - 5.0).abs() < f64::EPSILON);
        assert!(back[0].handle_in.is_none() && back[0].handle_out.is_none());
        assert_eq!(back[0].corner, CornerKind::Corner);
        assert!(back[0].smoothness <= 0.0);
    }

    #[test]
    fn symmetric_smooth_anchor_encodes_12slot_and_round_trips() {
        let pts = vec![TimestampedRoutePoint {
            position: [0.0, 0.0, 0.0],
            time_seconds: 0.0,
            handle_in: Some([-1.0, 0.0, 0.5]),
            handle_out: Some([1.0, 0.0, -0.5]),
            corner: CornerKind::Symmetric,
            smoothness: 0.5,
        }];
        let json = route_points_to_json(&pts);
        let row = json.as_array().unwrap()[0].as_array().unwrap();
        assert_eq!(row.len(), 12, "handle-carrying anchor must encode 12-slot");
        let back = json_to_route_points(&json);
        assert_eq!(back[0].handle_in, Some([-1.0, 0.0, 0.5]));
        assert_eq!(back[0].handle_out, Some([1.0, 0.0, -0.5]));
        assert_eq!(back[0].corner, CornerKind::Symmetric);
        assert!((back[0].smoothness - 0.5).abs() < f32::EPSILON);
        let before = route_points_to_json(&back);
        let projection = fe_ui::undo::PathProjection {
            node_id: "track".into(),
            petal_id: "p".into(),
            history_version: 8,
            scene_incarnation: 0,
            points: before.clone(),
            editor_policy: fe_ui::undo::PathEditorPolicy::Replace,
        };
        let mut pending = PendingPathEdits::default();
        assert!(pending.accept_path_projection(&projection));
        let older = fe_ui::undo::PathProjection {
            history_version: 7,
            ..projection.clone()
        };
        assert!(
            !pending.accept_path_projection(&older),
            "ordinary v7 cannot overwrite a consumed batch v8"
        );
        let unknown = fe_ui::undo::PathProjection {
            history_version: 0,
            ..projection.clone()
        };
        assert!(
            !pending.accept_path_projection(&unknown),
            "legacy reads cannot invent a committed version"
        );
        assert!(
            !projection.is_current(&VerseManager::default()),
            "missing objects cannot regain rendered routes"
        );
        let other_petal = fe_ui::undo::PathProjection {
            petal_id: "other".into(),
            history_version: 1,
            ..projection.clone()
        };
        assert!(pending.accept_path_projection(&other_petal));
        assert_eq!(
            pending.projected_versions.len(),
            1,
            "projection watermark is scoped to one Petal"
        );
        {
            use fe_ui::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
            let manager = VerseManager::from_verses(vec![VerseEntry {
                id: "v".into(), name: "V".into(), expanded: true, namespace_id: None,
                fractals: vec![FractalEntry { id: "f".into(), name: "F".into(), expanded: true,
                    petals: vec![PetalEntry { id: "p".into(), name: "P".into(), expanded: true, render_distance: None,
                        nodes: vec![NodeEntry { id: "track".into(), name: "Track".into(), has_asset: false, position: [0.0; 3], transform: Transform::IDENTITY, webpage_url: None, asset_path: None }] }] }],
            }]);
            let mut lifecycle = App::new();
            lifecycle.insert_resource(manager)
                .insert_resource(fe_ui::navigation_manager::NavigationManager { active_verse_id: Some("v".into()), active_fractal_id: Some("f".into()), active_petal_id: Some("p".into()), ..Default::default() })
                .insert_resource(ActivePetalTerrain {
                    raw_source_fingerprint: None,
                    assignment_fingerprint: None,
                    input_scale_valid: false, petal_id: Some("p".into()), ..Default::default() })
                .insert_resource(fe_ui::plugin::LocalUserRole { role: Some("owner".into()) })
                .init_resource::<PendingPathEdits>()
                .init_resource::<TrackRouteMap>()
                .init_resource::<TrackStyleMap>()
                .init_resource::<PendingPathOps>()
                .init_resource::<PendingGpxOps>()
                .init_resource::<PendingGpxImports>()
                .init_resource::<PathEditStatus>()
                .init_resource::<GpxImportStatus>()
                .add_message::<DbResult>();
            let (accepted_tx, accepted_commands) = crossbeam::channel::unbounded();
            let accepted_sender = DbCommandSender(accepted_tx);
            lifecycle.insert_resource(DbCommandSender(accepted_sender.0.clone()))
                .add_systems(Update, (request_petal_gpx_materialization, advance_host_outcomes, advance_gpx_imports.run_if(path_write_access)).chain());
            lifecycle.update();
            let old_create = host_request_id();
            let failed_create = host_request_id();
            let delayed_create = host_request_id();
            let property_request;
            {
                let mut pending = lifecycle.world_mut().resource_mut::<PendingPathEdits>();
                pending.submit_create(&accepted_sender, old_create.clone(), "p".into(), "Same name".into()).unwrap();
                pending.submit_create(&accepted_sender, failed_create.clone(), "p".into(), "Failure".into()).unwrap();
                pending.submit_create(&accepted_sender, delayed_create.clone(), "p".into(), "Delayed".into()).unwrap();
                property_request = pending.property_outcomes.submit(&accepted_sender, "track", GPX_POINTS_KEY, before.clone()).unwrap();
                pending.track_property_confirms.insert("track".into(), [property_request.clone()].into_iter().collect());
            }
            let import_plan = || PreparedTrack { draft: GpxNodeDraft { name: "Same name".into(), position: [0.0; 3], properties: vec![(GPX_TYPE_KEY.into(), serde_json::json!("track"))] }, waypoints: vec![], route_points: back.clone() };
            let old_import;
            let failed_waypoint;
            {
                let mut pending = lifecycle.world_mut().resource_mut::<PendingGpxImports>();
                pending.submit_track(&accepted_sender, "p", import_plan()).unwrap();
                old_import = pending.tracks.keys().next().unwrap().clone();
                pending.submit_waypoint(&accepted_sender, "p", GpxNodeDraft { name: "Waypoint".into(), position: [1.0; 3], properties: vec![] }, None).unwrap();
                failed_waypoint = pending.waypoints.keys().next().unwrap().clone();
            }
            assert_eq!(accepted_commands.try_iter().count(), 6, "only successful channel submissions acquire outcome owners");
            lifecycle.world_mut().resource_mut::<fe_ui::plugin::LocalUserRole>().role = None;
            lifecycle.world_mut().resource_mut::<TrackRouteMap>().routes.insert("track".into(), TrackRoute { points: back.clone(), total_duration_secs: 1.0 });
            lifecycle.world_mut().resource_mut::<TrackStyleMap>().styles.insert("track".into(), TrackStyle::default());
            {
                let mut pending = lifecycle.world_mut().resource_mut::<PendingPathEdits>();
                pending.in_flight_points.insert("track".into(), back.clone());
                pending.seed_pending.insert("track".into());
                pending.projected_versions.insert("track".into(), 8);
                pending.checked_reads.insert("old".into(), "track".into());
                pending.deferred_projections.insert("track".into(), DeferredProjection { projection: projection.clone(), deadline: std::time::Instant::now() });
            }
            let line = lifecycle.world_mut().spawn(GpxTrackLine { track_node_id: "track".into() }).id();
            let point = lifecycle.world_mut().spawn(SinglePointTrackNode { track_node_id: "track".into() }).id();
            lifecycle.update();
            assert!(lifecycle.world().get_entity(line).is_err() && lifecycle.world().get_entity(point).is_err(), "unknown same-Petal access removes both rendered route forms");
            let pending = lifecycle.world().resource::<PendingPathEdits>();
            assert!(pending.access_retired && pending.in_flight_points.is_empty() && pending.seed_pending.is_empty() && pending.projected_versions.is_empty() && pending.checked_reads.is_empty() && pending.deferred_projections.is_empty());
            assert!(lifecycle.world().resource::<TrackRouteMap>().routes.is_empty());
            assert!(lifecycle.world().resource::<TrackStyleMap>().styles.is_empty());
            assert!(lifecycle.world().resource::<PendingPathEdits>().creates[&old_create].retired);
            assert_eq!(lifecycle.world().resource::<PendingPathEdits>().creates[&old_create].payload.name, "Same name");
            assert_eq!(route_points_to_json(&lifecycle.world().resource::<PendingGpxImports>().tracks[&old_import].payload.route_points), before, "accepted import source payload survives access retirement");
            lifecycle.world_mut().resource_mut::<PendingPathEdits>().creates.get_mut(&delayed_create).unwrap().started -= std::time::Duration::from_secs(6);
            lifecycle.update();
            assert!(lifecycle.world().resource::<PathEditStatus>().error.as_deref().unwrap().contains("unknown"));
            assert!(lifecycle.world().resource::<PendingPathEdits>().creates.contains_key(&delayed_create), "timeouts report unknown without dropping or replaying accepted requests");
            lifecycle.world_mut().write_message(DbResult::NodeCreated { id: "old-created".into(), petal_id: "p".into(), name: "Same name".into(), has_asset: false, correlation_id: Some(old_create.clone()), position: [0.0; 3], scale: [1.0; 3] });
            lifecycle.world_mut().write_message(DbResult::RequestFailed { correlation_id: failed_create.clone(), error: "fixture denied".into() });
            lifecycle.world_mut().write_message(DbResult::NodePropertySet { node_id: "track".into(), key: GPX_POINTS_KEY.into(), correlation_id: Some(property_request) });
            lifecycle.world_mut().write_message(DbResult::NodeCreated { id: "old-import".into(), petal_id: "p".into(), name: "Same name".into(), has_asset: false, correlation_id: Some(old_import.clone()), position: [0.0; 3], scale: [1.0; 3] });
            lifecycle.world_mut().write_message(DbResult::RequestFailed { correlation_id: failed_waypoint, error: "fixture denied".into() });
            lifecycle.update();
            let pending = lifecycle.world().resource::<PendingPathEdits>();
            assert!(!pending.creates.contains_key(&old_create) && !pending.creates.contains_key(&failed_create));
            assert!(pending.creates.contains_key(&delayed_create));
            assert!(pending.property_outcomes.0.is_empty() && pending.track_property_confirms.is_empty());
            assert_eq!(lifecycle.world().resource::<PendingGpxImports>().pending_outcomes(), 0);
            assert!(accepted_commands.try_recv().is_err(), "late accepted births and property confirmations cannot issue continuation writes while unavailable");
            lifecycle.world_mut().resource_mut::<fe_ui::plugin::LocalUserRole>().role = Some("owner".into());
            lifecycle.update();
            lifecycle.world_mut().resource_mut::<PendingGpxImports>().submit_track(&accepted_sender, "p", import_plan()).unwrap();
            let new_import = lifecycle.world().resource::<PendingGpxImports>().tracks.keys().next().unwrap().clone();
            assert_ne!(new_import, old_import);
            assert!(matches!(accepted_commands.try_recv(), Ok(DbCommand::CreateNode { correlation_id: Some(ref id), .. }) if *id == new_import));
            lifecycle.world_mut().write_message(DbResult::NodeCreated { id: "old-import".into(), petal_id: "p".into(), name: "Same name".into(), has_asset: false, correlation_id: Some(old_import), position: [0.0; 3], scale: [1.0; 3] });
            lifecycle.update();
            assert!(lifecycle.world().resource::<PendingGpxImports>().tracks.contains_key(&new_import), "a late old birth cannot satisfy a new same-name/Petal import");
            assert!(accepted_commands.try_recv().is_err(), "no properties or child births continue from the stale tuple");
            lifecycle.world_mut().write_message(DbResult::RequestFailed { correlation_id: new_import, error: "fixture terminal".into() });
            lifecycle.update();
            assert_eq!(lifecycle.world().resource::<PendingGpxImports>().pending_outcomes(), 0);
            lifecycle.world_mut().resource_mut::<fe_ui::plugin::LocalUserRole>().role = Some("viewer".into());
            lifecycle.update();
            assert!(!lifecycle.world().resource::<PendingPathEdits>().access_retired);
            assert_eq!(lifecycle.world().resource::<VerseManager>().pending_scene_operations(), 1, "restored Viewer access rehydrates through a fresh complete read intent");
            lifecycle.world_mut().resource_mut::<fe_ui::plugin::LocalUserRole>().role = Some("none".into());
            lifecycle.update();
            assert!(lifecycle.world().resource::<PendingPathEdits>().access_retired);
            assert_eq!(lifecycle.world().resource::<VerseManager>().pending_scene_operations(), 0, "same-Petal revocation retires newly queued read identities too");
            lifecycle.world_mut().resource_mut::<fe_ui::plugin::LocalUserRole>().role = Some("viewer".into());
            lifecycle.update();
            assert_eq!(lifecycle.world().resource::<VerseManager>().pending_scene_operations(), 1);
        }
        let rows = projection.editor_rows();
        let nav = fe_ui::navigation_manager::NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("f".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        };
        let mut undo = fe_ui::undo::UndoState::default();
        let request = undo
            .reserve_path(
                &nav,
                "track",
                0,
                rows[0].position,
                [4.0, 3.0, 6.0],
                Some(7),
                &rows,
            )
            .unwrap()
            .unwrap();
        let current:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(serde_json::json!({"node_id":"track","petal_id":"p","position":[0.0,0.0,0.0],"elevation":0.0,"rotation":[0.0,0.0,0.0],"scale":[1.0,1.0,1.0],"interactive":false,"created_at":"test","edit_seq":0,"properties":{"gpx_points":before}})).unwrap();
        let (tx, rx) = crossbeam::channel::unbounded();
        undo.commit_path(&nav, &request, &current, &tx).unwrap();
        let DbCommand::CommitCheckedNodeMutation {
            mutation: fe_runtime::messages::NodeMutation::PropertySet { value, .. },
            expected,
            correlation_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("point move requires committed receipt contract")
        };
        assert_eq!(correlation_id, request);
        assert_eq!(expected.history_version, Some(7));
        assert_eq!(
            value[0].as_array().unwrap()[3..],
            before[0].as_array().unwrap()[3..]
        );
        assert!(undo.is_pending());
        assert!(
            undo.take_path_projection(&nav).is_none(),
            "no render projection before matching receipt"
        );
        let mut authored = fe_ui::undo::UndoState::default();
        let mut authored_rows = rows.clone();
        authored_rows[0].time_seconds = None;
        let id = authored
            .reserve_path(
                &nav,
                "track",
                0,
                authored_rows[0].position,
                [4.0, 3.0, 6.0],
                Some(7),
                &authored_rows,
            )
            .unwrap()
            .unwrap();
        authored.commit_path(&nav, &id, &current, &tx).unwrap();
        let DbCommand::CommitCheckedNodeMutation {
            mutation: fe_runtime::messages::NodeMutation::PropertySet { value, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("authored timestamp encoding must remain editable")
        };
        assert_eq!(value[0][3], before[0][3]);
        for (fractal, petal) in [("f", "another-petal"), ("another-fractal", "p")] {
            let mut delayed = fe_ui::undo::UndoState::default();
            let id = delayed
                .reserve_path(
                    &nav,
                    "track",
                    0,
                    rows[0].position,
                    [4.0, 3.0, 6.0],
                    Some(7),
                    &rows,
                )
                .unwrap()
                .unwrap();
            let changed = fe_ui::navigation_manager::NavigationManager {
                active_verse_id: Some("v".into()),
                active_fractal_id: Some(fractal.into()),
                active_petal_id: Some(petal.into()),
                ..Default::default()
            };
            assert!(delayed.commit_path(&changed, &id, &current, &tx).is_err());
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn legacy_4slot_decodes_to_corner_no_handles() {
        let json = serde_json::json!([[10.0, 0.0, -5.0, 1.0]]);
        let back = json_to_route_points(&json);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].position, [10.0, 0.0, -5.0]);
        assert!(back[0].handle_in.is_none() && back[0].handle_out.is_none());
        assert_eq!(back[0].corner, CornerKind::Corner);
        assert!(back[0].smoothness <= 0.0);
    }

    #[test]
    fn mixed_length_array_decodes_each_row() {
        let json = serde_json::json!([
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 1.0, -0.5, 0.0, 0.0, 0.5, 0.0, 0.0, 2.0, 0.75],
        ]);
        let back = json_to_route_points(&json);
        assert_eq!(back.len(), 2);
        assert!(back[0].handle_in.is_none() && back[0].handle_out.is_none());
        assert_eq!(back[0].corner, CornerKind::Corner);
        assert_eq!(back[1].handle_in, Some([-0.5, 0.0, 0.0]));
        assert_eq!(back[1].handle_out, Some([0.5, 0.0, 0.0]));
        assert_eq!(back[1].corner, CornerKind::Symmetric);
    }

    #[test]
    fn smooth_claim_with_zero_handles_normalizes_to_corner() {
        // 12-slot row claiming Smooth (code 1) but both handle-triples zero.
        let json =
            serde_json::json!([[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]]);
        let back = json_to_route_points(&json);
        assert_eq!(back.len(), 1);
        assert!(back[0].handle_in.is_none() && back[0].handle_out.is_none());
        assert_eq!(
            back[0].corner,
            CornerKind::Corner,
            "malformed smooth-with-no-handles normalizes to Corner"
        );
    }

    // pen_curve_tool_20260722 Phase 3: op-payload widening + read-modify-write
    // round-trips through the 4/12-slot codec.
    #[test]
    fn smooth_anchor_point_widens_payload_and_maps_corner_code() {
        let p = smooth_anchor_point(
            [1.0, 2.0, 3.0],
            Some([-0.5, 0.0, 0.25]),
            Some([0.5, 0.0, -0.25]),
            2.0,
            0.75,
        );
        assert_eq!(p.position, [1.0, 2.0, 3.0]);
        assert!(
            p.time_seconds.abs() < f64::EPSILON,
            "authored anchor has t=0"
        );
        assert_eq!(p.handle_in, Some([-0.5, 0.0, 0.25]));
        assert_eq!(p.handle_out, Some([0.5, 0.0, -0.25]));
        assert_eq!(p.corner, CornerKind::Symmetric);
        assert!((p.smoothness - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn append_smooth_persist_reload_preserves_handles() {
        // Simulates the AppendSmoothPoint arm's mutation + persist + reload.
        let points = vec![smooth_anchor_point(
            [1.0, 0.0, 2.0],
            Some([-0.5, 0.0, 0.0]),
            Some([0.5, 0.0, 0.0]),
            2.0,
            0.0,
        )];
        let back = json_to_route_points(&route_points_to_json(&points));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].position, [1.0, 0.0, 2.0]);
        assert_eq!(back[0].handle_in, Some([-0.5, 0.0, 0.0]));
        assert_eq!(back[0].handle_out, Some([0.5, 0.0, 0.0]));
        assert_eq!(back[0].corner, CornerKind::Symmetric);
    }

    #[test]
    fn set_anchor_handles_on_legacy_row_round_trips() {
        // Decode a legacy 4-slot list, apply the SetAnchorHandles mutation to
        // row 0, re-encode: it grows to 12-slot; untouched rows stay compact
        // (NFR-3: no intermediate lengths, timestamps preserved).
        let json = serde_json::json!([[0.0, 0.0, 0.0, 1.0], [4.0, 0.0, 0.0, 2.0]]);
        let mut points = json_to_route_points(&json);
        apply_anchor_handles(
            &mut points[0],
            Some([-1.0, 0.0, 0.0]),
            Some([1.0, 0.0, 0.0]),
            0.25,
        );
        let encoded = route_points_to_json(&points);
        let rows = encoded.as_array().unwrap();
        assert_eq!(rows[0].as_array().unwrap().len(), 12);
        assert_eq!(
            rows[1].as_array().unwrap().len(),
            4,
            "untouched legacy row stays compact"
        );
        let back = json_to_route_points(&encoded);
        assert_eq!(back[0].handle_in, Some([-1.0, 0.0, 0.0]));
        assert_eq!(back[0].handle_out, Some([1.0, 0.0, 0.0]));
        assert!((back[0].smoothness - 0.25).abs() < f32::EPSILON);
        assert!(
            (back[0].time_seconds - 1.0).abs() < f64::EPSILON,
            "t preserved"
        );
        assert!(back[1].handle_in.is_none() && back[1].handle_out.is_none());
    }

    #[test]
    fn set_anchor_corner_round_trips_alongside_handles() {
        let mut points = vec![smooth_anchor_point(
            [0.0, 0.0, 0.0],
            Some([-1.0, 0.0, 0.0]),
            Some([1.0, 0.0, 0.0]),
            2.0,
            0.0,
        )];
        // The SetAnchorCorner arm's mutation: corner only.
        points[0].corner = CornerKind::from_code(1.0);
        let back = json_to_route_points(&route_points_to_json(&points));
        assert_eq!(back[0].corner, CornerKind::Smooth);
        assert_eq!(
            back[0].handle_out,
            Some([1.0, 0.0, 0.0]),
            "handles untouched"
        );
    }

    #[test]
    fn move_point_rides_relative_handles() {
        // The MovePoint arm rewrites position ONLY — relative handles, corner
        // and smoothness survive the mutation + persist round-trip (FR-7).
        let mut points = vec![smooth_anchor_point(
            [1.0, 0.0, 1.0],
            Some([-0.5, 0.0, 0.0]),
            Some([0.5, 0.0, 0.0]),
            2.0,
            0.5,
        )];
        points[0].position = [9.0, 0.0, 9.0];
        let back = json_to_route_points(&route_points_to_json(&points));
        assert_eq!(back[0].position, [9.0, 0.0, 9.0]);
        assert_eq!(back[0].handle_in, Some([-0.5, 0.0, 0.0]));
        assert_eq!(back[0].handle_out, Some([0.5, 0.0, 0.0]));
        assert_eq!(back[0].corner, CornerKind::Symmetric);
        assert!((back[0].smoothness - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn push_append_read_handles_both_variants() {
        // The unified deferred-append drain applies plain and smooth reads alike.
        let mut points = Vec::new();
        push_append_read(
            &mut points,
            PendingPathRead::AppendPoint {
                position: [1.0, 0.0, 1.0],
                time_seconds: None,
            },
        );
        push_append_read(
            &mut points,
            PendingPathRead::AppendSmoothPoint {
                position: [2.0, 0.0, 2.0],
                handle_in: None,
                handle_out: Some([0.5, 0.0, 0.0]),
                corner_code: 0.0,
                smoothness: 0.0,
            },
        );
        assert_eq!(points.len(), 2);
        assert!(points[0].is_plain_corner());
        assert_eq!(points[1].handle_out, Some([0.5, 0.0, 0.0]));
        assert_eq!(points[1].corner, CornerKind::Corner);
        // Non-append reads are a no-op (callers match them separately).
        push_append_read(&mut points, PendingPathRead::RemovePoint { index: 0 });
        assert_eq!(points.len(), 2);
    }

    // Reply-count deficit repro (pen_curve review): AppendPoint sends ONE seed
    // read; SetAnchor*/MovePoint each send their own read; a later append is
    // seed-deduped (no read). Queue [Append, Set*, AppendSmooth] therefore gets
    // TWO replies for THREE entries — reply 1 must hoist the trailing append
    // past the in-place entry or it is stranded forever (silent point loss).
    #[test]
    fn deferred_drain_rescues_append_stranded_behind_set_corner() {
        let mut queue: VecDeque<PendingPathRead> = VecDeque::new();
        queue.push_back(PendingPathRead::AppendPoint {
            position: [1.0, 0.0, 1.0],
            time_seconds: None,
        });
        queue.push_back(PendingPathRead::SetAnchorCorner {
            index: 0,
            corner_code: 1.0,
        });
        queue.push_back(PendingPathRead::AppendSmoothPoint {
            position: [2.0, 0.0, 2.0],
            handle_in: Some([-0.5, 0.0, 0.0]),
            handle_out: Some([0.5, 0.0, 0.0]),
            corner_code: 2.0,
            smoothness: 0.5,
        });

        // Empty DB base; reply 1 pops the seed append then drains the queue.
        let mut points: Vec<TimestampedRoutePoint> = Vec::new();
        push_append_read(&mut points, queue.pop_front().unwrap());
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 1);
        assert_eq!(points.len(), 2, "trailing append hoisted past the Set");

        // Reply 2 pops the Set and applies it against the seeded buffer.
        match queue.pop_front().unwrap() {
            PendingPathRead::SetAnchorCorner { index, corner_code } => {
                points[index].corner = CornerKind::from_code(corner_code);
            }
            _ => panic!("expected SetAnchorCorner at the front"),
        }
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 0);
        assert!(queue.is_empty(), "no stranded reads after the last reply");

        // All three ops applied, in issue order.
        assert_eq!(points[0].position, [1.0, 0.0, 1.0]);
        assert_eq!(points[0].corner, CornerKind::Smooth);
        assert_eq!(points[1].position, [2.0, 0.0, 2.0]);
        assert_eq!(points[1].handle_out, Some([0.5, 0.0, 0.0]));
        // ...and the full list persists in order through the wire format.
        let back = json_to_route_points(&route_points_to_json(&points));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].position, [1.0, 0.0, 1.0]);
        assert_eq!(back[1].position, [2.0, 0.0, 2.0]);
        assert_eq!(back[1].corner, CornerKind::Symmetric);
    }

    #[test]
    fn deferred_drain_rescues_append_stranded_behind_move_point() {
        // Same deficit shape with MovePoint as the middle op (the pre-Phase-3
        // reachable variant).
        let mut queue: VecDeque<PendingPathRead> = VecDeque::new();
        queue.push_back(PendingPathRead::AppendPoint {
            position: [1.0, 0.0, 1.0],
            time_seconds: None,
        });
        queue.push_back(PendingPathRead::MovePoint {
            index: 0,
            position: [9.0, 0.0, 9.0],
        });
        queue.push_back(PendingPathRead::AppendSmoothPoint {
            position: [2.0, 0.0, 2.0],
            handle_in: None,
            handle_out: Some([0.5, 0.0, 0.0]),
            corner_code: 1.0,
            smoothness: 0.0,
        });

        let mut points: Vec<TimestampedRoutePoint> = Vec::new();
        push_append_read(&mut points, queue.pop_front().unwrap());
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 1);
        assert_eq!(points.len(), 2);

        match queue.pop_front().unwrap() {
            PendingPathRead::MovePoint { index, position } => {
                points[index].position =
                    [position[0] as f64, position[1] as f64, position[2] as f64];
            }
            _ => panic!("expected MovePoint at the front"),
        }
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 0);
        assert!(queue.is_empty());

        // Move hit the FIRST append (index 0), not the hoisted tail.
        assert_eq!(points[0].position, [9.0, 0.0, 9.0]);
        assert_eq!(points[1].position, [2.0, 0.0, 2.0]);
        let back = json_to_route_points(&route_points_to_json(&points));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].position, [9.0, 0.0, 9.0]);
        assert_eq!(back[1].position, [2.0, 0.0, 2.0]);
    }

    #[test]
    fn drain_stops_at_remove_point_and_resumes_on_its_own_reply() {
        // RemovePoint is index-shifting, so appends must NOT hoist past it —
        // its own (never seed-deduped) read reply applies it, then resumes.
        let mut queue: VecDeque<PendingPathRead> = VecDeque::new();
        queue.push_back(PendingPathRead::AppendPoint {
            position: [1.0, 0.0, 0.0],
            time_seconds: None,
        });
        queue.push_back(PendingPathRead::RemovePoint { index: 0 });
        queue.push_back(PendingPathRead::AppendSmoothPoint {
            position: [3.0, 0.0, 0.0],
            handle_in: None,
            handle_out: None,
            corner_code: 0.0,
            smoothness: 0.0,
        });

        // DB base has one pre-existing point (the Remove's target).
        let mut points = vec![TimestampedRoutePoint {
            position: [0.0, 0.0, 0.0],
            ..Default::default()
        }];
        // Reply 1 (seed read): pops the append; drain halts at the Remove.
        push_append_read(&mut points, queue.pop_front().unwrap());
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 0);
        assert_eq!(queue.len(), 2, "Remove + trailing append still queued");
        assert_eq!(points.len(), 2);

        // Reply 2 (the Remove's own read): pops it, applies, resumes the drain.
        match queue.pop_front().unwrap() {
            PendingPathRead::RemovePoint { index } => {
                assert!(index < points.len());
                points.remove(index);
            }
            _ => panic!("expected RemovePoint at the front"),
        }
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 1);
        assert!(queue.is_empty());
        assert_eq!(points.len(), 2);
        assert_eq!(
            points[0].position,
            [1.0, 0.0, 0.0],
            "base removed, first append kept"
        );
        assert_eq!(points[1].position, [3.0, 0.0, 0.0]);
    }

    #[test]
    fn drain_queued_appends_hoists_past_in_place_ops_preserving_their_order() {
        let mut queue: VecDeque<PendingPathRead> = VecDeque::new();
        queue.push_back(PendingPathRead::SetAnchorHandles {
            index: 0,
            handle_in: None,
            handle_out: Some([1.0, 0.0, 0.0]),
            smoothness: 0.0,
        });
        queue.push_back(PendingPathRead::AppendPoint {
            position: [1.0, 0.0, 0.0],
            time_seconds: None,
        });
        queue.push_back(PendingPathRead::AnnotatePoint {
            index: 0,
            title: "t".into(),
            body: "b".into(),
            color: "#ffffffff".into(),
        });
        queue.push_back(PendingPathRead::AppendSmoothPoint {
            position: [2.0, 0.0, 0.0],
            handle_in: None,
            handle_out: None,
            corner_code: 0.0,
            smoothness: 0.0,
        });

        let mut points = vec![TimestampedRoutePoint::default()];
        assert_eq!(drain_queued_appends(&mut queue, &mut points), 2);
        // Both appends extracted in queue order; in-place entries kept in order.
        assert_eq!(points.len(), 3);
        assert_eq!(points[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(points[2].position, [2.0, 0.0, 0.0]);
        assert_eq!(queue.len(), 2);
        assert!(matches!(
            queue.pop_front(),
            Some(PendingPathRead::SetAnchorHandles { .. })
        ));
        assert!(matches!(
            queue.pop_front(),
            Some(PendingPathRead::AnnotatePoint { .. })
        ));
    }

    #[test]
    fn first_seen_true_once_per_id_per_frame() {
        let mut seen = std::collections::HashSet::new();
        assert!(first_seen(&mut seen, "track-a"));
        assert!(
            !first_seen(&mut seen, "track-a"),
            "same-batch duplicate skipped"
        );
        assert!(
            first_seen(&mut seen, "track-b"),
            "different track unaffected"
        );
        assert!(!first_seen(&mut seen, "track-b"));
    }

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
    fn materialization_kind_thresholds() {
        // FR-3: 0 → nothing, 1 → node, ≥2 → line.
        assert_eq!(materialization_kind(0), MaterializationKind::None);
        assert_eq!(materialization_kind(1), MaterializationKind::Node);
        assert_eq!(materialization_kind(2), MaterializationKind::Line);
        assert_eq!(materialization_kind(50), MaterializationKind::Line);
    }

    #[test]
    fn style_from_properties_defaults_when_absent() {
        // track_styling_20260713 FR-4: no style props → the historic default.
        let props = serde_json::json!({ "gpx_type": "track" });
        let style = style_from_properties(&props);
        assert_eq!(style, TrackStyle::default());
    }

    #[test]
    fn delete_enqueue_reports_channel_failure_without_false_success() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        enqueue_track_delete(&sender, "track-1").expect("connected channel");
        assert!(matches!(
            rx.try_recv().expect("delete command"),
            DbCommand::DeleteNode { node_id } if node_id == "track-1"
        ));

        let (tx, rx) = crossbeam::channel::unbounded();
        drop(rx);
        let sender = DbCommandSender(tx);
        assert!(enqueue_track_delete(&sender, "track-2").is_err());
    }

    #[test]
    fn delete_projection_match_covers_line_and_single_point_cleanup() {
        assert!(projection_matches_track("track-1", "track-1"));
        assert!(!projection_matches_track("track-1", "track-2"));
    }

    #[test]
    fn style_from_properties_reads_zone_keys() {
        let props = serde_json::json!({
            "gis.track.color": "#ff0000ff",
            "gis.track.width": 5.5,
            "gis.track.visible": false,
            "gis.track.closed": true,
            "gis.track.fill.color": "#33669980",
        });
        let style = style_from_properties(&props);
        assert_eq!(style.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(style.width, 5.5);
        assert!(!style.visible);
        assert!(style.closed);
        assert_eq!(
            style.fill_color,
            [
                0x33 as f32 / 255.0,
                0x66 as f32 / 255.0,
                0x99 as f32 / 255.0,
                0x80 as f32 / 255.0,
            ]
        );
    }

    #[test]
    fn style_from_properties_invalid_fields_fall_back_per_field() {
        // FR-4: a bad color / non-positive width / wrong-typed visible each
        // fall back to that field's default without dropping the good ones.
        let props = serde_json::json!({
            "gis.track.color": "not-a-color",
            "gis.track.width": -3.0,
            "gis.track.visible": "yes",
        });
        let style = style_from_properties(&props);
        let d = TrackStyle::default();
        assert_eq!(style.color, d.color);
        assert_eq!(style.width, d.width);
        assert_eq!(style.visible, d.visible);
    }

    #[test]
    fn style_from_properties_caps_f64_width_before_f32_conversion() {
        let style = style_from_properties(&serde_json::json!({
            "gis.track.width": 1.0e100
        }));
        assert_eq!(style.width, 20.0);
        assert!(style.width.is_finite());
        let tiny = style_from_properties(&serde_json::json!({
            "gis.track.width": 1.0e-100
        }));
        assert_eq!(tiny.width, 0.1);
    }

    #[test]
    fn materialization_kind_covers_every_high1_transition() {
        // HIGH-1: the four live-edit transitions `reconcile_track_render` must
        // handle. The spawn/despawn itself needs `Commands` (not unit-testable),
        // but the pure kind decision at each end of a transition is — assert the
        // (before, after) kind pairs so a threshold regression is caught here.
        let kind = materialization_kind;
        // 0→1: None → Node (spawn the single-point node live).
        assert_eq!(
            (kind(0), kind(1)),
            (MaterializationKind::None, MaterializationKind::Node)
        );
        // 1→0: Node → None (despawn the node).
        assert_eq!(
            (kind(1), kind(0)),
            (MaterializationKind::Node, MaterializationKind::None)
        );
        // 1→2: Node → Line (despawn the stale node, spawn the polyline — no leak).
        assert_eq!(
            (kind(1), kind(2)),
            (MaterializationKind::Node, MaterializationKind::Line)
        );
        // 2→1: Line → Node (tear the line down, spawn the node — track stays visible).
        assert_eq!(
            (kind(2), kind(1)),
            (MaterializationKind::Line, MaterializationKind::Node)
        );
    }

    #[test]
    fn track_draft_positions_at_first_trackpoint_with_matching_origin() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        // Origin == first trackpoint exactly, so the projected position is ~[0,0,0].
        let projection = Projection::new(45.0, -122.0, 100.0);

        let draft = build_track_draft(&data, &projection, &stats).expect("track present");
        assert_eq!(draft.name, "Test Hike");
        assert!(
            draft.position[0].abs() < 1e-3,
            "x should be ~0: {:?}",
            draft.position
        );
        assert!(
            draft.position[1].abs() < 1e-3,
            "y should be ~0: {:?}",
            draft.position
        );
        assert!(
            draft.position[2].abs() < 1e-3,
            "z should be ~0: {:?}",
            draft.position
        );
    }

    #[test]
    fn track_draft_carries_gpx_type_and_all_cached_stat_keys() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let projection = Projection::new(45.0, -122.0, 100.0);
        let draft = build_track_draft(&data, &projection, &stats).unwrap();

        assert_eq!(prop(&draft, "gpx_type"), &serde_json::json!("track"));
        assert_eq!(prop(&draft, "path_kind"), &serde_json::json!("recorded"));
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
    fn authored_track_properties_are_designed() {
        let properties = authored_track_properties("Main Road");
        let find = |key: &str| &properties.iter().find(|(k, _)| *k == key).unwrap().1;
        assert_eq!(find("gpx_type"), &serde_json::json!("track"));
        assert_eq!(find("path_kind"), &serde_json::json!("designed"));
        assert_eq!(find("gis.track.name"), &serde_json::json!("Main Road"));
        assert_eq!(find("gpx_points"), &serde_json::json!([]));
    }

    #[test]
    fn road_track_create_echo_submits_designed_and_import_stays_recorded() {
        let (sender_inner, commands) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(sender_inner);
        let mut pending = PendingPathEdits::default();
        // Production-shaped correlation id minted by fe-ui's next_road_correlation_id.
        pending.submit_create(&sender, "road-track:1".into(), "p".into(), "Main Road".into()).unwrap();
        pending.creates.remove("road-track:1");
        assert!(submit_authored_track_properties(&mut pending, &sender, "node-1", "Main Road").is_none());
        let written: Vec<(String, serde_json::Value)> = commands
            .try_iter()
            .filter_map(|command| match command {
                DbCommand::SetNodeProperty { node_id, key, value, .. } if node_id == "node-1" => Some((key, value)),
                _ => None,
            })
            .collect();
        assert!(written.contains(&("path_kind".to_string(), serde_json::json!("designed"))));
        assert!(written.contains(&("gpx_type".to_string(), serde_json::json!("track"))));
        assert!(!written.iter().any(|(_, value)| value == &serde_json::json!("recorded")));

        let data = parse(SAMPLE_GPX);
        let draft = build_track_draft(&data, &Projection::new(45.0, -122.0, 100.0), &compute_stats(&data)).unwrap();
        assert_eq!(prop(&draft, "path_kind"), &serde_json::json!("recorded"));
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
        assert_eq!(
            prop(&drafts[0], "gis.annotation.title"),
            &serde_json::json!("Camp")
        );
    }

    #[test]
    fn waypoint_draft_falls_back_to_default_name_when_unnamed() {
        let data = parse(SAMPLE_GPX);
        let projection = Projection::new(45.0, -122.0, 100.0);
        let drafts = build_waypoint_drafts(&data, &projection);

        // Second <wpt> in the fixture has no <name>.
        assert_eq!(drafts[1].name, "Waypoint 2");
        assert_eq!(
            prop(&drafts[1], "gis.annotation.title"),
            &serde_json::json!("Waypoint 2")
        );
    }

    #[test]
    fn resolve_projection_prefers_active_terrain_when_petal_matches() {
        let data = parse(SAMPLE_GPX);
        let stats = compute_stats(&data);
        let expected_origin = Projection::new(1.0, 2.0, 3.0);
        let active = ActivePetalTerrain {
            raw_source_fingerprint: None,
            assignment_fingerprint: None,
            input_scale_valid: false,
            petal_id: Some("petal-1".to_string()),
            config: Some(TerrainConfig {
                origin: expected_origin.clone(),
                ..TerrainConfig::default()
            }),
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
            raw_source_fingerprint: None,
            assignment_fingerprint: None,
            input_scale_valid: false,
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

        let prepared =
            prepare_import("petal-1", &path, &active).expect("prepare_import should succeed");
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

        let prepared =
            prepare_import("petal-1", &path, &active).expect("prepare_import should succeed");
        assert!(prepared.track.is_none());
        assert_eq!(prepared.standalone_waypoints.len(), 1);
        assert_eq!(prepared.standalone_waypoints[0].name, "Lone Marker");
    }
}
