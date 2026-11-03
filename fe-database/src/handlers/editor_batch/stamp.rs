//! Canonical stamp source and exact override-presence validation.

use fe_runtime::{editor_batch::*, messages::StampOverrideEdit};
use serde_json::Value;
use std::collections::BTreeSet;

fn default_arc(
    edit: &StampOverrideEdit,
    member: &EditorBatchMember,
    world_scale: f32,
) -> anyhow::Result<f32> {
    let source = member
        .stamp_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("promoted stamp source unavailable"))?;
    let points = source
        .state
        .properties
        .get("gpx_points")
        .ok_or_else(|| anyhow::anyhow!("stamp route missing"))?;
    let anchors = fe_sdk::path_geometry::decode_anchor_rows(points)
        .ok_or_else(|| anyhow::anyhow!("stamp route unavailable"))?;
    let points = fe_sdk::path_geometry::flatten_anchor_path(
        &anchors,
        fe_sdk::path_geometry::FLATTEN_SAMPLES_PER_SEGMENT,
    );
    let total = fe_sdk::path_geometry::polyline_length(&points);
    let descriptor = fe_sdk::path_asset::PathAssetDescriptor::from_json(
        source
            .state
            .properties
            .get("path_asset")
            .ok_or_else(|| anyhow::anyhow!("stamp descriptor missing"))?,
    )?;
    let progresses =
        fe_sdk::path_asset::sample_progresses(&points, &descriptor, total, world_scale);
    let progress = progresses
        .get(edit.instance_index)
        .ok_or_else(|| anyhow::anyhow!("stamp sampler index unavailable"))?;
    Ok(*progress * total / world_scale)
}

pub(super) fn validate(
    edit: &StampOverrideEdit,
    member: &EditorBatchMember,
    scope: &str,
    world_scale: f32,
) -> anyhow::Result<()> {
    let source = member
        .stamp_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("promoted stamp source unavailable"))?;
    anyhow::ensure!(
        edit.node_id == member.node_id
            && edit.scope == scope
            && edit.history_version == member.history_version
            && edit.path_id == source.state.node_id
            && edit.path_version == source.history_version
            && edit.before_properties.as_object() == Some(&member.state.properties),
        "stamp edit does not match the complete selection capture"
    );
    let default = default_arc(edit, member, world_scale)?;
    let arc = member
        .state
        .properties
        .get("stamp.override.arc_m")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .unwrap_or(default);
    anyhow::ensure!(
        edit.arc_m == arc,
        "group transforms cannot move a stamp's path-derived origin"
    );
    Ok(())
}

pub(super) fn value(representation: &EditorStampOverrideValue) -> Option<Value> {
    match representation {
        EditorStampOverrideValue::Absent => None,
        EditorStampOverrideValue::Null => Some(Value::Null),
        EditorStampOverrideValue::Value(value) => Some(value.clone()),
    }
}

pub(super) fn validate_restore(
    edit: &StampOverrideEdit,
    overrides: &EditorStampOverrideSet,
    member: &EditorBatchMember,
    world_scale: f32,
) -> anyhow::Result<()> {
    if let EditorStampOverrideValue::Value(value) = &overrides.scale {
        let values = components(value, 3)?;
        anyhow::ensure!(
            values.iter().all(|v| (0.01..=10000.0).contains(v))
                && values.iter().map(|v| *v as f32).eq(edit.scale),
            "restored scale does not match the checked stamp target"
        );
    }
    if let EditorStampOverrideValue::Value(value) = &overrides.rotation {
        let values = components(value, 4)?;
        anyhow::ensure!(
            (values.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() <= 0.0001
                && values.iter().map(|v| *v as f32).eq(edit.rotation),
            "restored rotation does not match the checked stamp target"
        );
    }
    let arc = match &overrides.arc_m {
        EditorStampOverrideValue::Absent | EditorStampOverrideValue::Null => {
            default_arc(edit, member, world_scale)?
        }
        EditorStampOverrideValue::Value(value) => {
            let value = value
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..=1_000_000.0).contains(v))
                .ok_or_else(|| anyhow::anyhow!("restored stamp arc is invalid"))?;
            value as f32
        }
    };
    anyhow::ensure!(
        arc == edit.arc_m,
        "restoring inheritance cannot move a stamp's path-derived origin"
    );
    Ok(())
}

fn components(value: &Value, count: usize) -> anyhow::Result<Vec<f64>> {
    let values = value
        .as_array()
        .filter(|v| v.len() == count)
        .ok_or_else(|| anyhow::anyhow!("restored stamp vector has invalid shape"))?;
    values
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|v| v.is_finite() && (*v as f32).is_finite())
                .ok_or_else(|| anyhow::anyhow!("restored stamp vector must be finite"))
        })
        .collect()
}

pub(super) fn namespace(
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    prefix: &str,
) -> (Vec<String>, Vec<(String, Value)>) {
    let names: BTreeSet<_> = bindings.iter().map(|(name, _)| name.as_str()).collect();
    let statements = statements
        .into_iter()
        .map(|sql| {
            let mut out = String::with_capacity(sql.len());
            let mut chars = sql.chars().peekable();
            while let Some(ch) = chars.next() {
                out.push(ch);
                if ch == '$' {
                    let mut name = String::new();
                    while chars
                        .peek()
                        .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    {
                        name.push(chars.next().unwrap());
                    }
                    if names.contains(name.as_str()) {
                        out.push_str(prefix);
                        out.push('_');
                    }
                    out.push_str(&name);
                }
            }
            out
        })
        .collect();
    drop(names);
    (
        statements,
        bindings
            .into_iter()
            .map(|(name, value)| (format!("{prefix}_{name}"), value))
            .collect(),
    )
}
