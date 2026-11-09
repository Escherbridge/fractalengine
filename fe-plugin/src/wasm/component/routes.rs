//! Host side of the `routes` import: read-only geometry over a route node of the bound petal,
//! one shared implementation (fe-sdk path_geometry); see src/wasm/AGENTS.md §routes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fe_sdk::path_geometry::{
    cumulative_distances_f64, decode_anchor_rows, flatten_anchor_path, position_at_progress_f64,
    snap_to_polyline, FLATTEN_SAMPLES_PER_SEGMENT,
};
use fe_sdk::plugin_host::NodeReader;

use super::bindings::fractalengine::plugin::routes;
use super::bindings::fractalengine::plugin::types::{HostError, NodeId, Vec3};
use super::imports::vec3;
use super::state::{context_error, denied, invalid, InstanceState};
use crate::capability::{validate_access, CapabilityOperation};

/// Renderer route source: anchor rows with optional Bezier handles (petal-local metres).
pub const ROUTE_GEOMETRY_KEY: &str = "gpx_points";
/// Legacy `[{lat, ele, lon}]` points, read exactly as REST tracking reads them.
pub const LEGACY_ROUTE_KEY: &str = "route_points";
/// Routes kept flattened per instance.
const MAX_CACHED_ROUTES: usize = 32;

struct CachedRoute {
    /// The raw property the polyline came from; a different value rebuilds it.
    source: (&'static str, serde_json::Value),
    points: Vec<[f64; 3]>,
    cumdist: Vec<f64>,
    total: f64,
}

/// Flattened routes of one instance, rebuilt when the route node's geometry changes.
#[derive(Default)]
pub struct RouteCache {
    routes: HashMap<String, CachedRoute>,
}

impl RouteCache {
    /// Drop one route (the `host/node-changes` feed calls this through the instance).
    pub fn invalidate(&mut self, node_id: &str) {
        self.routes.remove(node_id);
    }

    fn resolve(&mut self, node_id: &str, source: (&'static str, serde_json::Value)) -> Result<&CachedRoute, HostError> {
        let stale = self
            .routes
            .get(node_id)
            .is_none_or(|cached| cached.source != source);
        if stale {
            let points = polyline(source.0, &source.1)?;
            if self.routes.len() >= MAX_CACHED_ROUTES && !self.routes.contains_key(node_id) {
                self.routes.clear();
            }
            let (cumdist, total) = cumulative_distances_f64(&points);
            self.routes
                .insert(node_id.to_string(), CachedRoute { source, points, cumdist, total });
        }
        self.routes
            .get(node_id)
            .ok_or_else(|| HostError::Backend("route cache lost an entry".into()))
    }
}

/// Decode a route property into a polyline of at least two finite points.
fn polyline(key: &str, value: &serde_json::Value) -> Result<Vec<[f64; 3]>, HostError> {
    let points: Vec<[f64; 3]> = if key == ROUTE_GEOMETRY_KEY {
        let anchors = decode_anchor_rows(value).ok_or_else(|| invalid("route geometry is malformed"))?;
        flatten_anchor_path(&anchors, FLATTEN_SAMPLES_PER_SEGMENT)
            .into_iter()
            .map(|point| [f64::from(point[0]), f64::from(point[1]), f64::from(point[2])])
            .collect()
    } else {
        value
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| Some([row["lat"].as_f64()?, row["ele"].as_f64().unwrap_or(0.0), row["lon"].as_f64()?]))
                    .collect()
            })
            .unwrap_or_default()
    };
    if points.iter().flatten().any(|component| !component.is_finite()) {
        return Err(invalid("route geometry is not finite"));
    }
    if points.len() < 2 {
        return Err(invalid("route has fewer than two points"));
    }
    Ok(points)
}

impl InstanceState {
    /// The route node's raw geometry property, read under the bound petal pin.
    fn route_source(&self, route: &str) -> Result<(&'static str, serde_json::Value), HostError> {
        self.lease_active()?;
        if !self.grants().is_some_and(|grants| grants.routes) {
            return Err(denied("routes"));
        }
        fe_sdk::storage::validate_node_id(route).map_err(|error| invalid(error.to_string()))?;
        self.take_read()?;
        let node = match &self.geometry_reader {
            Some(reader) => self.read_route_node(reader, route)?,
            None => self.context.get_node(route).map_err(context_error)?,
        };
        let node = node.ok_or_else(|| invalid(format!("{route} is not a node of the bound petal")))?;
        for key in [ROUTE_GEOMETRY_KEY, LEGACY_ROUTE_KEY] {
            if let Some(value) = node.properties.get(key) {
                return Ok((key, fe_sdk::plugin_host::property_json(value)));
            }
        }
        Err(invalid(format!("{route} carries no route geometry")))
    }

