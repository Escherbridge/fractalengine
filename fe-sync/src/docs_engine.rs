//! Real iroh 1.x P2P stack (iroh-docs 0.101 / iroh-blobs 0.103 /
//! iroh-gossip 0.101): Blobs (fs) + Gossip + Docs (persistent redb) + a
//! Router accepting all three ALPNs (A1, P2P Mycelium Phase F).
//!
//! [`DocsStack`] owns every long-lived protocol actor for one online sync
//! thread so that spawning and shutting them down is a single operation.
//! It is built over an **already-bound** endpoint; when the endpoint fails
//! to bind (or the stack cannot spawn) the sync thread runs without a
//! stack and the replicator layer degrades to the in-memory mock — never
//! a crash. See `fe-sync/src/AGENTS.md` §iroh-1.x and §store-migration.
//!
//! Persistent layout under the data dir ([`p2p_data_dir`], env
//! [`P2P_DIR_ENV_VAR`], default `data/p2p`):
//!
//! ```text
//! <dir>/blobs/             — iroh-blobs fs store (blobs.db + data/ outboards)
//! <dir>/docs.redb          — iroh-docs replica store
//! <dir>/default-author     — hex-encoded default author key (the endpoint identity)
//! <dir>/.fe-store-format   — store format marker (see p2p_store_migration.rs)
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use iroh::protocol::Router;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::BlobsProtocol;
use iroh_docs::protocol::Docs;
use iroh_gossip::Gossip;

/// Env var selecting the persistent P2P data dir.
pub const P2P_DIR_ENV_VAR: &str = "FE_P2P_DIR";

/// Default persistent P2P data dir, relative to the process working dir.
pub const DEFAULT_P2P_DIR: &str = "data/p2p";

/// Resolve the P2P data dir from [`P2P_DIR_ENV_VAR`] (default `data/p2p`).
///
/// Blank/whitespace values fall back to the default; the value is used
/// verbatim otherwise.
pub fn p2p_data_dir() -> PathBuf {
    resolve_p2p_dir(std::env::var(P2P_DIR_ENV_VAR).ok().as_deref())
}

/// Pure core of [`p2p_data_dir`], unit-testable without touching process env.
pub fn resolve_p2p_dir(env_value: Option<&str>) -> PathBuf {
    match env_value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(DEFAULT_P2P_DIR),
    }
}

/// The full P2P protocol stack owned by an online sync thread.
///
/// The [`Router`] accepts all three ALPNs so inbound peer connections are
/// routable: blobs (`/iroh-bytes/4`), gossip (`/iroh-gossip/1`), docs
/// (`/iroh-sync/1`). Before the router existed inbound protocol connections
/// were silently unroutable (see AGENTS.md §iroh-1.x).
pub struct DocsStack {
    endpoint: iroh::Endpoint,
    blob_store: FsStore,
    blobs: BlobsProtocol,
    gossip: Gossip,
    docs: Docs,
    router: Router,
    data_dir: PathBuf,
}

