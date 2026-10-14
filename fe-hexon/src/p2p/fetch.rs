//! `FetchStrategy`: the one verification + source-policy layer over every
//! partial-fetch transport — see `src/AGENTS.md` §partial-fetch.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bytes::Bytes;
use fe_format::integrity::{chunk_member_path, digest_hex, ASSETS_PREFIX, MANIFEST_PATH};
use fe_format::{limits, verify_header, HeaderDoc, IntegrityError, IntegrityPolicy, Layout};
use fe_runtime::blob_store::{hash_to_hex, BlobHash};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::config::{P2pConfig, PeerCandidate};
use crate::partial::{
    Attempt, FetchFailure, FetchOutcome, FetchSource, FetchedHeader, PartialFetchError,
    PartialHexonFetch,
};

/// A partial-fetch source the strategy can hold alongside others of any transport.
pub type SharedSource = Arc<dyn PartialHexonFetch>;

/// How long a caller waits for a free download slot.
const SLOT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default per-request timeout for one source call.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Idle time after which unused rate allowance is forgotten (no catch-up burst).
const PACING_IDLE_RESET: Duration = Duration::from_secs(1);

/// Delay owed after transferring `bytes` in `elapsed` under a `rate` cap
/// (bytes/sec); zero when uncapped or already slower than the cap.
pub fn pacing_delay(bytes: u64, rate: Option<u64>, elapsed: Duration) -> Duration {
    let Some(rate) = rate.filter(|r| *r > 0) else {
        return Duration::ZERO;
    };
    let target_nanos = u128::from(bytes) * 1_000_000_000 / u128::from(rate);
    let target = Duration::from_nanos(u64::try_from(target_nanos).unwrap_or(u64::MAX));
    target.saturating_sub(elapsed)
}

/// Shared rate window: bytes moved since `window_start`.
#[derive(Debug)]
struct Pacer {
    window_start: Instant,
    bytes: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Source policy + verification for partial hexon fetches: a semaphore slot
/// per call, peers before registries, demotion on integrity failure, an
/// optional publisher pin, and global rate pacing. Transports return
/// unverified bytes; every check lives here.
#[derive(Debug, Clone)]
pub struct FetchStrategy {
    config: P2pConfig,
    download_semaphore: Arc<Semaphore>,
    request_timeout: Duration,
    demoted: Arc<Mutex<BTreeSet<FetchSource>>>,
    pacer: Arc<Mutex<Pacer>>,
}

impl FetchStrategy {
    /// Create a new fetch strategy from configuration.
    pub fn new(config: P2pConfig) -> Self {
        let download_semaphore = Arc::new(Semaphore::new(config.max_concurrent_downloads));
        Self {
            config,
            download_semaphore,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            demoted: Arc::new(Mutex::new(BTreeSet::new())),
            pacer: Arc::new(Mutex::new(Pacer {
                window_start: Instant::now(),
                bytes: 0,
            })),
        }
    }

    /// Override the per-source request timeout.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sort peer candidates by priority (LAN first, then LowLatency, then Remote).
    pub fn prioritize_peers(candidates: &mut [PeerCandidate]) {
        candidates.sort_by_key(|c| c.priority);
    }

    /// Whether this node should re-seed downloaded crates to other peers.
    pub fn should_seed(&self) -> bool {
        self.config.seed_crates
    }

    /// Whether this node should permanently host published crates.
    pub fn should_host(&self) -> bool {
        self.config.crate_host
    }

    /// Access the underlying config.
    pub fn config(&self) -> &P2pConfig {
        &self.config
    }

    /// Acquire a download slot (held for a whole fetch); times out after 30 s.
    pub async fn acquire_download_slot(&self) -> Result<OwnedSemaphorePermit, PartialFetchError> {
        let semaphore = self.download_semaphore.clone();
        tokio::time::timeout(SLOT_TIMEOUT, semaphore.acquire_owned())
            .await
            .map_err(|_| PartialFetchError::Timeout)?
            .map_err(|_| PartialFetchError::Transport("download semaphore closed".into()))
    }

