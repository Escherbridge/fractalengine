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
use fe_terrain::mesh::track::track_centroid;
use fe_terrain::petal_binding::ActivePetalTerrain;
use fe_terrain::projection::Projection;
use fe_terrain::terrain_plugin::GpxTrackLine;
use fe_terrain::ExportNode;
use fe_ui::gpx_ops::{GpxImportStatus, GpxOp, PendingGpxOps};
use fe_ui::node_manager::TrackPickShape;
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
/// Per-track style properties (track_styling_20260713): a hex color string
/// (`#rrggbbaa`), a numeric ribbon width (meters), and a bool visibility. Read
/// into `TrackStyleMap` by `advance_path_materialization`; written by the Paths
/// tab via `SetNodeProperty`. Absent/invalid → `TrackStyle::default()` (FR-4).
const TRACK_COLOR_KEY: &str = "gis.track.color";
const TRACK_WIDTH_KEY: &str = "gis.track.width";
const TRACK_VISIBLE_KEY: &str = "gis.track.visible";

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
            style.width = width as f32;
        }
    }
    if let Some(visible) = properties.get(TRACK_VISIBLE_KEY).and_then(|v| v.as_bool()) {
        style.visible = visible;
    }
    style
}

/// FR-4: process-unique id for the authored-`CreateTrack` path so its
/// `NodeCreated` result is disambiguated by an explicit correlation id rather
/// than the ambiguous `(petal_id, name)` tuple the GPX-import path also uses —
/// which could otherwise let a same-named import steal the authored result.
/// See `src/AGENTS.md` §path-editor.
fn next_authored_track_correlation_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("authored-track:{}", COUNTER.fetch_add(1, Ordering::Relaxed))
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

