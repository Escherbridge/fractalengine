//! Observation hexon writer — SPEC-10 §3. Builds the two pinned Parquet
//! members, the statistics artifact, and packs the `.hexon` archive via
//! `fe_format::HexonArchive::export_stored_deterministic` (never a second zip
//! writer — `fe-query/src/AGENTS.md` §observation-hexon).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{
    FixedSizeBinaryBuilder, Float64Array, Int32Array, Int64Array, ListBuilder, RecordBatch,
    StringArray, StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema};
use fe_format::manifest::{HexonManifest, HexonType};
use fe_format::HexonArchive;
use fe_sdk::{legacy_identifier, LegacyTable};
use parquet::arrow::ArrowWriter;

use super::profile;
use super::{ObservationHexonError, PayloadRefRow, ReadingRow, Result};

/// A fully built, UNSEALED observation hexon plus its statistics artifact and
/// the summary B3's sealer needs to build the kind-5 payload (§4.3) without
/// re-reading the Parquet bytes it just wrote.
#[derive(Debug)]
pub struct ObservationHexonWrite {
    /// The `.hexon` archive bytes (unsigned, unsealed — the kind-5 operation's
    /// own signature binds it; §3.1 rule 4).
    pub hexon_bytes: Vec<u8>,
    /// The statistics artifact's raw Parquet bytes (§3.3 — not archive-wrapped).
    pub stats_bytes: Vec<u8>,
    pub summary: ObservationHexonSummary,
}

#[derive(Debug, Clone)]
pub struct ObservationHexonSummary {
    /// Lowercase hex `hexon_id` the manifest itself carries (§3.1 rule 4).
    pub hexon_id: String,
    /// Total rows across both Parquet members (§4.3 key 5).
    pub count: u64,
    /// Inclusive `(min, max)` ingestion HLC `(wall_ms, counter)` over every row (§4.3 key 2).
    pub hlc_range: ((i64, i32), (i64, i32)),
    /// Inclusive `recorded_at_ms` `(min, max)` over every row (§4.3 key 3).
    pub sensor_time_range_ms: (i64, i64),
    /// The distinct anchor set, strictly ascending (§4.3 key 6).
    pub anchors: Vec<[u8; 32]>,
    /// The distinct asset content-hash set, strictly ascending (§4.3 key 7).
    pub asset_refs: Vec<[u8; 32]>,
    /// `hexon_format_version` this writer wrote (§4.3 key 9).
    pub hexon_format_version: u16,
}

