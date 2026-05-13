//! API route declarations for the terrain extension.
//!
//! Declares the HTTP routes that the terrain extension exposes through
//! the engine's extension API router.

use fe_sdk::api::{ApiExtensionHandle, HttpMethod};

/// Build the [`ApiExtensionHandle`] for the terrain extension.
///
/// Declares two routes under the `terrain` base path:
/// - `GET /elevation-profile/:track_id` — elevation profile for a GPX track.
/// - `GET /stats/:track_id` — statistics (distance, elevation gain, etc.) for a track.
pub fn terrain_api_routes() -> ApiExtensionHandle {
    ApiExtensionHandle::new("terrain")
        .route(
            HttpMethod::Get,
            "/elevation-profile/:track_id",
            "get_elevation_profile",
        )
        .route(HttpMethod::Get, "/stats/:track_id", "get_track_stats")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_api_routes_correct() {
        let handle = terrain_api_routes();
        assert_eq!(handle.base_path, "terrain");
        assert_eq!(handle.routes.len(), 2);

        assert_eq!(handle.routes[0].method, HttpMethod::Get);
        assert_eq!(handle.routes[0].path, "/elevation-profile/:track_id");
        assert_eq!(handle.routes[0].handler_id, "get_elevation_profile");

        assert_eq!(handle.routes[1].method, HttpMethod::Get);
        assert_eq!(handle.routes[1].path, "/stats/:track_id");
        assert_eq!(handle.routes[1].handler_id, "get_track_stats");
    }
}