    /// Unfiltered read (geometry needs no property scope), with the same petal and verse pin as the context.
    fn read_route_node(
        &self,
        reader: &Arc<Mutex<dyn NodeReader>>,
        route: &str,
    ) -> Result<Option<crate::NodeSnapshot>, HostError> {
        let petal = self.context.petal_id();
        let token = self.context.capabilities();
        let pinned_scope = |reader: &dyn NodeReader| -> Result<String, HostError> {
            let scope = reader.petal_scope(petal).map_err(HostError::Backend)?;
            let parts = fe_policy::parse_scope(&scope).map_err(|error| HostError::Backend(error.to_string()))?;
            if parts.petal_id.as_deref() != Some(petal) {
                return Err(HostError::Backend("reader returned an unrelated petal scope".into()));
            }
            validate_access(token, &CapabilityOperation::PetalAccess { scope: scope.clone() })
                .and_then(|()| {
                    validate_access(
                        token,
                        &CapabilityOperation::VerseAccess { scope: format!("VERSE#{}", parts.verse_id) },
                    )
                })
                .map_err(|error| HostError::Revoked(error.to_string()))?;
            Ok(scope)
        };
        let reader = reader
            .lock()
            .map_err(|_| HostError::Backend("route reader lock poisoned".into()))?;
        let before = pinned_scope(&*reader)?;
        let node = reader.get_node(petal, route).map_err(HostError::Backend)?;
        if pinned_scope(&*reader)? != before {
            return Err(HostError::Revoked("petal ancestry changed during read".into()));
        }
        match node {
            Some(node) if node.node_id != route || node.petal_id != petal => {
                Err(HostError::Backend("reader returned an unrelated node".into()))
            }
            other => Ok(other),
        }
    }

    fn cached_route(&mut self, route: &str) -> Result<&CachedRoute, HostError> {
        let source = self.route_source(route)?;
        self.routes.resolve(route, source)
    }
}

/// Unit vector from `a` to `b`, zero on a degenerate segment.
fn direction(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let delta = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    if length > 1e-9 {
        [delta[0] / length, delta[1] / length, delta[2] / length]
    } else {
        [0.0; 3]
    }
}

impl routes::Host for InstanceState {
    fn info(&mut self, route: NodeId) -> Result<routes::RouteInfo, HostError> {
        let cached = self.cached_route(&route)?;
        Ok(routes::RouteInfo {
            length_m: cached.total,
            point_count: cached.points.len() as u32,
        })
    }

    fn sample(&mut self, route: NodeId, progress: f64) -> Result<routes::RouteSample, HostError> {
        if !progress.is_finite() {
            return Err(invalid("progress must be finite"));
        }
        let progress = progress.clamp(0.0, 1.0);
        let cached = self.cached_route(&route)?;
        let (points, last) = (&cached.points, cached.points.len() - 1);
        let target = progress * cached.total;
        // Same segment selection as position_at_progress_f64.
        let segment = cached
            .cumdist
            .iter()
            .position(|&distance| distance > target)
            .unwrap_or(last)
            .saturating_sub(1)
            .min(last - 1);
        Ok(routes::RouteSample {
            position: vec3(position_at_progress_f64(points, &cached.cumdist, cached.total, progress)),
            tangent: vec3(direction(points[segment], points[segment + 1])),
            distance_m: target,
            segment_index: segment as u32,
        })
    }

    fn snap(&mut self, route: NodeId, position: Vec3) -> Result<routes::SnapResult, HostError> {
        let position = [position.x, position.y, position.z];
        if position.iter().any(|component| !component.is_finite()) {
            return Err(invalid("position must be finite"));
        }
        let cached = self.cached_route(&route)?;
        let snap = snap_to_polyline(&cached.points, &cached.cumdist, cached.total, position);
        Ok(routes::SnapResult {
            nearest: vec3(snap.nearest_point),
            segment_index: snap.segment_index as u32,
            progress: snap.route_progress,
            distance_remaining_m: snap.distance_remaining_m,
            deviation_m: snap.deviation_m,
        })
    }
}