/// Build an observation hexon from readings + payload refs, per SPEC-10 §3.
/// Rows may be given in any order — the writer sorts them into the required
/// `segmentby=anchor, orderby=hlc` layout itself (§3.2 rules 5-6), and given
/// the same set of rows always produces byte-identical output (§3.2 rule 8).
pub fn write_observation_hexon(
    publisher_did: &str,
    readings: &[ReadingRow],
    payload_refs: &[PayloadRefRow],
    source_count: usize,
) -> Result<ObservationHexonWrite> {
    if readings.is_empty() && payload_refs.is_empty() {
        return Err(ObservationHexonError::EmptyBatch);
    }
    for r in readings {
        if r.source_index as usize >= source_count {
            return Err(ObservationHexonError::SourceIndexOutOfRange {
                index: r.source_index,
                source_count,
            });
        }
    }
    for p in payload_refs {
        if p.source_index as usize >= source_count {
            return Err(ObservationHexonError::SourceIndexOutOfRange {
                index: p.source_index,
                source_count,
            });
        }
    }

    // §4.5: one legacy-identifier computation per distinct node_id.
    let mut anchor_of: HashMap<&str, [u8; 32]> = HashMap::new();
    for node_id in readings
        .iter()
        .map(|r| r.node_id.as_str())
        .chain(payload_refs.iter().map(|p| p.node_id.as_str()))
    {
        if !anchor_of.contains_key(node_id) {
            let anchor = legacy_identifier(LegacyTable::Node, node_id)?;
            anchor_of.insert(node_id, anchor);
        }
    }

    let mut resolved_readings: Vec<ResolvedReading> = readings
        .iter()
        .map(|r| ResolvedReading::new(r, anchor_of[r.node_id.as_str()]))
        .collect();
    // §3.2 rules 5-6: ascending anchor, then (hlc_wall_ms, hlc_counter, id) within it.
    resolved_readings.sort_by(|a, b| {
        a.anchor
            .cmp(&b.anchor)
            .then(a.hlc_wall_ms.cmp(&b.hlc_wall_ms))
            .then(a.hlc_counter.cmp(&b.hlc_counter))
            .then(a.reading_id.cmp(&b.reading_id))
    });

    let mut resolved_payloads: Vec<ResolvedPayloadRef> = payload_refs
        .iter()
        .map(|p| ResolvedPayloadRef::new(p, anchor_of[p.node_id.as_str()]))
        .collect();
    resolved_payloads.sort_by(|a, b| {
        a.anchor
            .cmp(&b.anchor)
            .then(a.hlc_wall_ms.cmp(&b.hlc_wall_ms))
            .then(a.hlc_counter.cmp(&b.hlc_counter))
            .then(a.payload_id.cmp(&b.payload_id))
    });

    let readings_bytes = write_readings_parquet(&resolved_readings)?;
    let payload_refs_bytes = write_payload_refs_parquet(&resolved_payloads)?;
    let stats_bytes = write_stats_parquet(&resolved_readings, &resolved_payloads)?;

    let hexon_id = {
        let h1 = blake3::hash(&readings_bytes);
        let h2 = blake3::hash(&payload_refs_bytes);
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(h1.as_bytes());
        combined.extend_from_slice(h2.as_bytes());
        blake3::hash(&combined).to_hex().to_string()
    };

    let hlc_points: Vec<(i64, i32)> = resolved_readings
        .iter()
        .map(|r| (r.hlc_wall_ms, r.hlc_counter))
        .chain(
            resolved_payloads
                .iter()
                .map(|p| (p.hlc_wall_ms, p.hlc_counter)),
        )
        .collect();
    let min_hlc = *hlc_points.iter().min().expect("checked non-empty above");
    let max_hlc = *hlc_points.iter().max().expect("checked non-empty above");

    let sensor_times: Vec<i64> = resolved_readings
        .iter()
        .map(|r| r.recorded_at_ms)
        .chain(resolved_payloads.iter().map(|p| p.recorded_at_ms))
        .collect();
    let min_sensor_time = *sensor_times.iter().min().expect("checked non-empty above");
    let max_sensor_time = *sensor_times.iter().max().expect("checked non-empty above");

    let mut anchors: Vec<[u8; 32]> = anchor_of.values().copied().collect();
    anchors.sort();
    anchors.dedup();

    let mut asset_refs: Vec<[u8; 32]> = resolved_payloads
        .iter()
        .flat_map(|p| p.asset_content_hashes.iter().copied())
        .collect();
    asset_refs.sort();
    asset_refs.dedup();

    let timestamp = epoch_ms_to_rfc3339(max_hlc.0);
    let manifest = HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: hexon_id.clone(),
        hexon_type: HexonType::ObservationBatch,
        publisher_did: publisher_did.to_string(),
        publisher_name: None,
        version: "1".into(),
        build_id: None,
        name: "observation-batch".into(),
        description: None,
        tags: vec![],
        created_at: timestamp.clone(),
        updated_at: timestamp,
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        integrity: None,
        signature: None,
    };

    let members: [(&str, &[u8]); 2] = [
        (profile::READINGS_MEMBER, readings_bytes.as_slice()),
        (profile::PAYLOAD_REFS_MEMBER, payload_refs_bytes.as_slice()),
    ];
    let hexon_bytes = HexonArchive::export_stored_deterministic(&manifest, &members)
        .map_err(|e| ObservationHexonError::Io(format!("packing observation hexon: {e:#}")))?;

    Ok(ObservationHexonWrite {
        hexon_bytes,
        stats_bytes,
        summary: ObservationHexonSummary {
            hexon_id,
            count: (resolved_readings.len() + resolved_payloads.len()) as u64,
            hlc_range: (min_hlc, max_hlc),
            sensor_time_range_ms: (min_sensor_time, max_sensor_time),
            anchors,
            asset_refs,
            hexon_format_version: profile::HEXON_FORMAT_VERSION,
        },
    })
}