    /// Whether `source` served bytes failing the signed root earlier (tried last).
    pub fn is_demoted(&self, source: &FetchSource) -> bool {
        lock(&self.demoted).contains(source)
    }

    /// `sources` in try order: non-demoted first, then peers by priority before
    /// registries; ties keep the caller's order.
    pub fn order_sources(&self, sources: &[SharedSource]) -> Vec<SharedSource> {
        let demoted = lock(&self.demoted).clone();
        let mut keyed: Vec<((bool, u8, u8), SharedSource)> = sources
            .iter()
            .map(|source| {
                let who = source.source();
                let (tier, priority) = match &who {
                    FetchSource::Peer { priority, .. } => (0, *priority as u8),
                    FetchSource::Registry { .. } => (1, 0),
                };
                ((demoted.contains(&who), tier, priority), Arc::clone(source))
            })
            .collect();
        keyed.sort_by_key(|(key, _)| *key);
        keyed.into_iter().map(|(_, source)| source).collect()
    }

    /// Fetch and verify `uri`'s header under `RequireSealed { pin }`.
    pub async fn fetch_header(
        &self,
        uri: &str,
        sources: &[SharedSource],
        pin: Option<&str>,
    ) -> Result<FetchOutcome<FetchedHeader>, FetchFailure> {
        let _permit = self.slot().await?;
        let mut attempts = Vec::new();
        for source in self.order_sources(sources) {
            let who = source.source();
            let result = match self.timed(source.fetch_header(uri)).await {
                Ok(doc) => verify_fetched_header(uri, doc, pin),
                Err(error) => Err(error),
            };
            match result {
                Ok(header) => {
                    self.pace(header_len(&header.doc)).await;
                    return Ok(FetchOutcome {
                        value: header,
                        served_by: who,
                        attempts,
                    });
                }
                Err(error) => self.record(&mut attempts, who, error),
            }
        }
        Err(failure(attempts))
    }

    /// Fetch chunk `seq`, checked against the signed chunk index (container)
    /// or verified as the whole signed archive (monolithic, `seq == 0`).
    pub async fn fetch_chunk(
        &self,
        header: &FetchedHeader,
        seq: u32,
        sources: &[SharedSource],
    ) -> Result<FetchOutcome<Bytes>, FetchFailure> {
        let expected = match header.layout {
            Layout::Container { .. } => match header.verified.chunk_digest(seq) {
                Some(digest) => Some(digest.to_string()),
                None => return Err(immediate(PartialFetchError::OutOfRange)),
            },
            Layout::Monolithic if seq == 0 => None,
            Layout::Monolithic => return Err(immediate(PartialFetchError::OutOfRange)),
        };
        let uri = header.hexon_uri();
        let _permit = self.slot().await?;
        let mut attempts = Vec::new();
        for source in self.order_sources(sources) {
            let who = source.source();
            let result = match self.timed(source.fetch_chunk(&uri, seq)).await {
                Ok(bytes) => match &expected {
                    Some(digest) if digest_hex(&bytes) == *digest => Ok(bytes),
                    Some(_) => Err(PartialFetchError::Integrity(
                        IntegrityError::DigestMismatch {
                            path: chunk_member_path(seq),
                        },
                    )),
                    None => verify_whole_archive(header, bytes).await,
                },
                Err(error) => Err(error),
            };
            match result {
                Ok(bytes) => {
                    self.pace(bytes.len() as u64).await;
                    return Ok(FetchOutcome {
                        value: bytes,
                        served_by: who,
                        attempts,
                    });
                }
                Err(error) => self.record(&mut attempts, who, error),
            }
        }
        Err(failure(attempts))
    }

