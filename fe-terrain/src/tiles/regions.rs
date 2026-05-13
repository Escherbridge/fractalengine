//! Pre-defined geographic region presets for tileset building.

use serde::{Deserialize, Serialize};

/// A named geographic region with bounds and recommended zoom levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionPreset {
    pub name: &'static str,
    /// `[min_lat, min_lon, max_lat, max_lon]` (WGS84).
    pub bounds: [f64; 4],
    /// Recommended `(min_zoom, max_zoom)` for this region's tileset.
    pub recommended_zoom: (u8, u8),
}

pub const NA_PACIFIC_NORTHWEST: RegionPreset = RegionPreset {
    name: "North America — Pacific Northwest",
    bounds: [41.0, -125.0, 50.0, -116.0],
    recommended_zoom: (8, 11),
};

pub const NA_ROCKIES: RegionPreset = RegionPreset {
    name: "North America — Rockies",
    bounds: [35.0, -115.0, 49.0, -104.0],
    recommended_zoom: (8, 11),
};

pub const NA_APPALACHIAN: RegionPreset = RegionPreset {
    name: "North America — Appalachian",
    bounds: [34.0, -85.0, 45.0, -74.0],
    recommended_zoom: (8, 11),
};

pub const NA_GREAT_LAKES: RegionPreset = RegionPreset {
    name: "North America — Great Lakes",
    bounds: [41.0, -93.0, 49.0, -76.0],
    recommended_zoom: (8, 11),
};

pub const NA_SOUTHWEST: RegionPreset = RegionPreset {
    name: "North America — Southwest",
    bounds: [31.0, -119.0, 42.0, -103.0],
    recommended_zoom: (8, 11),
};

pub const NA_NORTHEAST: RegionPreset = RegionPreset {
    name: "North America — Northeast",
    bounds: [38.0, -80.0, 47.0, -66.0],
    recommended_zoom: (8, 11),
};

pub const NORTH_AMERICA_FULL: RegionPreset = RegionPreset {
    name: "North America — Full",
    bounds: [15.0, -170.0, 72.0, -50.0],
    recommended_zoom: (5, 8),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_have_valid_bounds() {
        let presets = [
            &NA_PACIFIC_NORTHWEST,
            &NA_ROCKIES,
            &NA_APPALACHIAN,
            &NA_GREAT_LAKES,
            &NA_SOUTHWEST,
            &NA_NORTHEAST,
            &NORTH_AMERICA_FULL,
        ];
        for p in presets {
            let [min_lat, min_lon, max_lat, max_lon] = p.bounds;
            assert!(min_lat < max_lat, "{}: min_lat >= max_lat", p.name);
            assert!(min_lon < max_lon, "{}: min_lon >= max_lon", p.name);
            assert!(min_lat >= -90.0 && max_lat <= 90.0, "{}: lat out of range", p.name);
            assert!(min_lon >= -180.0 && max_lon <= 180.0, "{}: lon out of range", p.name);
            let (min_z, max_z) = p.recommended_zoom;
            assert!(min_z <= max_z, "{}: min_zoom > max_zoom", p.name);
        }
    }
}
