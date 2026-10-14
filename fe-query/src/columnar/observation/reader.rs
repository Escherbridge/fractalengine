//! Observation hexon reader/validator — SPEC-10 §3.1/§3.2. Reads through
//! `fe_format::HexonArchive` only (never a second zip reader — see
//! `fe-query/src/AGENTS.md` §observation-hexon) and enforces every profile
//! rule a receiver can check without the kind-5 payload (the payload-vs-hexon
//! cross-check is hydration's job, §5.2 rule 4, owned by B3).

use arrow::array::{
    Array, FixedSizeBinaryArray, Float64Array, Int32Array, Int64Array, ListArray, StringArray,
};
use bytes::Bytes;
use fe_format::manifest::HexonType;
use fe_format::HexonArchive;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::metadata::{ParquetMetaData, RowGroupMetaData};

use super::profile;
use super::{ObservationHexonError, Result};

/// The decoded, profile-validated contents of an observation hexon.
#[derive(Debug)]
pub struct ObservationHexon {
    pub readings: Vec<ReadingOut>,
    pub payload_refs: Vec<PayloadRefOut>,
    /// The `hexon_format_version` every Parquet member agreed on (§3.2 rule 1).
    pub hexon_format_version: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadingOut {
    pub reading_id: String,
    pub anchor: [u8; 32],
    pub anchor_node_id: String,
    pub metric: String,
    pub value: f64,
    pub units: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    pub hlc_wall_ms: i64,
    pub hlc_counter: i32,
    pub payload_id: Option<String>,
    pub source_index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PayloadRefOut {
    pub payload_id: String,
    pub anchor: [u8; 32],
    pub anchor_node_id: String,
    pub format_id: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    pub hlc_wall_ms: i64,
    pub hlc_counter: i32,
    pub asset_ids: Vec<String>,
    pub asset_content_hashes: Vec<[u8; 32]>,
    pub source_index: u32,
}

/// Read and validate an observation hexon's container (§3.1) and Parquet
/// profile (§3.2), returning its decoded rows. Does not check the hexon
/// against a signed kind-5 payload — that cross-check is hydration's job
/// (§5.2 rule 4) once the payload is admitted.
pub fn read_observation_hexon(bytes: &[u8]) -> Result<ObservationHexon> {
    // §3.1 rules 2-3: exact member set, order, and storage method.
    let order = HexonArchive::member_order(bytes)?;
    let expected: Vec<(String, bool)> = profile::MEMBER_ORDER
        .iter()
        .map(|name| (name.to_string(), true))
        .collect();
    if order != expected {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "member set/order/storage method: got {order:?}, expected {expected:?}"
        )));
    }

    let data = HexonArchive::import(bytes)
        .map_err(|e| ObservationHexonError::Io(format!("importing observation hexon: {e:#}")))?;
    if !matches!(data.manifest.hexon_type, HexonType::ObservationBatch) {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "wrong hexon_type: {:?}, expected ObservationBatch",
            data.manifest.hexon_type
        )));
    }
    if !data.entries.is_empty() {
        return Err(ObservationHexonError::ProfileViolation(
            "entries.json must be the fixed empty catalog (§3.1 rule 5)".into(),
        ));
    }

    let readings_bytes = HexonArchive::read_member(
        bytes,
        profile::READINGS_MEMBER,
        fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
    )?
    .ok_or_else(|| {
        ObservationHexonError::ProfileViolation(format!(
            "missing required member {}",
            profile::READINGS_MEMBER
        ))
    })?;
    let payload_refs_bytes = HexonArchive::read_member(
        bytes,
        profile::PAYLOAD_REFS_MEMBER,
        fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
    )?
    .ok_or_else(|| {
        ObservationHexonError::ProfileViolation(format!(
            "missing required member {}",
            profile::PAYLOAD_REFS_MEMBER
        ))
    })?;

    let (readings, readings_format_version) = read_readings_member(readings_bytes)?;
    let (payload_refs, payload_refs_format_version) = read_payload_refs_member(payload_refs_bytes)?;

    if readings.is_empty() && payload_refs.is_empty() {
        return Err(ObservationHexonError::ProfileViolation(
            "both Parquet members hold zero rows (§3.1 rule 2)".into(),
        ));
    }
    if readings_format_version != payload_refs_format_version {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "hexon_format_version differs between members ({readings_format_version} vs {payload_refs_format_version})"
        )));
    }

    Ok(ObservationHexon {
        readings,
        payload_refs,
        hexon_format_version: readings_format_version,
    })
}