// ---------------------------------------------------------------------------
// Resolved rows (anchor computed, HLC unpacked, sorted)
// ---------------------------------------------------------------------------

struct ResolvedReading {
    reading_id: String,
    anchor: [u8; 32],
    anchor_node_id: String,
    metric: String,
    value: f64,
    units: String,
    recorded_at: String,
    recorded_at_ms: i64,
    hlc_wall_ms: i64,
    hlc_counter: i32,
    payload_id: Option<String>,
    source_index: i32,
}

impl ResolvedReading {
    fn new(r: &ReadingRow, anchor: [u8; 32]) -> Self {
        Self {
            reading_id: r.reading_id.clone(),
            anchor,
            anchor_node_id: r.node_id.clone(),
            metric: r.metric.clone(),
            value: r.value,
            units: r.units.clone(),
            recorded_at: r.recorded_at.clone(),
            recorded_at_ms: r.recorded_at_ms,
            // SPEC-10 §3.2 rule 3: hlc_wall_ms = hlc_timestamp >> 16, hlc_counter = & 0xFFFF
            // (the same packing `fe-database/src/op_log.rs::next_hlc_timestamp` produces).
            hlc_wall_ms: (r.hlc_timestamp >> 16) as i64,
            hlc_counter: (r.hlc_timestamp & 0xFFFF) as i32,
            payload_id: r.payload_id.clone(),
            source_index: r.source_index as i32,
        }
    }
}

struct ResolvedPayloadRef {
    payload_id: String,
    anchor: [u8; 32],
    anchor_node_id: String,
    format_id: String,
    recorded_at: String,
    recorded_at_ms: i64,
    hlc_wall_ms: i64,
    hlc_counter: i32,
    asset_ids: Vec<String>,
    asset_content_hashes: Vec<[u8; 32]>,
    source_index: i32,
}

impl ResolvedPayloadRef {
    fn new(p: &PayloadRefRow, anchor: [u8; 32]) -> Self {
        Self {
            payload_id: p.payload_id.clone(),
            anchor,
            anchor_node_id: p.node_id.clone(),
            format_id: p.format_id.clone(),
            recorded_at: p.recorded_at.clone(),
            recorded_at_ms: p.recorded_at_ms,
            hlc_wall_ms: (p.hlc_timestamp >> 16) as i64,
            hlc_counter: (p.hlc_timestamp & 0xFFFF) as i32,
            asset_ids: p.asset_ids.clone(),
            asset_content_hashes: p.asset_content_hashes.clone(),
            source_index: p.source_index as i32,
        }
    }
}

/// Split rows already sorted by anchor into consecutive same-anchor runs
/// (§3.2 rule 5: "A row group MUST hold rows of exactly one anchor").
fn group_by_anchor<T>(rows: &[T], anchor_of: impl Fn(&T) -> [u8; 32]) -> Vec<&[T]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=rows.len() {
        if i == rows.len() || anchor_of(&rows[i]) != anchor_of(&rows[start]) {
            out.push(&rows[start..i]);
            start = i;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Parquet member writers
// ---------------------------------------------------------------------------

fn write_readings_parquet(rows: &[ResolvedReading]) -> Result<Vec<u8>> {
    let schema = Arc::new(profile::readings_arrow_schema());
    let props = profile::writer_properties();
    let mut buf = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buf, schema.clone(), Some(props))?;
        // One `.write()` + `.flush()` per anchor group forces a row-group
        // boundary at every anchor change (§3.2 rule 5); `ArrowWriter::write`
        // itself splits further at `MAX_ROW_GROUP_ROWS` within a group
        // (§3.2 rule 6).
        for group in group_by_anchor(rows, |r| r.anchor) {
            let batch = readings_batch(schema.clone(), group)?;
            writer.write(&batch)?;
            writer.flush()?;
        }
        writer.close()?;
    }
    Ok(buf)
}

