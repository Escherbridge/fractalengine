//! Columnar observation hexon codec — SPEC-10 §3 (the `.hexon` container +
//! Parquet profile B3's sealer and hydrator read/write). See
//! `fe-query/src/AGENTS.md` §observation-hexon for the design rationale;
//! `docs/spec/canonical-log/observation-plane.md` is the normative source.
//!
//! This module is the writer/reader pair only: it has no fe-canonical-log,
//! fe-database, or iroh dependency, and never touches disk (the hexon is
//! held decrypted in memory start to finish, never written out unsealed).
//! `source_index` on every row is the caller's (B3's) index into the
//! kind-5 payload's `sources` list (§4.3 key 8) — this module does not
//! interpret it, only carries it and range-checks it against `source_count`.

#![cfg(feature = "parquet")]

mod profile;
mod reader;
mod writer;

pub use profile::{
    HEXON_FORMAT_VERSION, MAX_ROW_GROUP_ROWS, MEMBER_ORDER, STATS_TABLE_PAYLOAD,
    STATS_TABLE_READING,
};
pub use reader::{read_observation_hexon, ObservationHexon, PayloadRefOut, ReadingOut};
pub use writer::{write_observation_hexon, ObservationHexonSummary, ObservationHexonWrite};

/// One `iot_reading` row as the writer ingests it — the hot-plane fields the
/// writer needs to compute `anchor`/`hlc_wall_ms`/`hlc_counter` and place the
/// row in `observations/readings.parquet` (§3.2 rule 3).
#[derive(Debug, Clone)]
pub struct ReadingRow {
    pub reading_id: String,
    pub node_id: String,
    pub metric: String,
    pub value: f64,
    pub units: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    /// Packed `(wall_ms << 16) | (counter & 0xFFFF)`, the same `iot_reading.hlc_timestamp`
    /// encoding `fe-database/src/op_log.rs::next_hlc_timestamp` produces.
    pub hlc_timestamp: u64,
    pub payload_id: Option<String>,
    /// Index into the kind-5 payload's `sources` list (§4.3 key 8).
    pub source_index: u32,
}

/// One `device_payload` row as the writer ingests it — see [`ReadingRow`].
#[derive(Debug, Clone)]
pub struct PayloadRefRow {
    pub payload_id: String,
    pub node_id: String,
    pub format_id: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    pub hlc_timestamp: u64,
    /// Ordered 1:1 with `asset_content_hashes` (§3.2 rule 10); the ingest order
    /// of the payload's assets, never re-sorted.
    pub asset_ids: Vec<String>,
    /// `asset_content_hashes[i]` is the BLAKE3 content hash of `asset_ids[i]` (§3.2 rule 10).
    pub asset_content_hashes: Vec<[u8; 32]>,
    pub source_index: u32,
}

/// Why building or reading an observation hexon failed. Distinguishes a
/// caller-input defect (a row naming an out-of-range `source_index`, a
/// non-ASCII legacy id) from a profile violation found in bytes being read
/// back (SPEC-10 §3.1/§3.2), so callers can tell "my own inputs were bad"
/// from "the bytes I was handed are not a conforming observation hexon".
#[derive(Debug)]
pub enum ObservationHexonError {
    /// A row's `node_id` could not be mapped to a legacy identifier (SPEC-10 §4.5).
    LegacyIdentifier(fe_sdk::LegacyIdentifierError),
    /// The writer was given zero readings and zero payload refs (§4.3 rule 3:
    /// "A batch with nothing in it is not sealed" — the writer refuses to
    /// produce one rather than let an empty batch reach the sealer).
    EmptyBatch,
    /// A row's `source_index` is not less than the caller's `source_count`.
    SourceIndexOutOfRange { index: u32, source_count: usize },
    /// A reading's value is NaN or infinite (§3.2 rule 2: every `DOUBLE` must be finite).
    NonFiniteValue { reading_id: String },
    /// A lower-level archive/zip/parquet I/O failure (message only — the
    /// originating error type is `anyhow`/`zip`/`parquet`-internal and not
    /// meaningfully distinguishable at this layer).
    Io(String),
    /// Bytes handed to [`read_observation_hexon`] violate the container
    /// (§3.1) or Parquet (§3.2) profile; the message names the rule.
    ProfileViolation(String),
}

