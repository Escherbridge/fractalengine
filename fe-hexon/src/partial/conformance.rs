//! Transport-neutral partial-fetch conformance suite (PAS-P5 A7): every
//! transport must pass the same `run` — see `src/AGENTS.md` §partial-fetch.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_format::{
    seal, verifying_key_to_did, AssetEntry, ChunkInput, ElevationEncoding, EntryKind, HexonArchive,
    HexonManifest, HexonType, IntegrityError, Layout, License, TilesetMeta,
};

use crate::p2p::config::P2pConfig;
use crate::partial::{
    FetchFailure, FetchOutcome, FetchSource, FetchStrategy, FetchedHeader, PartialFetchError,
    SharedSource,
};

/// A uri no conforming source serves.
pub const UNKNOWN_URI: &str = "conformance-absent@0.0.0";
/// A publisher DID that signed none of the fixtures.
pub const WRONG_PUBLISHER: &str = "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK";
/// Number of chunks in the container fixture.
pub const CONTAINER_CHUNKS: u32 = 3;

/// Deterministic fixture signing key — never a real secret.
pub fn fixture_key() -> SigningKey {
    SigningKey::from_bytes(&[0x5a; 32])
}

/// One sealed fixture hexon plus what a conforming fetch must return for it.
#[derive(Debug, Clone)]
pub struct FixtureHexon {
    pub hexon_id: String,
    pub version: String,
    /// The sealed archive bytes a source must serve.
    pub bytes: Vec<u8>,
    /// Container chunk bytes in sequence order (empty when monolithic).
    pub chunks: Vec<Vec<u8>>,
    /// Body members (zip path, exact bytes) a member fetch must return.
    pub probe_members: Vec<(String, Vec<u8>)>,
}

impl FixtureHexon {
    /// `hexon_id@version`.
    pub fn uri(&self) -> String {
        format!("{}@{}", self.hexon_id, self.version)
    }

    /// The registry's on-disk file name for this fixture.
    pub fn file_name(&self) -> String {
        format!("{}.hexon", self.uri())
    }

    /// The layout the signed header must report.
    pub fn layout(&self) -> Layout {
        if self.chunks.is_empty() {
            Layout::Monolithic
        } else {
            Layout::Container {
                chunk_count: self.chunks.len() as u32,
            }
        }
    }
}

/// The three fixtures every transport is checked against.
#[derive(Debug, Clone)]
pub struct ConformanceFixtures {
    /// The DID of [`fixture_key`], which sealed every fixture.
    pub publisher_did: String,
    /// A sealed monolithic model hexon (one asset, one GPX body member).
    pub model: FixtureHexon,
    /// A sealed monolithic terrain tileset.
    pub tileset: FixtureHexon,
    /// A sealed container of [`CONTAINER_CHUNKS`] sealed tileset chunks.
    pub container: FixtureHexon,
}

impl ConformanceFixtures {
    /// Build every fixture with fe-format and [`fixture_key`].
    pub fn build() -> anyhow::Result<Self> {
        let key = fixture_key();
        Ok(Self {
            publisher_did: verifying_key_to_did(&key.verifying_key()),
            model: model_fixture(&key)?,
            tileset: tileset_fixture(&key)?,
            container: container_fixture(&key)?,
        })
    }

    /// Every fixture, for a transport test to publish or place on disk.
    pub fn all(&self) -> [&FixtureHexon; 3] {
        [&self.model, &self.tileset, &self.container]
    }
}

/// One named check and its outcome.
#[derive(Debug, Clone)]
pub struct CaseResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Every case `run` checked against one source.
#[derive(Debug, Clone)]
pub struct ConformanceReport {
    pub source: FetchSource,
    pub cases: Vec<CaseResult>,
}

impl ConformanceReport {
    /// True when every case passed.
    pub fn passed(&self) -> bool {
        self.cases.iter().all(|case| case.passed)
    }

    /// The cases that failed.
    pub fn failures(&self) -> Vec<&CaseResult> {
        self.cases.iter().filter(|case| !case.passed).collect()
    }
}

impl std::fmt::Display for ConformanceReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let failures = self.failures();
        writeln!(
            f,
            "conformance for {}: {}/{} cases passed",
            self.source,
            self.cases.len() - failures.len(),
            self.cases.len()
        )?;
        for case in failures {
            writeln!(f, "  FAIL {}: {}", case.name, case.detail)?;
        }
        Ok(())
    }
}

