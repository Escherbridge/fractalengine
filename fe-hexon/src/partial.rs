//! Partial hexon fetch: the transport trait, its error type and the verified
//! outcome shapes `FetchStrategy` returns — see `src/AGENTS.md` §partial-fetch.

use async_trait::async_trait;
use bytes::Bytes;
use fe_format::{
    HeaderDoc, HexonManifest as SignedManifest, IntegrityError, IntegrityStatus, Layout,
    VerifiedHeader,
};
use fe_runtime::blob_store::BlobHash;
use fe_runtime::blob_transport::ProviderRef;

use crate::p2p::config::PeerPriority;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use crate::p2p::fetch::{pacing_delay, verify_fetched_header, FetchStrategy, SharedSource};

/// Where a partial fetch is served from; the order `FetchStrategy` tries
/// sources in is this enum's ordering (peers by priority, then registries).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FetchSource {
    /// A peer reached over the P2P blob transport.
    Peer {
        priority: PeerPriority,
        peer_ref: ProviderRef,
    },
    /// A hosted fe-hexon-registry.
    Registry { base_url: String },
}

impl std::fmt::Display for FetchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer { priority, peer_ref } => {
                write!(f, "peer {} ({priority:?})", peer_ref.as_str())
            }
            Self::Registry { base_url } => write!(f, "registry {base_url}"),
        }
    }
}

/// Why one partial fetch failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartialFetchError {
    /// The source does not hold that hexon, chunk or member.
    #[error("not found")]
    NotFound,
    /// The signed header names no such chunk.
    #[error("chunk sequence out of range")]
    OutOfRange,
    /// Bytes or header failed the signed integrity root.
    #[error("integrity check failed: {0}")]
    Integrity(IntegrityError),
    /// A validly-signed header for a different hexon than the one requested.
    #[error("requested {requested} but the signed header is {signed}")]
    Substituted { requested: String, signed: String },
    /// The source could not be reached or answered with an error.
    #[error("transport error: {0}")]
    Transport(String),
    /// The source did not answer within the strategy's request timeout.
    #[error("timed out")]
    Timeout,
}

impl PartialFetchError {
    /// True when the source served bytes that contradict the signed root
    /// (the failures that demote a source).
    pub fn is_integrity(&self) -> bool {
        matches!(self, Self::Integrity(_) | Self::Substituted { .. })
    }
}

impl From<IntegrityError> for PartialFetchError {
    fn from(error: IntegrityError) -> Self {
        Self::Integrity(error)
    }
}

/// One transport able to serve a hexon in parts. Implementations return
/// UNVERIFIED bytes; every check happens in `FetchStrategy`, so no transport
/// can skip one.
#[async_trait]
pub trait PartialHexonFetch: Send + Sync {
    /// Who this source is (drives ordering and demotion).
    fn source(&self) -> FetchSource;

    /// The header document for `uri` (`hexon_id` or `hexon_id@version`).
    async fn fetch_header(&self, uri: &str) -> Result<HeaderDoc, PartialFetchError>;

    /// Container chunk `seq`; for a monolithic hexon `seq == 0` is the whole archive.
    async fn fetch_chunk(&self, uri: &str, seq: u32) -> Result<Bytes, PartialFetchError>;

    /// One raw member of the archive by zip path.
    async fn fetch_member(&self, uri: &str, path: &str) -> Result<Bytes, PartialFetchError>;

    /// A content-addressed blob by its blake3 hash; sources with no by-hash
    /// route (the hosted registry) answer `NotFound`.
    async fn fetch_blob(&self, _hash: BlobHash) -> Result<Bytes, PartialFetchError> {
        Err(PartialFetchError::NotFound)
    }
}

/// A header that passed `verify_header` under `RequireSealed`, was bound to
/// the requested uri, and had its layout re-derived from the signed coverage.
#[derive(Debug, Clone)]
pub struct FetchedHeader {
    /// The raw document as served (its text is what the signature covers).
    pub doc: HeaderDoc,
    /// fe-format's verification result (signed coverage inside).
    pub verified: VerifiedHeader,
    /// The signed manifest, parsed from `doc.manifest` after verification.
    pub manifest: SignedManifest,
    /// Layout derived from the signed chunk index, never from the doc's claim.
    pub layout: Layout,
}

impl FetchedHeader {
    /// The concrete signed `hexon_id@version` every follow-up fetch requests.
    pub fn hexon_uri(&self) -> String {
        format!("{}@{}", self.manifest.hexon_id, self.manifest.version)
    }

    /// The DID whose signature covers this header.
    pub fn publisher_did(&self) -> &str {
        match &self.verified.status {
            IntegrityStatus::Sealed { publisher_did } => publisher_did,
            IntegrityStatus::Unsealed => "",
        }
    }

    /// Number of chunks the signed index lists (0 for a monolithic hexon).
    pub fn chunk_count(&self) -> u32 {
        match self.layout {
            Layout::Container { chunk_count } => chunk_count,
            Layout::Monolithic => 0,
        }
    }

    /// The signed chunk hashes in sequence order — what a confirmed P2P
    /// download checks each child against (S7a's `expected_chunk_hashes`).
    pub fn expected_chunk_hashes(&self) -> Vec<BlobHash> {
        let Some(index) = self.verified.coverage().and_then(|c| c.chunk_index()) else {
            return Vec::new();
        };
        let mut bundles: Vec<_> = index.bundles.iter().collect();
        bundles.sort_by_key(|b| b.seq);
        bundles
            .iter()
            .filter_map(|b| fe_runtime::blob_store::hash_from_hex(&b.blake3_hex).ok())
            .collect()
    }
}

/// One source the strategy tried and moved past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub source: FetchSource,
    pub error: PartialFetchError,
}

/// A verified value plus which source served it and which were passed over.
#[derive(Debug, Clone)]
pub struct FetchOutcome<T> {
    pub value: T,
    pub served_by: FetchSource,
    pub attempts: Vec<Attempt>,
}

impl<T> FetchOutcome<T> {
    /// Sources demoted during this fetch (they served bytes failing the signed root).
    pub fn demoted(&self) -> impl Iterator<Item = &FetchSource> {
        self.attempts
            .iter()
            .filter(|a| a.error.is_integrity())
            .map(|a| &a.source)
    }
}

/// Every source failed; `error` is the most severe failure, `attempts` all of them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct FetchFailure {
    pub error: PartialFetchError,
    pub attempts: Vec<Attempt>,
}
