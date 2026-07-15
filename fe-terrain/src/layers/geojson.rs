//! GeoJSON parsing and mesh generation.
//!
//! Parses GeoJSON `FeatureCollection` documents and converts geometry types
//! into mesh data suitable for Bevy rendering.

use serde::{Deserialize, Serialize};

/// Result of parsing a GeoJSON feature collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoJsonResult {
    pub polygon_meshes: Vec<PolygonMesh>,
    pub polyline_meshes: Vec<PolylineMesh>,
    pub marker_positions: Vec<MarkerInstance>,
}

/// A triangulated polygon mesh (flat, y = 0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolygonMesh {
    /// Flat triangle-list positions [x, y, z] — all y values are 0.
    pub positions: Vec<[f32; 3]>,
    /// Fill color RGBA.
    pub fill_color: [f32; 4],
    /// Stroke color RGBA.
    pub stroke_color: [f32; 4],
    /// Stroke width in world units.
    pub stroke_width: f32,
}

/// A polyline mesh (line strip geometry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolylineMesh {
    /// Line strip positions [x, y, z].
    pub positions: Vec<[f32; 3]>,
    /// Stroke color RGBA.
    pub stroke_color: [f32; 4],
    /// Stroke width in world units.
    pub stroke_width: f32,
}

/// A point marker instance for instanced rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerInstance {
    /// World-space position [x, y, z].
    pub position: [f32; 3],
    /// Marker symbol / name from GeoJSON properties.
    pub label: String,
    /// Marker color RGBA.
    pub color: [f32; 4],
}

/// Default fill color.
const DEFAULT_FILL: [f32; 4] = [0.5, 0.5, 0.5, 0.5];
/// Default stroke color.
const DEFAULT_STROKE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Default stroke width.
const DEFAULT_STROKE_WIDTH: f32 = 1.0;

/// Parse a CSS-like color string (#RRGGBB or #RRGGBBAA) to RGBA `[f32; 4]`.
fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b, 1.0])
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
        let a = u8::from_str_radix(&s[6..8], 16).ok()? as f32 / 255.0;
        Some([r, g, b, a])
    } else {
        None
    }
}

/// Extract styling from a GeoJSON feature's properties.
fn extract_style(props: &geojson::JsonObject) -> ([f32; 4], [f32; 4], f32) {
    let fill = props
        .get("fill")
        .and_then(|v| v.as_str())
        .and_then(parse_color)
        .unwrap_or(DEFAULT_FILL);
    let stroke = props
        .get("stroke")
        .and_then(|v| v.as_str())
        .and_then(parse_color)
        .unwrap_or(DEFAULT_STROKE);
    let stroke_width = props
        .get("stroke-width")
        .and_then(|v| v.as_f64())
        .unwrap_or(DEFAULT_STROKE_WIDTH as f64) as f32;
    (fill, stroke, stroke_width)
}

/// Triangulate a simple polygon using earcut (via earcutr).
///
/// `vertices` is a list of [x, y] coordinates.
/// `holes` is a list of vertex indices where each hole starts.
fn triangulate(vertices: &[[f64; 2]], holes: &[usize]) -> Vec<u32> {
    let flat: Vec<f64> = vertices.iter().flat_map(|v| [v[0], v[1]]).collect();
    match earcutr::earcut(&flat, holes, 2) {
        Ok(indices) => indices.into_iter().map(|i| i as u32).collect(),
        Err(_) => Vec::new(),
    }
}

