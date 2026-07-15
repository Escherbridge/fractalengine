use super::parser::GpxData;
use super::stats::compute_stats;
use crate::projection::Projection;
use crate::DbCommand;

/// Convert parsed GPX data into a flat list of DbCommands for creating nodes.
///
/// Mapping:
/// - Track -> parent node {gpx_type: "track", name, stats}
/// - Segment -> child node {gpx_type: "segment"}
/// - Trackpoint -> child node {gpx_type: "trackpoint", lat, lon, ele, time, hr, cad, power}
/// - Waypoint -> standalone node {gpx_type: "waypoint", lat, lon, ele, name, desc, symbol}
pub fn gpx_to_scene_commands(
    data: &GpxData,
    petal_id: &str,
    projection: &Projection,
) -> Vec<DbCommand> {
    let mut commands = Vec::new();

    // Track hierarchy
    for (ti, track) in data.tracks.iter().enumerate() {
        let track_name = track
            .name
            .clone()
            .unwrap_or_else(|| format!("Track {}", ti + 1));

        // Compute stats for this track
        let track_data = GpxData {
            tracks: vec![track.clone()],
            routes: vec![],
            waypoints: vec![],
            metadata: None,
        };
        let stats = compute_stats(&track_data);
        let track_node_id = format!("track-{ti}");

        // Center the track node at the bounding box center
        let center_lat = (stats.bounding_box.min_lat + stats.bounding_box.max_lat) / 2.0;
        let center_lon = (stats.bounding_box.min_lon + stats.bounding_box.max_lon) / 2.0;
        let center_ele = stats.bounding_box.min_ele.unwrap_or(0.0);
        let pos = match projection.wgs84_to_local(center_lat, center_lon, center_ele) {
            Ok(p) => p,
            Err(_) => continue, // skip tracks with out-of-range coordinates
        };

        commands.push(DbCommand {
            petal_id: petal_id.to_string(),
            name: track_name,
            position: [pos[0] as f32, pos[1] as f32, pos[2] as f32],
            parent_node_id: None,
            properties: Some(serde_json::json!({
                "gpx_type": "track",
                "total_distance_m": stats.total_distance_m,
                "elevation_gain_m": stats.elevation_gain_m,
                "elevation_loss_m": stats.elevation_loss_m,
                "duration_s": stats.duration,
                "avg_speed_kmh": stats.avg_speed_kmh,
                "max_speed_kmh": stats.max_speed_kmh,
                "bounding_box": {
                    "min_lat": stats.bounding_box.min_lat,
                    "max_lat": stats.bounding_box.max_lat,
                    "min_lon": stats.bounding_box.min_lon,
                    "max_lon": stats.bounding_box.max_lon,
                }
            })),
        });

        // Segments
        for (si, segment) in track.segments.iter().enumerate() {
            let seg_node_id = format!("track-{ti}-seg-{si}");

            commands.push(DbCommand {
                petal_id: petal_id.to_string(),
                name: format!("Segment {}", si + 1),
                position: [pos[0] as f32, pos[1] as f32, pos[2] as f32],
                parent_node_id: Some(track_node_id.clone()),
                properties: Some(serde_json::json!({
                    "gpx_type": "segment",
                    "point_count": segment.points.len(),
                })),
            });

            // Trackpoints
            for (pi, point) in segment.points.iter().enumerate() {
                let local =
                    match projection.wgs84_to_local(point.lat, point.lon, point.ele.unwrap_or(0.0))
                    {
                        Ok(p) => p,
                        Err(_) => continue, // skip out-of-range trackpoints
                    };

                let mut props = serde_json::json!({
                    "gpx_type": "trackpoint",
                    "lat": point.lat,
                    "lon": point.lon,
                });

                if let Some(e) = point.ele {
                    props["ele"] = serde_json::json!(e);
                }
                if let Some(t) = &point.time {
                    props["time"] = serde_json::json!(t.to_rfc3339());
                }
                if let Some(hr) = point.hr {
                    props["hr"] = serde_json::json!(hr);
                }
                if let Some(cad) = point.cad {
                    props["cad"] = serde_json::json!(cad);
                }
                if let Some(power) = point.power {
                    props["power"] = serde_json::json!(power);
                }
                if let Some(speed) = point.speed {
                    props["speed"] = serde_json::json!(speed);
                }

                commands.push(DbCommand {
                    petal_id: petal_id.to_string(),
                    name: format!("Point {}", pi + 1),
                    position: [local[0] as f32, local[1] as f32, local[2] as f32],
                    parent_node_id: Some(seg_node_id.clone()),
                    properties: Some(props),
                });
            }
        }
    }

    // Standalone waypoints
    for (wi, wp) in data.waypoints.iter().enumerate() {
        let local = match projection.wgs84_to_local(wp.lat, wp.lon, wp.ele.unwrap_or(0.0)) {
            Ok(p) => p,
            Err(_) => continue, // skip out-of-range waypoints
        };
        let wp_name = wp
            .name
            .clone()
            .unwrap_or_else(|| format!("Waypoint {}", wi + 1));

        let mut props = serde_json::json!({
            "gpx_type": "waypoint",
            "lat": wp.lat,
            "lon": wp.lon,
        });
        if let Some(e) = wp.ele {
            props["ele"] = serde_json::json!(e);
        }
        if let Some(ref name) = wp.name {
            props["name"] = serde_json::json!(name);
        }
        if let Some(ref desc) = wp.description {
            props["desc"] = serde_json::json!(desc);
        }
        if let Some(ref sym) = wp.symbol {
            props["symbol"] = serde_json::json!(sym);
        }

        commands.push(DbCommand {
            petal_id: petal_id.to_string(),
            name: wp_name,
            position: [local[0] as f32, local[1] as f32, local[2] as f32],
            parent_node_id: None,
            properties: Some(props),
        });
    }

    commands
}
