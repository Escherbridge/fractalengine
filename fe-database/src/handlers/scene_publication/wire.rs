//! Versioned full-node wire shape; see scene_publication/AGENTS.md.

use serde_json::{json, Value};

pub(crate) const MARKER: &str = "_fe_scene";
pub(crate) const MAX_ROW_BYTES: usize = 96 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SceneClock {
    pub hlc: u64,
    pub publisher: String,
}

impl SceneClock {
    pub(crate) fn from_marker(row: &Value) -> anyhow::Result<Option<Self>> {
        let Some(marker) = row.get(MARKER) else {
            return Ok(None);
        };
        let marker = marker
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("scene marker is not an object"))?;
        anyhow::ensure!(
            marker.len() == 4
                && marker.get("version") == Some(&json!(1))
                && marker.get("complete") == Some(&json!(true)),
            "unsupported scene snapshot marker"
        );
        Ok(Some(Self::from_fields(
            &row[MARKER]["hlc"],
            &row[MARKER]["publisher"],
        )?))
    }

    pub(crate) fn from_fields(hlc: &Value, publisher: &Value) -> anyhow::Result<Self> {
        let text = hlc
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("scene HLC is not a decimal string"))?;
        let hlc: u64 = text.parse()?;
        let publisher = publisher
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or_else(|| anyhow::anyhow!("invalid scene publisher"))?;
        anyhow::ensure!(hlc > 0 && hlc.to_string() == text, "invalid scene HLC");
        Ok(Self {
            hlc,
            publisher: publisher.into(),
        })
    }

    pub(crate) fn marker(&self) -> Value {
        json!({"version":1,"complete":true,"hlc":self.hlc.to_string(),"publisher":self.publisher})
    }
}

/// A live complete snapshot `author` published, as its clock and petal; `None` for a
/// tombstone, a partial (markerless) row or another publisher's marker (QA-0164 lane).
pub(crate) fn live_snapshot_of(bytes: &[u8], author: &str) -> Option<(SceneClock, String)> {
    let row: Value = serde_json::from_slice(bytes).ok()?;
    if row.get("tombstone").is_some_and(|value| !value.is_null()) {
        return None;
    }
    let clock = SceneClock::from_marker(&row).ok()??;
    let petal = row.get("petal_id")?.as_str()?.to_owned();
    (clock.publisher == author).then_some((clock, petal))
}

/// Normalize only nullable node columns; property JSON and `_fe_clock` are kept intact.
pub(crate) fn normalize_optional_columns(row: &mut Value) {
    if let Some(object) = row.as_object_mut() {
        object.remove("id");
        for key in ["display_name", "asset_id", "properties", "tombstone"] {
            if object.get(key).is_some_and(Value::is_null) {
                object.remove(key);
            }
        }
    }
}

pub(crate) fn validate_complete(row: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        serde_json::to_vec(row)?.len() <= MAX_ROW_BYTES,
        "scene snapshot exceeds 96 KiB"
    );
    let object = row
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("scene snapshot is not an object"))?;
    const FIELDS: &[&str] = &[
        "node_id",
        "petal_id",
        "display_name",
        "asset_id",
        "position",
        "elevation",
        "rotation",
        "scale",
        "interactive",
        "created_at",
        "edit_seq",
        "properties",
        "tombstone",
        MARKER,
        crate::field_clock::CLOCK_COLUMN,
    ];
    anyhow::ensure!(
        object.keys().all(|key| FIELDS.contains(&key.as_str())),
        "unknown scene snapshot column"
    );
    for key in ["node_id", "petal_id", "created_at"] {
        anyhow::ensure!(
            row[key]
                .as_str()
                .is_some_and(|value| !value.is_empty() && value.len() <= 1024),
            "invalid scene {key}"
        );
    }
    for key in ["display_name", "asset_id"] {
        anyhow::ensure!(
            row.get(key)
                .is_none_or(|value| value.is_null()
                    || value.as_str().is_some_and(|value| value.len() <= 1024)),
            "invalid optional scene {key}"
        );
    }
    anyhow::ensure!(
        row["position"]["type"] == "Point"
            && row["position"]["coordinates"]
                .as_array()
                .is_some_and(|values| values.len() == 2 && values.iter().all(finite_render_number))
            && finite_render_number(&row["elevation"])
            && row["interactive"].is_boolean()
            && row["edit_seq"].as_i64().is_some_and(|value| value >= 0),
        "invalid scene geometry or edit state"
    );
    let rotation: Vec<f64> = serde_json::from_value(row["rotation"].clone())?;
    anyhow::ensure!(
        fe_runtime::node_history::stored_rotation_quaternion(&rotation).is_some(),
        "scene rotation is unavailable to renderer"
    );
    for (key, lengths) in [("rotation", &[3, 4][..]), ("scale", &[3][..])] {
        anyhow::ensure!(
            row[key]
                .as_array()
                .is_some_and(|values| lengths.contains(&values.len())
                    && values.iter().all(finite_render_number)),
            "invalid scene {key}"
        );
    }
    for key in ["properties", "tombstone"] {
        anyhow::ensure!(
            row.get(key)
                .is_none_or(|value| value.is_null() || value.is_object()),
            "invalid optional scene {key}"
        );
    }
    let clock =
        SceneClock::from_marker(row)?.ok_or_else(|| anyhow::anyhow!("scene marker missing"))?;
    // Optional per-key clock: strictest reader, authored by the snapshot publisher.
    crate::field_clock::parse_own("node", row, &clock.publisher)?;
    if let Some(marker) = row.get("tombstone").filter(|value| !value.is_null()) {
        anyhow::ensure!(
            marker["hlc"].as_u64() == Some(clock.hlc)
                && marker["source_did"]
                    .as_str()
                    .is_some_and(|source| !source.is_empty() && source.len() <= 512),
            "complete tombstone has no matching original commit clock or actor"
        );
    }
    Ok(())
}

fn finite_render_number(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|value| value.is_finite() && (value as f32).is_finite())
}