/// Convert a GeoJSON `FeatureCollection` from a raw JSON string.
///
/// Coordinates are assumed to be in [lon, lat] (WGS84).
/// They are converted to world-space [x, y, z] using the provided projection.
/// The `projection_fn` takes (lon, lat) and returns (x, z) in world units.
pub fn parse_geojson(
    json_str: &str,
    projection_fn: impl Fn(f64, f64) -> (f32, f32),
) -> anyhow::Result<GeoJsonResult> {
    let geo: geojson::GeoJson = json_str.parse()?;
    let fc = match geo {
        geojson::GeoJson::FeatureCollection(fc) => fc,
        geojson::GeoJson::Feature(_) => anyhow::bail!("Expected FeatureCollection, got Feature"),
        geojson::GeoJson::Geometry(_) => anyhow::bail!("Expected FeatureCollection, got Geometry"),
    };

    let mut polygon_meshes = Vec::new();
    let mut polyline_meshes = Vec::new();
    let mut marker_positions = Vec::new();

    for feature in &fc.features {
        let (fill, stroke, stroke_width) = feature
            .properties
            .as_ref()
            .map(extract_style)
            .unwrap_or((DEFAULT_FILL, DEFAULT_STROKE, DEFAULT_STROKE_WIDTH));

        let geometry = match &feature.geometry {
            Some(g) => g,
            None => continue,
        };

        match &geometry.value {
            geojson::Value::Polygon(rings) => {
                // First ring = exterior, rest = holes
                let exterior = &rings[0];
                let mut vertices: Vec<[f64; 2]> = Vec::new();
                for coord in exterior {
                    if let [lon, lat, ..] = coord.as_slice() {
                        vertices.push([*lon, *lat]);
                    }
                }

                let hole_indices: Vec<usize> = rings[1..]
                    .iter()
                    .map(|ring| {
                        let idx = vertices.len();
                        for coord in ring {
                            if let [lon, lat, ..] = coord.as_slice() {
                                vertices.push([*lon, *lat]);
                            }
                        }
                        idx
                    })
                    .collect();

                let indices = triangulate(&vertices, &hole_indices);

                let mut positions = Vec::with_capacity(indices.len());
                for &idx in &indices {
                    let (x, z) =
                        projection_fn(vertices[idx as usize][0], vertices[idx as usize][1]);
                    positions.push([x, 0.0, z]);
                }

                polygon_meshes.push(PolygonMesh {
                    positions,
                    fill_color: fill,
                    stroke_color: stroke,
                    stroke_width,
                });
            }
            geojson::Value::MultiPolygon(polygons) => {
                for rings in polygons {
                    let exterior = &rings[0];
                    let mut vertices: Vec<[f64; 2]> = Vec::new();
                    for coord in exterior {
                        if let [lon, lat, ..] = coord.as_slice() {
                            vertices.push([*lon, *lat]);
                        }
                    }

                    let hole_indices: Vec<usize> = rings[1..]
                        .iter()
                        .map(|ring| {
                            let idx = vertices.len();
                            for coord in ring {
                                if let [lon, lat, ..] = coord.as_slice() {
                                    vertices.push([*lon, *lat]);
                                }
                            }
                            idx
                        })
                        .collect();

                    let indices = triangulate(&vertices, &hole_indices);
                    let mut positions = Vec::with_capacity(indices.len());
                    for &idx in &indices {
                        let (x, z) =
                            projection_fn(vertices[idx as usize][0], vertices[idx as usize][1]);
                        positions.push([x, 0.0, z]);
                    }
                    polygon_meshes.push(PolygonMesh {
                        positions,
                        fill_color: fill,
                        stroke_color: stroke,
                        stroke_width,
                    });
                }
            }
            geojson::Value::LineString(points) => {
                let positions = points
                    .iter()
                    .filter_map(|c| {
                        if let [lon, lat, ..] = c.as_slice() {
                            let (x, z) = projection_fn(*lon, *lat);
                            Some([x, 0.0, z])
                        } else {
                            None
                        }
                    })
                    .collect();
                polyline_meshes.push(PolylineMesh {
                    positions,
                    stroke_color: stroke,
                    stroke_width,
                });
            }
            geojson::Value::MultiLineString(lines) => {
                for line in lines {
                    let positions = line
                        .iter()
                        .filter_map(|c| {
                            if let [lon, lat, ..] = c.as_slice() {
                                let (x, z) = projection_fn(*lon, *lat);
                                Some([x, 0.0, z])
                            } else {
                                None
                            }
                        })
                        .collect();
                    polyline_meshes.push(PolylineMesh {
                        positions,
                        stroke_color: stroke,
                        stroke_width,
                    });
                }
            }
            geojson::Value::Point(coord) => {
                if let [lon, lat, ..] = coord.as_slice() {
                    let (x, z) = projection_fn(*lon, *lat);
                    let label = feature
                        .properties
                        .as_ref()
                        .and_then(|p| p.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    marker_positions.push(MarkerInstance {
                        position: [x, 0.0, z],
                        label,
                        color: fill,
                    });
                }
            }
            geojson::Value::MultiPoint(points) => {
                for coord in points {
                    if let [lon, lat, ..] = coord.as_slice() {
                        let (x, z) = projection_fn(*lon, *lat);
                        marker_positions.push(MarkerInstance {
                            position: [x, 0.0, z],
                            label: String::new(),
                            color: fill,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(GeoJsonResult {
        polygon_meshes,
        polyline_meshes,
        marker_positions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_color_hex() {
        let c = parse_color("#ff0000").unwrap();
        assert!((c[0] - 1.0).abs() < 1e-6);
        assert!((c[1]).abs() < 1e-6);

        let ca = parse_color("#80808080").unwrap();
        assert!((ca[0] - 0.502).abs() < 0.01);
        assert!((ca[3] - 0.502).abs() < 0.01);
    }

    #[test]
    fn test_parse_point_geojson() {
        let json = r##"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [10.0, 50.0] },
                "properties": { "name": "test", "fill": "#ff0000" }
            }]
        }"##;
        let result =
            parse_geojson(json, |lon, lat| (lon as f32 * 100.0, lat as f32 * 100.0)).unwrap();
        assert_eq!(result.marker_positions.len(), 1);
        assert_eq!(result.marker_positions[0].label, "test");
        assert_eq!(result.polygon_meshes.len(), 0);
        assert_eq!(result.polyline_meshes.len(), 0);
    }

    #[test]
    fn test_parse_line_string_geojson() {
        let json = r##"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": [[10.0, 50.0], [11.0, 51.0]]
                },
                "properties": { "stroke": "#00ff00", "stroke-width": 2.0 }
            }]
        }"##;
        let result = parse_geojson(json, |lon, lat| (lon as f32, lat as f32)).unwrap();
        assert_eq!(result.polyline_meshes.len(), 1);
        assert_eq!(result.polyline_meshes[0].positions.len(), 2);
        assert!((result.polyline_meshes[0].stroke_color[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_polygon_geojson() {
        let json = r##"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[0,0],[1,0],[1,1],[0,1],[0,0]]]
                },
                "properties": {}
            }]
        }"##;
        let result = parse_geojson(json, |lon, lat| (lon as f32, lat as f32)).unwrap();
        assert_eq!(result.polygon_meshes.len(), 1);
        // Quad (5 verts including closing) triangulates to 2 triangles = 6 vertices
        assert_eq!(result.polygon_meshes[0].positions.len(), 6);
    }

    #[test]
    fn test_non_feature_collection_fails() {
        let json = r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]}}"#;
        assert!(parse_geojson(json, |_, _| (0.0, 0.0)).is_err());
    }
}
