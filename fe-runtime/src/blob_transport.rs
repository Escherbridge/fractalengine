//! Transport-agnostic content-addressed blob transfer (hash + HashSeq only).
//!
//! See `fe-runtime/src/AGENTS.md` §blob-transport.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::blob_store::{hash_to_hex, BlobHash};

/// A 32-byte BLAKE3 content hash (the iroh-blobs `Hash` byte layout).
pub type Hash32 = BlobHash;

/// Opaque provider address: an iroh `EndpointAddr` JSON string on the real
/// transport (the `SyncEvent::Started.node_addr` format), a peer name in memory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderRef(pub String);

impl ProviderRef {
    /// Wrap an opaque provider address string.
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    /// The opaque address string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// BLAKE3 hash of `bytes` as a [`Hash32`] (the identity every transport uses).
pub fn content_hash(bytes: &[u8]) -> Hash32 {
    *blake3::hash(bytes).as_bytes()
}

/// Encode `children` as a HashSeq blob (concatenated 32-byte hashes).
pub fn encode_hash_seq(children: &[Hash32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(children.len() * 32);
    for child in children {
        out.extend_from_slice(child);
    }
    out
}

/// Root hash of the HashSeq blob holding `children`.
pub fn hash_seq_root(children: &[Hash32]) -> Hash32 {
    content_hash(&encode_hash_seq(children))
}

/// The byte cap for a HashSeq root blob of at most `max_children` children.
pub fn hash_seq_byte_cap(max_children: usize) -> u64 {
    (max_children as u64).saturating_mul(32)
}

/// Refuse a blob of `len` bytes past `max_bytes` (the shared transport size guard).
pub fn ensure_within_cap(hash: &Hash32, len: u64, max_bytes: u64) -> Result<()> {
    if len > max_bytes {
        bail!(
            "blob {} is {len} bytes, over the {max_bytes}-byte cap",
            hash_to_hex(hash)
        );
    }
    Ok(())
}

/// Decode a HashSeq blob; `None` when its length is not a multiple of 32.
pub fn decode_hash_seq(bytes: &[u8]) -> Option<Vec<Hash32>> {
    if !bytes.len().is_multiple_of(32) {
        return None;
    }
    Some(bytes.as_chunks::<32>().0.to_vec())
}

/// Content-addressed blob transfer with HashSeq grouping and named tags.
///
/// No iroh type appears here (decision D-77). Callers re-hash whatever they
/// persist; no caller relies on a transport's own verification.
#[async_trait]
pub trait BlobTransport: Send + Sync {
    /// Store `bytes` locally; returns its BLAKE3 hash.
    async fn add(&self, bytes: Bytes) -> Result<Hash32>;
    /// Store a HashSeq blob over `children` locally; returns its root hash.
    async fn add_hash_seq(&self, children: &[Hash32]) -> Result<Hash32>;
    /// Point the named tag at HashSeq `root` (protecting root and children).
    async fn set_tag(&self, name: &str, root: Hash32) -> Result<()>;
    /// Remove the named tag (absent tags are not an error).
    async fn delete_tag(&self, name: &str) -> Result<()>;
    /// All tags whose name starts with `prefix`, as `(name, root)`.
    async fn list_tags(&self, prefix: &str) -> Result<Vec<(String, Hash32)>>;
    /// Fetch only the HashSeq root blob from `provider` (refused past `max_children`); returns its children.
    async fn fetch_hash_seq(
        &self,
        provider: &ProviderRef,
        root: Hash32,
        max_children: usize,
    ) -> Result<Vec<Hash32>>;
    /// Fetch one blob from `provider` (local copy first), refused past `max_bytes`
    /// before it is downloaded wherever the transport can learn the size first.
    async fn fetch(&self, provider: &ProviderRef, hash: Hash32, max_bytes: u64) -> Result<Bytes>;
    /// The complete local copy of `hash`, if any.
    async fn read_local(&self, hash: Hash32) -> Result<Option<Bytes>>;
    /// Size of the complete local copy of `hash`, if any.
    async fn local_size(&self, hash: Hash32) -> Result<Option<u64>>;
}

/// Lock a mutex, recovering the data from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One simulated peer's local store plus its test-hook state.
#[derive(Default)]
struct InMemoryPeer {
    blobs: Mutex<HashMap<Hash32, Bytes>>,
    tags: Mutex<BTreeMap<String, Hash32>>,
    tampered: Mutex<HashMap<Hash32, Bytes>>,
    withheld: Mutex<HashSet<Hash32>>,
    offline: AtomicBool,
}

impl InMemoryPeer {
    /// What this peer serves to a remote fetcher for `hash` (hooks applied).
    fn serve(&self, name: &str, hash: &Hash32) -> Result<Bytes> {
        if self.offline.load(Ordering::SeqCst) {
            bail!("provider {name} is offline");
        }
        if lock(&self.withheld).contains(hash) {
            bail!("provider {name} does not have blob {}", hash_to_hex(hash));
        }
        if let Some(bytes) = lock(&self.tampered).get(hash) {
            return Ok(bytes.clone());
        }
        lock(&self.blobs)
            .get(hash)
            .cloned()
            .ok_or_else(|| anyhow!("provider {name} does not have blob {}", hash_to_hex(hash)))
    }
}

/// A process-local "swarm" of [`InMemoryTransport`] peers (tests, harness, sim).
#[derive(Default)]
pub struct InMemoryNetwork {
    peers: Mutex<HashMap<String, Arc<InMemoryPeer>>>,
}

impl InMemoryNetwork {
    /// A new, empty network.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The transport for peer `name`; the same name always reuses one store.
    pub fn transport(self: &Arc<Self>, name: &str) -> InMemoryTransport {
        let peer = lock(&self.peers)
            .entry(name.to_string())
            .or_default()
            .clone();
        InMemoryTransport {
            network: Arc::clone(self),
            name: name.to_string(),
            peer,
        }
    }

    fn peer(&self, provider: &ProviderRef) -> Result<Arc<InMemoryPeer>> {
        lock(&self.peers)
            .get(provider.as_str())
            .cloned()
            .ok_or_else(|| anyhow!("unknown provider {}", provider.as_str()))
    }
}

/// A [`BlobTransport`] over an [`InMemoryNetwork`], with tamper/withhold hooks.
#[derive(Clone)]
pub struct InMemoryTransport {
    network: Arc<InMemoryNetwork>,
    name: String,
    peer: Arc<InMemoryPeer>,
}

impl InMemoryTransport {
    /// This peer's provider address (its name).
    pub fn provider_ref(&self) -> ProviderRef {
        ProviderRef::new(self.name.clone())
    }

    /// Serve `bytes` instead of the real content of `hash` to remote fetchers.
    pub fn tamper(&self, hash: Hash32, bytes: impl Into<Bytes>) {
        lock(&self.peer.tampered).insert(hash, bytes.into());
    }

    /// Refuse to serve `hash` to remote fetchers.
    pub fn withhold(&self, hash: Hash32) {
        lock(&self.peer.withheld).insert(hash);
    }

    /// Undo [`Self::tamper`] / [`Self::withhold`] for `hash`.
    pub fn repair(&self, hash: Hash32) {
        lock(&self.peer.tampered).remove(&hash);
        lock(&self.peer.withheld).remove(&hash);
    }

    /// Simulate the peer going offline (`false`) or coming back (`true`).
    pub fn set_online(&self, online: bool) {
        self.peer.offline.store(!online, Ordering::SeqCst);
    }

    /// Whether this peer's local store holds `hash`.
    pub fn has_local(&self, hash: &Hash32) -> bool {
        lock(&self.peer.blobs).contains_key(hash)
    }

    fn insert_local(&self, bytes: Bytes) -> Hash32 {
        let hash = content_hash(&bytes);
        lock(&self.peer.blobs).insert(hash, bytes);
        hash
    }

    fn local(&self, hash: &Hash32) -> Option<Bytes> {
        lock(&self.peer.blobs).get(hash).cloned()
    }

    /// Remote fetch: the provider's served bytes (refused past `max_bytes`), cached locally only when they verify.
    fn fetch_remote(&self, provider: &ProviderRef, hash: Hash32, max_bytes: u64) -> Result<Bytes> {
        if provider.as_str() == self.name {
            bail!("cannot fetch from self ({})", self.name);
        }
        let bytes = self
            .network
            .peer(provider)?
            .serve(provider.as_str(), &hash)?;
        ensure_within_cap(&hash, bytes.len() as u64, max_bytes)?;
        if content_hash(&bytes) == hash {
            lock(&self.peer.blobs).insert(hash, bytes.clone());
        }
        Ok(bytes)
    }
}

#[async_trait]
impl BlobTransport for InMemoryTransport {
    async fn add(&self, bytes: Bytes) -> Result<Hash32> {
        Ok(self.insert_local(bytes))
    }

    async fn add_hash_seq(&self, children: &[Hash32]) -> Result<Hash32> {
        Ok(self.insert_local(Bytes::from(encode_hash_seq(children))))
    }

    async fn set_tag(&self, name: &str, root: Hash32) -> Result<()> {
        lock(&self.peer.tags).insert(name.to_string(), root);
        Ok(())
    }

    async fn delete_tag(&self, name: &str) -> Result<()> {
        lock(&self.peer.tags).remove(name);
        Ok(())
    }

    async fn list_tags(&self, prefix: &str) -> Result<Vec<(String, Hash32)>> {
        Ok(lock(&self.peer.tags)
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, root)| (name.clone(), *root))
            .collect())
    }