impl DocsStack {
    /// Spawn the full stack over an already-bound endpoint.
    ///
    /// `data_dir` is created if missing; every persistent store lives under
    /// it. A legacy iroh 0.35 store is migrated first (backup copy + redb
    /// file-format upgrade — AGENTS.md §store-migration). Any failure returns
    /// an error — the caller degrades to offline/mock mode rather than
    /// panicking. Directory creation and the migration run on the blocking
    /// pool (see AGENTS.md §sync-thread-blocking-io).
    ///
    /// **One authenticated author identity (F20/M1 finding 2):** the
    /// endpoint's identity key is imported as the docs author and made the
    /// node-wide default, so every write the replicator layer publishes is
    /// authored by the endpoint identity and `entry.author()` maps to the
    /// same `did:key` the A3 inbound gate resolves (endpoint `EndpointId`
    /// did:key == the app DID — fe-identity's keypair and the iroh secret
    /// share the ed25519 seed). Left un-imported, the docs default author
    /// would be an unrelated per-`FE_P2P_DIR` key that maps to no role
    /// anywhere, and reconciliation replays would re-attribute rows to that
    /// role-less identity (denied outside the bootstrap window).
    pub async fn spawn(endpoint: iroh::Endpoint, data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        // redb and the docs author storage need the parent dir to exist;
        // the blobs fs store creates its own subtree. A 0.35-written store is
        // upgraded BEFORE any 1.x store opens it (redb 4 refuses v2 files).
        let dir = data_dir.clone();
        let migration = tokio::task::spawn_blocking(move || -> Result<_> {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating P2P data dir {}", dir.display()))?;
            crate::p2p_store_migration::migrate_legacy_p2p_store(&dir)
                .context("migrating the legacy iroh 0.35 P2P store")
        })
        .await
        .context("joining P2P data dir preparation task")??;
        if let crate::p2p_store_migration::MigrationOutcome::Migrated {
            backup_dir,
            upgraded,
        } = &migration
        {
            tracing::warn!(
                p2p_dir = %data_dir.display(),
                backup = %backup_dir.display(),
                upgraded = upgraded.len(),
                "Legacy iroh 0.35 P2P store migrated (the backup copy is the rollback)"
            );
        }

        let blob_store = FsStore::load(data_dir.join("blobs"))
            .await
            .map_err(|e| anyhow::anyhow!("opening persistent iroh-blobs store: {e}"))?;
        let blobs = BlobsProtocol::new(&blob_store, None);
        let gossip = Gossip::builder()
            // F7's distributed-query transport rides gossip envelopes (the
            // request carries a shard list; responses carry per-shard
            // aggregate partials or capped raw rows). iroh-gossip's default
            // 4096-byte cap is sized for control messages — raise it so a
            // realistic partial fits without a chunking protocol. The value
            // must be identical network-wide (all peers run this code); the
            // sync thread still bounds payloads itself via
            // `distributed_query::GOSSIP_ENVELOPE_BUDGET` + row caps.
            .max_message_size(crate::distributed_query::GOSSIP_MAX_MESSAGE_SIZE)
            .spawn(endpoint.clone());
        let docs = Docs::persistent(data_dir.clone())
            .spawn(
                endpoint.clone(),
                iroh_blobs::api::Store::from(blob_store.clone()),
                gossip.clone(),
            )
            .await
            .context("spawning persistent iroh-docs engine")?;

        // Import the endpoint identity as the docs author + make it the
        // node-wide default (persisted to <data_dir>/default-author), so
        // writes, live events, and snapshots all carry one authenticated
        // author identity — see the method doc comment.
        let identity_author = iroh_docs::Author::from_bytes(&endpoint.secret_key().to_bytes());
        let identity_author_id = identity_author.id();
        docs.author_import(identity_author)
            .await
            .context("importing the endpoint identity as the docs author")?;
        docs.author_set_default(identity_author_id)
            .await
            .context("setting the endpoint identity as the default docs author")?;
        tracing::info!(
            author = %identity_author_id,
            node_id = %endpoint.id().fmt_short(),
            "Docs author imported from the endpoint identity"
        );

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .accept(iroh_docs::ALPN, docs.clone())
            .spawn();
        tracing::info!(
            node_id = %endpoint.id().fmt_short(),
            p2p_dir = %data_dir.display(),
            blobs_alpn = %String::from_utf8_lossy(iroh_blobs::ALPN),
            gossip_alpn = %String::from_utf8_lossy(iroh_gossip::ALPN),
            docs_alpn = %String::from_utf8_lossy(iroh_docs::ALPN),
            "P2P stack online: blobs + gossip + docs routed"
        );
        Ok(Self {
            endpoint,
            blob_store,
            blobs,
            gossip,
            docs,
            router,
            data_dir,
        })
    }

    /// The bound endpoint the stack rides on.
    pub fn endpoint(&self) -> &iroh::Endpoint {
        &self.endpoint
    }

    /// This node's public key identifier.
    pub fn node_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// The persistent blobs protocol (routed for inbound blob requests).
    pub fn blobs(&self) -> &BlobsProtocol {
        &self.blobs
    }

    /// The persistent blob store (fs store under `<data_dir>/blobs`).
    pub fn blob_store(&self) -> &iroh_blobs::api::Store {
        &self.blob_store
    }

    /// The gossip instance routed for inbound swarm connections.
    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    /// The docs engine (persistent replica store at `<data_dir>/docs.redb`).
    pub fn docs(&self) -> &Docs {
        &self.docs
    }

    /// Clone of the in-process docs API (author/doc/entry ops).
    ///
    /// This is the handle the replicator layer uses for `import_namespace`,
    /// `set_bytes`, `subscribe`, `start_sync` and `close`.
    pub fn docs_client(&self) -> iroh_docs::api::DocsApi {
        self.docs.api().clone()
    }

