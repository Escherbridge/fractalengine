//! GeoParquet 1.0 read/write for entity snapshots — see `src/AGENTS.md` §geoparquet.

#![cfg(feature = "parquet")]

mod codec;

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use fe_entity_store::EntitySnapshot;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::format::KeyValue;

/// Parquet file-metadata key carrying the GeoParquet metadata document.
const GEO_KEY: &str = "geo";
/// Geometry column name assumed when a file lacks `geo` metadata.
const DEFAULT_GEOMETRY_COLUMN: &str = "position";

/// GeoParquet 1.0 file-level metadata inputs for the nodes table.
pub struct GeoParquetMeta {
    /// Name of the column that holds geometry data.
    pub primary_geometry_column: String,
    /// CRS descriptor — petal-local meters by default, never a silent EPSG:4326 (see AGENTS.md §geoparquet).
    pub crs: String,
    /// Geometry encoding format (GeoParquet 1.0 mandates `"WKB"`).
    pub encoding: String,
}

impl Default for GeoParquetMeta {
    fn default() -> Self {
        Self {
            primary_geometry_column: DEFAULT_GEOMETRY_COLUMN.into(),
            crs: "PETAL-LOCAL:meters;origin=unset".into(),
            encoding: "WKB".into(),
        }
    }
}

impl GeoParquetMeta {
    /// Render the GeoParquet 1.0 `geo` file-metadata JSON document.
    pub fn geo_metadata_json(&self) -> Result<String> {
        let doc = serde_json::json!({
            "version": "1.0.0",
            "primary_column": self.primary_geometry_column,
            "columns": {
                &self.primary_geometry_column: {
                    "encoding": self.encoding,
                    "geometry_types": ["Point Z"],
                    "crs": self.crs,
                }
            }
        });
        serde_json::to_string(&doc).context("serializing geo metadata")
    }
}

/// Write entity snapshots to a GeoParquet file with default (petal-local) metadata.
pub fn write_nodes_parquet(path: &Path, snapshots: &[EntitySnapshot]) -> Result<usize> {
    write_nodes_parquet_with_meta(path, snapshots, &GeoParquetMeta::default())
}

/// Write entity snapshots to a GeoParquet file with caller-supplied metadata.
pub fn write_nodes_parquet_with_meta(
    path: &Path,
    snapshots: &[EntitySnapshot],
    meta: &GeoParquetMeta,
) -> Result<usize> {
    let schema = Arc::new(codec::nodes_schema(&meta.primary_geometry_column));
    let batch = codec::snapshots_to_batch(schema.clone(), snapshots)?;
    let file =
        File::create(path).with_context(|| format!("creating parquet file {}", path.display()))?;
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new(
            GEO_KEY.to_string(),
            meta.geo_metadata_json()?,
        )]))
        .build();
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).context("opening parquet writer")?;
    writer.write(&batch).context("writing record batch")?;
    writer.close().context("closing parquet writer")?;
    Ok(snapshots.len())
}

/// Read entity snapshots back from a GeoParquet file.
pub fn read_nodes_parquet(path: &Path) -> Result<Vec<EntitySnapshot>> {
    let file =
        File::open(path).with_context(|| format!("opening parquet file {}", path.display()))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).context("reading parquet footer")?;
    let geometry_column =
        geometry_column_from_meta(builder.metadata().file_metadata().key_value_metadata());
    let reader = builder.build().context("building parquet reader")?;
    let mut out = Vec::new();
    for batch in reader {
        codec::batch_to_snapshots(
            &batch.context("decoding record batch")?,
            &geometry_column,
            &mut out,
        )?;
    }
    Ok(out)
}

/// Read the raw GeoParquet `geo` file-metadata JSON, if present.
pub fn read_geo_metadata(path: &Path) -> Result<Option<String>> {
    let file =
        File::open(path).with_context(|| format!("opening parquet file {}", path.display()))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).context("reading parquet footer")?;
    Ok(builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kvs| kvs.iter().find(|kv| kv.key == GEO_KEY))
        .and_then(|kv| kv.value.clone()))
}