// ---------------------------------------------------------------------------
// readings.parquet
// ---------------------------------------------------------------------------

fn read_readings_member(raw: Vec<u8>) -> Result<(Vec<ReadingOut>, u16)> {
    let bytes = Bytes::from(raw);
    let probe = ParquetRecordBatchReaderBuilder::try_new(bytes.clone())?;
    let expected_schema = profile::readings_arrow_schema();
    if probe.schema().as_ref() != &expected_schema {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "observations/readings.parquet: schema mismatch (missing/extra/wrong-type column); got {:?}",
            probe.schema()
        )));
    }
    let metadata = probe.metadata().clone();
    let format_version = check_file_metadata(&metadata, "observations/readings.parquet")?;
    drop(probe);

    let mut rows = Vec::new();
    let mut row_group_sizes = Vec::with_capacity(metadata.num_row_groups());
    for rg in 0..metadata.num_row_groups() {
        check_row_group_profile(metadata.row_group(rg), "observations/readings.parquet")?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(bytes.clone())?
            .with_row_groups(vec![rg])
            .build()?;
        let before = rows.len();
        for batch in reader {
            decode_readings_batch(&batch?, &mut rows)?;
        }
        row_group_sizes.push(rows.len() - before);
    }

    validate_segment_and_order(
        &rows,
        &row_group_sizes,
        |r| r.anchor,
        |r| (r.hlc_wall_ms, r.hlc_counter),
        |r| r.reading_id.as_str(),
        "observations/readings.parquet",
    )?;

    Ok((rows, format_version))
}

