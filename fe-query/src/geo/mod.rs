pub mod validate;

/// Coordinate reference system identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CRS {
    /// WGS 84 geographic coordinates (lon/lat in degrees).
    Epsg4326,
    /// Web Mercator projected coordinates (meters).
    Epsg3857,
    /// Local Cartesian (engine-internal, no bounds constraints).
    LocalCartesian,
}

/// A geographic point with an associated CRS.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoPoint {
    pub x: f64,
    pub y: f64,
    pub crs: CRS,
}

/// A geographic polygon with an associated CRS.
///
/// `rings[0]` is the exterior ring; subsequent rings are holes.
/// Each ring is a sequence of [x, y] coordinate pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoPolygon {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub crs: CRS,
}

impl GeoPoint {
    pub fn new(x: f64, y: f64, crs: CRS) -> Self {
        GeoPoint { x, y, crs }
    }

    /// Validate this point against its CRS bounds.
    pub fn validate(&self) -> Result<(), validate::GeoValidationError> {
        validate::validate_point(self.crs, self.x, self.y)
    }
}

impl GeoPolygon {
    pub fn new(rings: Vec<Vec<[f64; 2]>>, crs: CRS) -> Self {
        GeoPolygon { rings, crs }
    }

    /// Validate this polygon against its CRS bounds and geometry rules.
    pub fn validate(&self) -> Result<(), validate::GeoValidationError> {
        validate::validate_polygon(self.crs, &self.rings)
    }
}