/// Run the suite against `fetcher`, which must already serve every fixture
/// in `fixtures`; checks go through [`FetchStrategy`] exactly as production does.
pub async fn run(fetcher: SharedSource, fixtures: &ConformanceFixtures) -> ConformanceReport {
    let strategy =
        FetchStrategy::new(P2pConfig::default()).with_request_timeout(Duration::from_secs(30));
    let sources = vec![Arc::clone(&fetcher)];
    let mut cases = Vec::new();

    for fixture in fixtures.all() {
        let id = &fixture.hexon_id;
        let header = strategy.fetch_header(&fixture.uri(), &sources, None).await;
        cases.push(check(
            format!("{id}: header verifies under its publisher"),
            match &header {
                Ok(outcome) => header_matches(&outcome.value, fixture, &fixtures.publisher_did),
                Err(failure) => Err(failure.to_string()),
            },
        ));
        let pinned = strategy
            .fetch_header(&fixture.uri(), &sources, Some(&fixtures.publisher_did))
            .await;
        cases.push(check(
            format!("{id}: header passes its publisher pin"),
            pinned.map(|_| ()).map_err(|f| f.to_string()),
        ));
        let wrong_pin = strategy
            .fetch_header(&fixture.uri(), &sources, Some(WRONG_PUBLISHER))
            .await;
        cases.push(check(
            format!("{id}: a pinned wrong publisher is refused"),
            expect_failure(wrong_pin, |e| {
                matches!(
                    e,
                    PartialFetchError::Integrity(IntegrityError::PublisherMismatch { .. })
                )
            }),
        ));

        let Ok(header) = header else {
            cases.push(check(
                format!("{id}: chunk and member cases"),
                Err("skipped: no verified header".into()),
            ));
            continue;
        };
        let header = header.value;

        let expected_chunks: Vec<&[u8]> = match fixture.layout() {
            Layout::Monolithic => vec![fixture.bytes.as_slice()],
            Layout::Container { .. } => fixture.chunks.iter().map(Vec::as_slice).collect(),
        };
        for (seq, expected) in expected_chunks.iter().enumerate() {
            let seq = seq as u32;
            let fetched = strategy.fetch_chunk(&header, seq, &sources).await;
            cases.push(check(
                format!("{id}: chunk {seq} bytes equal the signed bytes"),
                expect_bytes(fetched, expected),
            ));
        }
        let past_end = expected_chunks.len() as u32;
        let out_of_range = strategy.fetch_chunk(&header, past_end, &sources).await;
        cases.push(check(
            format!("{id}: chunk {past_end} is out of range"),
            expect_failure(out_of_range, |e| *e == PartialFetchError::OutOfRange),
        ));

        for (path, expected) in &fixture.probe_members {
            let fetched = strategy.fetch_member(&header, path, &sources).await;
            cases.push(check(
                format!("{id}: member {path} bytes equal the archive's"),
                expect_bytes(fetched, expected),
            ));
        }
        let unnamed = strategy
            .fetch_member(&header, "terrain/tiles/99/99/99.png", &sources)
            .await;
        cases.push(check(
            format!("{id}: a member the signed root does not name is not found"),
            expect_failure(unnamed, |e| *e == PartialFetchError::NotFound),
        ));
    }

    let unknown = strategy.fetch_header(UNKNOWN_URI, &sources, None).await;
    cases.push(check(
        "unknown uri: strategy reports not found".into(),
        expect_failure(unknown, |e| *e == PartialFetchError::NotFound),
    ));
    let raw_unknown = fetcher.fetch_header(UNKNOWN_URI).await;
    cases.push(check(
        "unknown uri: the transport itself answers NotFound".into(),
        match raw_unknown {
            Err(PartialFetchError::NotFound) => Ok(()),
            other => Err(format!("expected NotFound, got {:?}", other.map(|d| d.uri))),
        },
    ));
    let raw_past_end = fetcher
        .fetch_chunk(&fixtures.container.uri(), CONTAINER_CHUNKS)
        .await;
    cases.push(check(
        "missing chunk: the transport itself answers NotFound or OutOfRange".into(),
        match raw_past_end {
            Err(PartialFetchError::NotFound | PartialFetchError::OutOfRange) => Ok(()),
            Err(other) => Err(format!("expected NotFound/OutOfRange, got {other}")),
            Ok(bytes) => Err(format!("expected an error, got {} bytes", bytes.len())),
        },
    ));

    ConformanceReport {
        source: fetcher.source(),
        cases,
    }
}

fn check(name: String, outcome: Result<(), String>) -> CaseResult {
    match outcome {
        Ok(()) => CaseResult {
            name,
            passed: true,
            detail: String::new(),
        },
        Err(detail) => CaseResult {
            name,
            passed: false,
            detail,
        },
    }
}

fn header_matches(
    header: &FetchedHeader,
    fixture: &FixtureHexon,
    publisher_did: &str,
) -> Result<(), String> {
    if header.publisher_did() != publisher_did {
        return Err(format!(
            "publisher {} != {publisher_did}",
            header.publisher_did()
        ));
    }
    if header.hexon_uri() != fixture.uri() {
        return Err(format!("uri {} != {}", header.hexon_uri(), fixture.uri()));
    }
    if header.layout != fixture.layout() {
        return Err(format!(
            "layout {:?} != {:?}",
            header.layout,
            fixture.layout()
        ));
    }
    Ok(())
}

