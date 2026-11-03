//! [`IrohBlobTransport`]: the iroh-blobs 0.103 implementation of
//! `fe_runtime::blob_transport::BlobTransport` — see `src/AGENTS.md` §tileset-transfer.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use bytes::Bytes;
use fe_runtime::blob_store::hash_to_hex;
use fe_runtime::blob_transport::{
    ensure_within_cap, hash_seq_byte_cap, BlobTransport, Hash32, ProviderRef,
};
use futures_lite::StreamExt as _;
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, RelayUrl, TransportAddr};
use iroh_blobs::api::proto::BlobStatus;
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::hashseq::HashSeq;
use iroh_blobs::{Hash, HashAndFormat};

/// Bound on dialing a provider.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound on one blob / HashSeq-root fetch (a 1 MiB bundle needs far less).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Bound on every local store request.
pub const LOCAL_STORE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest accepted wire-supplied provider address (`EndpointAddr` JSON).
pub const MAX_PROVIDER_ADDR_LEN: usize = 4 * 1024;

/// A wire-supplied provider address reduced to what this node may dial: the
/// endpoint id, its IP addresses, and only relay URLs this node already
/// trusts — never an arbitrary URL (S8b; `src/AGENTS.md` §tileset-security).
pub fn sanitize_provider_addr(
    addr: EndpointAddr,
    allowed_relays: &BTreeSet<RelayUrl>,
) -> EndpointAddr {
    let addrs = addr
        .addrs
        .into_iter()
        .filter(|transport| match transport {
            TransportAddr::Ip(_) => true,
            TransportAddr::Relay(url) => allowed_relays.contains(url),
            _ => false,
        })
        .collect();
    EndpointAddr { id: addr.id, addrs }
}

/// Lock a mutex, recovering the data from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `future` under `limit`, naming `what` in the timeout error.
async fn bounded<T, F>(limit: Duration, what: &str, future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| anyhow!("{what} timed out after {limit:?}"))?
}

/// [`BlobTransport`] over an iroh-blobs store and the sync endpoint.
pub struct IrohBlobTransport {
    store: Store,
    endpoint: iroh::Endpoint,
    /// One cached blobs-ALPN connection per provider.
    connections: Mutex<HashMap<iroh::EndpointId, Connection>>,
    /// Relay URLs a wire-supplied provider address may name (this node's relay map).
    allowed_relays: BTreeSet<RelayUrl>,
    /// Temp tags holding freshly added blobs until a named tag covers them.
    pending: Mutex<HashMap<Hash32, TempTag>>,
}