fn readings_batch(
    schema: Arc<Schema>,
    rows: &[ResolvedReading],
) -> std::result::Result<RecordBatch, arrow::error::ArrowError> {
    let reading_ids: StringArray = rows.iter().map(|r| Some(r.reading_id.as_str())).collect();
    let mut anchor_b = FixedSizeBinaryBuilder::new(32);
    for r in rows {
        anchor_b
            .append_value(r.anchor)
            .expect("anchor is always exactly 32 bytes");
    }
    let anchor = anchor_b.finish();
    let anchor_node_ids: StringArray = rows
        .iter()
        .map(|r| Some(r.anchor_node_id.as_str()))
        .collect();
    let metrics: StringArray = rows.iter().map(|r| Some(r.metric.as_str())).collect();
    let values = Float64Array::from_iter_values(rows.iter().map(|r| r.value));
    let units: StringArray = rows.iter().map(|r| Some(r.units.as_str())).collect();
    let recorded_at: StringArray = rows.iter().map(|r| Some(r.recorded_at.as_str())).collect();
    let recorded_at_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.recorded_at_ms));
    let hlc_wall_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.hlc_wall_ms));
    let hlc_counter = Int32Array::from_iter_values(rows.iter().map(|r| r.hlc_counter));
    let payload_ids: StringArray = rows.iter().map(|r| r.payload_id.as_deref()).collect();
    let source_index = Int32Array::from_iter_values(rows.iter().map(|r| r.source_index));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(reading_ids),
            Arc::new(anchor),
            Arc::new(anchor_node_ids),
            Arc::new(metrics),
            Arc::new(values),
            Arc::new(units),
            Arc::new(recorded_at),
            Arc::new(recorded_at_ms),
            Arc::new(hlc_wall_ms),
            Arc::new(hlc_counter),
            Arc::new(payload_ids),
            Arc::new(source_index),
        ],
    )
}

fn write_payload_refs_parquet(rows: &[ResolvedPayloadRef]) -> Result<Vec<u8>> {
    let schema = Arc::new(profile::payload_refs_arrow_schema());
    let props = profile::writer_properties();
    let mut buf = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buf, schema.clone(), Some(props))?;
        for group in group_by_anchor(rows, |r| r.anchor) {
            let batch = payload_refs_batch(schema.clone(), group)?;
            writer.write(&batch)?;
            writer.flush()?;
        }
        writer.close()?;
    }
    Ok(buf)
}

fn payload_refs_batch(
    schema: Arc<Schema>,
    rows: &[ResolvedPayloadRef],
) -> std::result::Result<RecordBatch, arrow::error::ArrowError> {
    let payload_ids: StringArray = rows.iter().map(|r| Some(r.payload_id.as_str())).collect();
    let mut anchor_b = FixedSizeBinaryBuilder::new(32);
    for r in rows {
        anchor_b
            .append_value(r.anchor)
            .expect("anchor is always exactly 32 bytes");
    }
    let anchor = anchor_b.finish();
    let anchor_node_ids: StringArray = rows
        .iter()
        .map(|r| Some(r.anchor_node_id.as_str()))
        .collect();
    let format_ids: StringArray = rows.iter().map(|r| Some(r.format_id.as_str())).collect();
    let recorded_at: StringArray = rows.iter().map(|r| Some(r.recorded_at.as_str())).collect();
    let recorded_at_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.recorded_at_ms));
    let hlc_wall_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.hlc_wall_ms));
    let hlc_counter = Int32Array::from_iter_values(rows.iter().map(|r| r.hlc_counter));

    let mut asset_ids_b = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        false,
    )));
    for r in rows {
        for id in &r.asset_ids {
            asset_ids_b.values().append_value(id);
        }
        asset_ids_b.append(true);
    }
    let asset_ids = asset_ids_b.finish();

    let mut hashes_b = ListBuilder::new(FixedSizeBinaryBuilder::new(32)).with_field(Arc::new(
        Field::new("item", DataType::FixedSizeBinary(32), false),
    ));
    for r in rows {
        for h in &r.asset_content_hashes {
            hashes_b
                .values()
                .append_value(h)
                .expect("asset content hash is always exactly 32 bytes");
        }
        hashes_b.append(true);
    }
    let hashes = hashes_b.finish();

    let source_index = Int32Array::from_iter_values(rows.iter().map(|r| r.source_index));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(payload_ids),
            Arc::new(anchor),
            Arc::new(anchor_node_ids),
            Arc::new(format_ids),
            Arc::new(recorded_at),
            Arc::new(recorded_at_ms),
            Arc::new(hlc_wall_ms),
            Arc::new(hlc_counter),
            Arc::new(asset_ids),
            Arc::new(hashes),
            Arc::new(source_index),
        ],
    )
}

