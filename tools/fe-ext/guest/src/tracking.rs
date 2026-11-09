//! Snap-to-alert: the same four metrics and `off_route` alert the REST transform path writes for a
//! node with `tracking_route_id` (fe-api `check_tracking_and_alert`), staged instead of sent.

use serde_json::{json, Value};

use crate::routes::SnapResult;
use crate::{window, HostError, Vec3};

/// REST's default `deviation_threshold_m`.
pub const DEFAULT_DEVIATION_THRESHOLD_M: f64 = 50.0;

/// The properties REST would write for a snap; the alert appears only when deviation is strictly over the threshold.
pub fn tracking_properties(
    progress: f64,
    distance_remaining_m: f64,
    deviation_m: f64,
    segment_index: u32,
    threshold_m: f64,
) -> Vec<(&'static str, Value)> {
    let mut properties = vec![
        ("route_progress", json!(progress)),
        ("distance_remaining_m", json!(distance_remaining_m)),
        ("deviation_m", json!(deviation_m)),
        ("nearest_segment_index", json!(segment_index)),
    ];
    if deviation_m > threshold_m {
        properties.push(("tracking_alert", json!("off_route")));
    }
    properties
}

/// The outcome of one snap: the host's numbers and whether the alert fired.
#[derive(Debug, Clone)]
pub struct Tracking {
    pub snap: SnapResult,
    pub off_route: bool,
}

/// Snap `position` to `route`, then stage the REST tracking properties on `node` (keys prefixed with `key_prefix`,
/// e.g. `"fleet."`, so they fall inside the manifest's `property_scope`; use `""` for REST's bare names).
/// Like REST, an alert is only ever set here; clearing it is the author's call.
pub fn snap_and_stage(
    node: &str,
    route: &str,
    position: [f64; 3],
    threshold_m: Option<f64>,
    key_prefix: &str,
) -> Result<Tracking, HostError> {
    let snap = crate::routes::snap(route, Vec3 { x: position[0], y: position[1], z: position[2] })?;
    let threshold = threshold_m.unwrap_or(DEFAULT_DEVIATION_THRESHOLD_M);
    let properties = tracking_properties(snap.progress, snap.distance_remaining_m, snap.deviation_m, snap.segment_index, threshold);
    let off_route = properties.iter().any(|(key, _)| *key == "tracking_alert");
    for (key, value) in properties {
        window::set_property(node, &format!("{key_prefix}{key}"), &value)?;
    }
    Ok(Tracking { snap, off_route })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_only_strictly_over_the_threshold() {
        let keys = |deviation: f64| {
            tracking_properties(0.5, 10.0, deviation, 2, 50.0).iter().map(|(key, _)| *key).collect::<Vec<_>>()
        };
        assert_eq!(keys(50.0).len(), 4);
        assert_eq!(keys(50.1).last(), Some(&"tracking_alert"));
    }
}
