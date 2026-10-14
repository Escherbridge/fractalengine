//! P2P partial fetch over the `BlobTransport` seam: a sealed hexon travels as
//! one HashSeq `[HeaderDoc, chunk_0..n]` (container) or `[HeaderDoc, whole
//! archive]` (monolithic) — see `src/AGENTS.md` §partial-fetch-p2p.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use bytes::Bytes;
use fe_format::integrity::{chunk_member_path, MANIFEST_PATH};
use fe_format::{
    limits, verify_header, HeaderDoc, HexonArchiveIndex, HexonManifest, IntegrityError,
    IntegrityPolicy, Layout,
};
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex, BlobHash};
use fe_runtime::blob_transport::{content_hash, hash_seq_root, BlobTransport, Hash32, ProviderRef};

use crate::p2p::config::PeerPriority;
use crate::partial::{FetchSource, PartialFetchError, PartialHexonFetch, SharedSource};

/// Tag prefix protecting a hexon published by [`publish_to_transport`].
pub const HEXON_TAG_PREFIX: &str = "fe/hexon/";
/// Largest HashSeq child 0 (`HeaderDoc` JSON) fetched — fe-sync's index cap.
pub const MAX_HEADER_DOC_BYTES: u64 = 16 * 1024 * 1024;
/// Largest container chunk fetched — fe-sync's bundle cap.
pub const MAX_CHUNK_BYTES: u64 = 64 * 1024 * 1024;
/// Largest monolithic archive fetched whole.
pub const MAX_WHOLE_ARCHIVE_BYTES: u64 = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
/// Most HashSeq children accepted (header + 65 536 chunks).
pub const MAX_HASH_SEQ_CHILDREN: usize = 1 + (1 << 16);
/// Resolved roots one fetcher remembers before forgetting them all.
const MAX_RESOLVED_ROOTS: usize = 64;

/// What [`publish_to_transport`] stored and tagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedHexon {
    /// The signed `hexon_id@version`.
    pub uri: String,
    /// HashSeq root over `children`.
    pub root: Hash32,
    /// `[HeaderDoc JSON, chunk_0..n]` or `[HeaderDoc JSON, whole archive]`.
    pub children: Vec<Hash32>,
    /// The layout the signed root describes.
    pub layout: Layout,
}

/// Lock a mutex, recovering the data from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run blocking archive work off the async runtime (NFR-3).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| anyhow!("blocking task failed: {e}"))?
}

/// Publish the sealed hexon at `archive_path` as one HashSeq tagged
/// `fe/hexon/<uri>`: a container re-seeds its existing signed chunks (never
/// re-bundled), a monolithic archive goes whole. Unsealed or tampered input
/// is refused before anything is stored.
pub async fn publish_to_transport(
    transport: &dyn BlobTransport,
    archive_path: &Path,
) -> anyhow::Result<PublishedHexon> {
    let path = archive_path.to_path_buf();
    let index = blocking(move || {
        HexonArchiveIndex::open_with_policy(&path, IntegrityPolicy::RequireSealed { pin: None })
            .context("refusing to publish: not a valid sealed hexon")
    })
    .await?;
    let header = index.header_doc();
    let layout = index.layout();
    let fe_format::IntegrityStatus::Sealed { publisher_did } = index.integrity_status() else {
        anyhow::bail!("refusing to publish: not a sealed hexon");
    };
    let mut children = vec![transport.add(Bytes::from(header.to_json())).await?];
    let mut index = Some(index);
    match layout {
        Layout::Container { chunk_count } => {
            for seq in 0..chunk_count {
                let reader = index.take().expect("index returned by the previous chunk");
                let (reader, chunk) = blocking(move || {
                    let chunk = reader.chunk(seq)?.with_context(|| {
                        format!("container lists chunk {seq} but has no member")
                    })?;
                    Ok((reader, chunk))
                })
                .await?;
                index = Some(reader);
                children.push(transport.add(Bytes::from(chunk)).await?);
            }
        }
        Layout::Monolithic => {
            let path = archive_path.to_path_buf();
            let signed_manifest = header.manifest.clone();
            let whole = blocking(move || {
                let bytes =
                    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                fe_format::verify_archive_against_header(
                    &bytes,
                    &signed_manifest,
                    &publisher_did,
                    limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
                )
                .context("refusing to publish: the archive body fails its own seal")?;
                Ok(bytes)
            })
            .await?;
            children.push(transport.add(Bytes::from(whole)).await?);
        }
    }
    let root = transport.add_hash_seq(&children).await?;
    transport
        .set_tag(&format!("{HEXON_TAG_PREFIX}{}", header.uri), root)
        .await?;
    Ok(PublishedHexon {
        uri: header.uri,
        root,
        children,
        layout,
    })
}

