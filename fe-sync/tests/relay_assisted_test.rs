//! IROH-1X's last acceptance criterion: relay-assisted connectivity against
//! a locally run `iroh-relay` 1.3 container — fe-sync/src/AGENTS.md
//! §relay-health "Self-hosting iroh-relay 1.x" (plan.md W6-8, §2.7).
//!
//! Every test here manages its own container through `docker`/`podman`
//! (`FE_CONTAINER_RUNTIME` override) and skips when no runtime is usable,
//! unless `FE_REQUIRE_RELAY_CONTAINER=1` turns a skip into a failure — the
//! same skip-or-fail contract as `p2p_store_migration.rs`'s
//! `skip_unless_required`. No container is started by `cargo check`; these
//! run only at the campaign final sweep / the CI "Relay-assisted
//! acceptance" step.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use fe_runtime::blob_transport::{content_hash, BlobTransport};
use fe_sync::endpoint::SyncEndpoint;
use fe_sync::relay_config::{RelayAccessToken, RelayConfig};
use fe_sync::tileset_transfer::MAX_HASH_SEQ_CHILDREN;
use fe_sync::{DocsStack, IrohBlobTransport, RelayHealth, RelayStatusSummary, TilesetTransfer};
use iroh::Watcher as _;
use tempfile::TempDir;

/// Pinned `iroh-relay` 1.3 image (plan.md §2.7 "Image" / A13).
const RELAY_IMAGE: &str = "n0computer/iroh-relay:v1.3.0@sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57";

/// Skip optional infrastructure, or fail required acceptance; see tests/AGENTS.md.
fn skip_unless_required(reason: &str) {
    if std::env::var("FE_REQUIRE_RELAY_CONTAINER").is_ok_and(|v| v == "1") {
        panic!("FE_REQUIRE_RELAY_CONTAINER=1 forbids skipping: {reason}");
    }
    eprintln!("skipping: {reason}");
}