impl std::fmt::Display for ObservationHexonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LegacyIdentifier(e) => write!(f, "legacy identifier: {e}"),
            Self::EmptyBatch => write!(f, "an observation batch with zero rows is not sealed"),
            Self::SourceIndexOutOfRange {
                index,
                source_count,
            } => write!(
                f,
                "source_index {index} is not less than source_count {source_count}"
            ),
            Self::NonFiniteValue { reading_id } => {
                write!(f, "reading {reading_id} holds a non-finite value")
            }
            Self::Io(msg) => write!(f, "{msg}"),
            Self::ProfileViolation(msg) => {
                write!(f, "observation hexon violates its profile: {msg}")
            }
        }
    }
}

impl std::error::Error for ObservationHexonError {}

impl From<fe_sdk::LegacyIdentifierError> for ObservationHexonError {
    fn from(e: fe_sdk::LegacyIdentifierError) -> Self {
        Self::LegacyIdentifier(e)
    }
}

impl From<anyhow::Error> for ObservationHexonError {
    fn from(e: anyhow::Error) -> Self {
        Self::Io(format!("{e:#}"))
    }
}

impl From<parquet::errors::ParquetError> for ObservationHexonError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<arrow::error::ArrowError> for ObservationHexonError {
    fn from(e: arrow::error::ArrowError) -> Self {
        Self::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ObservationHexonError>;

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{HexonManifest, HexonType};
    use fe_format::HexonArchive;

    fn reading(
        reading_id: &str,
        node_id: &str,
        metric: &str,
        value: f64,
        hlc_wall_ms: u64,
        hlc_counter: u16,
    ) -> ReadingRow {
        ReadingRow {
            reading_id: reading_id.into(),
            node_id: node_id.into(),
            metric: metric.into(),
            value,
            units: "celsius".into(),
            recorded_at: "2026-10-14T00:00:00.000+00:00".into(),
            recorded_at_ms: 1_760_400_000_000 + hlc_wall_ms as i64,
            hlc_timestamp: (hlc_wall_ms << 16) | (hlc_counter as u64),
            payload_id: None,
            source_index: 0,
        }
    }

    fn payload_ref(
        payload_id: &str,
        node_id: &str,
        format_id: &str,
        hlc_wall_ms: u64,
        hlc_counter: u16,
    ) -> PayloadRefRow {
        PayloadRefRow {
            payload_id: payload_id.into(),
            node_id: node_id.into(),
            format_id: format_id.into(),
            recorded_at: "2026-10-14T00:00:00.000+00:00".into(),
            recorded_at_ms: 1_760_400_000_000 + hlc_wall_ms as i64,
            hlc_timestamp: (hlc_wall_ms << 16) | (hlc_counter as u64),
            asset_ids: vec!["asset-1".into()],
            asset_content_hashes: vec![[7u8; 32]],
            source_index: 0,
        }
    }

    /// Conformance test 6 (`observation_hexon_round_trip_honours_segment_and_order_profile`,
    /// SPEC-10 §10): readings for three anchors written in shuffled order read
    /// back as per-anchor row groups in ascending HLC order with correct
    /// row-group statistics.
    #[test]
    fn observation_hexon_round_trip_honours_segment_and_order_profile() {
        // Shuffled input order: anchors and timestamps interleaved on purpose.
        let readings = vec![
            reading("r5", "n3", "temp", 5.0, 50, 0),
            reading("r1", "n1", "temp", 1.0, 10, 0),
            reading("r4", "n2", "temp", 4.0, 40, 0),
            reading("r2", "n1", "temp", 2.0, 20, 0),
            reading("r6", "n3", "temp", 6.0, 60, 0),
            reading("r3", "n1", "temp", 3.0, 30, 0),
        ];
        let payload_refs = vec![payload_ref("p1", "n2", "vendor.x/y@1", 45, 0)];

        let built = write_observation_hexon("did:key:z6Mktest", &readings, &payload_refs, 1)
            .expect("writer must accept a valid batch");
        let decoded =
            read_observation_hexon(&built.hexon_bytes).expect("reader must accept its own output");

        assert_eq!(decoded.readings.len(), 6);
        assert_eq!(decoded.payload_refs.len(), 1);
        assert_eq!(decoded.hexon_format_version, HEXON_FORMAT_VERSION);

        // Ascending by anchor, then by hlc within an anchor.
        for pair in decoded.readings.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(
                a.anchor < b.anchor
                    || (a.anchor == b.anchor
                        && (a.hlc_wall_ms, a.hlc_counter) < (b.hlc_wall_ms, b.hlc_counter)),
                "rows must be (anchor, hlc) ascending: {a:?} then {b:?}"
            );
        }
        // n1's three readings must all be present and in value order 1,2,3.
        let n1_values: Vec<f64> = decoded
            .readings
            .iter()
            .filter(|r| r.anchor_node_id == "n1")
            .map(|r| r.value)
            .collect();
        assert_eq!(n1_values, vec![1.0, 2.0, 3.0]);
    }