// ---------------------------------------------------------------------------
// Statistics artifact (§3.3) — one row per (anchor, table, key)
// ---------------------------------------------------------------------------

struct StatsAccumulator {
    row_count: i64,
    min_value: Option<f64>,
    max_value: Option<f64>,
    min_recorded_at_ms: i64,
    max_recorded_at_ms: i64,
    min_hlc_wall_ms: i64,
    min_hlc_counter: i32,
    max_hlc_wall_ms: i64,
    max_hlc_counter: i32,
}

impl StatsAccumulator {
    fn empty() -> Self {
        Self {
            row_count: 0,
            min_value: None,
            max_value: None,
            min_recorded_at_ms: i64::MAX,
            max_recorded_at_ms: i64::MIN,
            min_hlc_wall_ms: i64::MAX,
            min_hlc_counter: i32::MAX,
            max_hlc_wall_ms: i64::MIN,
            max_hlc_counter: i32::MIN,
        }
    }

    fn observe_common(&mut self, recorded_at_ms: i64, hlc_wall_ms: i64, hlc_counter: i32) {
        self.row_count += 1;
        self.min_recorded_at_ms = self.min_recorded_at_ms.min(recorded_at_ms);
        self.max_recorded_at_ms = self.max_recorded_at_ms.max(recorded_at_ms);
        self.min_hlc_wall_ms = self.min_hlc_wall_ms.min(hlc_wall_ms);
        self.max_hlc_wall_ms = self.max_hlc_wall_ms.max(hlc_wall_ms);
        self.min_hlc_counter = self.min_hlc_counter.min(hlc_counter);
        self.max_hlc_counter = self.max_hlc_counter.max(hlc_counter);
    }

    fn observe_reading(&mut self, r: &ResolvedReading) {
        self.observe_common(r.recorded_at_ms, r.hlc_wall_ms, r.hlc_counter);
        self.min_value = Some(self.min_value.map_or(r.value, |m| m.min(r.value)));
        self.max_value = Some(self.max_value.map_or(r.value, |m| m.max(r.value)));
    }

    fn observe_payload(&mut self, p: &ResolvedPayloadRef) {
        self.observe_common(p.recorded_at_ms, p.hlc_wall_ms, p.hlc_counter);
    }
}

struct StatsRow {
    anchor: [u8; 32],
    table: i32,
    key: String,
    acc: StatsAccumulator,
}

fn compute_stats_rows(
    readings: &[ResolvedReading],
    payloads: &[ResolvedPayloadRef],
) -> Vec<StatsRow> {
    // `BTreeMap<(anchor, table, key), _>` iterates strictly ascending by that
    // triple (§3.3 rule 2) for free — no separate sort step needed.
    let mut groups: BTreeMap<([u8; 32], i32, String), StatsAccumulator> = BTreeMap::new();
    for r in readings {
        groups
            .entry((r.anchor, profile::STATS_TABLE_READING, r.metric.clone()))
            .or_insert_with(StatsAccumulator::empty)
            .observe_reading(r);
    }
    for p in payloads {
        groups
            .entry((p.anchor, profile::STATS_TABLE_PAYLOAD, p.format_id.clone()))
            .or_insert_with(StatsAccumulator::empty)
            .observe_payload(p);
    }
    groups
        .into_iter()
        .map(|((anchor, table, key), acc)| StatsRow {
            anchor,
            table,
            key,
            acc,
        })
        .collect()
}

