//! Real iroh-docs 0.35 P2P stack: Blobs (fs) + Gossip + Docs (persistent
//! redb) + a Router accepting all three ALPNs (A1, P2P Mycelium Phase F).
//!
//! [`DocsStack`] owns every long-lived protocol actor for one online sync
//! thread so that spawning and shutting them down is a single operation.
//! It is built over an **already-bound** endpoint; when the endpoint fails
//! to bind (or the stack cannot spawn) the sync thread runs without a
//! stack and the replicator layer degrades to the in-memory mock — never
//! a crash. See `fe-sync/src/AGENTS.md` §iroh-0.35.
//!
//! Persistent layout under the data dir ([`p2p_data_dir`], env
//! [`P2P_DIR_ENV_VAR`], default `data/p2p`):
//!
//! ```text
//! <dir>/blobs/          — iroh-blobs fs store (bao data + outboards)
//! <dir>/docs.redb       — iroh-docs replica store
//! <dir>/default-author  — hex-encoded persistent default author key
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use iroh::protocol::Router;
use iroh_blobs::net_protocol::Blobs;
use iroh_blobs::store::fs::Store as FsBlobStore;
use iroh_docs::protocol::Docs;
use iroh_gossip::net::Gossip;

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
/// routable: blobs (`/iroh-bytes/4`), gossip (`/iroh-gossip/0`), docs
/// (`/iroh-sync/1`). Before the router existed inbound protocol connections
/// were silently unroutable (see AGENTS.md §iroh-0.35).
pub struct DocsStack {
    endpoint: iroh::Endpoint,
    blobs: Blobs<FsBlobStore>,
    gossip: Gossip,
    docs: Docs<FsBlobStore>,
    router: Router,
    data_dir: PathBuf,
}

impl DocsStack {
    /// Spawn the full stack over an already-bound endpoint.
    ///
    /// `data_dir` is created if missing; every persistent store lives under
    /// it. Any failure returns an error — the caller degrades to
    /// offline/mock mode rather than panicking. Directory creation runs on
    /// the blocking pool (see AGENTS.md §sync-thread-blocking-io).
    pub async fn spawn(endpoint: iroh::Endpoint, data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        // redb and the docs author storage need the parent dir to exist;
        // the blobs fs store creates its own subtree.
        let dir = data_dir.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&dir))
            .await
            .context("joining P2P data dir creation task")?
            .with_context(|| format!("creating P2P data dir {}", data_dir.display()))?;

        let blobs = Blobs::persistent(data_dir.join("blobs"))
            .await
            .context("opening persistent iroh-blobs store")?
            .build(&endpoint);
        let gossip = Gossip::builder()
            .spawn(endpoint.clone())
            .await
            .context("spawning iroh-gossip")?;
        let docs = Docs::persistent(data_dir.clone())
            .spawn(&blobs, &gossip)
            .await
            .context("spawning persistent iroh-docs engine")?;
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs.clone())
            .accept(iroh_gossip::net::GOSSIP_ALPN, gossip.clone())
            .accept(iroh_docs::ALPN, docs.clone())
            .spawn();
        tracing::info!(
            node_id = %endpoint.node_id().fmt_short(),
            p2p_dir = %data_dir.display(),
            blobs_alpn = %String::from_utf8_lossy(iroh_blobs::ALPN),
            gossip_alpn = %String::from_utf8_lossy(iroh_gossip::net::GOSSIP_ALPN),
            docs_alpn = %String::from_utf8_lossy(iroh_docs::ALPN),
            "P2P stack online: blobs + gossip + docs routed"
        );
        Ok(Self {
            endpoint,
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
    pub fn node_id(&self) -> iroh::NodeId {
        self.endpoint.node_id()
    }

    /// The persistent blobs protocol (fs store under `<data_dir>/blobs`).
    pub fn blobs(&self) -> &Blobs<FsBlobStore> {
        &self.blobs
    }

    /// The gossip instance routed for inbound swarm connections.
    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    /// The docs engine (persistent replica store at `<data_dir>/docs.redb`).
    pub fn docs(&self) -> &Docs<FsBlobStore> {
        &self.docs
    }

    /// Clone of the in-process docs RPC client (author/doc/entry ops).
    ///
    /// This is the handle the replicator layer uses for `import_namespace`,
    /// `set_bytes`, `subscribe`, `start_sync` and `close` (requires the
    /// iroh-docs `rpc` feature).
    pub fn docs_client(&self) -> iroh_docs::rpc::client::docs::MemClient {
        self.docs.client().clone()
    }

    /// The accept-loop router for all three ALPNs.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// The persistent data dir backing the stack's stores.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Shut the protocol handlers down (docs engine flush).
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
        // loaded at engine spawn — a default author proves the engine lives).
        let author = stack
            .docs_client()
            .authors()
            .default()
            .await
            .expect("default author from persistent storage");
        assert_ne!(author, iroh_docs::AuthorId::default());

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
        let client = iroh::Endpoint::builder()
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint bind");
        let addr = stack.endpoint().node_addr().await.expect("node addr");
        client.add_node_addr(addr).expect("add node addr");

        for alpn in [
            iroh_blobs::ALPN,
            iroh_gossip::net::GOSSIP_ALPN,
            iroh_docs::ALPN,
        ] {
            let alpn_name = String::from_utf8_lossy(alpn).to_string();
            match client.connect(stack.node_id(), alpn).await {
                Ok(_conn) => tracing::debug!(alpn_name, "routed"),
                Err(e) => panic!("ALPN {alpn_name} should route through the stack: {e}"),
            }
        }
        // Negative control: the router refuses an ALPN it does not accept.
        assert!(
            client
                .connect(stack.node_id(), b"/not-a-protocol/1")
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
    fn alpn_constants_match_iroh_035_wire_protocols() {
        assert_eq!(iroh_blobs::ALPN, b"/iroh-bytes/4");
        assert_eq!(iroh_gossip::net::GOSSIP_ALPN, b"/iroh-gossip/0");
        assert_eq!(iroh_docs::ALPN, b"/iroh-sync/1");
    }
}