    /// Conformance test 7 (`same_rows_any_input_order_produce_identical_hexon_bytes`).
    #[test]
    fn same_rows_any_input_order_produce_identical_hexon_bytes() {
        let order_a = vec![
            reading("r1", "n1", "temp", 1.0, 10, 0),
            reading("r2", "n2", "temp", 2.0, 20, 0),
            reading("r3", "n1", "temp", 3.0, 30, 0),
        ];
        let mut order_b = order_a.clone();
        order_b.reverse();

        let built_a = write_observation_hexon("did:key:z6Mktest", &order_a, &[], 1).unwrap();
        let built_b = write_observation_hexon("did:key:z6Mktest", &order_b, &[], 1).unwrap();

        assert_eq!(
            built_a.hexon_bytes, built_b.hexon_bytes,
            "hexon bytes must not depend on input row order"
        );
        assert_eq!(
            built_a.stats_bytes, built_b.stats_bytes,
            "statistics artifact bytes must not depend on input row order"
        );
        assert_eq!(built_a.summary.hexon_id, built_b.summary.hexon_id);
    }

    #[test]
    fn empty_batch_is_refused() {
        let err = write_observation_hexon("did:key:z6Mktest", &[], &[], 0).unwrap_err();
        assert!(matches!(err, ObservationHexonError::EmptyBatch));
    }

    #[test]
    fn out_of_range_source_index_is_refused() {
        let readings = vec![reading("r1", "n1", "temp", 1.0, 10, 0)];
        let err = write_observation_hexon("did:key:z6Mktest", &readings, &[], 0).unwrap_err();
        assert!(matches!(
            err,
            ObservationHexonError::SourceIndexOutOfRange {
                index: 0,
                source_count: 0
            }
        ));
    }

