use super::CRS;

/// Errors produced by GIS geometry validation.
#[derive(Debug, Clone, PartialEq)]
pub enum GeoValidationError {
    /// A coordinate value is outside the valid range for its CRS.
    OutOfBounds {
        field: String,
        value: f64,
        min: f64,
        max: f64,
    },
    /// A polygon ring intersects itself.
    SelfIntersecting,
    /// A polygon ring has fewer than 4 points (3 vertices + closing point).
    TooFewPoints { ring_index: usize, count: usize },
    /// A polygon ring is not closed (first point != last point).
    UnclosedRing { ring_index: usize },
}

impl std::fmt::Display for GeoValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeoValidationError::OutOfBounds {
                field,
                value,
                min,
                max,
            } => {
                write!(
                    f,
                    "{} value {} is out of bounds [{}, {}]",
                    field, value, min, max
                )
            }
            GeoValidationError::SelfIntersecting => {
                write!(f, "polygon ring is self-intersecting")
            }
            GeoValidationError::TooFewPoints { ring_index, count } => {
                write!(f, "ring {} has {} points, minimum is 4", ring_index, count)
            }
            GeoValidationError::UnclosedRing { ring_index } => {
                write!(
                    f,
                    "ring {} is not closed (first point != last point)",
                    ring_index
                )
            }
        }
    }
}

impl std::error::Error for GeoValidationError {}

// ── CRS bounds ──

const WGS84_LON_MIN: f64 = -180.0;
const WGS84_LON_MAX: f64 = 180.0;
const WGS84_LAT_MIN: f64 = -90.0;
const WGS84_LAT_MAX: f64 = 90.0;

const MERCATOR_MIN: f64 = -20_037_508.34;
const MERCATOR_MAX: f64 = 20_037_508.34;

/// Validate a point's coordinates against its CRS bounds.
///
/// - EPSG:4326 (WGS 84): x = longitude [-180, 180], y = latitude [-90, 90]
/// - EPSG:3857 (Web Mercator): x, y in [-20037508.34, 20037508.34]
/// - LocalCartesian: no bounds (always valid)
pub fn validate_point(crs: CRS, x: f64, y: f64) -> Result<(), GeoValidationError> {
    match crs {
        CRS::Epsg4326 => {
            if !(WGS84_LON_MIN..=WGS84_LON_MAX).contains(&x) {
                return Err(GeoValidationError::OutOfBounds {
                    field: "x (longitude)".into(),
                    value: x,
                    min: WGS84_LON_MIN,
                    max: WGS84_LON_MAX,
                });
            }
            if !(WGS84_LAT_MIN..=WGS84_LAT_MAX).contains(&y) {
                return Err(GeoValidationError::OutOfBounds {
                    field: "y (latitude)".into(),
                    value: y,
                    min: WGS84_LAT_MIN,
                    max: WGS84_LAT_MAX,
                });
            }
            Ok(())
        }
        CRS::Epsg3857 => {
            if !(MERCATOR_MIN..=MERCATOR_MAX).contains(&x) {
                return Err(GeoValidationError::OutOfBounds {
                    field: "x".into(),
                    value: x,
                    min: MERCATOR_MIN,
                    max: MERCATOR_MAX,
                });
            }
            if !(MERCATOR_MIN..=MERCATOR_MAX).contains(&y) {
                return Err(GeoValidationError::OutOfBounds {
                    field: "y".into(),
                    value: y,
                    min: MERCATOR_MIN,
                    max: MERCATOR_MAX,
                });
            }
            Ok(())
        }
        CRS::LocalCartesian => Ok(()),
    }
}