/// Read a 3-slot handle offset starting at `base`; `None` if any slot is absent
/// or the whole vector is ~zero (zero == absent, keeps legacy 4-slot rows -> None).
fn read_handle(a: &[serde_json::Value], base: usize) -> Option<[f64; 3]> {
    let x = a.get(base)?.as_f64()?;
    let y = a.get(base + 1)?.as_f64()?;
    let z = a.get(base + 2)?.as_f64()?;
    if x.abs() < f64::EPSILON && y.abs() < f64::EPSILON && z.abs() < f64::EPSILON {
        None
    } else {
        Some([x, y, z])
    }
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
                    let handle_in = read_handle(a, 4);
                    let handle_out = read_handle(a, 7);
                    let mut corner =
                        CornerKind::from_code(a.get(10).and_then(|v| v.as_f64()).unwrap_or(0.0));
                    let smoothness = a.get(11).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    // Open Q6: a claimed-smooth anchor with no handles is malformed.
                    if corner != CornerKind::Corner && handle_in.is_none() && handle_out.is_none() {
                        corner = CornerKind::Corner;
                    }
                    Some(TimestampedRoutePoint {
                        position: [x, y, z],
                        time_seconds: t,
                        handle_in,
                        handle_out,
                        corner,
                        smoothness,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
            if node.is_none() {
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

/// Make rendered track ribbons viewport-selectable by tagging them with
/// `fe_ui::plugin::SpawnedNodeMarker` (the component the viewport picker
/// iterates). `render_gpx_tracks` (fe-terrain) can't attach it — fe-terrain
/// must not depend on fe-ui — so this crate, which sees both `GpxTrackLine`
/// and `SpawnedNodeMarker`, tags them here. Runs the frame after the ribbon
/// mesh appears; `Without<SpawnedNodeMarker>` makes it idempotent. See
/// `src/AGENTS.md` §gpx track-picking.
/// Ribbon-mesh track lines not yet tagged selectable.
type UntaggedTrackLineQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static GpxTrackLine),
    (With<Mesh3d>, Without<fe_ui::plugin::SpawnedNodeMarker>),
>;

pub fn tag_track_lines_selectable(
    lines: UntaggedTrackLineQuery,
    active_terrain: Res<ActivePetalTerrain>,
    route_map: Res<TrackRouteMap>,
    style_map: Res<TrackStyleMap>,
    mut commands: Commands,
) {
    if lines.is_empty() {
        return;
    }
    let petal_id = active_terrain.petal_id.clone().unwrap_or_default();
    for (entity, line) in lines.iter() {
        // try_insert: track lines can be despawned (petal switch / track
        // re-render) between this system and command flush; replacements
        // re-tag next frame via the Without filter.
        let mut e = commands.entity(entity);
        e.try_insert(fe_ui::plugin::SpawnedNodeMarker {
            node_id: line.track_node_id.clone(),
            petal_id: petal_id.clone(),
        });
        // path_interaction_20260716 (FR-1/FR-4): attach the precise pick
        // polyline so `handle_viewport_click` ray-tests the actual ribbon (not a
        // km-scale flat AABB) AND the whole-path gimbal can bake about the SAME
        // centroid `render_gpx_tracks` anchored the mesh at. Built from the
        // identical filtered positions the render uses so the baseline matches.
        if let Some(shape) = track_pick_shape(&route_map, &style_map, &line.track_node_id) {
            e.try_insert(shape);
        }
    }
}

/// FR-1/FR-4: build the [`TrackPickShape`] for `track_node_id` from its current
/// route + style, or `None` if it has no ≥2-point route yet. Filters non-finite
/// points exactly like `render_gpx_tracks` so `centroid` equals the render
/// entity's baseline `Transform` translation.
fn track_pick_shape(
    route_map: &TrackRouteMap,
    style_map: &TrackStyleMap,
    track_node_id: &str,
) -> Option<TrackPickShape> {
    let route = route_map.routes.get(track_node_id)?;
    // pen_curve_tool_20260722 (Phase 2): flatten the SAME way render_gpx_tracks does
    // so the pick polyline matches the visible curve.
    let positions: Vec<[f32; 3]> = fe_terrain::mesh::curve::flatten_route(
        &route.points,
        fe_terrain::mesh::curve::SAMPLES_PER_SEGMENT,
    )
    .into_iter()
    .filter(|v| v[0].is_finite() && v[1].is_finite() && v[2].is_finite())
    .collect();
    if positions.len() < 2 {
        return None;
    }
    let centroid = track_centroid(&positions);
    let half_width = style_map.style_for(track_node_id).width.max(0.01) / 2.0;
    Some(TrackPickShape {
        points: positions
            .iter()
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect(),
        half_width,
        centroid: Vec3::new(centroid[0], centroid[1], centroid[2]),
    })
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
                    + prepared
                        .track
                        .as_ref()
                        .map(|t| t.waypoints.len())
                        .unwrap_or(0)) as u32;

                if let Some(track) = prepared.track {
                    let key = (petal_id.clone(), track.draft.name.clone());
                    db_tx
                        .0
                        .send(DbCommand::CreateNode {
                            petal_id: petal_id.clone(),
                            name: track.draft.name.clone(),
                            position: track.draft.position,
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .tracks
                        .entry(key)
                        .or_default()
                        .push_back(PendingTrackImport {
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
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .waypoints
                        .entry(key)
                        .or_default()
                        .push_back(PendingWaypointImport {
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
        let DbResult::NodeCreated {
            id,
            petal_id,
            name,
            correlation_id,
            ..
        } = result
        else {
            continue;
        };
        // FR-4: the GPX-import path only owns results with no correlation id.
        // A `Some(_)` id belongs to the authored-`CreateTrack` path
        // (`advance_path_edits`), which matches by that id — skip so a
        // same-named authored track can't steal this system's `NodeCreated`.
        if correlation_id.is_some() {
            continue;
        }
        let key = (petal_id.clone(), name.clone());

        if let Some(queue) = pending.tracks.get_mut(&key) {
            if let Some(track) = queue.pop_front() {
                if queue.is_empty() {
                    pending.tracks.remove(&key);
                }
                for (prop_key, value) in track.stats_props {
                    db_tx
                        .0
                        .send(DbCommand::SetNodeProperty {
                            node_id: id.clone(),
                            key: prop_key,
                            value,
                        })
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
                            correlation_id: None,
                        })
                        .ok();
                    pending
                        .waypoints
                        .entry(wp_key)
                        .or_default()
                        .push_back(PendingWaypointImport {
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
                    spawn_track_route(&mut route_map, &mut commands, id, track.route_points, None);
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
        save_path: std::path::PathBuf,
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

/// A `CreateTrack` node's `CreateNode` was sent; once `NodeCreated` arrives,
/// mark it as an (empty) track. No extra fields needed — the properties to
/// set are fixed (`gpx_type = "track"`, `gpx_points = []`).
struct PendingTrackCreate;

/// FR-4: tracks which of a just-created track's identity `SetNodeProperty`
/// keys have confirmed via `DbResult::NodePropertySet`. `PathEditStatus` is
/// only flipped to success once this set is empty — replaces the old
/// optimistic "success on NodeCreated" behavior (gpx_bridge.rs:714 prior to
/// this track) that could report success even if a property write silently
/// failed downstream.
type PendingPropertyConfirms = std::collections::HashSet<&'static str>;

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
    /// FR-4: authored-`CreateTrack` waiters keyed by a process-unique
    /// correlation id (not `(petal_id, name)`) so a same-named GPX import in
    /// the same window can never steal this path's `NodeCreated` — the tuple
    /// was the ambiguity that FR-4 closes. One waiter per id (unique ⇒ no FIFO).
    creates: HashMap<String, PendingTrackCreate>,
    annotate_creates: HashMap<(String, String), VecDeque<PendingAnnotateCreate>>,
    /// FR-4: node_id -> still-unconfirmed property keys for a just-created track.
    track_property_confirms: HashMap<String, PendingPropertyConfirms>,
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
    active_terrain: Res<ActivePetalTerrain>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    single_nodes: Query<(Entity, &SinglePointTrackNode)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut status: ResMut<PathEditStatus>,
    mut commands: Commands,
) {
    if ops.0.is_empty() {
        return;
    }
    let active_petal_id = active_terrain.petal_id.clone().unwrap_or_default();

    for op in ops.0.drain(..) {
        match op {
            PathOp::CreateTrack {
                petal_id,
                name,
                correlation_id,
            } => {
                // FR-4: tag this authored create with a process-unique id and
                // key the waiter by it, so `advance_path_edits` matches the
                // eventual `NodeCreated` by id — not the `(petal_id, name)`
                // tuple a concurrent same-named GPX import also uses.
                //
                // HIGH-1/HIGH-2 (pen_autocreate correlation-id fix): reuse the
                // fe-ui-supplied id when present (the Pen auto-create needs the
                // SAME id echoed back so fe-ui's deferred flush can match it);
                // only the manual "New Path" button leaves it `None`, for which
                // we generate a bridge-side id exactly as before.
                let correlation_id =
                    correlation_id.unwrap_or_else(next_authored_track_correlation_id);
                db_tx
                    .0
                    .send(DbCommand::CreateNode {
                        petal_id: petal_id.clone(),
                        name: name.clone(),
                        position: [0.0; 3],
                        correlation_id: Some(correlation_id.clone()),
                    })
                    .ok();
                pending.creates.insert(correlation_id, PendingTrackCreate);
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
                db_tx
                    .0
                    .send(DbCommand::DeleteNode {
                        node_id: track_node_id.clone(),
                    })
                    .ok();
                pending.in_flight_points.remove(&track_node_id);
                pending.seed_pending.remove(&track_node_id);
                route_map.routes.remove(&track_node_id);
                // Despawn ALL matching lines — a leaked duplicate ribbon must
                // not survive a single-`.find()` teardown.
                for (entity, _) in track_lines
                    .iter()
                    .filter(|(_, t)| t.track_node_id == track_node_id)
                {
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
                let Some(dialog_path) = prompt_gpx_save_path(&track_node_id) else {
                    tracing::debug!(track_node_id = %track_node_id, "GPX export cancelled by user");
                    continue;
                };
                db_tx
                    .0
                    .send(DbCommand::GetNodeProperties {
                        node_id: track_node_id.clone(),
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
) {
    if pending.reads.is_empty()
        && pending.creates.is_empty()
        && pending.annotate_creates.is_empty()
        && pending.track_property_confirms.is_empty()
    {
        return;
    }
    let active_petal_id = active_terrain.petal_id.clone().unwrap_or_default();

    for result in db_results.read() {
        match result {
            DbResult::NodeCreated {
                id,
                petal_id,
                name,
                correlation_id,
                ..
            } => {
                // FR-4: authored `CreateTrack` results carry the unique id set
                // in `drain_path_ops`; match them by that id so a same-named
                // GPX import can't consume this result (and vice versa). The
                // annotate-waypoint path below still uses `(petal_id, name)`
                // but only for results with no correlation id.
                if let Some(cid) = correlation_id {
                    let Some(_create) = pending.creates.remove(cid) else {
                        continue;
                    };
                    // FR-4: don't report success yet — wait for all three
                    // SetNodeProperty confirms (NodePropertySet) below before
                    // flipping PathEditStatus, so a silently-failed property
                    // write surfaces as an error instead of a false "created".
                    pending.track_property_confirms.insert(
                        id.clone(),
                        [GPX_TYPE_KEY, GPX_POINTS_KEY, TRACK_NAME_KEY]
                            .into_iter()
                            .collect(),
                    );
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
                    // Status is set once all three property confirms land — see
                    // the `DbResult::NodePropertySet` arm below.
                    continue;
                }

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

                // Non-authored (`correlation_id: None`) results with no
                // matching annotate waiter fall through — the import path
                // (`advance_gpx_imports`) or materialization handles them.
            }
            DbResult::NodePropertySet { node_id, key } => {
                // FR-4: gate CreateTrack's success status on the actual
                // property confirms instead of the optimistic set that used
                // to happen on NodeCreated above.
                if let Some(pending_keys) = pending.track_property_confirms.get_mut(node_id) {
                    pending_keys.remove(key.as_str());
                    if pending_keys.is_empty() {
                        pending.track_property_confirms.remove(node_id);
                        *status = PathEditStatus {
                            track_node_id: Some(node_id.clone()),
                            message: Some("Track created".to_string()),
                            error: None,
                        };
                    }
                }
            }
            DbResult::NodePropertiesLoaded {
                node_id,
                properties,
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
                        let key = (petal_id.clone(), title.clone());
                        db_tx
                            .0
                            .send(DbCommand::CreateNode {
                                petal_id,
                                name: title.clone(),
                                position,
                                correlation_id: None,
                            })
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
            DbResult::Error(msg) if msg.starts_with("Set node property failed") => {
                // FR-4: surface the entity_property.rs "matched no node"
                // failure (or any other SetNodeProperty error) against
                // whichever track is still awaiting its property confirms —
                // `Error` carries no node_id to correlate precisely, so this
                // is a best-effort match consistent with the existing
                // untagged-error routing idiom in db_results.rs.
                if let Some((node_id, _)) = pending.track_property_confirms.iter().next() {
                    let node_id = node_id.clone();
                    pending.track_property_confirms.remove(&node_id);
                    *status = PathEditStatus {
                        track_node_id: Some(node_id),
                        message: None,
                        error: Some(msg.clone()),
                    };
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
    db_tx
        .0
        .send(DbCommand::SetNodeProperty {
            node_id: track_node_id.to_string(),
            key: GPX_POINTS_KEY.to_string(),
            value: route_points_to_json(&points),
        })
        .ok();
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

/// FR-4: on petal load/switch, request `GetNodeProperties` for every node in
/// the newly-active petal so any track carrying `gpx_points` can be
/// re-materialized by `advance_path_materialization`. Cheap no-op for petals
/// with no GPX tracks (unmatched results are just ignored downstream).
pub fn request_petal_gpx_materialization(
    active_terrain: Res<ActivePetalTerrain>,
    verse_mgr: Res<VerseManager>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingPathEdits>,
    mut last_revision: Local<u64>,
) {
    if !active_terrain.is_changed() || active_terrain.revision == *last_revision {
        return;
    }
    *last_revision = active_terrain.revision;
    // FR-5/MEDIUM-2: drop stale in-flight append buffers on petal switch /
    // reload. A buffer seeded against the previous petal's DB base (or before
    // a P2P-sync remote write) must not fast-path a later append — clearing
    // here forces the next append to re-seed fresh, and prevents unbounded
    // growth across petal switches.
    pending.in_flight_points.clear();
    pending.seed_pending.clear();
    let Some(petal_id) = &active_terrain.petal_id else {
        return;
    };
    let Some(petal) = verse_mgr.find_petal(petal_id) else {
        return;
    };

    for node in &petal.nodes {
        db_tx
            .0
            .send(DbCommand::GetNodeProperties {
                node_id: node.id.clone(),
            })
            .ok();
    }
}

/// `true` only the first time `id` is seen this frame — the same-batch
/// duplicate-`NodePropertiesLoaded` guard. Pure so it is unit-testable.
fn first_seen(seen: &mut std::collections::HashSet<String>, id: &str) -> bool {
    seen.insert(id.to_string())
}

/// FR-6: the single projection path from DB events to render state
/// (`TrackRouteMap` + `GpxTrackLine`). Treats the DB node row as the sole
/// source of truth: any `NodePropertiesLoaded` carrying a track's current
/// `gpx_points` (re)materializes its render entity, and any `NodeDeleted`
/// tears it down — covering both the petal-load batch from
/// `request_petal_gpx_materialization` and live edits/deletes from the
/// path editor and GIS panel, through one system instead of per-op
/// hand-updates of a subset of sinks.
#[allow(clippy::too_many_arguments)]
pub fn advance_path_materialization(
    mut db_results: MessageReader<DbResult>,
    db_tx: Res<DbCommandSender>,
    mut route_map: ResMut<TrackRouteMap>,
    mut style_map: ResMut<TrackStyleMap>,
    active_terrain: Res<ActivePetalTerrain>,
    track_lines: Query<(Entity, &GpxTrackLine)>,
    single_nodes: Query<(Entity, &SinglePointTrackNode)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    // Per-frame dedupe: petal entry can queue TWO GetNodeProperties for the
    // same track (petal materialization + fe-ui asset-less fallback); if both
    // results land in one batch, the second reconcile can't see the first's
    // deferred spawn and would duplicate the ribbon. First occurrence wins.
    let mut reconciled: std::collections::HashSet<String> = std::collections::HashSet::new();
    for result in db_results.read() {
        match result {
            DbResult::NodePropertiesLoaded {
                node_id,
                properties,
            } => {
                let is_track =
                    properties.get(GPX_TYPE_KEY).and_then(|v| v.as_str()) == Some("track");
                if !is_track {
                    continue;
                }
                if !first_seen(&mut reconciled, node_id) {
                    continue;
                }
                // track_styling_20260713: refresh this track's style from its
                // `gis.track.*` props (FR-3/FR-4: default when absent/invalid)
                // BEFORE reconciling the render entity, so a fresh spawn reads
                // the current style. If the style actually changed for an
                // already-rendered line, despawn it so the reconcile below
                // respawns it `Without<Mesh3d>` and `render_gpx_tracks` rebuilds
                // the ribbon with the new color/width (its build is gated on
                // `Without<Mesh3d>`, mirroring the point-edit force-redraw).
                let new_style = style_from_properties(properties);
                let style_changed = style_map.styles.get(node_id).copied() != Some(new_style);
                style_map.styles.insert(node_id.clone(), new_style);
                let mut force_line_redraw = false;
                if style_changed {
                    if let Some((entity, _)) = track_lines
                        .iter()
                        .find(|(_, t)| t.track_node_id == *node_id)
                    {
                        commands.entity(entity).despawn();
                        // The line no longer exists for the reconcile below, so
                        // it will respawn from scratch — force it even though
                        // petal-load normally passes `false`.
                        force_line_redraw = true;
                    }
                }
                let Some(points_json) = properties.get(GPX_POINTS_KEY) else {
                    continue;
                };
                let points = json_to_route_points(points_json);
                // FR-3: reconcile the render entity against the point count via
                // the SAME helper the live-edit path uses, so editing 1↔2↔1
                // never leaks entities regardless of which path fired. Petal-load
                // runs once per batch, so a matching existing line is left alone
                // (`force_line_redraw = false`) unless the style just changed.
                // Best-effort petal id mirrors AnnotatePoint / resolve_projection.
                let petal_id = active_terrain.petal_id.clone().unwrap_or_default();
                reconcile_track_render(
                    &mut route_map,
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &track_lines,
                    &single_nodes,
                    &petal_id,
                    node_id,
                    points,
                    force_line_redraw,
                );
            }
            DbResult::NodePropertySet { node_id, key } => {
                // track_styling_20260713: a live style edit lands as
                // `NodePropertySet` (no property bag) — re-read the node so the
                // `NodePropertiesLoaded` arm above refreshes `TrackStyleMap` and
                // forces the ribbon rebuild. Only for the three style keys so an
                // unrelated property write doesn't trigger a spurious round trip.
                if key == TRACK_COLOR_KEY || key == TRACK_WIDTH_KEY || key == TRACK_VISIBLE_KEY {
                    db_tx
                        .0
                        .send(DbCommand::GetNodeProperties {
                            node_id: node_id.clone(),
                        })
                        .ok();
                }
            }
            DbResult::NodeDeleted { node_id, .. } => {
                // Projection teardown: despawn EVERY rendered line and
                // single-point node for the deleted id (a leaked duplicate must
                // not survive a `.find()`-single teardown), covering deletes
                // that bypass PathOp::DeleteTrack's own optimistic cleanup.
                route_map.routes.remove(node_id);
                style_map.styles.remove(node_id);
                for (entity, _) in track_lines
                    .iter()
                    .filter(|(_, t)| t.track_node_id == *node_id)
                {
                    commands.entity(entity).despawn();
                }
                for (entity, _) in single_nodes
                    .iter()
                    .filter(|(_, t)| t.track_node_id == *node_id)
                {
                    commands.entity(entity).despawn();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_terrain::config::TerrainConfig;
    use std::path::PathBuf;

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
    fn style_from_properties_reads_all_three_keys() {
        let props = serde_json::json!({
            "gis.track.color": "#ff0000ff",
            "gis.track.width": 5.5,
            "gis.track.visible": false,
        });
        let style = style_from_properties(&props);
        assert_eq!(style.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(style.width, 5.5);
        assert!(!style.visible);
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