fn write_stats_parquet(
    readings: &[ResolvedReading],
    payloads: &[ResolvedPayloadRef],
) -> Result<Vec<u8>> {
    let rows = compute_stats_rows(readings, payloads);
    let schema = Arc::new(profile::stats_arrow_schema());
    let props = profile::writer_properties();
    let mut buf = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buf, schema.clone(), Some(props))?;
        if !rows.is_empty() {
            let batch = stats_batch(schema, &rows)?;
            writer.write(&batch)?;
        }
        writer.close()?;
    }
    Ok(buf)
}

fn stats_batch(
    schema: Arc<Schema>,
    rows: &[StatsRow],
) -> std::result::Result<RecordBatch, arrow::error::ArrowError> {
    let mut anchor_b = FixedSizeBinaryBuilder::new(32);
    for r in rows {
        anchor_b
            .append_value(r.anchor)
            .expect("anchor is always exactly 32 bytes");
    }
    let anchor = anchor_b.finish();
    let table = Int32Array::from_iter_values(rows.iter().map(|r| r.table));
    let key: StringArray = rows.iter().map(|r| Some(r.key.as_str())).collect();
    let row_count = Int64Array::from_iter_values(rows.iter().map(|r| r.acc.row_count));
    let min_value: Float64Array = rows.iter().map(|r| r.acc.min_value).collect();
    let max_value: Float64Array = rows.iter().map(|r| r.acc.max_value).collect();
    let min_recorded_at_ms =
        Int64Array::from_iter_values(rows.iter().map(|r| r.acc.min_recorded_at_ms));
    let max_recorded_at_ms =
        Int64Array::from_iter_values(rows.iter().map(|r| r.acc.max_recorded_at_ms));
    let min_hlc_wall_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.acc.min_hlc_wall_ms));
    let min_hlc_counter = Int32Array::from_iter_values(rows.iter().map(|r| r.acc.min_hlc_counter));
    let max_hlc_wall_ms = Int64Array::from_iter_values(rows.iter().map(|r| r.acc.max_hlc_wall_ms));
    let max_hlc_counter = Int32Array::from_iter_values(rows.iter().map(|r| r.acc.max_hlc_counter));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(anchor),
            Arc::new(table),
            Arc::new(key),
            Arc::new(row_count),
            Arc::new(min_value),
            Arc::new(max_value),
            Arc::new(min_recorded_at_ms),
            Arc::new(max_recorded_at_ms),
            Arc::new(min_hlc_wall_ms),
            Arc::new(min_hlc_counter),
            Arc::new(max_hlc_wall_ms),
            Arc::new(max_hlc_counter),
        ],
    )
}

// ---------------------------------------------------------------------------
// Epoch-ms -> RFC 3339 (no chrono/time dependency; fe-query/src/AGENTS.md
// §observation-hexon — the manifest's `created_at`/`updated_at` are the only
// consumer so far)
// ---------------------------------------------------------------------------

/// Epoch milliseconds (UTC) -> RFC 3339 `YYYY-MM-DDTHH:MM:SS.sssZ`, millisecond
/// precision (SPEC-10 §3.1 rule 4).
fn epoch_ms_to_rfc3339(ms: i64) -> String {
    let millis = ms.rem_euclid(1000);
    let secs_total = ms.div_euclid(1000);
    let days = secs_total.div_euclid(86_400);
    let secs_of_day = secs_total.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days` algorithm: days since the Unix epoch ->
/// `(year, month, day)` in the proleptic Gregorian calendar.
/// <https://howardhinnant.github.io/date_algorithms.html>
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod civil_date_tests {
    use super::*;

    #[test]
    fn known_dates_round_trip() {
        let cases: &[(i64, &str)] = &[
            (0, "1970-01-01T00:00:00.000Z"),
            (1_000, "1970-01-01T00:00:01.000Z"),
            (1_760_000_000_000, "2025-10-09T08:53:20.000Z"),
            // Leap day.
            (1_709_164_800_000, "2024-02-29T00:00:00.000Z"),
        ];
        for &(ms, expected) in cases {
            assert_eq!(epoch_ms_to_rfc3339(ms), expected, "ms = {ms}");
        }
    }
}