/// Validate a polygon's rings against CRS bounds and geometry rules.
///
/// Each ring must:
/// 1. Have at least 4 points (3 vertices + closing duplicate)
/// 2. Be closed (first point == last point)
/// 3. Have all coordinates within CRS bounds
pub fn validate_polygon(crs: CRS, rings: &[Vec<[f64; 2]>]) -> Result<(), GeoValidationError> {
    for (ring_idx, ring) in rings.iter().enumerate() {
        // Minimum 4 points (triangle + closing point)
        if ring.len() < 4 {
            return Err(GeoValidationError::TooFewPoints {
                ring_index: ring_idx,
                count: ring.len(),
            });
        }

        // Ring must be closed
        let first = ring.first().unwrap();
        let last = ring.last().unwrap();
        if (first[0] - last[0]).abs() > f64::EPSILON || (first[1] - last[1]).abs() > f64::EPSILON {
            return Err(GeoValidationError::UnclosedRing {
                ring_index: ring_idx,
            });
        }

        // All coordinates must be within CRS bounds
        for coord in ring {
            validate_point(crs, coord[0], coord[1])?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Point validation ──

    #[test]
    fn valid_wgs84_point() {
        assert!(validate_point(CRS::Epsg4326, 0.0, 0.0).is_ok());
        assert!(validate_point(CRS::Epsg4326, -180.0, -90.0).is_ok());
        assert!(validate_point(CRS::Epsg4326, 180.0, 90.0).is_ok());
        assert!(validate_point(CRS::Epsg4326, -73.9857, 40.7484).is_ok()); // NYC
    }

    #[test]
    fn invalid_wgs84_longitude() {
        let err = validate_point(CRS::Epsg4326, 181.0, 0.0).unwrap_err();
        match err {
            GeoValidationError::OutOfBounds { field, value, .. } => {
                assert!(field.contains("longitude"));
                assert_eq!(value, 181.0);
            }
            _ => panic!("expected OutOfBounds"),
        }
    }

    #[test]
    fn invalid_wgs84_latitude() {
        let err = validate_point(CRS::Epsg4326, 0.0, -91.0).unwrap_err();
        match err {
            GeoValidationError::OutOfBounds { field, value, .. } => {
                assert!(field.contains("latitude"));
                assert_eq!(value, -91.0);
            }
            _ => panic!("expected OutOfBounds"),
        }
    }

    #[test]
    fn valid_mercator_point() {
        assert!(validate_point(CRS::Epsg3857, 0.0, 0.0).is_ok());
        assert!(validate_point(CRS::Epsg3857, -20_037_508.34, 20_037_508.34).is_ok());
    }

    #[test]
    fn invalid_mercator_point() {
        let err = validate_point(CRS::Epsg3857, 20_037_509.0, 0.0).unwrap_err();
        match err {
            GeoValidationError::OutOfBounds { value, .. } => {
                assert_eq!(value, 20_037_509.0);
            }
            _ => panic!("expected OutOfBounds"),
        }
    }

    #[test]
    fn local_cartesian_always_valid() {
        assert!(validate_point(CRS::LocalCartesian, 999_999.0, -999_999.0).is_ok());
        assert!(validate_point(CRS::LocalCartesian, f64::MAX, f64::MIN).is_ok());
    }

    // ── Polygon validation ──

    #[test]
    fn valid_polygon() {
        let rings = vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]];
        assert!(validate_polygon(CRS::Epsg4326, &rings).is_ok());
    }

    #[test]
    fn polygon_too_few_points() {
        let rings = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]]];
        let err = validate_polygon(CRS::Epsg4326, &rings).unwrap_err();
        match err {
            GeoValidationError::TooFewPoints { ring_index, count } => {
                assert_eq!(ring_index, 0);
                assert_eq!(count, 3);
            }
            _ => panic!("expected TooFewPoints"),
        }
    }

    #[test]
    fn polygon_unclosed_ring() {
        let rings = vec![vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0], // not closed — doesn't match [0.0, 0.0]
        ]];
        let err = validate_polygon(CRS::Epsg4326, &rings).unwrap_err();
        match err {
            GeoValidationError::UnclosedRing { ring_index } => {
                assert_eq!(ring_index, 0);
            }
            _ => panic!("expected UnclosedRing"),
        }
    }

    #[test]
    fn polygon_out_of_bounds() {
        let rings = vec![vec![
            [0.0, 0.0],
            [200.0, 0.0], // out of WGS84 bounds
            [200.0, 10.0],
            [0.0, 0.0],
        ]];
        let err = validate_polygon(CRS::Epsg4326, &rings).unwrap_err();
        assert!(matches!(err, GeoValidationError::OutOfBounds { .. }));
    }

    #[test]
    fn polygon_with_hole() {
        let rings = vec![
            // Exterior ring
            vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, -10.0]],
            // Interior hole
            vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0]],
        ];
        assert!(validate_polygon(CRS::Epsg4326, &rings).is_ok());
    }

    #[test]
    fn polygon_local_cartesian_large_coords() {
        let rings = vec![vec![
            [0.0, 0.0],
            [100_000.0, 0.0],
            [100_000.0, 100_000.0],
            [0.0, 0.0],
        ]];
        assert!(validate_polygon(CRS::LocalCartesian, &rings).is_ok());
    }

    #[test]
    fn error_display() {
        let err = GeoValidationError::OutOfBounds {
            field: "x".into(),
            value: 200.0,
            min: -180.0,
            max: 180.0,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("200"));
        assert!(msg.contains("out of bounds"));
    }
}
