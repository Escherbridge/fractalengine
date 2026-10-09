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

    // 2. Revoke: insert into the SAME cache `auth_middleware` reads
    // (fe-api/src/auth.rs:31-32). In a running process this cache is fed by
    // a channel from the DB thread's `RevokeApiToken` handler
    // (fe-api/src/lib.rs::run_server) — this harness is router-only (no DB
    // thread), so there is no such channel to drive here. Writing the cache
    // directly still exercises the real enforcement point `auth_middleware`
    // checks on every request; the DB-thread revocation handler itself is a
    // separate, already-covered seam (fe-database's own tests mint/revoke
    // through `api_token_store`).
    h.state
        .revoked_jtis
        .write()
        .await
        .insert(claims.jti.clone());

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
}
