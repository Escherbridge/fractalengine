use serde::{Deserialize, Serialize};

use super::parser::{GpxData, GpxTrackpoint};

/// Computed statistics for a track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackStats {
    pub total_distance_m: f64,
    pub elevation_gain_m: f64,
    pub elevation_loss_m: f64,
    pub duration: Option<f64>,
    pub avg_speed_kmh: Option<f64>,
    pub max_speed_kmh: Option<f64>,
    pub bounding_box: BoundingBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
    pub min_ele: Option<f64>,
    pub max_ele: Option<f64>,
}

/// Haversine distance between two WGS84 points in meters.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0; // Earth radius in meters
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    R * c
}

/// Compute statistics for all tracks in a GpxData.
pub fn compute_stats(data: &GpxData) -> TrackStats {
    let all_points: Vec<&GpxTrackpoint> = data
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| s.points.iter())
        .collect();

    if all_points.is_empty() {
        return TrackStats {
            total_distance_m: 0.0,
            elevation_gain_m: 0.0,
            elevation_loss_m: 0.0,
            duration: None,
            avg_speed_kmh: None,
            max_speed_kmh: None,
            bounding_box: BoundingBox {
                min_lat: 0.0,
                max_lat: 0.0,
                min_lon: 0.0,
                max_lon: 0.0,
                min_ele: None,
                max_ele: None,
            },
        };
    }

    let mut total_distance = 0.0f64;
    let mut elevation_gain = 0.0f64;
    let mut elevation_loss = 0.0f64;
    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    let mut min_ele: Option<f64> = None;
    let mut max_ele: Option<f64> = None;

    for pt in &all_points {
        min_lat = min_lat.min(pt.lat);
        max_lat = max_lat.max(pt.lat);
        min_lon = min_lon.min(pt.lon);
        max_lon = max_lon.max(pt.lon);
        if let Some(e) = pt.ele {
            min_ele = Some(min_ele.map_or(e, |v: f64| v.min(e)));
            max_ele = Some(max_ele.map_or(e, |v: f64| v.max(e)));
        }
    }

    // Compute distance and elevation changes per-segment to avoid cross-segment jumps
    for track in &data.tracks {
        for seg in &track.segments {
            for pair in seg.points.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                total_distance += haversine_m(a.lat, a.lon, b.lat, b.lon);
                if let (Some(ea), Some(eb)) = (a.ele, b.ele) {
                    let diff = eb - ea;
                    if diff > 0.0 {
                        elevation_gain += diff;
                    } else {
                        elevation_loss += diff.abs();
                    }
                }
            }
        }
    }

    // Duration from first to last point
    let first_time = all_points.first().and_then(|p| p.time);
    let last_time = all_points.last().and_then(|p| p.time);
    let duration = match (first_time, last_time) {
        (Some(a), Some(b)) => Some((b - a).num_seconds() as f64),
        _ => None,
    };

    let avg_speed_kmh = duration
        .filter(|d| *d > 0.0)
        .map(|d| (total_distance / 1000.0) / (d / 3600.0));

    // Max speed from consecutive point pairs
    let mut max_speed_kmh: Option<f64> = None;
    for track in &data.tracks {
        for seg in &track.segments {
            for pair in seg.points.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                if let (Some(ta), Some(tb)) = (a.time, b.time) {
                    let dt = (tb - ta).num_seconds() as f64;
                    if dt > 0.0 {
                        let dist = haversine_m(a.lat, a.lon, b.lat, b.lon);
                        let speed = (dist / 1000.0) / (dt / 3600.0);
                        max_speed_kmh = Some(max_speed_kmh.map_or(speed, |v: f64| v.max(speed)));
                    }
                }
            }
        }
    }

    TrackStats {
        total_distance_m: total_distance,
        elevation_gain_m: elevation_gain,
        elevation_loss_m: elevation_loss,
        duration,
        avg_speed_kmh,
        max_speed_kmh,
        bounding_box: BoundingBox {
            min_lat,
            max_lat,
            min_lon,
            max_lon,
            min_ele,
            max_ele,
        },
    }
}