/// `FE_CONTAINER_RUNTIME` (`docker`|`podman`), else the first of `docker`,
/// `podman` for which `<rt> info` exits 0.
fn container_runtime() -> Option<&'static str> {
    if let Ok(rt) = std::env::var("FE_CONTAINER_RUNTIME") {
        return match rt.as_str() {
            "docker" => Some("docker"),
            "podman" => Some("podman"),
            other => {
                eprintln!("FE_CONTAINER_RUNTIME={other:?} must be \"docker\" or \"podman\"");
                None
            }
        };
    }
    ["docker", "podman"].into_iter().find(|rt| {
        Command::new(rt)
            .arg("info")
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

/// A running `iroh-relay --dev` container on `127.0.0.1`. Removed by its
/// exact name (never a pattern) on `Drop`, which also runs on panic/test
/// failure (profile unwind), per the user's container-cleanup rule.
struct RelayContainer {
    runtime: &'static str,
    name: String,
    port: u16,
}

impl RelayContainer {
    /// Start the container. `config_mount` optionally bind-mounts a
    /// read-only host TOML at a container path and passes
    /// `--config-path <that path>`. `env_vars` are each set on the `docker
    /// run`/`podman run` *child process's own environment* and passed
    /// through with a bare `-e NAME` (never `-e NAME=value`), so a secret
    /// value is never written to argv, a file, or this process's own
    /// environment (A14 / the user's SECRETS rule).
    fn start(
        runtime: &'static str,
        config_mount: Option<(&Path, &str)>,
        env_vars: &[(&str, &str)],
    ) -> Option<Self> {
        let name = format!(
            "fe-relay-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );

        let mut cmd = Command::new(runtime);
        cmd.args([
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-p",
            "127.0.0.1::3340",
        ]);
        if let Some((host_path, container_path)) = config_mount {
            cmd.arg("-v")
                .arg(format!("{}:{}:ro", host_path.display(), container_path));
        }
        for &(key, value) in env_vars {
            cmd.env(key, value);
            cmd.arg("-e").arg(key);
        }
        cmd.arg(RELAY_IMAGE).arg("--dev");
        if let Some((_, container_path)) = config_mount {
            cmd.arg("--config-path").arg(container_path);
        }

        let output = cmd.output().ok()?;
        if !output.status.success() {
            eprintln!(
                "{runtime} run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }

        let port_output = Command::new(runtime)
            .args(["port", &name, "3340/tcp"])
            .output()
            .ok()?;
        if !port_output.status.success() {
            let _ = Command::new(runtime).args(["rm", "-f", &name]).output();
            return None;
        }
        let port_text = String::from_utf8_lossy(&port_output.stdout);
        let Some(port) = port_text
            .lines()
            .next()
            .and_then(|line| line.trim().rsplit(':').next())
            .and_then(|p| p.trim().parse::<u16>().ok())
        else {
            let _ = Command::new(runtime).args(["rm", "-f", &name]).output();
            return None;
        };

        let container = Self {
            runtime,
            name,
            port,
        };
        if container.wait_healthy(Duration::from_secs(60)) {
            println!(
                "iroh-relay container {} started on 127.0.0.1:{}",
                container.name, container.port
            );
            Some(container)
        } else {
            eprintln!(
                "iroh-relay container {} never answered /healthz",
                container.name
            );
            None
        }
    }

    /// Poll `/healthz` until a 200 or `timeout`. Every socket op is bounded
    /// (refine R3): the runtime's port forwarder accepts a connection before
    /// the relay listens, and an unbounded `read_to_end` on such a socket
    /// could outlive the deadline, which is only checked between attempts.
    fn wait_healthy(&self, timeout: Duration) -> bool {
        const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
        let deadline = Instant::now() + timeout;
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], self.port));
        while Instant::now() < deadline {
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
                let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
                let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
                let request =
                    b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
                if stream.write_all(request).is_ok() {
                    let mut buf = Vec::new();
                    if stream.read_to_end(&mut buf).is_ok() {
                        let text = String::from_utf8_lossy(&buf);
                        if text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200") {
                            return true;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    }

    fn relay_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for RelayContainer {
    fn drop(&mut self) {
        match Command::new(self.runtime)
            .args(["rm", "-f", &self.name])
            .output()
        {
            Ok(out) if out.status.success() => {
                println!("removed iroh-relay container {}", self.name);
            }
            Ok(out) => eprintln!(
                "failed to remove container {}: {}",
                self.name,
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(e) => eprintln!("failed to remove container {}: {e}", self.name),
        }
    }
}

/// Start a plain (`--dev`, no access control) relay, or skip/fail per
/// `skip_unless_required`.
fn start_plain_relay() -> Option<RelayContainer> {
    let runtime = container_runtime().or_else(|| {
        skip_unless_required("no usable container runtime (docker/podman info failed)");
        None
    })?;
    RelayContainer::start(runtime, None, &[]).or_else(|| {
        skip_unless_required("iroh-relay container failed to start or become healthy");
        None
    })
}

/// Start a `shared_token`-gated relay. The config file's own
/// `access.shared_token` entry is a decoy — `IROH_RELAY_ACCESS_TOKEN`
/// (passed only through the child process's environment, A14) overrides it
/// with `real_token`, so the decoy is never a valid credential.
fn start_token_relay(real_token: &str) -> Option<RelayContainer> {
    let runtime = container_runtime().or_else(|| {
        skip_unless_required("no usable container runtime (docker/podman info failed)");
        None
    })?;
    let config_dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            skip_unless_required(&format!("could not create a config tempdir: {e}"));
            return None;
        }
    };
    let config_path = config_dir.path().join("relay.toml");
    if let Err(e) = std::fs::write(
        &config_path,
        "http_bind_addr = \"0.0.0.0:3340\"\n\
         enable_metrics = false\n\
         access.shared_token = [\"decoy-token-never-the-real-one\"]\n",
    ) {
        skip_unless_required(&format!("could not write the relay config: {e}"));
        return None;
    }

    let container = RelayContainer::start(
        runtime,
        Some((&config_path, "/relay.toml")),
        &[("IROH_RELAY_ACCESS_TOKEN", real_token)],
    );
    // Keep `config_dir` alive until the container is up; the bind mount is
    // read once at container start, so it is safe to drop afterward.
    drop(config_dir);
    container.or_else(|| {
        skip_unless_required("token-gated iroh-relay container failed to start or become healthy");
        None
    })
}

/// Bind a [`SyncEndpoint`] against `relay_config`; `None` where the bind
/// itself fails (sandboxed env, no UDP).
async fn online_endpoint(seed: u8, relay_config: &RelayConfig) -> Option<SyncEndpoint> {
    match SyncEndpoint::new(iroh::SecretKey::from_bytes(&[seed; 32]), relay_config).await {
        Ok(ep) => Some(ep),
        Err(e) => {
            skip_unless_required(&format!("endpoint setup failed: {e}"));
            None
        }
    }
}

/// Like [`online_endpoint`], with an explicit relay access token.
async fn token_endpoint(
    seed: u8,
    relay_config: &RelayConfig,
    token: Option<RelayAccessToken>,
) -> Option<SyncEndpoint> {
    match SyncEndpoint::with_relay_token(
        iroh::SecretKey::from_bytes(&[seed; 32]),
        relay_config,
        token,
    )
    .await
    {
        Ok(ep) => Some(ep),
        Err(e) => {
            skip_unless_required(&format!("token endpoint setup failed: {e}"));
            None
        }
    }
}

/// Fold the endpoint's current home-relay status into [`RelayHealth`]
/// (`RelayHealth::on_relay_status` + `RelayStatusSummary::from_status`,
/// exactly as `status.rs::drain_sync_events` does in production), polling
/// up to `timeout` for a terminal `Healthy`.
async fn relay_health_within(endpoint: &SyncEndpoint, timeout: Duration) -> RelayHealth {
    let mut watcher = endpoint.home_relay_status();
    let deadline = Instant::now() + timeout;
    loop {
        let statuses: Vec<RelayStatusSummary> = watcher
            .get()
            .iter()
            .map(RelayStatusSummary::from_status)
            .collect();
        let health = RelayHealth::Unknown.on_relay_status(&statuses);
        if matches!(health, RelayHealth::Healthy) || Instant::now() >= deadline {
            return health;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// `addr` stripped of every direct IP transport — only its relay URL(s)
/// remain. This is the iroh_1_0 "direct IP paths unusable" shape
/// (`conductor/tracks/iroh_1_0_upgrade_20260711/plan.md:123-126`).
fn relay_only_addr(addr: iroh::EndpointAddr) -> iroh::EndpointAddr {
    let addrs = addr
        .addrs
        .into_iter()
        .filter(|a| matches!(a, iroh::TransportAddr::Relay(_)))
        .collect();
    iroh::EndpointAddr { id: addr.id, addrs }
}

/// A synthetic tileset (index + N tiny bundles) written under `dir`, for
/// publishing through [`TilesetTransfer`]. Mirrors the shape of
/// `tileset_transfer_test.rs::SourceTileset` without importing across test
/// binaries.
fn write_bundles(dir: &Path, tileset_id: &str, count: usize) -> (Bytes, Vec<PathBuf>) {
    let bundles_dir = dir.join("bundles");
    std::fs::create_dir_all(&bundles_dir).unwrap();
    let index_json = Bytes::from(format!(
        r#"{{"schema":1,"tileset_id":"{tileset_id}","bundles":{count}}}"#
    ));
    let mut paths = Vec::new();
    for seq in 0..count {
        let mut bytes = vec![0u8; 4096];
        blake3::Hasher::new()
            .update(format!("{tileset_id}/{seq}").as_bytes())
            .finalize_xof()
            .fill(&mut bytes);
        let path = bundles_dir.join(format!("{seq}.bundle"));
        std::fs::write(&path, &bytes).unwrap();
        paths.push(path);
    }
    (index_json, paths)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_assisted_tileset_fetch_through_a_self_hosted_iroh_relay() {
    let Some(container) = start_plain_relay() else {
        return;
    };
    let relay_config = RelayConfig::parse(&container.relay_url()).expect("valid relay url");

    let (Some(alice), Some(bob)) = (
        online_endpoint(81, &relay_config).await,
        online_endpoint(82, &relay_config).await,
    ) else {
        return;
    };

    let alice_addr = alice.node_addr().await;
    assert!(
        alice_addr
            .addrs
            .iter()
            .any(|a| matches!(a, iroh::TransportAddr::Relay(_))),
        "alice's address should carry the relay URL once connected: {alice_addr:?}"
    );
    let alice_health = relay_health_within(&alice, Duration::from_secs(30)).await;
    assert!(
        matches!(alice_health, RelayHealth::Healthy),
        "expected alice's relay health to be Healthy, got {alice_health:?}"
    );

    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let alice_stack = DocsStack::spawn(alice.inner().clone(), alice_dir.path().join("p2p"))
        .await
        .expect("alice stack");
    let bob_stack = DocsStack::spawn(bob.inner().clone(), bob_dir.path().join("p2p"))
        .await
        .expect("bob stack");

    let (index_json, bundle_paths) = write_bundles(alice_dir.path(), "relay-summit", 3);
    let publisher = TilesetTransfer::for_p2p_dir(
        Arc::new(IrohBlobTransport::from_stack(&alice_stack)),
        alice_stack.data_dir(),
    );
    let entry = publisher
        .publish("relay-summit", index_json, &bundle_paths)
        .await
        .unwrap();

    // The production derivation (`sync_thread.rs:620-628`): only relays
    // this node's own relay map already holds may be dialed (S8b).
    let allowed_relays: Vec<iroh::RelayUrl> =
        relay_config.to_relay_mode().unwrap().relay_map().urls();
    let bob_transport =
        Arc::new(IrohBlobTransport::from_stack(&bob_stack).with_allowed_relays(allowed_relays));

    // Alice's address with every direct IP path stripped — bob can only
    // ever reach her through the relay.
    let alice_relay_only = relay_only_addr(alice.node_addr().await);
    let provider = IrohBlobTransport::provider_ref(&alice_relay_only).unwrap();

    let children = bob_transport
        .fetch_hash_seq(&provider, entry.root, MAX_HASH_SEQ_CHILDREN)
        .await
        .expect("fetch over the self-hosted relay");
    assert_eq!(children.len(), 1 + 3, "index + 3 bundles");

    // Bob knew alice ONLY by her relay URL, so first contact went through the relay. These
    // are full production endpoints on one host, so iroh may then hole-punch a direct path
    // and retire the relay one — legitimate, and why "no direct path ever" is proven by
    // `a_relay_only_dialer_reaches_its_peer_solely_through_the_relay` (an IP-less dialer)
    // rather than here.
    let conn = bob
        .inner()
        .connect(alice_relay_only, iroh_blobs::ALPN)
        .await
        .expect("connect from a relay-only address");
    assert!(
        conn.paths().iter().next().is_some(),
        "a connection dialed from a relay-only address has a live path"
    );

    alice_stack.shutdown().await;
    bob_stack.shutdown().await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_only_dialer_reaches_its_peer_solely_through_the_relay() {
    let Some(container) = start_plain_relay() else {
        return;
    };
    let relay_config = RelayConfig::parse(&container.relay_url()).expect("valid relay url");
    let relay_mode = relay_config.to_relay_mode().expect("relay mode");

    let Some(alice) = online_endpoint(83, &relay_config).await else {
        return;
    };
    let alice_dir = TempDir::new().unwrap();
    let alice_stack = DocsStack::spawn(alice.inner().clone(), alice_dir.path().join("p2p"))
        .await
        .expect("alice stack");
    let (index_json, bundle_paths) = write_bundles(alice_dir.path(), "relay-only-dialer", 1);
    let publisher = TilesetTransfer::for_p2p_dir(
        Arc::new(IrohBlobTransport::from_stack(&alice_stack)),
        alice_stack.data_dir(),
    );
    let entry = publisher
        .publish("relay-only-dialer", index_json, &bundle_paths)
        .await
        .unwrap();
    let provider_addr = relay_only_addr(alice.node_addr().await);
    if provider_addr.addrs.is_empty() {
        skip_unless_required("relay-only dialer provider never reached the relay");
        return;
    }

    // Test scaffolding mirroring `SyncEndpoint::new`, minus IP transports:
    // the only way this endpoint can ever reach a peer is through the
    // relay (iroh_1_0's open task, plan.md §2.7).
    let dialer = match iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(iroh::SecretKey::from_bytes(&[84u8; 32]))
        .relay_mode(relay_mode)
        .clear_ip_transports()
        .bind()
        .await
    {
        Ok(ep) => ep,
        Err(e) => {
            skip_unless_required(&format!("relay-only dialer endpoint setup failed: {e}"));
            return;
        }
    };

    let conn = dialer
        .connect(provider_addr.clone(), iroh_blobs::ALPN)
        .await
        .expect("connect solely through the relay");
    assert!(
        conn.paths().iter().all(|p| p.is_relay()),
        "a relay-only endpoint must never open a direct IP path"
    );

    let bob_blob_dir = TempDir::new().unwrap();
    let bob_store = iroh_blobs::store::fs::FsStore::load(bob_blob_dir.path())
        .await
        .expect("bob blob store");
    let bob_transport = IrohBlobTransport::new(bob_store.into(), dialer.clone());
    let provider = IrohBlobTransport::provider_ref(&provider_addr).unwrap();
    let children = bob_transport
        .fetch_hash_seq(&provider, entry.root, MAX_HASH_SEQ_CHILDREN)
        .await
        .expect("fetch solely through the relay");
    assert_eq!(children.len(), 1 + 1, "index + 1 bundle");

    alice_stack.shutdown().await;
    alice.shutdown().await;
    dialer.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_naming_an_untrusted_relay_is_never_dialed() {
    let Some(container) = start_plain_relay() else {
        return;
    };
    let relay_config = RelayConfig::parse(&container.relay_url()).expect("valid relay url");

    let Some(alice) = online_endpoint(85, &relay_config).await else {
        return;
    };
    let provider_addr = relay_only_addr(alice.node_addr().await);
    if provider_addr.addrs.is_empty() {
        skip_unless_required("untrusted-relay test provider never reached the relay");
        return;
    }
    let provider = IrohBlobTransport::provider_ref(&provider_addr).unwrap();

    let Some(bob) = online_endpoint(86, &relay_config).await else {
        return;
    };
    let bob_dir = TempDir::new().unwrap();
    let bob_stack = DocsStack::spawn(bob.inner().clone(), bob_dir.path().join("p2p"))
        .await
        .expect("bob stack");
    // Deliberately no `.with_allowed_relays(..)`: bob trusts no relay at
    // all — not even this real, legitimate one — so the sanitized address
    // keeps nothing and the fetch is refused locally (S8b).
    let bob_transport = Arc::new(IrohBlobTransport::from_stack(&bob_stack));

    let started = Instant::now();
    let result = bob_transport
        .fetch_hash_seq(&provider, content_hash(b"irrelevant placeholder"), 4)
        .await;
    assert!(
        result.is_err(),
        "an untrusted-relay-only provider must never be dialed"
    );
    assert!(
        started.elapsed() < fe_sync::blob_transport::CONNECT_TIMEOUT,
        "the refusal must be a local decision, not a timed-out dial attempt: took {:?}",
        started.elapsed()
    );

    bob_stack.shutdown().await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_gated_relay_admits_the_configured_token_and_refuses_others() {
    let real_token = format!("fe-relay-test-token-{}", ulid::Ulid::new());
    let Some(container) = start_token_relay(&real_token) else {
        return;
    };
    let relay_config = RelayConfig::parse(&container.relay_url()).expect("valid relay url");

    let Some(good) =
        token_endpoint(87, &relay_config, Some(RelayAccessToken::new(real_token))).await
    else {
        return;
    };
    let good_health = relay_health_within(&good, Duration::from_secs(30)).await;
    assert!(
        matches!(good_health, RelayHealth::Healthy),
        "the configured token should be admitted, got {good_health:?}"
    );

    let Some(wrong) = token_endpoint(
        88,
        &relay_config,
        Some(RelayAccessToken::new("definitely-not-the-token")),
    )
    .await
    else {
        return;
    };
    let wrong_health = relay_health_within(&wrong, Duration::from_secs(5)).await;
    assert!(
        !matches!(wrong_health, RelayHealth::Healthy),
        "a wrong token must never be admitted, got {wrong_health:?}"
    );

    let Some(none) = token_endpoint(89, &relay_config, None).await else {
        return;
    };
    let none_health = relay_health_within(&none, Duration::from_secs(5)).await;
    assert!(
        !matches!(none_health, RelayHealth::Healthy),
        "no token must never be admitted, got {none_health:?}"
    );

    good.shutdown().await;
    wrong.shutdown().await;
    none.shutdown().await;
}
