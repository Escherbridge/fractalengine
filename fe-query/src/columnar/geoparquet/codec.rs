//! Snapshot ↔ Arrow/WKB codec for GeoParquet I/O — see `src/AGENTS.md` §geoparquet.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow::array::{
    Array, ArrayRef, BinaryArray, Float32Array, RecordBatch, StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use fe_entity_store::EntitySnapshot;

/// ISO WKB geometry type code for Point Z.
const WKB_POINT_Z: u32 = 1001;
/// Byte length of a little-endian ISO WKB Point Z (1 + 4 + 3×8).
const WKB_POINT_Z_LEN: usize = 29;

/// Arrow schema for the nodes table (rotation/scale flattened for BI friendliness).
pub(super) fn nodes_schema(geometry_column: &str) -> Schema {
    Schema::new(vec![
        Field::new("node_id", DataType::Utf8, false),
        Field::new("petal_id", DataType::Utf8, false),
        Field::new(geometry_column, DataType::Binary, false),
        Field::new("rotation_x", DataType::Float32, false),
        Field::new("rotation_y", DataType::Float32, false),
        Field::new("rotation_z", DataType::Float32, false),
        Field::new("scale_x", DataType::Float32, false),
        Field::new("scale_y", DataType::Float32, false),
        Field::new("scale_z", DataType::Float32, false),
        Field::new("properties", DataType::Utf8, true),
        Field::new("updated_at_ms", DataType::UInt64, false),
    ])
}

/// Map snapshots into a single RecordBatch matching [`nodes_schema`].
pub(super) fn snapshots_to_batch(
    schema: Arc<Schema>,
    snapshots: &[EntitySnapshot],
) -> Result<RecordBatch> {
    let node_ids: StringArray = snapshots.iter().map(|s| Some(s.node_id.as_str())).collect();
    let petal_ids: StringArray = snapshots.iter().map(|s| Some(s.petal_id.as_str())).collect();
    let wkb: BinaryArray = snapshots.iter().map(|s| Some(point_z_to_wkb(s.position))).collect();
    let f32_values = |get: &dyn Fn(&EntitySnapshot) -> f32| -> ArrayRef {
        Arc::new(Float32Array::from_iter_values(snapshots.iter().map(get)))
    };
    let props: StringArray =
        snapshots.iter().map(|s| s.properties.as_ref().map(|v| v.to_string())).collect();
    let updated = UInt64Array::from_iter_values(snapshots.iter().map(|s| s.updated_at_ms));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(node_ids),
            Arc::new(petal_ids),
            Arc::new(wkb),
            f32_values(&|s| s.rotation[0]),
            f32_values(&|s| s.rotation[1]),
            f32_values(&|s| s.rotation[2]),
            f32_values(&|s| s.scale[0]),
            f32_values(&|s| s.scale[1]),
            f32_values(&|s| s.scale[2]),
            Arc::new(props),
            Arc::new(updated),
        ],
    )
    .context("building nodes RecordBatch")
}

/// Decode one RecordBatch back into snapshots (node_log is not exported; reads back empty).
pub(super) fn batch_to_snapshots(
    batch: &RecordBatch,
    geometry_column: &str,
    out: &mut Vec<EntitySnapshot>,
) -> Result<()> {
    let node_ids = str_col(batch, "node_id")?;
    let petal_ids = str_col(batch, "petal_id")?;
    let wkb = batch
        .column_by_name(geometry_column)
        .with_context(|| format!("missing geometry column `{geometry_column}`"))?
        .as_any()
        .downcast_ref::<BinaryArray>()
        .with_context(|| format!("geometry column `{geometry_column}` is not Binary"))?;
    let rot = [
        f32_col(batch, "rotation_x")?,
        f32_col(batch, "rotation_y")?,
        f32_col(batch, "rotation_z")?,
    ];
    let scale =
        [f32_col(batch, "scale_x")?, f32_col(batch, "scale_y")?, f32_col(batch, "scale_z")?];
    let props = str_col(batch, "properties")?;
    let updated = batch
        .column_by_name("updated_at_ms")
        .context("missing column `updated_at_ms`")?
        .as_any()
        .downcast_ref::<UInt64Array>()
        .context("column `updated_at_ms` is not UInt64")?;
    for i in 0..batch.num_rows() {
        let properties = if props.is_null(i) {
            None
        } else {
            Some(serde_json::from_str(props.value(i)).context("parsing properties JSON")?)
        };
        out.push(EntitySnapshot {
            node_id: node_ids.value(i).to_string(),
            petal_id: petal_ids.value(i).to_string(),
            position: wkb_to_point_z(wkb.value(i))?,
            rotation: [rot[0].value(i), rot[1].value(i), rot[2].value(i)],
            scale: [scale[0].value(i), scale[1].value(i), scale[2].value(i)],
            properties,
            updated_at_ms: updated.value(i),
            node_log: Vec::new(),
        });
    }
    Ok(())
}

/// Fetch a named Utf8 column from a batch.
fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("column `{name}` is not Utf8"))
}

/// Fetch a named Float32 column from a batch.
fn f32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float32Array> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<Float32Array>()
        .with_context(|| format!("column `{name}` is not Float32"))
}

/// Encode a petal-local position as little-endian ISO WKB Point Z.
fn point_z_to_wkb(position: [f32; 3]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(WKB_POINT_Z_LEN);
    buf.push(1u8); // little-endian
    buf.extend_from_slice(&WKB_POINT_Z.to_le_bytes());
    for component in position {
        buf.extend_from_slice(&f64::from(component).to_le_bytes());
    }
    buf
}

/// Decode a little-endian ISO WKB Point Z back to a petal-local position.
fn wkb_to_point_z(wkb: &[u8]) -> Result<[f32; 3]> {
    if wkb.len() != WKB_POINT_Z_LEN {
        bail!("WKB length {} != expected {WKB_POINT_Z_LEN}", wkb.len());
    }
    if wkb[0] != 1 {
        bail!("only little-endian WKB is supported (byte order marker {})", wkb[0]);
    }
    let geom_type = u32::from_le_bytes(wkb[1..5].try_into().context("WKB type bytes")?);
    if geom_type != WKB_POINT_Z {
        bail!("unsupported WKB geometry type {geom_type} (expected Point Z {WKB_POINT_Z})");
    }
    let mut position = [0f32; 3];
    for (i, slot) in position.iter_mut().enumerate() {
        let start = 5 + i * 8;
        let raw = f64::from_le_bytes(wkb[start..start + 8].try_into().context("WKB coord bytes")?);
        *slot = raw as f32;
    }
    Ok(position)
}
