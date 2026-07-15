use flat_projection::FlatProjection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("latitude {0} out of range [-90, 90]")]
    LatitudeOutOfRange(f64),
    #[error("longitude {0} out of range [-180, 180]")]
    LongitudeOutOfRange(f64),
}

/// WGS84-to-local flat Earth projection.
///
/// Uses `flat_projection` crate internally for fast equirectangular projection,
/// accurate up to ~500 km from the origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projection {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub origin_ele: f64,
}

impl Projection {
    pub fn new(origin_lat: f64, origin_lon: f64, origin_ele: f64) -> Self {
        Self {
            origin_lat,
            origin_lon,
            origin_ele,
        }
    }

    fn validate(lat: f64, lon: f64) -> Result<(), ProjectionError> {
        if !(-90.0..=90.0).contains(&lat) {
            return Err(ProjectionError::LatitudeOutOfRange(lat));
        }
        if !(-180.0..=180.0).contains(&lon) {
            return Err(ProjectionError::LongitudeOutOfRange(lon));
        }
        Ok(())
    }

    /// Project WGS84 coordinates to local [x, y, z] in meters.
    ///
    /// - x = easting (meters)
    /// - y = elevation above origin (meters)
    /// - z = northing (meters)
    ///
    /// Returns error if lat or lon are out of WGS84 bounds.
    pub fn wgs84_to_local(
        &self,
        lat: f64,
        lon: f64,
        ele: f64,
    ) -> Result<[f64; 3], ProjectionError> {
        Self::validate(lat, lon)?;

        let proj = FlatProjection::new(self.origin_lon, self.origin_lat);
        let flat = proj.project(lon, lat);
        // flat_projection returns km, convert to meters
        let x = flat.x * 1000.0;
        let z = flat.y * 1000.0;
        let y = ele - self.origin_ele;
        Ok([x, y, z])
    }

    /// Inverse: local [x, y, z] in meters back to WGS84 (lat, lon, ele).
    pub fn local_to_wgs84(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let proj = FlatProjection::new(self.origin_lon, self.origin_lat);
        // Convert meters to km for flat_projection
        let flat = flat_projection::FlatPoint {
            x: x / 1000.0,
            y: z / 1000.0,
        };
        let (lon, lat) = proj.unproject(&flat);
        let ele = y + self.origin_ele;
        (lat, lon, ele)
    }
}
