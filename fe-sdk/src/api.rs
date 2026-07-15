//! API extension handle for plugins that expose HTTP routes.
//!
//! Extensions can declare additional API routes via [`ApiExtensionHandle`].
//! The engine merges these into the main axum router under a namespaced
//! prefix (e.g. `/api/v1/ext/{base_path}/...`).

use serde::{Deserialize, Serialize};

/// HTTP method for an extension route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// A single route declared by an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionRoute {
    /// The HTTP method this route handles.
    pub method: HttpMethod,
    /// The path relative to the extension's base path (e.g. "/elevation-profile/:track_id").
    pub path: String,
    /// A unique identifier for the handler function within the extension.
    pub handler_id: String,
}

/// Describes the HTTP routes an extension wants to expose.
///
/// The engine mounts these under `/api/v1/ext/{base_path}/...`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiExtensionHandle {
    /// Base path segment for all routes (e.g. "terrain").
    pub base_path: String,
    /// The set of routes this extension declares.
    pub routes: Vec<ExtensionRoute>,
}

impl ApiExtensionHandle {
    /// Create a new handle with the given base path and no routes.
    pub fn new(base_path: impl Into<String>) -> Self {
        Self {
            base_path: base_path.into(),
            routes: Vec::new(),
        }
    }

    /// Add a route to this handle.
    pub fn route(
        mut self,
        method: HttpMethod,
        path: impl Into<String>,
        handler_id: impl Into<String>,
    ) -> Self {
        self.routes.push(ExtensionRoute {
            method,
            path: path.into(),
            handler_id: handler_id.into(),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_extension_handle_builder() {
        let handle = ApiExtensionHandle::new("terrain")
            .route(
                HttpMethod::Get,
                "/elevation-profile/:track_id",
                "get_elevation_profile",
            )
            .route(HttpMethod::Get, "/stats/:track_id", "get_track_stats");

        assert_eq!(handle.base_path, "terrain");
        assert_eq!(handle.routes.len(), 2);
        assert_eq!(handle.routes[0].method, HttpMethod::Get);
        assert_eq!(handle.routes[0].path, "/elevation-profile/:track_id");
        assert_eq!(handle.routes[0].handler_id, "get_elevation_profile");
        assert_eq!(handle.routes[1].path, "/stats/:track_id");
    }

    #[test]
    fn api_extension_handle_serde_roundtrip() {
        let handle =
            ApiExtensionHandle::new("terrain").route(HttpMethod::Post, "/import", "import_data");

        let json = serde_json::to_string(&handle).unwrap();
        let roundtripped: ApiExtensionHandle = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtripped.base_path, "terrain");
        assert_eq!(roundtripped.routes.len(), 1);
        assert_eq!(roundtripped.routes[0].method, HttpMethod::Post);
    }
}