/// Where a hexon can be fetched: its HashSeq root and the peers serving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorEntry {
    pub root: Hash32,
    pub providers: Vec<ProviderRef>,
}

/// Maps a hexon uri (`hexon_id` or `hexon_id@version`) to the HashSeq root
/// and providers serving it — filled from P3 tileset advertisements or by
/// the caller. A lookup of `id@version` falls back to a bare `id` entry.
#[derive(Debug, Default)]
pub struct HexonLocator {
    entries: Mutex<HashMap<String, LocatorEntry>>,
}

impl HexonLocator {
    /// An empty locator.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// `providers` serve `uri` under `root`; the same root merges providers,
    /// a new root replaces the entry.
    pub fn insert(
        &self,
        uri: &str,
        root: Hash32,
        providers: impl IntoIterator<Item = ProviderRef>,
    ) {
        let mut entries = lock(&self.entries);
        let entry = entries.entry(uri.to_string()).or_insert(LocatorEntry {
            root,
            providers: Vec::new(),
        });
        if entry.root != root {
            *entry = LocatorEntry {
                root,
                providers: Vec::new(),
            };
        }
        for provider in providers {
            if !entry.providers.contains(&provider) {
                entry.providers.push(provider);
            }
        }
    }

    /// Record one P3 tileset advertisement (`root_hash` hex, `provider_addr`).
    pub fn insert_advertisement(
        &self,
        tileset_id: &str,
        root_hash: &str,
        provider_addr: &str,
    ) -> anyhow::Result<()> {
        let root = hash_from_hex(root_hash).map_err(|e| anyhow!("bad root hash: {e}"))?;
        self.insert(tileset_id, root, [ProviderRef::new(provider_addr)]);
        Ok(())
    }

    /// The entry for `uri`, else for its bare `hexon_id`.
    pub fn lookup(&self, uri: &str) -> Option<LocatorEntry> {
        let entries = lock(&self.entries);
        entries.get(uri).cloned().or_else(|| {
            let (id, _) = uri.split_once('@')?;
            entries.get(id).cloned()
        })
    }

    /// One [`P2pPartialFetch`] per provider known for `uri`, all at `priority`.
    pub fn sources(
        self: &Arc<Self>,
        transport: &Arc<dyn BlobTransport>,
        uri: &str,
        priority: PeerPriority,
    ) -> Vec<SharedSource> {
        self.lookup(uri)
            .map(|entry| entry.providers)
            .unwrap_or_default()
            .into_iter()
            .map(|peer| {
                Arc::new(P2pPartialFetch::new(
                    Arc::clone(transport),
                    Arc::clone(self),
                    peer,
                    priority,
                )) as SharedSource
            })
            .collect()
    }
}

/// A located root, resolved from one peer: its children and parsed header.
struct Resolved {
    children: Vec<Hash32>,
    header: HeaderDoc,
    /// The signed `hexon_id@version`, when the header parses.
    signed_uri: Option<String>,
    /// Why the header fails `RequireSealed`, if it does (chunks are then refused).
    header_error: Option<IntegrityError>,
    /// Signed chunk digests by seq, when the header verifies as a container.
    signed_chunks: Option<Vec<String>>,
}

