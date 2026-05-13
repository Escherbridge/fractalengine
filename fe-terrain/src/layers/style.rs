//! Layer color style definitions and vertex color computation.
//!
//! Color modes control how GPX trackpoints are mapped to RGBA vertex colors.

use serde::{Deserialize, Serialize};

/// Color mode for track vertex coloring.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ColorMode {
    Solid { color: [u8; 4] },
    ElevationGradient {
        min_color: [f32; 4],
        max_color: [f32; 4],
    },
    SpeedGradient {
        slow_color: [f32; 4],
        fast_color: [f32; 4],
    },
    TimeGradient {
        start_color: [f32; 4],
        end_color: [f32; 4],
    },
}

impl ColorMode {
    /// Interpolate between two RGBA colors given a t value in [0, 1].
    fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ]
    }
}

/// A simplified track point for coloring purposes.
#[derive(Debug, Clone)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
    pub time: Option<chrono::DateTime<chrono::Utc>>,
    pub speed: Option<f64>,
}

/// Compute per-vertex RGBA colors for a set of track points using the given color mode.
///
/// Returns a `Vec<[f32; 4]>` with one color per point.
pub fn compute_vertex_colors(points: &[TrackPoint], mode: &ColorMode) -> Vec<[f32; 4]> {
    match mode {
        ColorMode::Solid { color } => {
            let rgba = [
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
                color[3] as f32 / 255.0,
            ];
            vec![rgba; points.len()]
        }
        ColorMode::ElevationGradient {
            min_color,
            max_color,
        } => {
            // Find min/max elevation
            let elevations: Vec<f64> = points.iter().filter_map(|p| p.ele).collect();
            if elevations.len() < 2 {
                return vec![*min_color; points.len()];
            }
            let min_ele = elevations.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_ele = elevations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let range = (max_ele - min_ele).max(1e-6);

            points
                .iter()
                .map(|p| {
                    let ele = p.ele.unwrap_or(min_ele);
                    let t = ((ele - min_ele) / range) as f32;
                    ColorMode::lerp(*min_color, *max_color, t)
                })
                .collect()
        }
        ColorMode::SpeedGradient {
            slow_color,
            fast_color,
        } => {
            let speeds: Vec<f64> = points.iter().filter_map(|p| p.speed).collect();
            if speeds.len() < 2 {
                return vec![*slow_color; points.len()];
            }
            let min_speed = speeds.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_speed = speeds.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let range = (max_speed - min_speed).max(1e-6);

            points
                .iter()
                .map(|p| {
                    let spd = p.speed.unwrap_or(min_speed);
                    let t = ((spd - min_speed) / range) as f32;
                    ColorMode::lerp(*slow_color, *fast_color, t)
                })
                .collect()
        }
        ColorMode::TimeGradient {
            start_color,
            end_color,
        } => {
            let times: Vec<chrono::DateTime<chrono::Utc>> =
                points.iter().filter_map(|p| p.time).collect();
            if times.len() < 2 {
                return vec![*start_color; points.len()];
            }
            let min_time = times.first().unwrap().timestamp_millis() as f64;
            let max_time = times.last().unwrap().timestamp_millis() as f64;
            let range = (max_time - min_time).max(1e-6);

            points
                .iter()
                .map(|p| {
                    let t_ms = p
                        .time
                        .map(|t| t.timestamp_millis() as f64)
                        .unwrap_or(min_time);
                    let t = ((t_ms - min_time) / range) as f32;
                    ColorMode::lerp(*start_color, *end_color, t)
                })
                .collect()
        }
    }
}

/// The viridis color ramp (approximation) — used by heatmaps.
///
/// Returns an RGBA `[f32; 4]` for a normalized value in [0, 1].
pub fn viridis(t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    // Piecewise cubic approximation of viridis
    let r = if t < 0.25 {
        0.267004 + t * 0.398
    } else if t < 0.5 {
        0.122312 + t * 1.56
    } else if t < 0.75 {
        0.207648 + t * 1.036
    } else {
        0.993248 - (1.0 - t) * 1.0
    };
    let g = if t < 0.25 {
        0.004874 + t * 1.17
    } else if t < 0.5 {
        0.539888 + t * 0.72
    } else if t < 0.75 {
        0.827392 + t * 0.16
    } else {
        0.940016 - (1.0 - t) * 0.2
    };
    let b = if t < 0.25 {
        0.329415 + t * 0.52
    } else if t < 0.5 {
        0.543288 - t * 0.36
    } else if t < 0.75 {
        0.441568 + t * 0.48
    } else {
        0.975248 - (1.0 - t) * 0.3
    };
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), 1.0]
}

/// A heatmap mesh with vertex colors computed from kernel density estimation.
#[derive(Debug, Clone)]
pub struct HeatmapMesh {
    /// Triangle-list positions [x, y, z] — a flat grid draped at terrain elevation.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex RGBA colors from kernel density × viridis ramp.
    pub colors: Vec<[f32; 4]>,
}

