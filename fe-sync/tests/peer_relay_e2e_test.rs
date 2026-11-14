//! Peer-relay end-to-end evidence (P15): relay-only peers through an in-process members-only relay.
//!
//! One journey per acceptance cluster; the relay is real (`run_relay_server_with_access` + `RelayMembershipGate`),
//! only the clock and DNS are faked. See `fe-sync/tests/AGENTS.md` for the batch (R8, slow sub-batch).
#![cfg(feature = "relay-server")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fe_identity::NodeKeypair;
use fe_policy::RoleLevel;
use fe_runtime::net_diagnostics::PeerConnectionKind;
use fe_runtime::relay_advertisement::{
    canonical_body, RelayAdvertisementRow, RELAY_ADVERT_ROW_VERSION, RELAY_ADVERT_SIGNING_DOMAIN,
};
use fe_sync::gossip_auth::{
    open_ingress, refusal, GossipFrameKind, GossipIngress, ReplayGuard, SignedGossipFrame, SigningMode,
};
use fe_sync::net_diagnostics::{build_snapshot, collect_peer_rows, SnapshotInputs};
use fe_sync::relay_advertise::{AdvertChange, RelayAdvertisementBook};
use fe_sync::relay_config::RelayAdvertSignedMode;
use fe_sync::relay_reconcile::{RelayOp, RelayReconcileInputs, RelayReconciler};
use fe_sync::relay_server::{
    GateLimits, GateSettings, RelayMembershipGate, ServedVerses, EVICTION_TARGET, RECONCILE_INTERVAL,
};
use fe_sync::relay_trust::{evaluate_coverage, AdoptionMode};
use fe_sync::replicator::IrohDocsEngineHolder;
use fe_sync::sharding::RelayAdmissionSnapshot;
use fe_sync::{DocsStack, IrohDocsReplicator, RelayAdvertisePolicy, RelayHealth, VerseReplicator};
use futures_lite::StreamExt;
use iroh::address_lookup::MemoryLookup;
use iroh::endpoint::{presets, RelayStatus};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher};
use tempfile::TempDir;

const VERSE: &str = "01JVERSE0000000000000000AA";
const WAIT: Duration = Duration::from_secs(20);

fn secret_of(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

fn did_of(seed: u8) -> String {
    fe_identity::did_key::did_key_from_public_key_bytes(secret_of(seed).public().as_bytes())
        .expect("seed is a valid ed25519 key")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as u64).unwrap_or(0)
}

fn snapshot(seeds: &[u8]) -> RelayAdmissionSnapshot {
    let members: BTreeSet<String> = seeds.iter().map(|seed| did_of(*seed)).collect();
    RelayAdmissionSnapshot {
        members_by_verse: BTreeMap::from([(VERSE.to_string(), members)]),
        ..Default::default()
    }
}

/// An in-process members-only relay: the real gate in front of iroh's relay server.
struct RelayLab {
    gate: Arc<RelayMembershipGate>,
    url: RelayUrl,
    server: iroh_relay::server::Server,
    _map: iroh::RelayMap,
}

impl RelayLab {
    async fn start(member_seeds: &[u8]) -> Self {
        let gate = RelayMembershipGate::new(GateSettings {
            limits: GateLimits { max_clients: 16, max_connections_per_endpoint: 2 },
            served_verses: ServedVerses::Listed([VERSE.to_string()].into()),
            log_client_ips: false,
        });
        gate.apply_snapshot(&snapshot(member_seeds));
        let (map, url, server) = iroh::test_utils::run_relay_server_with_access(false, gate.clone())
            .await
            .expect("in-process relay");
        Self { gate, url, server, _map: map }
    }

    async fn stop(self) {
        if let Err(error) = self.server.shutdown().await {
            eprintln!("relay shutdown reported an error: {error}");
        }
    }

    /// Stand-in for the embedded relay's reconciler: re-issues `disconnect` every tick until the gate saw `on_disconnect`.
    async fn evict_until_gone(&self, endpoint: &EndpointId) -> Duration {
        let started = Instant::now();
        while self.gate.tracked_connections(endpoint) > 0 {
            assert!(started.elapsed() <= EVICTION_TARGET, "eviction exceeded the 5 s target");
            if let Some(service) = self.server.relay_service() {
                service.clients().disconnect(*endpoint, None);
            }
            tokio::time::sleep(RECONCILE_INTERVAL).await;
        }
        started.elapsed()
    }
}

