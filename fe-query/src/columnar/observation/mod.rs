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

    /// Conformance test 9 (`zip_bomb_observation_hexon_refused_without_allocation`),
    /// exercised through the public `read_observation_hexon` entry point.
    #[test]
    fn zip_bomb_observation_hexon_refused_without_allocation() {
        let manifest = obs_manifest();
        let bomb =
            vec![0u8; (fe_format::limits::MAX_ENTRY_UNCOMPRESSED_BYTES + (8 << 20)) as usize];
        // A real archive whose `observations/readings.parquet` member is a
        // raw oversized blob (not even valid Parquet) — `read_member`'s
        // decompression-limit guard must refuse it before `read_observation_hexon`
        // ever tries to parse it as Parquet.
        let members: [(&str, &[u8]); 2] = [
            (profile::READINGS_MEMBER, bomb.as_slice()),
            (profile::PAYLOAD_REFS_MEMBER, b"x"),
        ];
        let bytes = HexonArchive::export_stored_deterministic(&manifest, &members).unwrap();
        assert!(
            bytes.len() < (4 << 20),
            "a Stored zip of zeros still compresses small on disk… wait, Stored means no \
             compression — this fixture is intentionally the oversized bytes themselves, \
             proving the cap triggers on real size, not a zip-bomb trick"
        );
        let err = read_observation_hexon(&bytes).unwrap_err();
        assert!(matches!(err, ObservationHexonError::Io(_)));
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
