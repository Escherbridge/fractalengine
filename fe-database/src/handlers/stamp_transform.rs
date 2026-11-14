//! Transform writes on a promoted stamp mirror into its `stamp.override.*` properties; see handlers/AGENTS.md §stamp-transform.

use bevy::math::{EulerRot, Quat};
use serde_json::{json, Value};

use super::field_def;
use crate::repo::Db;

/// Per-stamp scale override key the renderer reads ([sx, sy, sz]).
pub(crate) const STAMP_SCALE_KEY: &str = "stamp.override.scale";
/// Per-stamp rotation override key the renderer reads (quaternion [x, y, z, w]).
pub(crate) const STAMP_ROTATION_KEY: &str = "stamp.override.rotation";

/// Quaternion [x, y, z, w] for a radian Euler triple in the Inspector/gimbal order (`EulerRot::XYZ`).
pub(crate) fn euler_to_quat_xyzw(rotation: [f32; 3]) -> [f32; 4] {
    Quat::from_euler(EulerRot::XYZ, rotation[0], rotation[1], rotation[2])
        .normalize()
        .to_array()
}

/// Guarded property writes and journal changes that mirror a stamp's transform into its overrides.
pub(crate) struct StampOverrideMirror {
    pub statements: Vec<String>,
    pub bindings: Vec<(String, Value)>,
    pub changes: Vec<(String, Value, Value)>,
}

/// Validate and stage the scale/rotation override writes for stamp `$node_id` under `scope`.
pub(crate) async fn mirror_overrides(
    db: &Db,
    scope: &str,
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<StampOverrideMirror> {
    anyhow::ensure!(
        scale.iter().all(|v| v.is_finite() && (0.01..=10_000.0).contains(v)),
        "stamp scale must be finite and 0.01..=10000"
    );
    let values = [
        (STAMP_SCALE_KEY, json!(scale)),
        (STAMP_ROTATION_KEY, json!(euler_to_quat_xyzw(rotation))),
    ];
    let mut mirror = StampOverrideMirror {
        statements: Vec::new(),
        bindings: Vec::new(),
        changes: Vec::new(),
    };
    for (index, (key, value)) in values.into_iter().enumerate() {
        let rules = field_def::capture_property_rules(db, scope, key).await?;
        let schemas = field_def::validate_captured_property_rules(scope, key, &value, &rules)?;
        let rule_name = format!("stamp_transform_rule_{index}");
        let key_name = format!("stamp_transform_key_{index}");
        let value_name = format!("stamp_transform_value_{index}");
        mirror.statements.push(format!(
            "IF ({}) != ${rule_name} {{ THROW 'stamp schema changed during save'; }}",
            field_def::PROPERTY_RULES_SQL
                .replace("$key", &format!("${key_name}"))
                .replace("$scopes", "$stamp_transform_scopes")
        ));
        mirror.statements.push(format!(
            "UPDATE node SET properties[${key_name}] = ${value_name} WHERE node_id = $node_id"
        ));
        mirror.bindings.extend([
            (rule_name, json!(rules)),
            (key_name, json!(key)),
            (value_name, value.clone()),
        ]);
        mirror.changes.push((
            "property_set".into(),
            json!({"key": key, "value": value}),
            schemas,
        ));
    }
    mirror.bindings.push((
        "stamp_transform_scopes".into(),
        json!(field_def::property_rule_scopes(scope)),
    ));
    Ok(mirror)
}

#[cfg(test)]
mod tests {
    use super::euler_to_quat_xyzw;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn euler_to_quat_xyzw_matches_the_inspector_order() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let cases: [([f32; 3], [f32; 4]); 3] = [
            ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            ([0.0, FRAC_PI_2, 0.0], [0.0, half, 0.0, half]),
            // Intrinsic X then Y: Rx(90) * Ry(90); the reversed order would give z = -0.5.
            ([FRAC_PI_2, FRAC_PI_2, 0.0], [0.5, 0.5, 0.5, 0.5]),
        ];
        for (euler, expected) in cases {
            let quat = euler_to_quat_xyzw(euler);
            let sign = if quat[3] < 0.0 { -1.0 } else { 1.0 };
            for (got, want) in quat.iter().zip(expected) {
                assert!((got * sign - want).abs() < 1e-6, "{euler:?} -> {quat:?}, want {expected:?}");
            }
        }
    }
}