fn decode_readings_batch(
    batch: &arrow::array::RecordBatch,
    out: &mut Vec<ReadingOut>,
) -> Result<()> {
    let reading_id = str_col(batch, "reading_id")?;
    let anchor = fixed32_col(batch, "anchor")?;
    let anchor_node_id = str_col(batch, "anchor_node_id")?;
    let metric = str_col(batch, "metric")?;
    let value = f64_col(batch, "value")?;
    let units = str_col(batch, "units")?;
    let recorded_at = str_col(batch, "recorded_at")?;
    let recorded_at_ms = i64_col(batch, "recorded_at_ms")?;
    let hlc_wall_ms = i64_col(batch, "hlc_wall_ms")?;
    let hlc_counter = i32_col(batch, "hlc_counter")?;
    let payload_id = str_col(batch, "payload_id")?;
    let source_index = i32_col(batch, "source_index")?;

    for i in 0..batch.num_rows() {
        out.push(ReadingOut {
            reading_id: reading_id.value(i).to_string(),
            anchor: fixed32_value(anchor, i)?,
            anchor_node_id: anchor_node_id.value(i).to_string(),
            metric: metric.value(i).to_string(),
            value: value.value(i),
            units: units.value(i).to_string(),
            recorded_at: recorded_at.value(i).to_string(),
            recorded_at_ms: recorded_at_ms.value(i),
            hlc_wall_ms: hlc_wall_ms.value(i),
            hlc_counter: hlc_counter.value(i),
            payload_id: if payload_id.is_null(i) {
                None
            } else {
                Some(payload_id.value(i).to_string())
            },
            source_index: non_negative_u32(source_index.value(i), "source_index")?,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// payload_refs.parquet
// ---------------------------------------------------------------------------

fn read_payload_refs_member(raw: Vec<u8>) -> Result<(Vec<PayloadRefOut>, u16)> {
    let bytes = Bytes::from(raw);
    let probe = ParquetRecordBatchReaderBuilder::try_new(bytes.clone())?;
    let expected_schema = profile::payload_refs_arrow_schema();
    if probe.schema().as_ref() != &expected_schema {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "observations/payload_refs.parquet: schema mismatch (missing/extra/wrong-type column); got {:?}",
            probe.schema()
        )));
    }
    let metadata = probe.metadata().clone();
    let format_version = check_file_metadata(&metadata, "observations/payload_refs.parquet")?;
    drop(probe);

    let mut rows = Vec::new();
    let mut row_group_sizes = Vec::with_capacity(metadata.num_row_groups());
    for rg in 0..metadata.num_row_groups() {
        check_row_group_profile(metadata.row_group(rg), "observations/payload_refs.parquet")?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(bytes.clone())?
            .with_row_groups(vec![rg])
            .build()?;
        let before = rows.len();
        for batch in reader {
            decode_payload_refs_batch(&batch?, &mut rows)?;
        }
        row_group_sizes.push(rows.len() - before);
    }

    validate_segment_and_order(
        &rows,
        &row_group_sizes,
        |r| r.anchor,
        |r| (r.hlc_wall_ms, r.hlc_counter),
        |r| r.payload_id.as_str(),
        "observations/payload_refs.parquet",
    )?;

    Ok((rows, format_version))
}

fn decode_payload_refs_batch(
    batch: &arrow::array::RecordBatch,
    out: &mut Vec<PayloadRefOut>,
) -> Result<()> {
    let payload_id = str_col(batch, "payload_id")?;
    let anchor = fixed32_col(batch, "anchor")?;
    let anchor_node_id = str_col(batch, "anchor_node_id")?;
    let format_id = str_col(batch, "format_id")?;
    let recorded_at = str_col(batch, "recorded_at")?;
    let recorded_at_ms = i64_col(batch, "recorded_at_ms")?;
    let hlc_wall_ms = i64_col(batch, "hlc_wall_ms")?;
    let hlc_counter = i32_col(batch, "hlc_counter")?;
    let asset_ids = list_col(batch, "asset_ids")?;
    let asset_content_hashes = list_col(batch, "asset_content_hashes")?;
    let source_index = i32_col(batch, "source_index")?;

    for i in 0..batch.num_rows() {
        let ids_array = asset_ids.value(i);
        let ids = ids_array
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                ObservationHexonError::ProfileViolation("asset_ids item is not Utf8".into())
            })?
            .iter()
            .map(|v| {
                v.map(str::to_string).ok_or_else(|| {
                    ObservationHexonError::ProfileViolation("asset_ids holds a null item".into())
                })
            })
            .collect::<Result<Vec<String>>>()?;

        let hashes_array = asset_content_hashes.value(i);
        let hashes_col = hashes_array
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .ok_or_else(|| {
                ObservationHexonError::ProfileViolation(
                    "asset_content_hashes item is not FixedSizeBinary(32)".into(),
                )
            })?;
        let mut hashes = Vec::with_capacity(hashes_col.len());
        for j in 0..hashes_col.len() {
            hashes.push(fixed32_value(hashes_col, j)?);
        }

        if ids.is_empty() || ids.len() != hashes.len() {
            return Err(ObservationHexonError::ProfileViolation(format!(
                "payload row {} has {} asset_ids and {} asset_content_hashes (§3.2 rule 10 requires equal, non-zero length)",
                payload_id.value(i), ids.len(), hashes.len()
            )));
        }

        out.push(PayloadRefOut {
            payload_id: payload_id.value(i).to_string(),
            anchor: fixed32_value(anchor, i)?,
            anchor_node_id: anchor_node_id.value(i).to_string(),
            format_id: format_id.value(i).to_string(),
            recorded_at: recorded_at.value(i).to_string(),
            recorded_at_ms: recorded_at_ms.value(i),
            hlc_wall_ms: hlc_wall_ms.value(i),
            hlc_counter: hlc_counter.value(i),
            asset_ids: ids,
            asset_content_hashes: hashes,
            source_index: non_negative_u32(source_index.value(i), "source_index")?,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared validation helpers
// ---------------------------------------------------------------------------

/// Validates §3.2 rules 5-6 over an already-decoded, physical-order row
/// sequence plus the row count of each of its row groups: no row group mixes
/// anchors, row groups are non-decreasing by anchor (so the same anchor may
/// only ever appear in a consecutive run), and within one anchor rows are
/// strictly ascending by `(hlc_wall_ms, hlc_counter, id)`.
fn validate_segment_and_order<T>(
    rows: &[T],
    row_group_sizes: &[usize],
    anchor_of: impl Fn(&T) -> [u8; 32],
    hlc_of: impl Fn(&T) -> (i64, i32),
    id_of: impl Fn(&T) -> &str,
    member: &str,
) -> Result<()> {
    let mut offset = 0;
    let mut last_group_anchor: Option<[u8; 32]> = None;
    for &size in row_group_sizes {
        let group = &rows[offset..offset + size];
        offset += size;
        let Some(first) = group.first() else {
            continue;
        };
        let anchor = anchor_of(first);
        if group.iter().any(|r| anchor_of(r) != anchor) {
            return Err(ObservationHexonError::ProfileViolation(format!(
                "{member}: a row group mixes more than one anchor (§3.2 rule 5)"
            )));
        }
        if let Some(prev) = last_group_anchor {
            if anchor < prev {
                return Err(ObservationHexonError::ProfileViolation(format!(
                    "{member}: row groups are not ordered by ascending anchor (§3.2 rule 5)"
                )));
            }
        }
        last_group_anchor = Some(anchor);
    }

    for pair in rows.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if anchor_of(a) == anchor_of(b) {
            let ka = (hlc_of(a), id_of(a));
            let kb = (hlc_of(b), id_of(b));
            if ka >= kb {
                return Err(ObservationHexonError::ProfileViolation(format!(
                    "{member}: rows are not strictly ascending by (hlc_wall_ms, hlc_counter, id) within an anchor (§3.2 rule 6)"
                )));
            }
        }
    }
    Ok(())
}

fn check_file_metadata(metadata: &ParquetMetaData, member: &str) -> Result<u16> {
    let file_meta = metadata.file_metadata();
    if file_meta.created_by() != Some(profile::CREATED_BY) {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "{member}: created_by is {:?}, expected {:?} (§3.2 rule 1)",
            file_meta.created_by(),
            profile::CREATED_BY
        )));
    }
    let kvs: &[parquet::format::KeyValue] = file_meta
        .key_value_metadata()
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    if kvs.len() != 1 || kvs[0].key != profile::FORMAT_VERSION_KEY {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "{member}: expected exactly one key-value metadata entry {:?}, got {:?} (§3.2 rule 1)",
            profile::FORMAT_VERSION_KEY,
            kvs
        )));
    }
    let value = kvs[0].value.as_deref().unwrap_or("");
    if value != profile::FORMAT_VERSION_VALUE {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "{member}: hexon_format_version {value:?} != {:?}",
            profile::FORMAT_VERSION_VALUE
        )));
    }
    Ok(profile::HEXON_FORMAT_VERSION)
}