/// Compute a kernel-density heatmap mesh from a set of point positions.
///
/// Points are given as `[x, z]` world-space coordinates (y = elevation).
/// A grid of `grid_width × grid_height` cells is evaluated at the given
/// resolution (world units per cell). Each point contributes a Gaussian kernel
/// with the given `radius_m` (standard deviation).
///
/// The output is a `HeatmapMesh` ready for Bevy rendering, draped at `y = elevation + 0.1`.
///
/// # Arguments
/// * `points` — world-space `[x, z]` positions of data points.
/// * `min_x`, `max_x`, `min_z`, `max_z` — bounding box in world units.
/// * `radius_m` — Gaussian kernel radius (standard deviation in meters).
/// * `color_ramp` — optional color ramp function; defaults to viridis.
/// * `grid_resolution` — world units per grid cell (lower = finer).
/// * `elevation` — base Y value for the heatmap mesh (typically terrain height).
pub fn compute_heatmap<F>(
    points: &[[f32; 2]],
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
    radius_m: f32,
    color_ramp: F,
    grid_resolution: f32,
    elevation: f32,
) -> HeatmapMesh
where
    F: Fn(f32) -> [f32; 4],
{
    let width = ((max_x - min_x) / grid_resolution).ceil().max(2.0) as usize;
    let height = ((max_z - min_z) / grid_resolution).ceil().max(2.0) as usize;

    // Evaluate kernel density at each grid cell
    let mut density: Vec<f32> = vec![0.0; width * height];
    let radius_sq = radius_m * radius_m;
    let inv_2pi = 1.0 / (2.0 * std::f32::consts::PI);

    for &[px, pz] in points {
        for gz in 0..height {
            for gx in 0..width {
                let cx = min_x + (gx as f32 + 0.5) * grid_resolution;
                let cz = min_z + (gz as f32 + 0.5) * grid_resolution;
                let dx = px - cx;
                let dz = pz - cz;
                let dist_sq = dx * dx + dz * dz;
                // Gaussian kernel
                density[gz * width + gx] += inv_2pi / radius_sq * (-0.5 * dist_sq / radius_sq).exp();
            }
        }
    }

    // Normalize to [0, 1]
    let max_density = density.iter().cloned().fold(0.0f32, f32::max);
    let normalized: Vec<f32> = if max_density > 1e-9 {
        density.iter().map(|&d| d / max_density).collect()
    } else {
        density
    };

    // Build triangle mesh: each grid cell → 2 triangles (6 vertices)
    let y = elevation + 0.1; // slight offset above terrain
    let mut positions = Vec::with_capacity(width * height * 6);
    let mut colors = Vec::with_capacity(width * height * 6);

    for gz in 0..(height - 1) {
        for gx in 0..(width - 1) {
            let x0 = min_x + gx as f32 * grid_resolution;
            let z0 = min_z + gz as f32 * grid_resolution;
            let x1 = x0 + grid_resolution;
            let z1 = z0 + grid_resolution;

            let d00 = normalized[gz * width + gx];
            let d10 = normalized[gz * width + gx + 1];
            let d01 = normalized[(gz + 1) * width + gx];
            let d11 = normalized[(gz + 1) * width + gx + 1];

            let c00 = color_ramp(d00);
            let c10 = color_ramp(d10);
            let c01 = color_ramp(d01);
            let c11 = color_ramp(d11);

            // Triangle 1: bottom-left → bottom-right → top-left
            positions.push([x0, y, z0]); colors.push(c00);
            positions.push([x1, y, z0]); colors.push(c10);
            positions.push([x0, y, z1]); colors.push(c01);

            // Triangle 2: bottom-right → top-right → top-left
            positions.push([x1, y, z0]); colors.push(c10);
            positions.push([x1, y, z1]); colors.push(c11);
            positions.push([x0, y, z1]); colors.push(c01);
        }
    }

    HeatmapMesh { positions, colors }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_point(ele: f64, speed: f64) -> TrackPoint {
        TrackPoint {
            lat: 0.0,
            lon: 0.0,
            ele: Some(ele),
            time: None,
            speed: Some(speed),
        }
    }

    #[test]
    fn test_solid_color() {
        let pts = vec![make_point(10.0, 5.0); 3];
        let mode = ColorMode::Solid {
            color: [255, 0, 0, 128],
        };
        let colors = compute_vertex_colors(&pts, &mode);
        assert_eq!(colors.len(), 3);
        assert!((colors[0][0] - 1.0).abs() < 1e-6);
        assert!((colors[0][3] - 0.502).abs() < 0.01);
    }

    #[test]
    fn test_elevation_gradient() {
        let pts = vec![
            make_point(0.0, 5.0),
            make_point(50.0, 5.0),
            make_point(100.0, 5.0),
        ];
        let mode = ColorMode::ElevationGradient {
            min_color: [0.0, 0.0, 1.0, 1.0],
            max_color: [1.0, 0.0, 0.0, 1.0],
        };
        let colors = compute_vertex_colors(&pts, &mode);
        // First point should be blue
        assert!(colors[0][2] > colors[0][0]);
        // Last point should be red
        assert!(colors[2][0] > colors[2][2]);
    }

    #[test]
    fn test_speed_gradient() {
        let pts = vec![
            make_point(10.0, 0.0),
            make_point(10.0, 5.0),
            make_point(10.0, 10.0),
        ];
        let mode = ColorMode::SpeedGradient {
            slow_color: [0.0, 0.0, 1.0, 1.0],
            fast_color: [1.0, 0.0, 0.0, 1.0],
        };
        let colors = compute_vertex_colors(&pts, &mode);
        assert!(colors[0][2] > colors[0][0]); // slow → blue
        assert!(colors[2][0] > colors[2][2]); // fast → red
    }

    #[test]
    fn test_viridis_range() {
        let c0 = viridis(0.0);
        let c1 = viridis(1.0);
        let cm = viridis(0.5);
        // All channels should be in [0, 1]
        for &c in &[c0, c1, cm] {
            for ch in c {
                assert!((0.0..=1.0).contains(&ch));
            }
        }
        // viridis goes from dark purple to bright yellow
        assert!(c0[0] < c1[0]); // red increases
        assert!(c0[1] < c1[1]); // green increases
    }
}