    /// The accept-loop router for all three ALPNs.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// The persistent data dir backing the stack's stores.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Shut the protocol handlers down (docs engine flush, blob store
    /// shutdown — the router calls each handler's `shutdown`).
    ///
    /// The endpoint close stays with the caller (`SyncEndpoint::shutdown`)
    /// so the existing sync-thread shutdown order is preserved.
    pub async fn shutdown(&self) {
        if let Err(e) = self.router.shutdown().await {
            tracing::warn!("P2P router shutdown error: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_config::RelayConfig;

    /// Bind a hermetic loopback endpoint (RelayMode::Disabled — no relay
    /// contact, satisfies the loopback-only P2P test rule).
    ///
    /// Returns `None` in sandboxed environments where UDP binding is
    /// unavailable (same precedent as `endpoint.rs` tests).
    async fn bind_test_endpoint(seed: u8) -> Option<iroh::Endpoint> {
        let secret = iroh::SecretKey::from_bytes(&[seed; 32]);
        match crate::endpoint::SyncEndpoint::new(secret, &RelayConfig::Disabled).await {
            Ok(ep) => Some(ep.inner().clone()),
            Err(e) => {
                tracing::warn!("endpoint bind failed (sandboxed env?) — skipping: {e}");
                None
            }
        }
    }

    #[test]
    fn resolve_p2p_dir_defaults_when_env_missing_or_blank() {
        assert_eq!(resolve_p2p_dir(None), PathBuf::from(DEFAULT_P2P_DIR));
        assert_eq!(resolve_p2p_dir(Some("")), PathBuf::from(DEFAULT_P2P_DIR));
        assert_eq!(resolve_p2p_dir(Some("   ")), PathBuf::from(DEFAULT_P2P_DIR));
    }

    #[test]
    fn resolve_p2p_dir_uses_env_override_verbatim() {
        assert_eq!(
            resolve_p2p_dir(Some("/tmp/p2p-x")),
            PathBuf::from("/tmp/p2p-x")
        );
        assert_eq!(
            resolve_p2p_dir(Some("  D:\\fe\\p2p  ")),
            PathBuf::from("D:\\fe\\p2p")
        );
    }

    #[test]
    fn p2p_dir_env_var_name_is_stable() {
        assert_eq!(P2P_DIR_ENV_VAR, "FE_P2P_DIR");
    }

    #[tokio::test]
    async fn docs_stack_spawns_all_persistent_stores() {
        let Some(endpoint) = bind_test_endpoint(21).await else {
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        let stack = DocsStack::spawn(endpoint, dir.path())
            .await
            .expect("stack spawn");

        // Persistent stores exist under the data dir (READ-BACK, not just Ok).
        assert!(dir.path().join("blobs").is_dir(), "blobs fs store dir");
        assert!(dir.path().join("docs.redb").is_file(), "docs redb store");
        assert!(
            dir.path().join("default-author").is_file(),
            "persistent default author"
        );
        assert_eq!(stack.data_dir(), dir.path());

        // The docs engine answers through the client (author storage is
        // loaded at engine spawn — a default author proves the engine lives),
        // and it is the endpoint identity (F20 alignment).
        let author = stack
            .docs_client()
            .author_default()
            .await
            .expect("default author from persistent storage");
        assert_ne!(author, iroh_docs::AuthorId::default());
        assert_eq!(author.as_bytes(), stack.node_id().as_bytes());
        // A fresh dir is stamped with the 1.x store-format marker.
        assert!(
            dir.path()
                .join(crate::p2p_store_migration::STORE_FORMAT_MARKER)
                .is_file(),
            "store format marker"
        );

        stack.shutdown().await;
    }

    #[tokio::test]
    async fn docs_stack_routes_all_three_alpns() {
        let Some(endpoint) = bind_test_endpoint(22).await else {
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        let stack = DocsStack::spawn(endpoint, dir.path())
            .await
            .expect("stack spawn");

        // A second bare endpoint connects to the stack's node for each
        // routed ALPN; an unknown ALPN must be refused by the handshake.
        let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint bind");
        let addr = stack.endpoint().addr();

        for alpn in [iroh_blobs::ALPN, iroh_gossip::ALPN, iroh_docs::ALPN] {
            let alpn_name = String::from_utf8_lossy(alpn).to_string();
            match client.connect(addr.clone(), alpn).await {
                Ok(_conn) => tracing::debug!(alpn_name, "routed"),
                Err(e) => panic!("ALPN {alpn_name} should route through the stack: {e}"),
            }
        }
        // Negative control: the router refuses an ALPN it does not accept.
        assert!(
            client
                .connect(addr.clone(), b"/not-a-protocol/1")
                .await
                .is_err(),
            "unknown ALPN must be refused"
        );

        stack.shutdown().await;
    }

    #[tokio::test]
    async fn docs_stack_spawn_failure_is_an_error_not_a_panic() {
        // Offline degrade vector: a data dir path that cannot be created
        // (its parent is a regular file) makes spawn fail cleanly — the
        // sync thread's caller degrades to mock on this error.
        let parent = tempfile::TempDir::new().unwrap();
        let blocker = parent.path().join("blocker");
        std::fs::write(&blocker, b"file, not a dir").unwrap();
        let bad_dir = blocker.join("p2p");

        let Some(endpoint) = bind_test_endpoint(23).await else {
            return;
        };
        let result = DocsStack::spawn(endpoint, &bad_dir).await;
        assert!(result.is_err(), "spawn under a file path must fail");
    }

    #[test]
    fn alpn_constants_match_iroh_1x_wire_protocols() {
        assert_eq!(iroh_blobs::ALPN, b"/iroh-bytes/4");
        // iroh-gossip 0.101 bumped the gossip ALPN (0.35 was /iroh-gossip/0).
        assert_eq!(iroh_gossip::ALPN, b"/iroh-gossip/1");
        assert_eq!(iroh_docs::ALPN, b"/iroh-sync/1");
    }
}
