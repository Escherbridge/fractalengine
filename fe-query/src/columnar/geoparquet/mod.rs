//! GeoParquet 1.0 read/write for entity snapshots — see `src/AGENTS.md` §geoparquet.

#![cfg(feature = "parquet")]

mod codec;

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use fe_entity_store::{EntitySnapshot, ReadingSnapshot};
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
    /// CRS descriptor — petal-local meters by default, never a silent EPSG:4326.
    /// DEC-C16: this label is NOT written to the spec `crs` key (which must be
    /// PROJJSON/null/absent); it is carried in the custom `fe:crs` key instead,
    /// alongside the x-fe-crs header and CSV `# crs=` line (see AGENTS.md §geoparquet).
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
    ///
    /// DEC-C16: `crs` is always spec-legal `null` (petal-local frames have no
    /// PROJJSON CRS; `null` means "unspecified" and claims nothing — unlike a
    /// silent EPSG:4326 default would). The honest free-text label still lives
    /// in `fe:crs`, a spec-tolerated custom key alongside `crs`.
    pub fn geo_metadata_json(&self) -> Result<String> {
        let doc = serde_json::json!({
            "version": "1.0.0",
            "primary_column": self.primary_geometry_column,
            "columns": {
                &self.primary_geometry_column: {
                    "encoding": self.encoding,
                    "geometry_types": ["Point Z"],
                    "crs": serde_json::Value::Null,
                    "fe:crs": self.crs,
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
    let file =
        File::create(path).with_context(|| format!("creating parquet file {}", path.display()))?;
    write_nodes_to(file, snapshots, meta)?;
    Ok(snapshots.len())
}

/// Write entity snapshots to an in-memory GeoParquet buffer (HTTP-egress shim for fe-api).
pub fn write_nodes_parquet_bytes(
    snapshots: &[EntitySnapshot],
    meta: &GeoParquetMeta,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_nodes_to(&mut buf, snapshots, meta)?;
    Ok(buf)
}

/// Shared writer core over any `Write` sink.
fn write_nodes_to<W: std::io::Write + Send>(
    sink: W,
    snapshots: &[EntitySnapshot],
    meta: &GeoParquetMeta,
) -> Result<()> {
    let schema = Arc::new(codec::nodes_schema(&meta.primary_geometry_column));
    let batch = codec::snapshots_to_batch(schema.clone(), snapshots)?;
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new(
            GEO_KEY.to_string(),
            meta.geo_metadata_json()?,
        )]))
        .build();
    let mut writer =
        ArrowWriter::try_new(sink, schema, Some(props)).context("opening parquet writer")?;
    writer.write(&batch).context("writing record batch")?;
    writer.close().context("closing parquet writer")?;
    Ok(())
}

/// Write IoT reading rows to an in-memory GeoParquet-shaped buffer (F11/A23
/// HTTP-egress shim for fe-api, sibling of [`write_nodes_parquet_bytes`]).
/// The anchor-position geometry column is nullable — see
/// `codec::readings_schema`.
pub fn write_readings_parquet_bytes(
    rows: &[ReadingSnapshot],
    meta: &GeoParquetMeta,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_readings_to(&mut buf, rows, meta)?;
    Ok(buf)
}

/// Shared readings writer core over any `Write` sink.
fn write_readings_to<W: std::io::Write + Send>(
    sink: W,
    rows: &[ReadingSnapshot],
    meta: &GeoParquetMeta,
) -> Result<()> {
    let schema = Arc::new(codec::readings_schema(&meta.primary_geometry_column));
    let batch = codec::readings_to_batch(schema.clone(), rows)?;
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new(
            GEO_KEY.to_string(),
            meta.geo_metadata_json()?,
        )]))
        .build();
    let mut writer =
        ArrowWriter::try_new(sink, schema, Some(props)).context("opening parquet writer")?;
    writer.write(&batch).context("writing record batch")?;
    writer.close().context("closing parquet writer")?;
    Ok(())
}

