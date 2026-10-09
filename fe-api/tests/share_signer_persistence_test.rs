//! Integration tests for A24/DEC-C9: the share-URL signing key must survive
//! a process restart. Two independently constructed `ApiHarness` instances
//! sharing one `share_signer` simulate "mint before restart, redeem after" —
//! see `fe-api/AGENTS.md` §share ("Key lifetime — CLOSED").

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fe_identity::NodeKeypair;
use fractalengine_test_harness::api::{body_bytes, ApiHarness};

fn mint_request(token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/query/share")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({
                "sql": "SELECT * FROM node",
                "format": "json",
                "ttl_secs": 3600,
            })
            .to_string(),
        ))
        .expect("build mint request")
}

fn redeem_request(share_token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/v1/shared/{share_token}"))
        .body(Body::empty())
        .expect("build redeem request")
}

async fn mint_share_token(h: &ApiHarness) -> String {
    let bearer = h.mint_token("VERSE#v1-FRACTAL#f1-PETAL#p1", "viewer");
    let resp = h.request(mint_request(&bearer)).await;
    assert_eq!(resp.status(), StatusCode::OK, "mint must succeed");
    let body: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    body["data"]["token"]
        .as_str()
        .expect("minted token")
        .to_string()
}

/// The persistence case: a key shared across two `ApiState`s (standing in
/// for "same secret-store slot, two process launches") lets instance 2
/// verify a token instance 1 minted.
#[tokio::test]
async fn token_minted_on_one_instance_redeems_on_another_sharing_the_key() {
    let shared_signer = Arc::new(NodeKeypair::generate());

    let h1 = ApiHarness::spawn_with_share_signer(shared_signer.clone())
        .await
        .expect("spawn harness 1");
    let h2 = ApiHarness::spawn_with_share_signer(shared_signer)
        .await
        .expect("spawn harness 2 (simulated restart)");

    let share_token = mint_share_token(&h1).await;

    // Redeemed on a DIFFERENT ApiState — the only thing h1 and h2 share is
    // the share_signer keypair, exactly as a restart would preserve it via
    // the secret store.
    let resp = h2.request(redeem_request(&share_token)).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a shared signer must let instance 2 verify instance 1's token"
    );
}

/// The negative case: `verify_strict` actually binds to the signing key, not
/// just the token's shape — a token minted under a DIFFERENT key must still
/// be rejected even though the format is otherwise identical.
#[tokio::test]
async fn token_minted_under_a_different_keypair_is_rejected() {
    let h1 = ApiHarness::spawn_with_share_signer(Arc::new(NodeKeypair::generate()))
        .await
        .expect("spawn harness 1");
    let h2 = ApiHarness::spawn_with_share_signer(Arc::new(NodeKeypair::generate()))
        .await
        .expect("spawn harness 2 (independent key)");

    let share_token = mint_share_token(&h1).await;

    let resp = h2.request(redeem_request(&share_token)).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a token signed by a different key must not verify"
    );
}