    /// Fetch one member, checked against the signed header map, `members.json`,
    /// or the asset self-certification; members the root does not name are
    /// `NotFound` without touching any source.
    pub async fn fetch_member(
        &self,
        header: &FetchedHeader,
        path: &str,
        sources: &[SharedSource],
    ) -> Result<FetchOutcome<Bytes>, FetchFailure> {
        let Some(coverage) = header.verified.coverage() else {
            return Err(immediate(IntegrityError::NotSealed.into()));
        };
        let named = path == MANIFEST_PATH
            || coverage.committed_digest(path).is_some()
            || path.starts_with(ASSETS_PREFIX);
        if !named {
            return Err(immediate(PartialFetchError::NotFound));
        }
        let uri = header.hexon_uri();
        let _permit = self.slot().await?;
        let mut attempts = Vec::new();
        for source in self.order_sources(sources) {
            let who = source.source();
            let result = match self.timed(source.fetch_member(&uri, path)).await {
                Ok(bytes) if path == MANIFEST_PATH => {
                    if bytes.as_ref() == header.doc.manifest.as_bytes() {
                        Ok(bytes)
                    } else {
                        Err(IntegrityError::DigestMismatch { path: path.into() }.into())
                    }
                }
                Ok(bytes) => coverage
                    .check_member(path, &bytes)
                    .map(|()| bytes)
                    .map_err(PartialFetchError::from),
                Err(error) => Err(error),
            };
            match result {
                Ok(bytes) => {
                    self.pace(bytes.len() as u64).await;
                    return Ok(FetchOutcome {
                        value: bytes,
                        served_by: who,
                        attempts,
                    });
                }
                Err(error) => self.record(&mut attempts, who, error),
            }
        }
        Err(failure(attempts))
    }

    /// Fetch a content-addressed blob from any source, accepted only when its
    /// blake3 equals `hash` (the generic asset primitive, §8 cross-cuts).
    pub async fn fetch_by_hash(
        &self,
        hash: BlobHash,
        sources: &[SharedSource],
    ) -> Result<FetchOutcome<Bytes>, FetchFailure> {
        let _permit = self.slot().await?;
        let mut attempts = Vec::new();
        for source in self.order_sources(sources) {
            let who = source.source();
            let result = match self.timed(source.fetch_blob(hash)).await {
                Ok(bytes) if *blake3::hash(&bytes).as_bytes() == hash => Ok(bytes),
                Ok(_) => Err(IntegrityError::DigestMismatch {
                    path: hash_to_hex(&hash),
                }
                .into()),
                Err(error) => Err(error),
            };
            match result {
                Ok(bytes) => {
                    self.pace(bytes.len() as u64).await;
                    return Ok(FetchOutcome {
                        value: bytes,
                        served_by: who,
                        attempts,
                    });
                }
                Err(error) => self.record(&mut attempts, who, error),
            }
        }
        Err(failure(attempts))
    }

    async fn slot(&self) -> Result<OwnedSemaphorePermit, FetchFailure> {
        self.acquire_download_slot().await.map_err(immediate)
    }

    /// Run one source call under the request timeout.
    async fn timed<T>(
        &self,
        call: impl std::future::Future<Output = Result<T, PartialFetchError>>,
    ) -> Result<T, PartialFetchError> {
        tokio::time::timeout(self.request_timeout, call)
            .await
            .unwrap_or(Err(PartialFetchError::Timeout))
    }

    /// Note a failed source; an integrity failure demotes it for later calls.
    fn record(&self, attempts: &mut Vec<Attempt>, source: FetchSource, error: PartialFetchError) {
        if error.is_integrity() {
            tracing::warn!(%source, %error, "demoting partial-fetch source");
            lock(&self.demoted).insert(source.clone());
        }
        attempts.push(Attempt { source, error });
    }