/// [`PartialHexonFetch`] over a [`BlobTransport`] from one peer. Returns
/// unverified bytes like every transport, but refuses (as `Integrity`) any
/// blob whose hash is not the HashSeq child asked for, and any chunk child
/// the peer's own signed chunk index does not name — before downloading it.
pub struct P2pPartialFetch {
    transport: Arc<dyn BlobTransport>,
    locator: Arc<HexonLocator>,
    peer: ProviderRef,
    priority: PeerPriority,
    resolved: Mutex<HashMap<Hash32, Arc<Resolved>>>,
}

impl P2pPartialFetch {
    /// Fetch from `peer` over `transport`, finding roots through `locator`.
    pub fn new(
        transport: Arc<dyn BlobTransport>,
        locator: Arc<HexonLocator>,
        peer: ProviderRef,
        priority: PeerPriority,
    ) -> Self {
        Self {
            transport,
            locator,
            peer,
            priority,
            resolved: Mutex::new(HashMap::new()),
        }
    }

    /// One blob from the peer, refused unless it hashes to `hash`.
    async fn blob(&self, hash: Hash32, cap: u64, what: &str) -> Result<Bytes, PartialFetchError> {
        let bytes = self
            .transport
            .fetch(&self.peer, hash, cap)
            .await
            .map_err(|e| PartialFetchError::Transport(format!("{e:#}")))?;
        if content_hash(&bytes) != hash {
            return Err(IntegrityError::DigestMismatch { path: what.into() }.into());
        }
        Ok(bytes)
    }

    /// Children + parsed header of the root `uri` locates (cached per root).
    async fn resolve(&self, uri: &str) -> Result<Arc<Resolved>, PartialFetchError> {
        let entry = self
            .locator
            .lookup(uri)
            .ok_or(PartialFetchError::NotFound)?;
        if let Some(resolved) = lock(&self.resolved).get(&entry.root) {
            return Ok(Arc::clone(resolved));
        }
        let children = self
            .transport
            .fetch_hash_seq(&self.peer, entry.root, MAX_HASH_SEQ_CHILDREN)
            .await
            .map_err(|e| PartialFetchError::Transport(format!("{e:#}")))?;
        if children.len() < 2 || hash_seq_root(&children) != entry.root {
            return Err(IntegrityError::Malformed(format!(
                "HashSeq {} does not resolve to [header, body..]",
                hash_to_hex(&entry.root)
            ))
            .into());
        }
        let raw = self
            .blob(children[0], MAX_HEADER_DOC_BYTES, "header document")
            .await?;
        let header: HeaderDoc = serde_json::from_slice(&raw)
            .map_err(|e| IntegrityError::Malformed(format!("header document: {e}")))?;
        let signed_uri = serde_json::from_str::<HexonManifest>(&header.manifest)
            .ok()
            .map(|m| format!("{}@{}", m.hexon_id, m.version));
        let (signed_chunks, header_error) =
            match verify_header(&header, &IntegrityPolicy::RequireSealed { pin: None }) {
                Ok(verified) => {
                    let chunks = verified
                        .coverage()
                        .and_then(|c| c.chunk_index())
                        .map(|index| {
                            let mut bundles = index.bundles.clone();
                            bundles.sort_by_key(|b| b.seq);
                            bundles.into_iter().map(|b| b.blake3_hex).collect()
                        });
                    (chunks, None)
                }
                Err(error) => (None, Some(error)),
            };
        let resolved = Arc::new(Resolved {
            children,
            header,
            signed_uri,
            header_error,
            signed_chunks,
        });
        let mut cache = lock(&self.resolved);
        if cache.len() >= MAX_RESOLVED_ROOTS {
            cache.clear();
        }
        cache.insert(entry.root, Arc::clone(&resolved));
        Ok(resolved)
    }

    /// Refuse a root whose signed header names a hexon other than `uri`.
    fn bound(resolved: &Resolved, uri: &str) -> Result<(), PartialFetchError> {
        let signed = resolved.signed_uri.as_deref().unwrap_or("");
        let (id, version) = match uri.split_once('@') {
            Some((id, version)) => (id, Some(version)),
            None => (uri, None),
        };
        let matches = signed
            .split_once('@')
            .is_some_and(|(signed_id, signed_version)| {
                signed_id == id && version.is_none_or(|v| v == signed_version)
            });
        if matches {
            Ok(())
        } else {
            Err(PartialFetchError::Substituted {
                requested: uri.to_string(),
                signed: signed.to_string(),
            })
        }
    }