    /// Conformance test 9 (`zip_bomb_observation_hexon_refused_without_allocation`): a member
    /// that inflates past the per-entry limit is refused from the central directory alone — by
    /// the storage-method rule before any decompression, and by `read_member`'s declared-size
    /// cap before any buffer is reserved.
    #[test]
    fn zip_bomb_observation_hexon_refused_without_allocation() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let stored = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1));
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(profile::MANIFEST_MEMBER, stored).unwrap();
        zip.write_all(&serde_json::to_vec(&obs_manifest()).unwrap())
            .unwrap();
        zip.start_file(profile::ENTRIES_MEMBER, stored).unwrap();
        zip.write_all(br#"{"entries":[]}"#).unwrap();
        zip.start_file(profile::READINGS_MEMBER, deflated).unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..(fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES / (1 << 20) + 8) {
            zip.write_all(&chunk).unwrap();
        }
        zip.start_file(profile::PAYLOAD_REFS_MEMBER, stored)
            .unwrap();
        zip.write_all(b"x").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(bytes.len() < (4 << 20), "the bomb must compress small");

        match read_observation_hexon(&bytes) {
            Err(ObservationHexonError::ProfileViolation(message)) => {
                assert!(message.contains("storage method"), "{message}")
            }
            other => panic!("a deflated bomb member must be refused, got {other:?}"),
        }
        let err = HexonArchive::read_member(
            &bytes,
            profile::READINGS_MEMBER,
            fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
        )
        .expect_err("the declared size is over the per-entry cap");
        assert!(format!("{err:#}").contains(fe_format::limits::DECOMPRESSION_LIMIT_ERROR));
    }

    /// Conformance test 8 (`profile_violations_rejected`), the subset
    /// constructible without a direct `zip`-crate test dependency (fe-query
    /// does not depend on `zip`; `fe_format::HexonArchive::export_stored_deterministic`
    /// always writes `Stored`, so the zip-level "compressed member" violation,
    /// and the Parquet-page-compression violation — `parquet` here is built
    /// with no codec compiled in, `fe-query/Cargo.toml` — are both
    /// structurally unreachable through this crate's own writer, and are
    /// covered instead by `fe_format::archive`'s own decompression-limit
    /// tests).
    #[test]
    fn profile_violations_rejected() {
        let readings = vec![reading("r1", "n1", "temp", 1.0, 10, 0)];
        let built = write_observation_hexon("did:key:z6Mktest", &readings, &[], 1).unwrap();
        let readings_bytes = HexonArchive::read_member(
            &built.hexon_bytes,
            profile::READINGS_MEMBER,
            fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
        )
        .unwrap()
        .unwrap();
        let payload_refs_bytes = HexonArchive::read_member(
            &built.hexon_bytes,
            profile::PAYLOAD_REFS_MEMBER,
            fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
        )
        .unwrap()
        .unwrap();

        // Case 1: wrong hexon_type.
        let mut wrong_type = obs_manifest();
        wrong_type.hexon_type = HexonType::Scene;
        let members: [(&str, &[u8]); 2] = [
            (profile::READINGS_MEMBER, &readings_bytes),
            (profile::PAYLOAD_REFS_MEMBER, &payload_refs_bytes),
        ];
        let bytes = HexonArchive::export_stored_deterministic(&wrong_type, &members).unwrap();
        assert!(matches!(
            read_observation_hexon(&bytes).unwrap_err(),
            ObservationHexonError::ProfileViolation(_)
        ));

        // Case 2: wrong member order (payload_refs before readings).
        let members_swapped: [(&str, &[u8]); 2] = [
            (profile::PAYLOAD_REFS_MEMBER, &payload_refs_bytes),
            (profile::READINGS_MEMBER, &readings_bytes),
        ];
        let bytes =
            HexonArchive::export_stored_deterministic(&obs_manifest(), &members_swapped).unwrap();
        assert!(matches!(
            read_observation_hexon(&bytes).unwrap_err(),
            ObservationHexonError::ProfileViolation(_)
        ));

        // Case 3: a missing required member.
        let members_missing: [(&str, &[u8]); 1] = [(profile::READINGS_MEMBER, &readings_bytes)];
        let bytes =
            HexonArchive::export_stored_deterministic(&obs_manifest(), &members_missing).unwrap();
        assert!(matches!(
            read_observation_hexon(&bytes).unwrap_err(),
            ObservationHexonError::ProfileViolation(_)
        ));

        // Case 4: an extra column not in the profile (readings.parquet rebuilt
        // with the pinned schema plus one bogus trailing column).
        let bad_readings = {
            let mut fields = profile::readings_arrow_schema().fields().to_vec();
            fields.push(std::sync::Arc::new(arrow::datatypes::Field::new(
                "extra",
                arrow::datatypes::DataType::Utf8,
                true,
            )));
            let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(fields));
            let mut buf = Vec::new();
            {
                let writer = parquet::arrow::ArrowWriter::try_new(
                    &mut buf,
                    schema.clone(),
                    Some(profile::writer_properties()),
                )
                .unwrap();
                writer.close().unwrap();
            }
            buf
        };
        let members_bad_schema: [(&str, &[u8]); 2] = [
            (profile::READINGS_MEMBER, &bad_readings),
            (profile::PAYLOAD_REFS_MEMBER, &payload_refs_bytes),
        ];
        let bytes = HexonArchive::export_stored_deterministic(&obs_manifest(), &members_bad_schema)
            .unwrap();
        assert!(matches!(
            read_observation_hexon(&bytes).unwrap_err(),
            ObservationHexonError::ProfileViolation(_)
        ));
    }

    /// A one-row `readings.parquet` member written with `props` (bypassing the pinned writer)
    /// so the reader's own refusals can be exercised on bytes the writer would never emit.
    fn readings_member_with(
        value: f64,
        props: parquet::file::properties::WriterProperties,
    ) -> Vec<u8> {
        use arrow::array::{
            FixedSizeBinaryArray, Float64Array, Int32Array, Int64Array, StringArray,
        };
        use std::sync::Arc;
        let schema = Arc::new(profile::readings_arrow_schema());
        let text = |v: &str| Arc::new(StringArray::from(vec![v])) as arrow::array::ArrayRef;
        let anchor = fe_sdk::legacy_identifier(fe_sdk::LegacyTable::Node, "n1").expect("anchor");
        let batch = arrow::array::RecordBatch::try_new(
            schema.clone(),
            vec![
                text("r1"),
                Arc::new(
                    FixedSizeBinaryArray::try_from_iter(std::iter::once(anchor)).expect("anchor"),
                ),
                text("n1"),
                text("temp"),
                Arc::new(Float64Array::from(vec![value])),
                text("celsius"),
                text("2026-10-14T00:00:00.000+00:00"),
                Arc::new(Int64Array::from(vec![1_760_400_000_010i64])),
                Arc::new(Int64Array::from(vec![10i64])),
                Arc::new(Int32Array::from(vec![0i32])),
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(Int32Array::from(vec![0i32])),
            ],
        )
        .expect("batch");
        let mut buf = Vec::new();
        let options = parquet::arrow::arrow_writer::ArrowWriterOptions::new()
            .with_properties(props)
            .with_skip_arrow_metadata(true);
        let mut writer =
            parquet::arrow::ArrowWriter::try_new_with_options(&mut buf, schema, options)
                .expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
        buf
    }

    fn hexon_with_readings_member(readings_member: &[u8]) -> Vec<u8> {
        let built = write_observation_hexon(
            "did:key:z6Mktest",
            &[],
            &[payload_ref("p1", "n2", "vendor.x/y@1", 45, 0)],
            1,
        )
        .expect("payload-only batch");
        let payload_refs_bytes = HexonArchive::read_member(
            &built.hexon_bytes,
            profile::PAYLOAD_REFS_MEMBER,
            fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES,
        )
        .unwrap()
        .unwrap();
        let members: [(&str, &[u8]); 2] = [
            (profile::READINGS_MEMBER, readings_member),
            (profile::PAYLOAD_REFS_MEMBER, &payload_refs_bytes),
        ];
        HexonArchive::export_stored_deterministic(&obs_manifest(), &members).unwrap()
    }

    /// A scalar batch's `payload_id` chunk is all null, so parquet writes its null_count without
    /// a min/max; §3.2 rule 7 must not refuse it (or no scalar-only batch could ever hydrate).
    #[test]
    fn scalar_only_batch_with_all_null_payload_ids_reads_back() {
        let readings = vec![
            reading("r1", "n1", "temp", 1.0, 10, 0),
            reading("r2", "n2", "temp", 2.0, 20, 0),
        ];
        assert!(readings.iter().all(|r| r.payload_id.is_none()));
        let built = write_observation_hexon("did:key:z6Mktest", &readings, &[], 1).unwrap();
        let decoded = read_observation_hexon(&built.hexon_bytes).expect("scalar-only batch");
        assert_eq!(decoded.readings.len(), 2);
        assert!(decoded.readings.iter().all(|r| r.payload_id.is_none()));
    }

    /// §3.1 rule 6 / §3.2 rule 1: a dictionary-encoded member (which can expand one stored string
    /// into every row) is refused from its page headers, before any column is decoded; the same
    /// rows under the pinned PLAIN profile read back.
    #[test]
    fn dictionary_encoded_member_is_refused_before_decoding() {
        let plain = readings_member_with(1.0, profile::writer_properties());
        assert!(read_observation_hexon(&hexon_with_readings_member(&plain)).is_ok());

        let dictionary_props = parquet::file::properties::WriterProperties::builder()
            .set_writer_version(parquet::file::properties::WriterVersion::PARQUET_1_0)
            .set_dictionary_enabled(true)
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk)
            .set_created_by(profile::CREATED_BY.to_string())
            .set_key_value_metadata(Some(vec![parquet::format::KeyValue::new(
                profile::FORMAT_VERSION_KEY.to_string(),
                profile::FORMAT_VERSION_VALUE.to_string(),
            )]))
            .build();
        let dictionary = readings_member_with(1.0, dictionary_props);
        match read_observation_hexon(&hexon_with_readings_member(&dictionary)) {
            Err(ObservationHexonError::ProfileViolation(message)) => {
                assert!(message.contains("PLAIN v1 data pages"), "{message}")
            }
            other => panic!("a dictionary-encoded member must be refused, got {other:?}"),
        }
    }

    /// §3.2 rule 2: the writer never emits a non-finite DOUBLE and the reader refuses one.
    #[test]
    fn non_finite_value_is_refused_by_writer_and_reader() {
        let readings = vec![reading("r1", "n1", "temp", f64::INFINITY, 10, 0)];
        assert!(matches!(
            write_observation_hexon("did:key:z6Mktest", &readings, &[], 1).unwrap_err(),
            ObservationHexonError::NonFiniteValue { .. }
        ));
        let member = readings_member_with(f64::INFINITY, profile::writer_properties());
        match read_observation_hexon(&hexon_with_readings_member(&member)) {
            Err(ObservationHexonError::ProfileViolation(message)) => {
                assert!(message.contains("non-finite"), "{message}")
            }
            other => panic!("a non-finite value must be refused, got {other:?}"),
        }
    }

    fn obs_manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "test".into(),
            hexon_type: HexonType::ObservationBatch,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "1".into(),
            build_id: None,
            name: "observation-batch".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }
}