    /// Sleep off any rate debt for `bytes` just transferred (global across calls).
    async fn pace(&self, bytes: u64) {
        let rate = self.config.max_download_rate_bytes_per_sec;
        if !matches!(rate, Some(r) if r > 0) {
            return;
        }
        let delay = {
            let mut pacer = lock(&self.pacer);
            let now = Instant::now();
            let owed = pacing_delay(pacer.bytes, rate, Duration::ZERO);
            if now.duration_since(pacer.window_start) > owed + PACING_IDLE_RESET {
                pacer.window_start = now;
                pacer.bytes = 0;
            }
            pacer.bytes = pacer.bytes.saturating_add(bytes);
            pacing_delay(pacer.bytes, rate, now.duration_since(pacer.window_start))
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

/// Verify a served header for `requested_uri` under `RequireSealed { pin }`,
/// bind it to the uri, and re-derive its layout from the signed coverage —
/// the one header check every transport's bytes go through.
pub fn verify_fetched_header(
    requested_uri: &str,
    doc: HeaderDoc,
    pin: Option<&str>,
) -> Result<FetchedHeader, PartialFetchError> {
    let policy = IntegrityPolicy::RequireSealed {
        pin: pin.map(str::to_string),
    };
    let verified = verify_header(&doc, &policy)?;
    let coverage = verified.coverage().ok_or(IntegrityError::NotSealed)?;
    let manifest: fe_format::HexonManifest = serde_json::from_str(&doc.manifest)
        .map_err(|e| IntegrityError::Malformed(format!("{MANIFEST_PATH}: {e}")))?;

    let (requested_id, requested_version) = match requested_uri.split_once('@') {
        Some((id, version)) => (id, Some(version)),
        None => (requested_uri, None),
    };
    if manifest.hexon_id != requested_id || requested_version.is_some_and(|v| v != manifest.version)
    {
        return Err(PartialFetchError::Substituted {
            requested: requested_uri.to_string(),
            signed: format!("{}@{}", manifest.hexon_id, manifest.version),
        });
    }

    let layout = match coverage.chunk_index() {
        Some(index) => Layout::Container {
            chunk_count: index.bundles.len() as u32,
        },
        None => Layout::Monolithic,
    };
    if doc.layout != layout {
        return Err(IntegrityError::Unsupported(format!(
            "header claims {:?} but the signed root says {layout:?}",
            doc.layout
        ))
        .into());
    }
    Ok(FetchedHeader {
        doc,
        verified,
        manifest,
        layout,
    })
}

/// Verify a whole monolithic archive off the async thread.
async fn verify_whole_archive(
    header: &FetchedHeader,
    bytes: Bytes,
) -> Result<Bytes, PartialFetchError> {
    let pin = header.publisher_did().to_string();
    let signed_manifest = header.doc.manifest.clone();
    tokio::task::spawn_blocking(move || {
        check_whole_archive(&bytes, &signed_manifest, &pin).map(|()| bytes)
    })
    .await
    .map_err(|e| PartialFetchError::Transport(format!("verification task failed: {e}")))?
}

/// The served archive must carry the header's exact signed manifest (no
/// same-publisher substitution) and pass full `RequireSealed` verification.
fn check_whole_archive(
    bytes: &[u8],
    signed_manifest: &str,
    pin: &str,
) -> Result<(), PartialFetchError> {
    let malformed = |e: &dyn std::fmt::Display| IntegrityError::Malformed(format!("{e:#}"));
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| malformed(&e))?;
    let manifest = archive
        .by_name(MANIFEST_PATH)
        .map_err(|_| IntegrityError::MissingMember {
            path: MANIFEST_PATH.into(),
        })?;
    let declared = manifest.size();
    let mut budget = limits::MAX_JSON_ENTRY_BYTES;
    let served = fe_format::read_entry_capped(
        manifest,
        declared,
        limits::MAX_JSON_ENTRY_BYTES,
        &mut budget,
    )
    .map_err(|e| malformed(&e))?;
    if served != signed_manifest.as_bytes() {
        return Err(IntegrityError::DigestMismatch {
            path: MANIFEST_PATH.into(),
        }
        .into());
    }
    fe_format::verify_archive(
        bytes,
        &IntegrityPolicy::RequireSealed {
            pin: Some(pin.to_string()),
        },
    )?;
    Ok(())
}

fn header_len(doc: &HeaderDoc) -> u64 {
    let members: usize = doc.header_members.values().map(String::len).sum();
    (doc.manifest.len() + members) as u64
}

/// Higher is more alarming; the reported failure is the most alarming attempt.
fn severity(error: &PartialFetchError) -> u8 {
    match error {
        PartialFetchError::Integrity(_) | PartialFetchError::Substituted { .. } => 5,
        PartialFetchError::Timeout => 4,
        PartialFetchError::Transport(_) => 3,
        PartialFetchError::OutOfRange => 2,
        PartialFetchError::NotFound => 1,
    }
}

fn failure(attempts: Vec<Attempt>) -> FetchFailure {
    let error = attempts
        .iter()
        .max_by_key(|a| severity(&a.error))
        .map(|a| a.error.clone())
        .unwrap_or_else(|| PartialFetchError::Transport("no partial-fetch sources".into()));
    FetchFailure { error, attempts }
}

fn immediate(error: PartialFetchError) -> FetchFailure {
    FetchFailure {
        error,
        attempts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::config::PeerPriority;
    use ed25519_dalek::SigningKey;
    use fe_format::{
        ChunkInput, ElevationEncoding, HexonArchive, HexonArchiveIndex, HexonType, TilesetMeta,
    };

    #[test]
    fn test_prioritize_peers_ordering() {
        let mut candidates = vec![
            PeerCandidate {
                peer_did: "did:key:remote1".to_string(),
                priority: PeerPriority::Remote,
                last_seen_ms: Some(5000),
            },
            PeerCandidate {
                peer_did: "did:key:lan1".to_string(),
                priority: PeerPriority::Lan,
                last_seen_ms: Some(100),
            },
            PeerCandidate {
                peer_did: "did:key:low1".to_string(),
                priority: PeerPriority::LowLatency,
                last_seen_ms: None,
            },
            PeerCandidate {
                peer_did: "did:key:lan2".to_string(),
                priority: PeerPriority::Lan,
                last_seen_ms: Some(200),
            },
        ];

        FetchStrategy::prioritize_peers(&mut candidates);

        assert_eq!(candidates[0].priority, PeerPriority::Lan);
        assert_eq!(candidates[1].priority, PeerPriority::Lan);
        assert_eq!(candidates[2].priority, PeerPriority::LowLatency);
        assert_eq!(candidates[3].priority, PeerPriority::Remote);
    }

    #[test]
    fn test_fetch_strategy_policy() {
        let config = P2pConfig {
            seed_crates: true,
            crate_host: false,
            max_concurrent_downloads: 5,
            max_download_rate_bytes_per_sec: None,
        };
        let strategy = FetchStrategy::new(config);

        assert!(strategy.should_seed());
        assert!(!strategy.should_host());
    }

    #[tokio::test]
    async fn test_acquire_download_slot_success() {
        let config = P2pConfig {
            seed_crates: false,
            crate_host: false,
            max_concurrent_downloads: 2,
            max_download_rate_bytes_per_sec: None,
        };
        let strategy = FetchStrategy::new(config);

        let permit1 = strategy.acquire_download_slot().await;
        assert!(permit1.is_ok());

        let permit2 = strategy.acquire_download_slot().await;
        assert!(permit2.is_ok());

        // Both slots consumed — drop one to free
        drop(permit1);
        let permit3 = strategy.acquire_download_slot().await;
        assert!(permit3.is_ok());
    }

    /// The pacing delay is the time the bytes should have taken at the cap,
    /// minus the time they actually took (never negative, never when uncapped).
    #[test]
    fn pacing_delay_table() {
        let ms = Duration::from_millis;
        let rows: &[(u64, Option<u64>, Duration, Duration)] = &[
            // (bytes, rate, elapsed, expected delay)
            (1_000, None, ms(0), ms(0)),
            (1_000, Some(0), ms(0), ms(0)),
            (1_000, Some(1_000), ms(0), ms(1_000)),
            (1_000, Some(1_000), ms(250), ms(750)),
            (1_000, Some(1_000), ms(1_000), ms(0)),
            (1_000, Some(1_000), ms(5_000), ms(0)),
            (500, Some(2_000), ms(100), ms(150)),
            (0, Some(1_000), ms(0), ms(0)),
            (u64::MAX, Some(1), ms(0), Duration::from_nanos(u64::MAX)),
        ];
        for (bytes, rate, elapsed, expected) in rows {
            assert_eq!(
                pacing_delay(*bytes, *rate, *elapsed),
                *expected,
                "bytes={bytes} rate={rate:?} elapsed={elapsed:?}"
            );
        }
    }

    fn manifest(id: &str) -> fe_format::HexonManifest {
        fe_format::HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mkplaceholder".into(),
            publisher_name: None,
            version: "1.0.0".into(),
            build_id: None,
            name: id.into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
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

    fn meta(tile_count: u32) -> TilesetMeta {
        TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count,
            region_name: "fetch".into(),
            ..Default::default()
        }
    }

    /// The header doc a transport would serve for `sealed` (via the lazy reader).
    fn header_doc_of(sealed: &[u8]) -> HeaderDoc {
        let path = std::env::temp_dir().join(format!("fe_hexon_fetch_{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, sealed).unwrap();
        let doc = HexonArchiveIndex::open(&path).unwrap().header_doc();
        let _ = std::fs::remove_file(&path);
        doc
    }

    fn sealed_container(id: &str, key: &SigningKey) -> Vec<u8> {
        let chunks: Vec<ChunkInput> = (0..2u32)
            .map(|seq| {
                let tiles = vec![(format!("10/{seq}/0"), vec![seq as u8; 64])];
                let chunk = HexonArchive::export_tileset(
                    manifest(&format!("{id}-{seq}")),
                    &meta(1),
                    &tiles,
                    &[],
                    None,
                )
                .unwrap();
                ChunkInput {
                    seq,
                    tile_count: 1,
                    zoom: 10,
                    x_range: [seq, seq],
                    y_range: [0, 0],
                    hilbert_range: [u64::from(seq), u64::from(seq)],
                    archive_bytes: fe_format::seal(&chunk, key).unwrap(),
                }
            })
            .collect();
        let container = HexonArchive::export_chunked(manifest(id), &meta(2), &chunks).unwrap();
        fe_format::seal(&container, key).unwrap()
    }

    /// The header check binds a validly-signed header to the uri it was
    /// requested under, and takes the layout from the signed index — a
    /// transport can neither substitute another hexon nor downgrade a
    /// container to "monolithic" (where seq 0 would be accepted unchecked).
    #[test]
    fn served_headers_are_bound_to_the_requested_uri_and_signed_layout() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let honest = header_doc_of(&sealed_container("bound", &key));
        assert_eq!(honest.layout, Layout::Container { chunk_count: 2 });

        let mut downgraded = honest.clone();
        downgraded.layout = Layout::Monolithic;

        let rows: Vec<(&str, HeaderDoc, Option<&str>)> = vec![
            ("bound", honest.clone(), None),
            ("bound@1.0.0", honest.clone(), None),
            ("other", honest.clone(), Some("substituted")),
            ("bound@2.0.0", honest.clone(), Some("substituted")),
            ("bound@1.0.0", downgraded, Some("layout")),
        ];
        for (uri, doc, refusal) in rows {
            let result = verify_fetched_header(uri, doc, None);
            match refusal {
                None => {
                    let header = result.unwrap_or_else(|e| panic!("{uri}: {e}"));
                    assert_eq!(header.hexon_uri(), "bound@1.0.0");
                    assert_eq!(header.chunk_count(), 2);
                    assert_eq!(header.expected_chunk_hashes().len(), 2);
                }
                Some("substituted") => assert!(
                    matches!(result, Err(PartialFetchError::Substituted { .. })),
                    "{uri}: {result:?}"
                ),
                Some(_) => assert!(
                    matches!(
                        result,
                        Err(PartialFetchError::Integrity(IntegrityError::Unsupported(_)))
                    ),
                    "{uri}: {result:?}"
                ),
            }
        }
    }
}