    async fn fetch_hash_seq(
        &self,
        provider: &ProviderRef,
        root: Hash32,
        max_children: usize,
    ) -> Result<Vec<Hash32>> {
        let bytes = self
            .fetch(provider, root, hash_seq_byte_cap(max_children))
            .await?;
        decode_hash_seq(&bytes)
            .ok_or_else(|| anyhow!("blob {} is not a HashSeq", hash_to_hex(&root)))
    }

    async fn fetch(&self, provider: &ProviderRef, hash: Hash32, max_bytes: u64) -> Result<Bytes> {
        match self.local(&hash) {
            Some(bytes) => {
                ensure_within_cap(&hash, bytes.len() as u64, max_bytes)?;
                Ok(bytes)
            }
            None => self.fetch_remote(provider, hash, max_bytes),
        }
    }

    async fn read_local(&self, hash: Hash32) -> Result<Option<Bytes>> {
        Ok(self.local(&hash))
    }

    async fn local_size(&self, hash: Hash32) -> Result<Option<u64>> {
        Ok(self.local(&hash).map(|bytes| bytes.len() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_seq_codec_round_trips_and_rejects_ragged_blobs() {
        let children = [content_hash(b"a"), content_hash(b"b"), content_hash(b"c")];
        let encoded = encode_hash_seq(&children);
        assert_eq!(decode_hash_seq(&encoded).unwrap(), children.to_vec());
        assert_eq!(hash_seq_root(&children), content_hash(&encoded));
        assert!(decode_hash_seq(&encoded[..40]).is_none());
        assert_eq!(decode_hash_seq(&[]).unwrap(), Vec::<Hash32>::new());
    }

    /// A generous per-fetch cap for tests that are not about size limits.
    const CAP: u64 = 1 << 20;

    #[tokio::test]
    async fn a_peer_fetches_what_another_published_and_hooks_change_what_is_served() {
        let network = InMemoryNetwork::new();
        let alice = network.transport("alice");
        let bob = network.transport("bob");
        let hash = alice.add(Bytes::from_static(b"tile bytes")).await.unwrap();
        let root = alice.add_hash_seq(&[hash]).await.unwrap();

        assert_eq!(
            bob.fetch_hash_seq(&alice.provider_ref(), root, 8)
                .await
                .unwrap(),
            vec![hash]
        );
        alice.tamper(content_hash(b"other"), Bytes::from_static(b"forged"));
        alice.withhold(content_hash(b"missing"));
        assert_eq!(
            bob.fetch(&alice.provider_ref(), hash, CAP).await.unwrap(),
            Bytes::from_static(b"tile bytes")
        );
        assert!(
            bob.has_local(&hash),
            "verified fetches land in the local store"
        );

        let carol = network.transport("carol");
        alice.tamper(hash, Bytes::from_static(b"forged"));
        let forged = carol.fetch(&alice.provider_ref(), hash, CAP).await.unwrap();
        assert_eq!(forged, Bytes::from_static(b"forged"));
        assert!(
            !carol.has_local(&hash),
            "unverified bytes never enter the store"
        );

        alice.withhold(hash);
        assert!(carol.fetch(&alice.provider_ref(), hash, CAP).await.is_err());
        alice.repair(hash);
        alice.set_online(false);
        assert!(carol.fetch(&alice.provider_ref(), hash, CAP).await.is_err());
        alice.set_online(true);
        assert_eq!(
            carol.fetch(&alice.provider_ref(), hash, CAP).await.unwrap(),
            Bytes::from_static(b"tile bytes")
        );
    }

    /// S8b: a blob over the caller's cap is refused and never lands in the local store.
    #[tokio::test]
    async fn an_oversized_blob_is_refused_and_never_stored() {
        let network = InMemoryNetwork::new();
        let alice = network.transport("alice");
        let bob = network.transport("bob");
        let big = alice.add(Bytes::from(vec![7u8; 64])).await.unwrap();
        let err = bob
            .fetch(&alice.provider_ref(), big, 63)
            .await
            .expect_err("a 64-byte blob over a 63-byte cap is refused");
        assert!(err.to_string().contains("cap"), "{err:#}");
        assert!(!bob.has_local(&big), "refused bytes never enter the store");

        let children: Vec<Hash32> = (0..3u8).map(|i| [i; 32]).collect();
        let root = alice.add_hash_seq(&children).await.unwrap();
        assert!(
            bob.fetch_hash_seq(&alice.provider_ref(), root, 2)
                .await
                .is_err(),
            "a HashSeq with more children than allowed is refused"
        );
        assert_eq!(
            bob.fetch_hash_seq(&alice.provider_ref(), root, 3)
                .await
                .unwrap(),
            children
        );
    }

    #[tokio::test]
    async fn tags_list_by_prefix_and_delete() {
        let network = InMemoryNetwork::new();
        let peer = network.transport("p");
        peer.set_tag("fe/tileset/a", [1; 32]).await.unwrap();
        peer.set_tag("fe/tileset/b", [2; 32]).await.unwrap();
        peer.set_tag("fe/download/a", [3; 32]).await.unwrap();
        let listed = peer.list_tags("fe/tileset/").await.unwrap();
        assert_eq!(
            listed,
            vec![
                ("fe/tileset/a".to_string(), [1; 32]),
                ("fe/tileset/b".to_string(), [2; 32])
            ]
        );
        peer.delete_tag("fe/tileset/a").await.unwrap();
        peer.delete_tag("never-existed").await.unwrap();
        assert_eq!(peer.list_tags("fe/tileset/").await.unwrap().len(), 1);
    }
}
