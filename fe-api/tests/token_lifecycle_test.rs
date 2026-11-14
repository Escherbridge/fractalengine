//! Token lifecycle through the LIVE auth middleware (F14/T3).
//!
//! `fe-identity`'s own unit tests cover JWT encode/decode in isolation, and
//! `fe-database::session_cache` has its own store-layer revocation coverage,
//! but nothing previously drove mint → authed request → revoke → denial, or
//! expiry, through `auth_middleware` running over the real router (the gap
//! the M5 recon flagged). Both tests here dispatch every step via
//! `ApiHarness::request`/`get` — the same `tower::oneshot` path every other
//! fe-api integration test uses — so the exact production middleware
//! (`fe-api/src/auth.rs::auth_middleware`) is what accepts or rejects.

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;

#[tokio::test]
async fn minted_token_authenticates_then_is_revoked_live() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let token = h.mint_token("VERSE#v1", "viewer");

    // 1. A freshly minted token authenticates over the real middleware.
    let (status, _) = h.get("/api/v1/hierarchy", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "fresh token must authenticate");

    // Decode to learn its jti — a revocation caller (the real session-cache
    // revocation handler, or this test) always starts from the token itself.
    let claims = fe_identity::api_token::verify_api_token(&token, &h.keypair.verifying_key())
        .expect("decode minted token");

    let record = fe_database::api_token_store::ApiTokenRecord {
        jti: claims.jti.clone(),
        scope: claims.scope.clone(),
        max_role: claims.max_role.clone(),
        label: None,
        sub: claims.sub.clone(),
        created_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        revoked: false,
        client: None,
    };
    fe_database::api_token_store::store_api_token(&h.db, &record)
        .await
        .unwrap();
    let (mut running, notifications, address) = start_owned_api(&h).await;
    assert!(live_get(address, &token).await.starts_with("HTTP/1.1 200"));
    assert!(
        fe_database::api_token_store::revoke_api_token(&h.db, &claims.jti, &claims.sub)
            .await
            .unwrap()
    );
    assert!(
        h.state.revoked_jtis.read().await.is_empty(),
        "Persisted enforcement must not depend on a live cache hint"
    );
    assert!(live_get(address, &token).await.starts_with("HTTP/1.1 401"));
    stop_owned_api(&mut running).await;
    drop(notifications);
    let (mut restarted, notifications, address) = start_owned_api(&h).await;
    let rejected = live_get(address, &token).await;
    assert!(
        rejected.starts_with("HTTP/1.1 401") && !rejected.contains(&token),
        "A fresh API runtime must retain durable revocation without exposing credentials"
    );
    stop_owned_api(&mut restarted).await;
    drop(notifications);

    // 3. The identical token is now rejected live, not merely by a unit
    // check against the store layer.
    let (status, _) = h.get("/api/v1/hierarchy", Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked token must be denied by the live middleware"
    );

    // Revocation is per-jti: a second, never-revoked token for the SAME
    // scope must still work — a bug here would blanket-deny the scope
    // instead of the one revoked token.
    let other = h.mint_token("VERSE#v1", "viewer");
    let (status, _) = h.get("/api/v1/hierarchy", Some(&other)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "revoking one jti must not deny a sibling token for the same scope"
    );

    let (notifications, receiver) = tokio::sync::broadcast::channel(1);
    notifications.send("missed-hint".into()).unwrap();
    notifications.send("observed-hint".into()).unwrap();
    let listener = tokio::spawn(fe_api::auth::listen_for_revocations(
        h.state.clone(),
        receiver,
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !h.state.revoked_jtis.read().await.contains("observed-hint") {
        assert!(
            std::time::Instant::now() < deadline,
            "Lagged auth feed must recover against durable authority"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&token)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&other)).await.0,
        StatusCode::OK
    );
    drop(notifications);
    tokio::time::timeout(std::time::Duration::from_secs(5), listener)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&other)).await.0,
        StatusCode::OK,
        "Closed feed uses durable per-request enforcement"
    );

    let generation = fe_database::api_auth::current_generation(&h.db)
        .await
        .unwrap();
    h.db.query("DELETE api_auth_state:current")
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&other)).await.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "Missing authority cannot downgrade to standalone verification"
    );
    assert!(
        fe_database::api_auth::apply_schema(&h.db).await.is_err(),
        "An existing missing singleton must not be recreated as legacy"
    );
    let (feed, receiver) = tokio::sync::broadcast::channel(1);
    let (listening, unavailable) = crossbeam::channel::bounded(1);
    let mut refused = fe_api::spawn_api_service(owned_api_config(&h, receiver, listening));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !refused.try_join().unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "Unavailable durable authority must fail startup without binding"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(matches!(
        unavailable.try_recv(),
        Err(crossbeam::channel::TryRecvError::Disconnected)
    ));
    drop(feed);
    h.db.query("CREATE api_auth_state:current SET generation=$generation")
        .bind(("generation", generation))
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&other)).await.0,
        StatusCode::OK
    );

    let owned = h.seed_hierarchy().await.unwrap();
    let old_owner = h.mint_token(&owned.verse_scope(), "owner");
    let permit = fe_database::admin::DevelopmentResetPermit::capture_for_offline_store();
    fe_database::admin::clear_for_development_reset(&h.db, &h.did(), &permit)
        .await
        .unwrap();
    let (status, _) = h
        .post_json(
            "/api/v1/verses",
            Some(&old_owner),
            &serde_json::json!({"name":"must-not-create"}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "An old Owner cannot create a fresh Verse after reset"
    );
    assert!(
        h.api_command_receiver().try_recv().is_err(),
        "Denied old Owner cannot enqueue an unscoped mutation"
    );
    let current = fe_database::api_auth::current_generation(&h.db)
        .await
        .unwrap();
    assert!(fe_database::api_auth::accepts_jti(
        &current,
        &fe_database::api_auth::new_jti(&current).unwrap()
    ));
    for malformed in [
        "fe1.bad.bad",
        "fe1.legacy.not-a-nonce",
        "",
        "fe1.legacy.00000000000000000000000000.extra",
    ] {
        assert!(!fe_database::api_auth::accepts_jti(&current, malformed));
    }
    let fresh = fe_identity::mint_api_token(
        &h.keypair,
        "VERSE#after-reset",
        "viewer",
        3600,
        &fe_database::api_auth::new_jti(&current).unwrap(),
    )
    .unwrap();
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(&fresh)).await.0,
        StatusCode::OK,
        "A current-generation unknown JTI is valid"
    );
    for sql in [
        "SELECT * FROM api_auth_state",
        "SELECT api_auth_state:current AS credential_generation FROM node",
    ] {
        assert!(fe_query::sql_guard::prepare_scoped_sql(
            sql,
            &[],
            fe_query::sql_guard::GuardMode::Query
        )
        .is_err());
        assert!(fe_query::sql_guard::validate_elevated_sql(
            sql,
            fe_query::sql_guard::ELEVATED_TABLES
        )
        .is_err());
    }
    let rebuilt = h.seed_hierarchy().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("tokens.json");
    let client = fe_identity::new_client_did();
    fe_api::dev_token::emit_dev_tokens(
        &output,
        "127.0.0.1:8765".parse().unwrap(),
        &h.db,
        &h.keypair,
        &client,
        std::slice::from_ref(&rebuilt.verse_id),
    )
    .await
    .unwrap();
    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    let dev = published["tokens"][&rebuilt.verse_id].as_str().unwrap();
    assert_eq!(
        h.get("/api/v1/hierarchy", Some(dev)).await.0,
        StatusCode::OK,
        "Fresh protected dev bootstrap remains valid without a token metadata row"
    );
    let dev_claims = fe_identity::verify_api_token(dev, &h.keypair.verifying_key()).unwrap();
    let mut absent =
        h.db.query("SELECT jti FROM api_token WHERE jti=$jti")
            .bind(("jti", dev_claims.jti))
            .await
            .unwrap()
            .check()
            .unwrap();
    assert!(absent.take::<Vec<serde_json::Value>>(0).unwrap().is_empty());
}

