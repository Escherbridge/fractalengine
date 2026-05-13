use super::source::TileCoord;
use serde::{Deserialize, Serialize};

/// A tile request with priority (lower distance = higher priority).
#[derive(Debug, Clone)]
pub struct TileRequest {
    pub coord: TileCoord,
    pub distance: f64,
}

/// LOD manager that determines which tiles to load based on camera position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileLodManager {
    pub min_zoom: u8,
    pub max_zoom: u8,
}

impl TileLodManager {
    pub fn new(min_zoom: u8, max_zoom: u8) -> Self {
        Self { min_zoom, max_zoom }
    }

    /// Compute the set of tiles needed for the given camera position and view.
    ///
    /// Returns tile requests sorted by distance to camera (closest first).
    ///
    /// Algorithm: starts at `min_zoom` covering the visible area, then
    /// subdivides tiles closer to the camera up to `max_zoom`.
    pub fn compute_tile_requests(
        &self,
        camera_lat: f64,
        camera_lon: f64,
        view_radius_deg: f64,
    ) -> Vec<TileRequest> {
        let mut requests = Vec::new();

        // Start at min_zoom: find all tiles in the view
        let min_lat = (camera_lat - view_radius_deg).max(-85.0511);
        let max_lat = (camera_lat + view_radius_deg).min(85.0511);
        let min_lon = (camera_lon - view_radius_deg).max(-180.0);
        let max_lon = (camera_lon + view_radius_deg).min(180.0);

        // Get bounding tiles at min_zoom
        let tl = TileCoord::from_lat_lon(max_lat, min_lon, self.min_zoom);
        let br = TileCoord::from_lat_lon(min_lat, max_lon, self.min_zoom);

        // Collect base tiles
        let mut base_tiles = Vec::new();
        for x in tl.x..=br.x {
            for y in tl.y..=br.y {
                let coord = TileCoord::new(x, y, self.min_zoom);
                base_tiles.push(coord);
            }
        }

        // Subdivide tiles closer to camera to higher zoom levels
        for base in &base_tiles {
            self.subdivide(
                *base,
                camera_lat,
                camera_lon,
                view_radius_deg,
                &mut requests,
            );
        }

        // Sort by distance (closest first)
        requests.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        requests
    }

    fn subdivide(
        &self,
        coord: TileCoord,
        camera_lat: f64,
        camera_lon: f64,
        view_radius_deg: f64,
        requests: &mut Vec<TileRequest>,
    ) {
        let (tile_lat, tile_lon) = coord.to_lat_lon();
        // Approximate tile center
        let next_coord = TileCoord::new(coord.x + 1, coord.y + 1, coord.zoom);
        let (next_lat, next_lon) = next_coord.to_lat_lon();
        let center_lat = (tile_lat + next_lat) / 2.0;
        let center_lon = (tile_lon + next_lon) / 2.0;

        let dist = haversine_deg(camera_lat, camera_lon, center_lat, center_lon);

        // Determine the desired zoom for this distance
        // Closer tiles get higher zoom. Use a simple linear mapping.
        let max_dist = view_radius_deg;
        let frac = (dist / max_dist).min(1.0);
        let desired_zoom = self.max_zoom as f64 - frac * (self.max_zoom - self.min_zoom) as f64;
        let desired_zoom = desired_zoom.round() as u8;

        if coord.zoom >= desired_zoom || coord.zoom >= self.max_zoom {
            // This tile is at the right zoom level
            requests.push(TileRequest {
                coord,
                distance: dist,
            });
        } else {
            // Subdivide into 4 child tiles
            let child_zoom = coord.zoom + 1;
            let cx = coord.x * 2;
            let cy = coord.y * 2;
            for dx in 0..2 {
                for dy in 0..2 {
                    let child = TileCoord::new(cx + dx, cy + dy, child_zoom);
                    self.subdivide(child, camera_lat, camera_lon, view_radius_deg, requests);
                }
            }
        }
    }
}

/// Simple haversine distance in degrees (approximate, for LOD sorting).
fn haversine_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    (dlat * dlat + dlon * dlon).sqrt()
}