/// A relay-only endpoint (no IP transports) with its own address book.
struct RelayOnlyPeer {
    endpoint: Endpoint,
    lookup: MemoryLookup,
}

impl RelayOnlyPeer {
    async fn bind(seed: u8, url: &RelayUrl) -> Self {
        let map: iroh::RelayMap = iroh::RelayConfig::new(url.clone(), None).into();
        let lookup = MemoryLookup::new();
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(secret_of(seed))
            .relay_mode(RelayMode::Custom(map))
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .address_lookup(lookup.clone())
            .clear_ip_transports()
            .bind()
            .await
            .expect("bind relay-only peer");
        Self { endpoint, lookup }
    }

    /// This peer's address as another peer can only know it: its id plus the shared relay.
    fn relay_addr(&self, url: &RelayUrl) -> EndpointAddr {
        EndpointAddr {
            id: self.endpoint.id(),
            addrs: [TransportAddr::Relay(url.clone())].into_iter().collect(),
        }
    }

    fn learn(&self, other: &RelayOnlyPeer, url: &RelayUrl) {
        self.lookup.add_endpoint_info(other.relay_addr(url));
    }
}

/// Waits until the home-relay status satisfies `wanted`; false on timeout.
async fn relay_status_reaches(endpoint: &Endpoint, wanted: impl Fn(&RelayStatus) -> bool) -> bool {
    let mut stream = endpoint.home_relay_status().stream();
    tokio::time::timeout(WAIT, async {
        while let Some(statuses) = stream.next().await {
            if statuses.iter().any(&wanted) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

async fn is_admitted(endpoint: &Endpoint) -> bool {
    relay_status_reaches(endpoint, RelayStatus::is_connected).await
}

async fn is_denied_with(endpoint: &Endpoint, reason: &str) -> bool {
    relay_status_reaches(endpoint, |status| status.auth_denied_reason() == Some(reason)).await
}

fn clocked_row(name: &str, author: &str, hlc: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "verse_id": VERSE,
        "name": name,
        "_fe_clock": {"v": 1, "author": author, "own": {"name": {"hlc": hlc.to_string(), "value": name}}},
    }))
    .expect("clocked row serializes")
}

/// AC-3 / AC-13 / AC-9: two relay-only members sync a row through the relay and diagnostics name that relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_only_members_sync_a_row_and_diagnostics_report_relayed() {
    let lab = RelayLab::start(&[1, 2]).await;
    let alice = RelayOnlyPeer::bind(1, &lab.url).await;
    let bob = RelayOnlyPeer::bind(2, &lab.url).await;
    assert!(is_admitted(&alice.endpoint).await && is_admitted(&bob.endpoint).await, "both members reach the relay");
    alice.learn(&bob, &lab.url);
    bob.learn(&alice, &lab.url);

    let workdir = TempDir::new().expect("tempdir");
    let alice_stack = Arc::new(
        DocsStack::spawn(alice.endpoint.clone(), workdir.path().join("alice")).await.expect("alice stack"),
    );
    let bob_stack = Arc::new(DocsStack::spawn(bob.endpoint.clone(), workdir.path().join("bob")).await.expect("bob stack"));
    let secret_hex = hex::encode([7u8; 32]);
    let alice_replica = IrohDocsReplicator::new(
        VERSE.to_string(),
        secret_hex.clone(),
        secret_hex.clone(),
        did_of(1),
        Arc::new(IrohDocsEngineHolder::online(alice_stack.clone())),
    );
    let bob_replica = IrohDocsReplicator::new(
        VERSE.to_string(),
        secret_hex.clone(),
        secret_hex,
        did_of(2),
        Arc::new(IrohDocsEngineHolder::online(bob_stack.clone())),
    );
    alice_replica.open_document().await.expect("alice opens the verse doc");
    bob_replica.open_document().await.expect("bob opens the verse doc");

    let mut bob_changes = bob_replica.subscribe().await.expect("bob subscribes");
    bob_replica.start_sync(vec![alice.relay_addr(&lab.url)]).await.expect("bob syncs through the relay");
    alice_replica.start_sync(Vec::new()).await.expect("alice serves her doc");

    let payload = clocked_row("Relayed Verse", &did_of(1), 100);
    alice_replica.write_row("verse", VERSE, &payload).await.expect("alice writes");
    let change = tokio::time::timeout(WAIT, bob_changes.recv())
        .await
        .expect("the row crosses the relay")
        .expect("bob's pump stays alive");
    assert_eq!(change.record_id, VERSE);
    assert_eq!(change.data, payload, "bob received alice's exact bytes");
    assert_eq!(change.author_id, did_of(1), "authorship is the endpoint identity");
    assert_eq!(lab.gate.total_connections(), 2, "only the two admitted members hold relay connections");

    // AC-13: the live path to bob is relay-only, and the relay host is named.
    let relay_host = lab.url.host_str().map(str::to_owned);
    let mut row = None;
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        let rows = collect_peer_rows(&alice.endpoint, vec![(bob.endpoint.id(), vec![VERSE.to_string()])], false).await;
        if rows[0].kind != PeerConnectionKind::Unknown {
            row = Some(rows[0].clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let row = row.expect("diagnostics classify the live peer");
    assert_eq!(row.kind, PeerConnectionKind::Relayed, "a relay-only pair is Relayed, never Direct or Mixed");
    assert_eq!(row.relay_host, relay_host, "the row names the relay host");
    assert!(row.addrs.is_empty(), "a relay-only peer exposes no IP address");

    // AC-13 / AC-9: the snapshot hint states the downgrade and the hazard-B warning for an unadvertised home relay.
    let unadvertised: BTreeSet<String> = [VERSE.to_string()].into();
    let snapshot = build_snapshot(SnapshotInputs {
        relay_mode: "custom",
        home_relay: relay_host,
        relay_health: RelayHealth::Healthy,
        portmapper: "unknown",
        reachability_verdict: None,
        relay_server_state: "not-built",
        unadvertised_home_verses: &unadvertised,
        peers: vec![row],
    });
    let hint = snapshot.node.downgrade_hint.expect("a relay-only node carries a hint");
    assert!(hint.contains("All peers connect through a relay"), "downgrade hint: {hint}");
    assert!(hint.contains(VERSE) && hint.contains("not advertised"), "hazard-B warning names the verse: {hint}");
    let clean = build_snapshot(SnapshotInputs {
        relay_mode: "custom",
        home_relay: None,
        relay_health: RelayHealth::Healthy,
        portmapper: "unknown",
        reachability_verdict: None,
        relay_server_state: "not-built",
        unadvertised_home_verses: &BTreeSet::new(),
        peers: Vec::new(),
    });
    assert!(clean.node.downgrade_hint.is_none(), "no peers and no unadvertised verse means no hazard-B warning");

    alice_stack.shutdown().await;
    bob_stack.shutdown().await;
    alice.endpoint.close().await;
    bob.endpoint.close().await;
    lab.stop().await;
}

/// AC-4 / AC-5: a non-member is denied by name even with default viewer access; revoke evicts within 5 s and reconnect is denied.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_only_relay_denies_strangers_and_evicts_revoked_members() {
    let lab = RelayLab::start(&[1, 2]).await;

    let kept = RelayOnlyPeer::bind(1, &lab.url).await;
    let revoked = RelayOnlyPeer::bind(2, &lab.url).await;
    assert!(is_admitted(&kept.endpoint).await, "explicit member is admitted");
    assert!(is_admitted(&revoked.endpoint).await, "second explicit member is admitted");

    // Seed 9 stands for a peer who only has default_access Viewer: the snapshot carries explicit members only.
    let default_viewer = RelayOnlyPeer::bind(9, &lab.url).await;
    assert!(is_denied_with(&default_viewer.endpoint, "not_member").await, "default_access is not membership");
    assert_eq!(lab.gate.total_connections(), 2, "a denied connection is never counted");

    let generation_before = lab.gate.generation();
    lab.gate.revoke(&did_of(2));
    assert!(lab.gate.generation() > generation_before, "a revoke bumps the generation");
    let evicted_in = lab.evict_until_gone(&revoked.endpoint.id()).await;
    assert!(evicted_in <= EVICTION_TARGET, "evicted in {evicted_in:?}");
    assert!(is_denied_with(&revoked.endpoint, "not_member").await, "the revoked member's reconnect is denied");
    assert_eq!(lab.gate.tracked_connections(&kept.endpoint.id()), 1, "revoking one member leaves the others connected");

    // A newer snapshot that drops the remaining member evicts it as well.
    lab.gate.apply_snapshot(&snapshot(&[]));
    lab.evict_until_gone(&kept.endpoint.id()).await;
    assert!(is_denied_with(&kept.endpoint, "not_member").await, "a member removed from the snapshot is denied");

    for peer in [kept, revoked, default_viewer] {
        peer.endpoint.close().await;
    }
    lab.stop().await;
}

fn signed_advert(keys: &NodeKeypair, url: &str, issued_at_ms: u64, expires_at_ms: u64) -> RelayAdvertisementRow {
    let mut row = RelayAdvertisementRow {
        v: RELAY_ADVERT_ROW_VERSION,
        url: url.to_string(),
        advertised_by: keys.to_did_key(),
        note: None,
        verse_id: Some(VERSE.to_string()),
        expires_at_ms: Some(expires_at_ms),
        issued_at_ms: Some(issued_at_ms),
        dialback: None,
        sig: None,
    };
    let mut message = RELAY_ADVERT_SIGNING_DOMAIN.to_vec();
    message.extend_from_slice(&canonical_body(&row));
    row.sig = Some(hex::encode(keys.sign(&message).to_bytes()));
    row
}

/// AC-6 / AC-17: refused adverts never reach the relay map; the coverage rule adopts a relay as home eligible and demotes Default only once it is home.
#[test]
fn advert_book_refuses_bad_adverts_and_coverage_rule_adopts_home_relay() {
    const HOUR_MS: u64 = 3_600_000;
    let now = now_ms();
    let manager = NodeKeypair::from_bytes(&[41u8; 32]).expect("manager key");
    let viewer = NodeKeypair::from_bytes(&[42u8; 32]).expect("viewer key");
    let manager_did = manager.to_did_key();
    let role_of = |did: &str, _scope: &str| if did == manager_did { RoleLevel::Manager } else { RoleLevel::Viewer };

    let good_url = "https://good.fe-test.example/";
    let mut book = RelayAdvertisementBook::with_signed_mode(RelayAdvertSignedMode::Require);
    let observe = |book: &mut RelayAdvertisementBook, record: &str, author: &str, row: &RelayAdvertisementRow| {
        book.observe_at(VERSE, record, author, &serde_json::to_vec(row).expect("row serializes"), false, role_of, now)
    };

    // Bad signature: a valid signature over a different url.
    let mut forged = signed_advert(&manager, "https://forged-source.fe-test.example/", now, now + HOUR_MS);
    forged.url = "https://bad-sig.fe-test.example/".to_string();
    // Expired: signed correctly, but its lifetime ended before now.
    let expired = signed_advert(&manager, "https://expired.fe-test.example/", now - 2 * HOUR_MS, now - HOUR_MS);
    // Refused destination: a loopback relay can never be dialled for a peer-advertised relay.
    let loopback = signed_advert(&manager, "https://127.0.0.1/", now, now + HOUR_MS);
    // Unsigned: refused outright under Require.
    let mut unsigned = signed_advert(&manager, "https://unsigned.fe-test.example/", now, now + HOUR_MS);
    unsigned.sig = None;
    // Issued in the future beyond the skew allowance.
    let future = signed_advert(&manager, "https://future.fe-test.example/", now + 10 * HOUR_MS, now + 11 * HOUR_MS);
    for (record, row) in [
        ("bad-sig", &forged),
        ("expired", &expired),
        ("loopback", &loopback),
        ("unsigned", &unsigned),
        ("future", &future),
    ] {
        let changes = observe(&mut book, record, &manager_did, row);
        assert!(changes.is_empty(), "{record} must not be admitted: {changes:?}");
    }
    assert!(book.urls_by_verse().is_empty(), "no refused advert is visible to the relay planner");
    assert_eq!(book.quarantine_len(), 0, "refused adverts are dropped, not quarantined");

    // A well-formed advert from a non-Manager waits in quarantine; it is not adopted either.
    let viewer_did = viewer.to_did_key();
    let from_viewer = signed_advert(&viewer, "https://viewer.fe-test.example/", now, now + HOUR_MS);
    assert!(observe(&mut book, "viewer", &viewer_did, &from_viewer).is_empty());
    assert_eq!(book.quarantine_len(), 1);
    assert!(book.urls_by_verse().is_empty());

    // The planner under a policy that would pin every one of those urls inserts nothing.
    let every_url: Vec<String> = [&forged, &expired, &loopback, &unsigned, &future, &from_viewer]
        .iter()
        .map(|row| row.url.clone())
        .chain([good_url.to_string()])
        .collect();
    let pinned = RelayAdvertisePolicy::Pinned(every_url);
    let mut reconciler = RelayReconciler::new(BTreeSet::new());
    assert!(reconciler.plan(&pinned, &book.urls_by_verse()).is_empty(), "insert_relay is never reached for refused adverts");

    // Control: the Manager's valid advert is admitted and the same pinned plan now inserts exactly it.
    let good = signed_advert(&manager, good_url, now, now + HOUR_MS);
    let changes = observe(&mut book, "good", &manager_did, &good);
    assert_eq!(
        changes,
        vec![AdvertChange::Admitted { verse_id: VERSE.into(), record_id: "good".into(), url: good_url.into() }]
    );
    let advertised = book.urls_by_verse();
    assert_eq!(reconciler.plan(&pinned, &advertised), vec![RelayOp::Insert(good_url.to_string())]);

    // AC-17: coverage adopts the relay as HomeEligible; Default is demoted only once that relay is the connected home.
    let default_relay = "https://default.fe-test.example/".to_string();
    let advertised_by_verse: BTreeMap<String, BTreeSet<String>> =
        BTreeMap::from([(VERSE.to_string(), [good_url.to_string()].into())]);
    let participating: BTreeSet<String> = [VERSE.to_string()].into();
    let coverage = evaluate_coverage(&participating, &advertised_by_verse);
    assert!(coverage.contains(good_url));
    let policy = RelayAdvertisePolicy::DialOnly;
    let mut tiers = RelayReconciler::with_tiers(BTreeSet::new(), [default_relay.clone()].into());
    let plan = |tiers: &mut RelayReconciler, coverage: &BTreeSet<String>, home: Option<&str>, health: RelayHealth| {
        tiers.plan_tiers(&RelayReconcileInputs {
            policy: &policy,
            advertised: &advertised_by_verse,
            home_connected: home.map(str::to_owned),
            coverage: coverage.clone(),
            auto_home: true,
            home_health: health,
        })
    };
    // The covered relay is claimed as home before it is in the map: adopted, but Default stays.
    let first = plan(&mut tiers, &coverage, Some(good_url), RelayHealth::Healthy);
    assert_eq!(first, vec![RelayOp::Insert(good_url.to_string())]);
    assert_eq!(tiers.adoption().get(good_url), Some(&AdoptionMode::HomeEligible));
    assert!(!tiers.defaults_demoted(), "Default is not demoted before the covered relay was in the map");
    // Now the covered relay is the connected, healthy home: Default leaves the map.
    let second = plan(&mut tiers, &coverage, Some(good_url), RelayHealth::Healthy);
    assert_eq!(second, vec![RelayOp::Remove(default_relay.clone())]);
    assert!(tiers.defaults_demoted());
    // Any failure signal restores the Default tier in the same pass.
    let third = plan(&mut tiers, &coverage, Some(good_url), RelayHealth::Degraded);
    assert_eq!(third, vec![RelayOp::Insert(default_relay.clone())]);
    assert!(!tiers.defaults_demoted());
    // An unhealthy or uncovered relay is never adopted as home: joining a verse that does not advertise it empties coverage.
    let participating_two: BTreeSet<String> = [VERSE.to_string(), "01JVERSE0000000000000000BB".to_string()].into();
    let uncovered = evaluate_coverage(&participating_two, &advertised_by_verse);
    assert!(uncovered.is_empty());
    let fourth = plan(&mut tiers, &uncovered, Some(good_url), RelayHealth::Healthy);
    assert_eq!(fourth, vec![RelayOp::Remove(good_url.to_string()), RelayOp::Known(good_url.to_string())]);
    assert_eq!(tiers.adoption().get(good_url), Some(&AdoptionMode::DialOnly));
    assert!(!tiers.defaults_demoted(), "Default stays while no covered relay is home");
}

/// AC-11: over a real relayed gossip topic, unsigned and wrongly signed frames are dropped and a valid one is admitted once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relayed_gossip_drops_unsigned_and_wrongly_signed_frames() {
    let lab = RelayLab::start(&[1, 2]).await;
    let author = RelayOnlyPeer::bind(1, &lab.url).await;
    let receiver = RelayOnlyPeer::bind(2, &lab.url).await;
    assert!(is_admitted(&author.endpoint).await && is_admitted(&receiver.endpoint).await);
    receiver.learn(&author, &lab.url);
    author.learn(&receiver, &lab.url);

    let workdir = TempDir::new().expect("tempdir");
    let author_stack = DocsStack::spawn(author.endpoint.clone(), workdir.path().join("author")).await.expect("author stack");
    let receiver_stack =
        DocsStack::spawn(receiver.endpoint.clone(), workdir.path().join("receiver")).await.expect("receiver stack");
    let topic = iroh_gossip::TopicId::from([7u8; 32]);
    let (sender, _author_events) = author_stack.gossip().subscribe(topic, Vec::new()).await.expect("author joins").split();
    let (_receiver_sender, mut receiver_events) = tokio::time::timeout(
        WAIT,
        receiver_stack.gossip().subscribe_and_join(topic, vec![author.endpoint.id()]),
    )
    .await
    .expect("receiver joins in time")
    .expect("receiver joins through the relay")
    .split();

    let at = now_ms();
    let author_secret = secret_of(1);
    let valid = SignedGossipFrame::signed(&author_secret, GossipFrameKind::ReachProbe, VERSE, at, b"probe".to_vec())
        .expect("valid frame")
        .encode()
        .expect("encodes");
    // Signed by seed 2 but claiming seed 1 as author: the signature cannot verify under the author's key.
    let mut forged =
        SignedGossipFrame::signed(&secret_of(2), GossipFrameKind::ReachProbe, VERSE, at, b"forged".to_vec()).expect("frame");
    forged.author_did = did_of(1);
    let forged = forged.encode().expect("encodes");
    let unsigned = br#"{"type":"compute","note":"unsigned"}"#.to_vec();
    for bytes in [&unsigned, &forged, &valid] {
        sender.broadcast(Bytes::copy_from_slice(bytes)).await.expect("broadcast over the relay");
    }

    let mut received: Vec<Bytes> = Vec::new();
    tokio::time::timeout(WAIT, async {
        while received.len() < 3 {
            match receiver_events.next().await {
                Some(Ok(iroh_gossip::api::Event::Received(message))) => received.push(message.content),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            }
        }
    })
    .await
    .expect("all three frames cross the relay");
    assert_eq!(received.len(), 3, "the relay delivers every frame; dropping is the receiver's job");

    let mut guard = ReplayGuard::new();
    let judge = |bytes: &[u8], mode: SigningMode, guard: &ReplayGuard| open_ingress(bytes, VERSE, at, mode, guard);
    let mut admitted = 0;
    for message in &received {
        match judge(message, SigningMode::Require, &guard) {
            Ok(GossipIngress::Signed(frame)) => {
                assert_eq!(frame.payload, b"probe", "only the validly signed frame is admitted");
                guard.record(&frame);
                admitted += 1;
            }
            Ok(GossipIngress::Legacy) => panic!("require mode must never admit an unsigned frame"),
            Err(reason) => assert!(
                reason == refusal::LEGACY_REFUSED || reason == refusal::BAD_SIGNATURE,
                "dropped for a named reason, got: {reason}"
            ),
        }
    }
    assert_eq!(admitted, 1, "exactly the valid frame passes");
    assert_eq!(judge(&unsigned, SigningMode::Require, &guard), Err(refusal::LEGACY_REFUSED));
    assert_eq!(judge(&forged, SigningMode::Require, &guard), Err(refusal::BAD_SIGNATURE));
    assert_eq!(judge(&valid, SigningMode::Require, &guard), Err(refusal::REPLAYED), "a replayed frame is dropped");
    assert!(matches!(judge(&unsigned, SigningMode::Compat, &guard), Ok(GossipIngress::Legacy)), "compat mode still lets legacy through");
    assert_eq!(judge(&forged, SigningMode::Compat, &guard), Err(refusal::BAD_SIGNATURE), "a signed attempt never falls back to legacy");

    author_stack.shutdown().await;
    receiver_stack.shutdown().await;
    author.endpoint.close().await;
    receiver.endpoint.close().await;
    lab.stop().await;
}