    /// Child `seq + 1`, cross-checked against the signed chunk index first.
    async fn chunk_of(&self, resolved: &Resolved, seq: u32) -> Result<Bytes, PartialFetchError> {
        if let Some(error) = &resolved.header_error {
            return Err(error.clone().into());
        }
        let Some(child) = resolved.children.get(seq as usize + 1).copied() else {
            return Err(PartialFetchError::OutOfRange);
        };
        let path = chunk_member_path(seq);
        match &resolved.signed_chunks {
            Some(signed) => {
                if signed.get(seq as usize).map(String::as_str)
                    != Some(hash_to_hex(&child).as_str())
                {
                    return Err(IntegrityError::DigestMismatch { path }.into());
                }
                self.blob(child, MAX_CHUNK_BYTES, &path).await
            }
            // Monolithic: child 1 is the whole archive (the strategy verifies it).
            None if seq == 0 && resolved.children.len() == 2 => {
                self.blob(child, MAX_WHOLE_ARCHIVE_BYTES, "whole archive")
                    .await
            }
            None => Err(PartialFetchError::OutOfRange),
        }
    }
}

/// One member of an in-memory archive, under the per-entry caps; `None` when absent.
fn read_archive_member(archive: &[u8], path: &str) -> Result<Option<Vec<u8>>, PartialFetchError> {
    let malformed = |e: &dyn std::fmt::Display| IntegrityError::Malformed(format!("{e:#}"));
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| malformed(&e))?;
    let entry = match zip.by_name(path) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(malformed(&e).into()),
    };
    let cap = if path.ends_with(".json") {
        limits::MAX_JSON_ENTRY_BYTES
    } else {
        limits::MAX_ENTRY_UNCOMPRESSED_BYTES
    };
    let mut budget = cap;
    let declared = entry.size();
    fe_format::read_entry_capped(entry, declared, cap, &mut budget)
        .map(Some)
        .map_err(|e| malformed(&e).into())
}

#[async_trait]
impl PartialHexonFetch for P2pPartialFetch {
    fn source(&self) -> FetchSource {
        FetchSource::Peer {
            priority: self.priority,
            peer_ref: self.peer.clone(),
        }
    }

    async fn fetch_header(&self, uri: &str) -> Result<HeaderDoc, PartialFetchError> {
        Ok(self.resolve(uri).await?.header.clone())
    }

    async fn fetch_chunk(&self, uri: &str, seq: u32) -> Result<Bytes, PartialFetchError> {
        let resolved = self.resolve(uri).await?;
        Self::bound(&resolved, uri)?;
        self.chunk_of(&resolved, seq).await
    }

    async fn fetch_member(&self, uri: &str, path: &str) -> Result<Bytes, PartialFetchError> {
        let resolved = self.resolve(uri).await?;
        Self::bound(&resolved, uri)?;
        if path == MANIFEST_PATH {
            return Ok(Bytes::from(resolved.header.manifest.clone()));
        }
        if let Some(text) = resolved.header.header_members.get(path) {
            return Ok(Bytes::from(text.clone()));
        }
        if resolved.signed_chunks.is_some() {
            // A container's only body members are its chunks.
            let seq = (0..resolved.children.len() as u32 - 1)
                .find(|seq| chunk_member_path(*seq) == path)
                .ok_or(PartialFetchError::NotFound)?;
            return self.chunk_of(&resolved, seq).await;
        }
        let archive = self.chunk_of(&resolved, 0).await?;
        let path = path.to_string();
        let member = tokio::task::spawn_blocking(move || read_archive_member(&archive, &path))
            .await
            .map_err(|e| PartialFetchError::Transport(format!("member read task: {e}")))??;
        member.map(Bytes::from).ok_or(PartialFetchError::NotFound)
    }

    async fn fetch_blob(&self, hash: BlobHash) -> Result<Bytes, PartialFetchError> {
        self.blob(hash, MAX_CHUNK_BYTES, &hash_to_hex(&hash)).await
    }
}