impl IrohBlobTransport {
    /// Transport over `store`, dialing providers through `endpoint`.
    pub fn new(store: Store, endpoint: iroh::Endpoint) -> Self {
        Self {
            store,
            endpoint,
            connections: Mutex::new(HashMap::new()),
            allowed_relays: BTreeSet::new(),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Allow wire-supplied provider addresses to name these relays (the node's own relay map).
    pub fn with_allowed_relays(mut self, relays: impl IntoIterator<Item = RelayUrl>) -> Self {
        self.allowed_relays = relays.into_iter().collect();
        self
    }

    /// Transport over a running [`crate::DocsStack`]'s blob store and endpoint.
    pub fn from_stack(stack: &crate::DocsStack) -> Self {
        Self::new(stack.blob_store().clone(), stack.endpoint().clone())
    }

    /// The [`ProviderRef`] for `addr` (its `EndpointAddr` JSON).
    pub fn provider_ref(addr: &iroh::EndpointAddr) -> Result<ProviderRef> {
        Ok(ProviderRef::new(
            serde_json::to_string(addr).context("encoding EndpointAddr")?,
        ))
    }

    /// Parse a wire-supplied provider address (only a bounded `EndpointAddr` is accepted).
    pub fn parse_provider(provider: &ProviderRef) -> Result<iroh::EndpointAddr> {
        if provider.as_str().len() > MAX_PROVIDER_ADDR_LEN {
            return Err(anyhow!(
                "provider address is over {MAX_PROVIDER_ADDR_LEN} bytes"
            ));
        }
        serde_json::from_str(provider.as_str())
            .map_err(|e| anyhow!("provider is not EndpointAddr JSON: {e}"))
    }

    /// A live blobs-ALPN connection to `provider` (cached, dialed under [`CONNECT_TIMEOUT`]).
    async fn connection(&self, provider: &ProviderRef) -> Result<Connection> {
        let addr = sanitize_provider_addr(Self::parse_provider(provider)?, &self.allowed_relays);
        let id = addr.id;
        if let Some(conn) = lock(&self.connections).get(&id) {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }
        let endpoint = self.endpoint.clone();
        let conn = bounded(CONNECT_TIMEOUT, "dialing blob provider", async move {
            endpoint
                .connect(addr, iroh_blobs::ALPN)
                .await
                .map_err(|e| anyhow!("dialing blob provider: {e}"))
        })
        .await?;
        lock(&self.connections).insert(id, conn.clone());
        Ok(conn)
    }

    /// Require a live provider's authenticated blob policy even when a native cache hit exists.
    pub(crate) async fn verify_provider_size(&self,provider:&ProviderRef,hash:Hash32,max_bytes:u64)->Result<u64> {
        let conn=self.connection(provider).await?;
        bounded(FETCH_TIMEOUT,"admitting scene blob provider",async {
            let (size,_)=iroh_blobs::get::request::get_verified_size(&conn,&Hash::from(hash)).await
                .map_err(|error|anyhow!("scene blob provider refused size: {error}"))?;
            ensure_within_cap(&hash,size,max_bytes)?;Ok(size)
        }).await
    }

    pub(crate) fn discard_temporary(&self,hashes:&[Hash32]) {
        let mut pending=lock(&self.pending);for hash in hashes {pending.remove(hash);}
    }

    /// Fetch `hash` (raw) from `provider` into the local store; local state is honoured.
    ///
    /// Asks for the blob's BAO-verified size first, so a blob over `max_bytes`
    /// is refused before any of its content is downloaded.
    async fn fetch_into_store(
        &self,
        provider: &ProviderRef,
        hash: Hash32,
        max_bytes: u64,
    ) -> Result<Bytes> {
        if let Some(size) = self.local_size(hash).await? {
            ensure_within_cap(&hash, size, max_bytes)?;
            if let Some(bytes) = self.read_local(hash).await? {
                return Ok(bytes);
            }
        }
        let conn = self.connection(provider).await?;
        let content = HashAndFormat::raw(Hash::from(hash));
        let fetched = bounded(FETCH_TIMEOUT, "fetching blob", async {
            let (size, _) = iroh_blobs::get::request::get_verified_size(&conn, &Hash::from(hash))
                .await
                .map_err(|e| anyhow!("asking the blob size: {e}"))?;
            ensure_within_cap(&hash, size, max_bytes)?;
            self.store
                .remote()
                .fetch(conn, content)
                .complete()
                .await
                .map_err(|e| anyhow!("fetching blob: {e}"))
        })
        .await;
        if let Err(e) = fetched {
            if let Ok(addr) = Self::parse_provider(provider) {
                lock(&self.connections).remove(&addr.id);
            }
            return Err(e.context(format!("blob {}", hash_to_hex(&hash))));
        }
        self.read_local(hash)
            .await?
            .ok_or_else(|| anyhow!("blob {} incomplete after fetch", hash_to_hex(&hash)))
    }

    /// Hold a temp tag for `tag` until a named tag covers it.
    fn hold(&self, tag: TempTag) -> Hash32 {
        let hash: Hash32 = tag.hash().into();
        lock(&self.pending).insert(hash, tag);
        hash
    }
}

#[async_trait]
impl BlobTransport for IrohBlobTransport {
    async fn add(&self, bytes: Bytes) -> Result<Hash32> {
        let tag = bounded(LOCAL_STORE_TIMEOUT, "adding blob", async {
            self.store
                .blobs()
                .add_bytes(bytes)
                .temp_tag()
                .await
                .map_err(|e| anyhow!("adding blob: {e}"))
        })
        .await?;
        Ok(self.hold(tag))
    }

    async fn add_hash_seq(&self, children: &[Hash32]) -> Result<Hash32> {
        let seq: HashSeq = children.iter().map(|child| Hash::from(*child)).collect();
        let bytes: Bytes = seq.into();
        self.add(bytes).await
    }

    async fn set_tag(&self, name: &str, root: Hash32) -> Result<()> {
        let value = HashAndFormat::hash_seq(Hash::from(root));
        bounded(LOCAL_STORE_TIMEOUT, "setting tag", async {
            self.store
                .tags()
                .set(name, value)
                .await
                .map_err(|e| anyhow!("setting tag: {e}"))
        })
        .await?;
        // The named HashSeq tag now protects root + children: release their temp tags.
        let mut covered = vec![root];
        if let Some(bytes) = self.read_local(root).await? {
            if let Some(children) = fe_runtime::blob_transport::decode_hash_seq(&bytes) {
                covered.extend(children);
            }
        }
        let mut pending = lock(&self.pending);
        for hash in covered {
            pending.remove(&hash);
        }
        Ok(())
    }

    async fn delete_tag(&self, name: &str) -> Result<()> {
        bounded(LOCAL_STORE_TIMEOUT, "deleting tag", async {
            self.store
                .tags()
                .delete(name)
                .await
                .map(|_| ())
                .map_err(|e| anyhow!("deleting tag: {e}"))
        })
        .await
    }

    async fn list_tags(&self, prefix: &str) -> Result<Vec<(String, Hash32)>> {
        bounded(LOCAL_STORE_TIMEOUT, "listing tags", async {
            let stream = self
                .store
                .tags()
                .list_prefix(prefix)
                .await
                .map_err(|e| anyhow!("listing tags: {e}"))?;
            let mut stream = std::pin::pin!(stream);
            let mut out = Vec::new();
            while let Some(item) = stream.next().await {
                let info = item.map_err(|e| anyhow!("listing tags: {e}"))?;
                match String::from_utf8(info.name.0.to_vec()) {
                    Ok(name) => out.push((name, info.hash.into())),
                    Err(_) => tracing::warn!("skipping a non-UTF-8 blob tag"),
                }
            }
            Ok(out)
        })
        .await
    }

    async fn fetch_hash_seq(
        &self,
        provider: &ProviderRef,
        root: Hash32,
        max_children: usize,
    ) -> Result<Vec<Hash32>> {
        let bytes = self
            .fetch_into_store(provider, root, hash_seq_byte_cap(max_children))
            .await?;
        let seq = HashSeq::new(bytes)
            .ok_or_else(|| anyhow!("blob {} is not a HashSeq", hash_to_hex(&root)))?;
        Ok(seq.iter().map(Hash32::from).collect())
    }

    async fn fetch(&self, provider: &ProviderRef, hash: Hash32, max_bytes: u64) -> Result<Bytes> {
        self.fetch_into_store(provider, hash, max_bytes).await
    }

    async fn read_local(&self, hash: Hash32) -> Result<Option<Bytes>> {
        if self.local_size(hash).await?.is_none() {
            return Ok(None);
        }
        bounded(LOCAL_STORE_TIMEOUT, "reading blob", async {
            self.store
                .blobs()
                .get_bytes(Hash::from(hash))
                .await
                .map(Some)
                .map_err(|e| anyhow!("reading blob: {e}"))
        })
        .await
    }

    async fn local_size(&self, hash: Hash32) -> Result<Option<u64>> {
        let status = bounded(LOCAL_STORE_TIMEOUT, "blob status", async {
            self.store
                .blobs()
                .status(Hash::from(hash))
                .await
                .map_err(|e| anyhow!("blob status: {e}"))
        })
        .await?;
        Ok(match status {
            BlobStatus::Complete { size } => Some(size),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S8b: a wire-supplied provider keeps its id and IP addresses but only
    /// relay URLs this node already trusts, so a forged ad can never make
    /// this node dial an arbitrary URL.
    #[test]
    fn a_provider_address_keeps_only_trusted_relays() {
        let id = iroh::SecretKey::from_bytes(&[11u8; 32]).public();
        let trusted: RelayUrl = "https://relay.trusted.example".parse().unwrap();
        let foreign: RelayUrl = "https://169.254.169.254".parse().unwrap();
        let ip: std::net::SocketAddr = "192.0.2.7:4433".parse().unwrap();
        let addr = EndpointAddr {
            id,
            addrs: [
                TransportAddr::Relay(trusted.clone()),
                TransportAddr::Relay(foreign),
                TransportAddr::Ip(ip),
            ]
            .into_iter()
            .collect(),
        };
        let allowed: BTreeSet<RelayUrl> = [trusted.clone()].into_iter().collect();
        let sanitized = sanitize_provider_addr(addr.clone(), &allowed);
        assert_eq!(sanitized.id, id);
        let kept: Vec<_> = sanitized.addrs.into_iter().collect();
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.contains(&TransportAddr::Relay(trusted)));
        assert!(kept.contains(&TransportAddr::Ip(ip)));

        let none_allowed = sanitize_provider_addr(addr, &BTreeSet::new());
        assert_eq!(none_allowed.addrs.len(), 1);
        assert!(none_allowed
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Ip(_))));
    }

    #[test]
    fn an_oversized_provider_address_is_refused_before_parsing() {
        let huge = ProviderRef::new("x".repeat(MAX_PROVIDER_ADDR_LEN + 1));
        assert!(IrohBlobTransport::parse_provider(&huge).is_err());
    }
}
