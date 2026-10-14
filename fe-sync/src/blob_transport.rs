//! [`IrohBlobTransport`]: the iroh-blobs 0.103 implementation of
//! `fe_runtime::blob_transport::BlobTransport` — see `src/AGENTS.md` §tileset-transfer.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use bytes::Bytes;
use fe_runtime::blob_store::hash_to_hex;
use fe_runtime::blob_transport::{BlobTransport, Hash32, ProviderRef};
use futures_lite::StreamExt as _;
use iroh::endpoint::Connection;
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
            pending: Mutex::new(HashMap::new()),
        }
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

    /// Parse a wire-supplied provider address (only an `EndpointAddr` is accepted).
    pub fn parse_provider(provider: &ProviderRef) -> Result<iroh::EndpointAddr> {
        serde_json::from_str(provider.as_str())
            .map_err(|e| anyhow!("provider is not EndpointAddr JSON: {e}"))
    }

    /// A live blobs-ALPN connection to `provider` (cached, dialed under [`CONNECT_TIMEOUT`]).
    async fn connection(&self, provider: &ProviderRef) -> Result<Connection> {
        let addr = Self::parse_provider(provider)?;
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

    /// Fetch `hash` (raw) from `provider` into the local store; local state is honoured.
    async fn fetch_into_store(&self, provider: &ProviderRef, hash: Hash32) -> Result<Bytes> {
        if let Some(bytes) = self.read_local(hash).await? {
            return Ok(bytes);
        }
        let conn = self.connection(provider).await?;
        let content = HashAndFormat::raw(Hash::from(hash));
        let fetched = bounded(FETCH_TIMEOUT, "fetching blob", async {
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

    async fn fetch_hash_seq(&self, provider: &ProviderRef, root: Hash32) -> Result<Vec<Hash32>> {
        let bytes = self.fetch_into_store(provider, root).await?;
        let seq = HashSeq::new(bytes)
            .ok_or_else(|| anyhow!("blob {} is not a HashSeq", hash_to_hex(&root)))?;
        Ok(seq.iter().map(Hash32::from).collect())
    }

    async fn fetch(&self, provider: &ProviderRef, hash: Hash32) -> Result<Bytes> {
        self.fetch_into_store(provider, hash).await
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
