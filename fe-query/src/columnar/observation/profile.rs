//! Pinned constants and schemas for the observation hexon's Parquet profile
//! (SPEC-10 §3.2) — see `fe-query/src/AGENTS.md` §observation-hexon. Every
//! value here is part of `hexon_format_version = 1`; changing any of them is
//! a new format version, never a silent edit.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::format::KeyValue;

/// `hexon_format_version` this crate writes and accepts (§3.2 rule 1).
pub const HEXON_FORMAT_VERSION: u16 = 1;

/// Pinned `created_by` string (§3.2 rule 1) — identifies this writer, not the
/// `parquet` crate's own version, so a dependency bump alone never changes it.
pub const CREATED_BY: &str = "fe-observation-hexon-v1";

/// The file's one required key-value metadata entry (§3.2 rule 1).
pub const FORMAT_VERSION_KEY: &str = "fe.observation.hexon_format_version";
pub const FORMAT_VERSION_VALUE: &str = "1";

/// Pinned maximum row-group row count (§3.2 rule 6: "An anchor's rows split
/// into row groups only at the pinned maximum row count").
pub const MAX_ROW_GROUP_ROWS: usize = 50_000;

/// Pinned data-page size limit (§3.2 rule 1's "fixed constants").
pub const DATA_PAGE_SIZE_LIMIT: usize = 1 << 20;

/// `.hexon` archive member names, SPEC-10 §3.1 rule 2.
pub const MANIFEST_MEMBER: &str = "manifest.json";
pub const ENTRIES_MEMBER: &str = "entries.json";
pub const READINGS_MEMBER: &str = "observations/readings.parquet";
pub const PAYLOAD_REFS_MEMBER: &str = "observations/payload_refs.parquet";

/// The archive's exact required member order, every member `Stored` (§3.1 rules 2-3).
pub const MEMBER_ORDER: [&str; 4] = [
    MANIFEST_MEMBER,
    ENTRIES_MEMBER,
    READINGS_MEMBER,
    PAYLOAD_REFS_MEMBER,
];

/// `observations/readings.parquet` columns, in order (§3.2 rule 3).
pub fn readings_arrow_schema() -> Schema {
    Schema::new(vec![
        Field::new("reading_id", DataType::Utf8, false),
        Field::new("anchor", DataType::FixedSizeBinary(32), false),
        Field::new("anchor_node_id", DataType::Utf8, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("units", DataType::Utf8, false),
        Field::new("recorded_at", DataType::Utf8, false),
        Field::new("recorded_at_ms", DataType::Int64, false),
        Field::new("hlc_wall_ms", DataType::Int64, false),
        Field::new("hlc_counter", DataType::Int32, false),
        Field::new("payload_id", DataType::Utf8, true),
        Field::new("source_index", DataType::Int32, false),
    ])
}

/// `observations/payload_refs.parquet` columns, in order (§3.2 rule 4).
pub fn payload_refs_arrow_schema() -> Schema {
    let utf8_item = || Arc::new(Field::new("item", DataType::Utf8, false));
    let fixed32_item = || Arc::new(Field::new("item", DataType::FixedSizeBinary(32), false));
    Schema::new(vec![
        Field::new("payload_id", DataType::Utf8, false),
        Field::new("anchor", DataType::FixedSizeBinary(32), false),
        Field::new("anchor_node_id", DataType::Utf8, false),
        Field::new("format_id", DataType::Utf8, false),
        Field::new("recorded_at", DataType::Utf8, false),
        Field::new("recorded_at_ms", DataType::Int64, false),
        Field::new("hlc_wall_ms", DataType::Int64, false),
        Field::new("hlc_counter", DataType::Int32, false),
        Field::new("asset_ids", DataType::List(utf8_item()), false),
        Field::new(
            "asset_content_hashes",
            DataType::List(fixed32_item()),
            false,
        ),
        Field::new("source_index", DataType::Int32, false),
    ])
}

/// The statistics artifact's columns, in order (§3.3 rule 2) — one row per
/// `(anchor, table, key)`. `table` is `1` for a readings aggregate (`key` =
/// metric) and `2` for a payload aggregate (`key` = format_id); `min_value`/
/// `max_value` are null for `table = 2` (no scalar value to aggregate).
pub fn stats_arrow_schema() -> Schema {
    Schema::new(vec![
        Field::new("anchor", DataType::FixedSizeBinary(32), false),
        Field::new("table", DataType::Int32, false),
        Field::new("key", DataType::Utf8, false),
        Field::new("row_count", DataType::Int64, false),
        Field::new("min_value", DataType::Float64, true),
        Field::new("max_value", DataType::Float64, true),
        Field::new("min_recorded_at_ms", DataType::Int64, false),
        Field::new("max_recorded_at_ms", DataType::Int64, false),
        Field::new("min_hlc_wall_ms", DataType::Int64, false),
        Field::new("min_hlc_counter", DataType::Int32, false),
        Field::new("max_hlc_wall_ms", DataType::Int64, false),
        Field::new("max_hlc_counter", DataType::Int32, false),
    ])
}

/// `table` discriminant for a readings-aggregate stats row (§3.3 rule 2).
pub const STATS_TABLE_READING: i32 = 1;
/// `table` discriminant for a payload-aggregate stats row (§3.3 rule 2).
pub const STATS_TABLE_PAYLOAD: i32 = 2;

/// The one pinned `WriterProperties` every Parquet member in an observation
/// hexon (and its statistics artifact) is written with (§3.2 rule 1):
/// uncompressed pages — `parquet` is built `default-features = false` so no
/// codec is even compiled in (`fe-query/Cargo.toml`), chunk-level statistics,
/// no dictionary encoding (determinism: §3.2 rule 8 only has to hold for our
/// own canonical row order, but disabling the dictionary removes one more
/// data-dependent encoding choice), and a fixed `created_by` + format-version
/// key.
pub fn writer_properties() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(Compression::UNCOMPRESSED)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_data_page_size_limit(DATA_PAGE_SIZE_LIMIT)
        .set_max_row_group_size(MAX_ROW_GROUP_ROWS)
        .set_created_by(CREATED_BY.to_string())
        .set_key_value_metadata(Some(vec![KeyValue::new(
            FORMAT_VERSION_KEY.to_string(),
            FORMAT_VERSION_VALUE.to_string(),
        )]))
        .build()
}