#[tokio::test]
async fn short_ttl_token_authenticates_then_expires_live() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();

    // A token minted to expire one second from "now" is still valid right
    // now — proves the expiry path doesn't false-positive on a live token.
    let fresh = fe_identity::api_token::mint_api_token_at(
        &h.keypair,
        "VERSE#v1",
        "viewer",
        now,
        now + 1,
        &ulid::Ulid::new().to_string(),
    )
    .expect("mint short-ttl token");
    let (status, _) = h.get("/api/v1/hierarchy", Some(&fresh)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a token that has not yet expired must authenticate"
    );

    // An `exp` 120s in the past is comfortably past `jsonwebtoken`'s default
    // 60s validation leeway (confirmed in jsonwebtoken::validation — a
    // `ttl_secs` of 0/1 plus a short sleep would NOT reliably trip this,
    // since the leeway tolerates up to 60s of clock skew around `exp`).
    let expired = fe_identity::api_token::mint_api_token_at(
        &h.keypair,
        "VERSE#v1",
        "viewer",
        now - 200,
        now - 120,
        &ulid::Ulid::new().to_string(),
    )
    .expect("mint expired token");
    let (status, _) = h.get("/api/v1/hierarchy", Some(&expired)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an expired token must be denied by the live middleware"
    );
    let held_read = h.state.revoked_jtis.write().await;
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let deadline = started + 2;
    let delayed_token = fe_identity::api_token::mint_api_token_at(
        &h.keypair,
        "VERSE#v1",
        "viewer",
        started,
        deadline,
        "delayed-credential-expiry",
    )
    .unwrap();
    let delayed = fe_api::auth::authenticate(&h.state, &delayed_token);
    tokio::pin!(delayed);
    std::future::poll_fn(
        |context| match std::future::Future::poll(delayed.as_mut(), context) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(result) => {
                panic!("Credential read must wait on the held authority lock: {result:?}")
            }
        },
    )
    .await;
    let expires_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(deadline);
    tokio::time::sleep(
        expires_at
            .duration_since(std::time::SystemTime::now())
            .expect("credential starts unexpired"),
    )
    .await;
    drop(held_read);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), delayed).await.unwrap().unwrap_err(),
        StatusCode::UNAUTHORIZED,
        "A credential expiring while its authority read waits must not release a protected response"
    );
    let standalone = ApiHarness::spawn_elevated_without_db_reader()
        .await
        .unwrap();
    let standalone_token = standalone.mint_token("VERSE#standalone", "viewer");
    assert!(
        fe_api::auth::authenticate(&standalone.state, &standalone_token)
            .await
            .is_ok(),
        "Explicit DB-less hosts retain standalone signature authentication"
    );
    let (notifications, receiver) = tokio::sync::broadcast::channel(1);
    notifications.send("lost-one".into()).unwrap();
    notifications.send("lost-two".into()).unwrap();
    let held_authority = standalone.state.revoked_jtis.write().await;
    let pending_admission = fe_api::auth::authenticate(&standalone.state, &standalone_token);
    tokio::pin!(pending_admission);
    std::future::poll_fn(
        |context| match std::future::Future::poll(pending_admission.as_mut(), context) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(result) => {
                panic!("Initial authority read must wait on the held lock: {result:?}")
            }
        },
    )
    .await;
    let pending_feed_loss =
        fe_api::auth::listen_for_revocations(standalone.state.clone(), receiver);
    tokio::pin!(pending_feed_loss);
    std::future::poll_fn(
        |context| match std::future::Future::poll(pending_feed_loss.as_mut(), context) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(()) => {
                panic!("Feed-loss write must queue behind the initial authority read")
            }
        },
    )
    .await;
    drop(held_authority);
    // Tokio's FIFO lock places this final read behind the queued feed-loss writer.
    std::future::poll_fn(
        |context| match std::future::Future::poll(pending_admission.as_mut(), context) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(result) => {
                panic!("Final authority read must wait for feed-loss publication: {result:?}")
            }
        },
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), pending_feed_loss)
        .await
        .expect("queued feed-loss listener completes");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), pending_admission)
            .await
            .unwrap()
            .unwrap_err(),
        StatusCode::SERVICE_UNAVAILABLE,
        "Feed loss while waiting for the final authority read must deny standalone admission"
    );
    assert_eq!(
        fe_api::auth::authenticate(&standalone.state, &standalone_token)
            .await
            .unwrap_err(),
        StatusCode::SERVICE_UNAVAILABLE,
        "A standalone host cannot recover unknown lost revocations"
    );
}