/// Resolve the geometry column name from the file's `geo` metadata (fallback: `position`).
fn geometry_column_from_meta(kvs: Option<&Vec<KeyValue>>) -> String {
    kvs.and_then(|kvs| kvs.iter().find(|kv| kv.key == GEO_KEY))
        .and_then(|kv| kv.value.as_deref())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|doc| doc.get("primary_column")?.as_str().map(str::to_string))
        .unwrap_or_else(|| DEFAULT_GEOMETRY_COLUMN.to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("fe_query_geoparquet_{name}_{}.parquet", std::process::id()))
    }

    fn snap(id: &str, pos: [f32; 3], props: Option<serde_json::Value>, ts: u64) -> EntitySnapshot {
        EntitySnapshot {
            node_id: id.into(),
            petal_id: "p1".into(),
            position: pos,
            rotation: [0.1, 0.2, 0.3],
            scale: [1.0, 2.0, 3.0],
            properties: props,
            updated_at_ms: ts,
            node_log: vec![],
        }
    }

    #[test]
    fn geoparquet_meta_defaults_are_local_meters() {
        let meta = GeoParquetMeta::default();
        assert_eq!(meta.primary_geometry_column, "position");
        assert_eq!(meta.encoding, "WKB");
        // Local meters must never masquerade as EPSG:4326 degrees.
        assert!(meta.crs.contains("PETAL-LOCAL"));
        assert!(!meta.crs.contains("4326"));
    }

    #[test]
    fn round_trip_preserves_rows_and_geo_metadata() {
        let path = tmp("round_trip");
        let snaps = vec![
            snap(
                "n1",
                [1.5, -2.25, 3.0],
                Some(serde_json::json!({"gis.annotation.title": "alpha", "depth": 4})),
                1000,
            ),
            snap("n2", [4.0, 5.0, -6.5], None, 2000),
        ];
        let count = write_nodes_parquet(&path, &snaps).unwrap();
        assert_eq!(count, 2);

        let back = read_nodes_parquet(&path).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in snaps.iter().zip(&back) {
            assert_eq!(a.node_id, b.node_id);
            assert_eq!(a.petal_id, b.petal_id);
            assert_eq!(a.position, b.position);
            assert_eq!(a.rotation, b.rotation);
            assert_eq!(a.scale, b.scale);
            assert_eq!(a.properties, b.properties);
            assert_eq!(a.updated_at_ms, b.updated_at_ms);
            assert!(b.node_log.is_empty());
        }

        let raw = read_geo_metadata(&path).unwrap().expect("geo metadata present");
        let geo: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(geo["version"], "1.0.0");
        assert_eq!(geo["primary_column"], "position");
        assert_eq!(geo["columns"]["position"]["encoding"], "WKB");
        assert_eq!(geo["columns"]["position"]["geometry_types"][0], "Point Z");
        assert!(geo["columns"]["position"]["crs"].as_str().unwrap().contains("PETAL-LOCAL"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_slice_writes_readable_file() {
        let path = tmp("empty");
        assert_eq!(write_nodes_parquet(&path, &[]).unwrap(), 0);
        assert!(read_nodes_parquet(&path).unwrap().is_empty());
        assert!(read_geo_metadata(&path).unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_read_is_err_not_panic() {
        let path = tmp("corrupt");
        std::fs::write(&path, b"definitely not a parquet file").unwrap();
        assert!(read_nodes_parquet(&path).is_err());
        assert!(read_geo_metadata(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn custom_crs_honored_in_geo_metadata() {
        let path = tmp("custom_crs");
        let meta = GeoParquetMeta {
            crs: "PETAL-LOCAL:meters;origin=47.6062,-122.3321,56.0".into(),
            ..Default::default()
        };
        write_nodes_parquet_with_meta(&path, &[snap("n1", [0.0, 0.0, 0.0], None, 1)], &meta)
            .unwrap();
        let raw = read_geo_metadata(&path).unwrap().expect("geo metadata present");
        let geo: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(geo["columns"]["position"]["crs"]
            .as_str()
            .unwrap()
            .contains("origin=47.6062,-122.3321,56.0"));
        let _ = std::fs::remove_file(&path);
    }
}