fn expect_bytes(
    fetched: Result<FetchOutcome<bytes::Bytes>, FetchFailure>,
    expected: &[u8],
) -> Result<(), String> {
    match fetched {
        Ok(outcome) if outcome.value.as_ref() == expected => Ok(()),
        Ok(outcome) => Err(format!(
            "{} bytes returned, {} expected (content differs)",
            outcome.value.len(),
            expected.len()
        )),
        Err(failure) => Err(failure.to_string()),
    }
}

fn expect_failure<T>(
    result: Result<FetchOutcome<T>, FetchFailure>,
    expected: impl Fn(&PartialFetchError) -> bool,
) -> Result<(), String> {
    match result {
        Err(failure) if expected(&failure.error) => Ok(()),
        Err(failure) => Err(format!("wrong error: {}", failure.error)),
        Ok(outcome) => Err(format!(
            "expected a failure, served by {}",
            outcome.served_by
        )),
    }
}

fn manifest(id: &str, hexon_type: HexonType) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type,
        // `seal` replaces this with the signing key's DID.
        publisher_did: "did:key:unsealed".into(),
        publisher_name: Some("Conformance".into()),
        version: "1.0.0".into(),
        build_id: None,
        name: id.into(),
        description: Some("partial-fetch conformance fixture".into()),
        tags: vec!["conformance".into()],
        created_at: "2026-10-14T00:00:00Z".into(),
        updated_at: "2026-10-14T00:00:00Z".into(),
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

fn tileset_meta(tile_count: u32) -> TilesetMeta {
    TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 10,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        tile_count,
        region_name: "Conformance".into(),
        ..Default::default()
    }
}

/// Deterministic, non-repeating fixture bytes.
fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn model_fixture(key: &SigningKey) -> anyhow::Result<FixtureHexon> {
    let id = "conformance-model";
    let blob = pattern(1, 4096);
    let hash = fe_format::integrity::digest_hex(&blob);
    let entry = AssetEntry {
        entry_id: "model".into(),
        kind: EntryKind::Model,
        asset_hash: hash.clone(),
        format: "glb".into(),
        label: "Conformance model".into(),
        tags: vec![],
        description: None,
        is_placeable: true,
        is_private: false,
        auto_scale: false,
        preview_image: None,
        center: None,
        extents: None,
        sub_assets: None,
        address: None,
        metadata: None,
    };
    let gpx = b"<gpx version=\"1.1\"><trk><name>conformance</name></trk></gpx>".to_vec();
    let archive = HexonArchive::export(
        manifest(id, HexonType::Model),
        vec![entry],
        vec![(hash.clone(), blob.clone())],
        Some(License::default()),
        None,
        Some(&[("route.gpx".to_string(), gpx.clone())]),
        None,
    )?;
    Ok(FixtureHexon {
        hexon_id: id.into(),
        version: "1.0.0".into(),
        bytes: seal(&archive, key)?,
        chunks: vec![],
        probe_members: vec![
            (format!("assets/{hash}"), blob),
            ("terrain/tracks/route.gpx".into(), gpx),
        ],
    })
}

fn tileset_fixture(key: &SigningKey) -> anyhow::Result<FixtureHexon> {
    let id = "conformance-tileset";
    let tiles = vec![
        ("10/0/0".to_string(), pattern(2, 2048)),
        ("10/1/0".to_string(), pattern(3, 2048)),
    ];
    let archive = HexonArchive::export_tileset(
        manifest(id, HexonType::TerrainTileset),
        &tileset_meta(tiles.len() as u32),
        &tiles,
        &[],
        Some(License::default()),
    )?;
    Ok(FixtureHexon {
        hexon_id: id.into(),
        version: "1.0.0".into(),
        bytes: seal(&archive, key)?,
        chunks: vec![],
        probe_members: vec![("terrain/tiles/10/0/0.png".into(), tiles[0].1.clone())],
    })
}

fn container_fixture(key: &SigningKey) -> anyhow::Result<FixtureHexon> {
    let id = "conformance-container";
    let mut inputs = Vec::new();
    for seq in 0..CONTAINER_CHUNKS {
        let tiles = vec![(format!("10/{seq}/0"), pattern(10 + seq as u8, 1024))];
        let chunk = HexonArchive::export_tileset(
            manifest(&format!("{id}-chunk-{seq}"), HexonType::TerrainTileset),
            &tileset_meta(1),
            &tiles,
            &[],
            None,
        )?;
        inputs.push(ChunkInput {
            seq,
            tile_count: 1,
            zoom: 10,
            x_range: [seq, seq],
            y_range: [0, 0],
            hilbert_range: [u64::from(seq), u64::from(seq)],
            archive_bytes: seal(&chunk, key)?,
        });
    }
    let archive = HexonArchive::export_chunked(
        manifest(id, HexonType::TerrainTileset),
        &tileset_meta(CONTAINER_CHUNKS),
        &inputs,
    )?;
    Ok(FixtureHexon {
        hexon_id: id.into(),
        version: "1.0.0".into(),
        bytes: seal(&archive, key)?,
        chunks: inputs.into_iter().map(|c| c.archive_bytes).collect(),
        probe_members: vec![],
    })
}