async fn start_owned_api(
    h: &ApiHarness,
) -> (
    fe_runtime::service_lifecycle::OwnedService,
    tokio::sync::broadcast::Sender<String>,
    std::net::SocketAddr,
) {
    let (notifications, revocation_rx) = tokio::sync::broadcast::channel(4);
    let (listening_tx, listening_rx) = crossbeam::channel::bounded(1);
    let service = fe_api::spawn_api_service(owned_api_config(h, revocation_rx, listening_tx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let address = loop {
        if let Ok(address) = listening_rx.try_recv() {
            break address;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Owned API startup must finish"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    (service, notifications, address)
}

async fn stop_owned_api(service: &mut fe_runtime::service_lifecycle::OwnedService) {
    service.request_stop();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !service.try_join().unwrap() {
        assert!(std::time::Instant::now() < deadline, "Owned API must join");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn live_get(address: std::net::SocketAddr, token: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(format!("GET /api/v1/hierarchy HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn owned_api_config(
    h: &ApiHarness,
    revocation_rx: tokio::sync::broadcast::Receiver<String>,
    listening_tx: crossbeam::channel::Sender<std::net::SocketAddr>,
) -> fe_api::ApiConfig {
    fe_api::ApiConfig {
        bind_addr: "127.0.0.1:0".into(),
        api_cmd_tx: h.state.api_cmd_tx.clone(),
        transform_broadcast_tx: h.state.transform_broadcast_tx.clone(),
        entity_change_tx: h.state.entity_change_tx.clone(),
        verifying_key: h.keypair.verifying_key(),
        revocation_rx,
        blob_store: None,
        cors_origins: None,
        api_db_reader: h.state.db_reader.clone(),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: h.state.share_signer.clone(),
        elevated_query_allowed: false,
        upload_quota_bytes: fe_api::upload::DEFAULT_UPLOAD_QUOTA_BYTES,
        screenshot: fe_api::screenshot::ScreenshotSurface::disabled(),
        input: fe_api::input::InputSurface::disabled(),
        listening_tx: Some(listening_tx),
        extension_host: None,
        binding_host: None,
        import_hooks: None,
        network_ops: Default::default(),
    }
}
