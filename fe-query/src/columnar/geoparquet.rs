//! GeoParquet read/write for entity snapshots.
//!
//! Provides helpers to persist [`EntitySnapshot`](fe_entity_store::EntitySnapshot)
//! data as GeoParquet files. The current implementation is a stub that returns
//! placeholder results; full Arrow-backed I/O will land once the `parquet`
//! crate is added to the workspace.

#![cfg(feature = "datafusion")]

use std::path::Path;

use anyhow::Result;

/// GeoParquet metadata for the nodes table.
///
/// Follows the GeoParquet 1.0 spec: the `primary_geometry_column` identifies
/// which column carries spatial data, `crs` is the coordinate reference system,
/// and `encoding` describes the geometry encoding (e.g. `"point"`, `"WKB"`).
pub struct GeoParquetMeta {
    /// Name of the column that holds geometry data.
    pub primary_geometry_column: String,
    /// Coordinate Reference System identifier (e.g. `"EPSG:4326"`).
    pub crs: String,
    /// Geometry encoding format.
    pub encoding: String,
}

impl Default for GeoParquetMeta {
    fn default() -> Self {
        Self {
            primary_geometry_column: "position".into(),
            crs: "EPSG:4326".into(),
            encoding: "point".into(),
        }
    }
}

/// Write entity snapshots to a GeoParquet file.
///
/// **Stub**: converts snapshots to a `RecordBatch` and writes via
/// `parquet::arrow::ArrowWriter`. Full geoarrow-rs integration is deferred
/// until that crate stabilizes.
///
/// Returns the number of snapshots written on success.
pub fn write_nodes_parquet(
    path: &Path,
    snapshots: &[fe_entity_store::EntitySnapshot],
) -> Result<usize> {
    // Stub: acknowledge inputs to avoid unused-variable warnings.
    let _ = path;
    Ok(snapshots.len())
}

/// Read entity snapshots from a GeoParquet file.
///
/// **Stub**: full implementation deferred until the `parquet` crate is added.
/// Currently returns an empty vector.
pub fn read_nodes_parquet(path: &Path) -> Result<Vec<fe_entity_store::EntitySnapshot>> {
    let _ = path;
    Ok(Vec::new())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn geoparquet_meta_defaults() {
        let meta = GeoParquetMeta::default();
        assert_eq!(meta.primary_geometry_column, "position");
        assert_eq!(meta.crs, "EPSG:4326");
        assert_eq!(meta.encoding, "point");
    }

    #[test]
    fn write_stub_returns_count() {
        let path = PathBuf::from("/tmp/test.parquet");
        let snapshots = vec![
            fe_entity_store::EntitySnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
                properties: None,
                updated_at_ms: 1000,
                node_log: vec![],
            },
            fe_entity_store::EntitySnapshot {
                node_id: "n2".into(),
                petal_id: "p1".into(),
                position: [4.0, 5.0, 6.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
                properties: None,
                updated_at_ms: 2000,
                node_log: vec![],
            },
        ];
        let count = write_nodes_parquet(&path, &snapshots).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn read_stub_returns_empty() {
        let path = PathBuf::from("/tmp/test.parquet");
        let result = read_nodes_parquet(&path).unwrap();
        assert!(result.is_empty());
    }
}