/// Read reading rows back from a GeoParquet-shaped file (test/read-back use).
pub fn read_readings_parquet(path: &Path) -> Result<Vec<ReadingSnapshot>> {
    let file =
        File::open(path).with_context(|| format!("opening parquet file {}", path.display()))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).context("reading parquet footer")?;
    let geometry_column =
        geometry_column_from_meta(builder.metadata().file_metadata().key_value_metadata());
    let reader = builder.build().context("building parquet reader")?;
    let mut out = Vec::new();
    for batch in reader {
        codec::batch_to_readings(
            &batch.context("decoding record batch")?,
            &geometry_column,
            &mut out,
        )?;
    }
    Ok(out)
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
        std::env::temp_dir().join(format!(
            "fe_query_geoparquet_{name}_{}.parquet",
            std::process::id()
        ))
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

        let raw = read_geo_metadata(&path)
            .unwrap()
            .expect("geo metadata present");
        let geo: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(geo["version"], "1.0.0");
        assert_eq!(geo["primary_column"], "position");
        assert_eq!(geo["columns"]["position"]["encoding"], "WKB");
        assert_eq!(geo["columns"]["position"]["geometry_types"][0], "Point Z");
        // DEC-C16: spec `crs` is null; the honest label lives in `fe:crs`.
        assert!(geo["columns"]["position"]["crs"].is_null());
        assert!(geo["columns"]["position"]["fe:crs"]
            .as_str()
            .unwrap()
            .contains("PETAL-LOCAL"));
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
    fn bytes_writer_matches_file_writer_round_trip() {
        let path = tmp("bytes");
        let snaps = vec![snap("n1", [1.0, 2.0, 3.0], None, 42)];
        let bytes = write_nodes_parquet_bytes(&snaps, &GeoParquetMeta::default()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let back = read_nodes_parquet(&path).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].node_id, "n1");
        assert_eq!(back[0].position, [1.0, 2.0, 3.0]);
        assert!(read_geo_metadata(&path).unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    fn reading(id: &str, node_id: &str, value: f64, anchor: Option<[f32; 3]>) -> ReadingSnapshot {
        ReadingSnapshot {
            reading_id: id.into(),
            node_id: node_id.into(),
            petal_id: "p1".into(),
            metric: "temperature_c".into(),
            value,
            units: "celsius".into(),
            recorded_at: "2026-10-09T00:00:00+00:00".into(),
            recorded_at_ms: 1_760_000_000_000,
            anchor_position: anchor,
        }
    }

    #[test]
    fn readings_round_trip_preserves_rows_and_bit_exact_value() {
        let path = tmp("readings_round_trip");
        // A value whose f32 truncation would be lossy — proves Float64 column.
        let rows = vec![
            reading("r1", "n1", 23.456_789_012_345, Some([1.0, 2.0, 3.0])),
            reading("r2", "n1", -40.0, Some([1.0, 2.0, 3.0])),
        ];
        let bytes = write_readings_parquet_bytes(&rows, &GeoParquetMeta::default()).unwrap();
        let path_written = {
            std::fs::write(&path, &bytes).unwrap();
            path.clone()
        };
        let back = read_readings_parquet(&path_written).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in rows.iter().zip(&back) {
            assert_eq!(a.reading_id, b.reading_id);
            assert_eq!(a.node_id, b.node_id);
            assert_eq!(a.petal_id, b.petal_id);
            assert_eq!(a.metric, b.metric);
            assert_eq!(a.value, b.value, "f64 value must round-trip bit-exact");
            assert_eq!(a.units, b.units);
            assert_eq!(a.recorded_at, b.recorded_at);
            assert_eq!(a.recorded_at_ms, b.recorded_at_ms);
            assert_eq!(a.anchor_position, b.anchor_position);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn readings_null_anchor_round_trips_as_none() {
        let path = tmp("readings_null_anchor");
        let rows = vec![reading("r1", "n-deleted", 1.0, None)];
        let bytes = write_readings_parquet_bytes(&rows, &GeoParquetMeta::default()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let back = read_readings_parquet(&path).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].anchor_position, None);
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
        let raw = read_geo_metadata(&path)
            .unwrap()
            .expect("geo metadata present");
        let geo: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(geo["columns"]["position"]["crs"].is_null());
        assert!(geo["columns"]["position"]["fe:crs"]
            .as_str()
            .unwrap()
            .contains("origin=47.6062,-122.3321,56.0"));
        let _ = std::fs::remove_file(&path);
    }
}
