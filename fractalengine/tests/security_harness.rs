use ed25519_dalek::{Signer, SigningKey, Verifier};
use rand::rngs::OsRng;

#[test]
fn test_tampered_ed25519_signature() {
    let key = SigningKey::generate(&mut OsRng);
    let msg = b"test message";
    let sig = key.sign(msg);
    let mut bad_sig_bytes = sig.to_bytes();
    bad_sig_bytes[0] ^= 0xff;
    let bad_sig = ed25519_dalek::Signature::from_bytes(&bad_sig_bytes);
    assert!(key.verifying_key().verify_strict(msg, &bad_sig).is_err());
}

#[test]
fn test_ttl_limit_enforced() {
    use fe_identity::jwt::mint_session_token;
    use fe_identity::NodeKeypair;
    let kp = NodeKeypair::generate();
    assert!(mint_session_token(&kp, "petal-1", "admin", 301).is_err());
}

#[test]
fn test_rfc1918_url_blocked() {
    use fe_webview::security::is_url_allowed;
    let blocked = [
        "http://10.0.0.1/",
        "http://172.16.0.1/",
        "http://192.168.1.1/",
        "http://127.0.0.1/",
        "http://localhost/",
    ];
    for url in &blocked {
        assert!(
            !is_url_allowed(&url.parse().unwrap()),
            "should block: {}",
            url
        );
    }
}

#[test]
fn test_oversized_asset_rejected() {
    use fe_renderer::ingester::{AssetIngester, GltfIngester, MAX_ASSET_SIZE_BYTES};
    let big = vec![0u8; MAX_ASSET_SIZE_BYTES + 1];
    assert!(GltfIngester.ingest(&big).is_err());
}

#[test]
fn test_role_cache_invalidated_after_revoke() {
    use fe_auth::cache::SessionCache;
    use fe_database::RoleId;
    let mut cache = SessionCache::new();
    let key = [42u8; 32];
    cache.insert(key, RoleId("admin".to_string()));
    assert!(cache.get(&key).is_some());
    cache.revoke(&key);
    assert!(cache.get(&key).is_none());
}