/// §3.2 rule 7: every column chunk of every row group must carry row-group
/// statistics, over uncompressed pages (§3.2 rule 1), and no row group may
/// exceed the pinned maximum row count (§3.2 rule 6).
fn check_row_group_profile(rg: &RowGroupMetaData, member: &str) -> Result<()> {
    if rg.num_rows() as usize > profile::MAX_ROW_GROUP_ROWS {
        return Err(ObservationHexonError::ProfileViolation(format!(
            "{member}: a row group has {} rows, over the pinned maximum {}",
            rg.num_rows(),
            profile::MAX_ROW_GROUP_ROWS
        )));
    }
    for col in rg.columns() {
        if col.compression() != Compression::UNCOMPRESSED {
            return Err(ObservationHexonError::ProfileViolation(format!(
                "{member}: column {:?} is compressed (§3.2 rule 1 requires uncompressed pages)",
                col.column_path()
            )));
        }
        let stats = col.statistics().ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!(
                "{member}: column {:?} has no row-group statistics (§3.2 rule 7)",
                col.column_path()
            ))
        })?;
        if stats.min_bytes_opt().is_none()
            || stats.max_bytes_opt().is_none()
            || stats.null_count_opt().is_none()
        {
            return Err(ObservationHexonError::ProfileViolation(format!(
                "{member}: column {:?} is missing min/max/null_count statistics (§3.2 rule 7)",
                col.column_path()
            )));
        }
    }
    Ok(())
}

fn non_negative_u32(value: i32, column: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| {
        ObservationHexonError::ProfileViolation(format!("{column} is negative: {value}"))
    })
}

fn str_col<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn fixed32_col<'a>(
    batch: &'a arrow::array::RecordBatch,
    name: &str,
) -> Result<&'a FixedSizeBinaryArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn f64_col<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> Result<&'a Float64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn i64_col<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn i32_col<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> Result<&'a Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn list_col<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> Result<&'a ListArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<ListArray>())
        .ok_or_else(|| {
            ObservationHexonError::ProfileViolation(format!("missing/wrong-type column `{name}`"))
        })
}

fn fixed32_value(array: &FixedSizeBinaryArray, i: usize) -> Result<[u8; 32]> {
    array.value(i).try_into().map_err(|_| {
        ObservationHexonError::ProfileViolation("fixed-size-binary value is not 32 bytes".into())
    })
}
