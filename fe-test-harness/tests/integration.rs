//! Integration tests for the fractalengine test harness.
//!
//! These wrap the same scenarios as `cargo run` but execute via `cargo test`.

// We need to import the harness crate as a library, but since it's a binary
// crate, we replicate the key test logic here using the same underlying APIs.

use std::sync::Arc;

use fe_database::{hash_to_hex, BlobStoreHandle};
use fe_sync::FsBlobStore;

/// Create a minimal valid GLB file (same as fixtures::create_minimal_glb).
fn create_minimal_glb() -> Vec<u8> {
    let json = br#"{"asset":{"version":"2.0"}}"#;
    let json_padded_len = (json.len() + 3) & !3;
    let total_len = 12 + 8 + json_padded_len;
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(b"glTF");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&(total_len as u32).to_le_bytes());
    buf.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
    buf.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    buf.extend_from_slice(json);
    buf.resize(total_len, 0x20);
    buf
}

#[test]
fn test_blob_store_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let blob_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("blobs")).unwrap());

    let glb_bytes = create_minimal_glb();
    let hash = blob_store.add_blob(&glb_bytes).unwrap();

    // BLAKE3 hash matches
    let expected = *blake3::hash(&glb_bytes).as_bytes();
    assert_eq!(hash, expected);

    // blob:// URL is correct
    let content_hash_hex = hash_to_hex(&hash);
    let asset_path = format!("blob://{}.glb", content_hash_hex);
    assert!(asset_path.starts_with("blob://"));

    // Blob is retrievable
    assert!(blob_store.has_blob(&hash));
    let path = blob_store.get_blob_path(&hash).unwrap();
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk, glb_bytes);

    // Idempotent
    let hash2 = blob_store.add_blob(&glb_bytes).unwrap();
    assert_eq!(hash, hash2);
}

#[test]
fn test_migration_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    let blob_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("blobs")).unwrap());

    let original = b"fake GLB for migration";
    let b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        original,
    );
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &b64,
    )
    .unwrap();

    let hash = blob_store.add_blob(&decoded).unwrap();
    let expected = *blake3::hash(original).as_bytes();
    assert_eq!(hash, expected);
    assert!(blob_store.has_blob(&hash));
}

#[test]
fn test_invite_token_roundtrip() {
    use fe_database::invite::VerseInvite;
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let creator_addr = hex::encode(kp.verifying_key().to_bytes());

    let mut invite = VerseInvite {
        namespace_id: "aabb".into(),
        namespace_secret: Some("secret123".into()),
        creator_node_addr: creator_addr,
        verse_name: "Test Verse".into(),
        verse_id: "verse-001".into(),
        expiry_timestamp: u64::MAX,
        signature: String::new(),
    };
    invite.sign(&kp);

    // Serialization roundtrip
    let s = invite.to_invite_string();
    let parsed = VerseInvite::from_invite_string(&s).unwrap();
    assert_eq!(parsed.verse_id, "verse-001");
    assert_eq!(parsed.verse_name, "Test Verse");
    assert!(parsed.verify(&kp.verifying_key()));

    // Tamper detection
    let mut tampered = parsed.clone();
    tampered.verse_name = "Evil".into();
    assert!(!tampered.verify(&kp.verifying_key()));
}

#[test]
fn test_minimal_glb_structure() {
    let glb = create_minimal_glb();
    assert!(glb.len() >= 20);
    assert_eq!(&glb[0..4], b"glTF");
    let version = u32::from_le_bytes([glb[4], glb[5], glb[6], glb[7]]);
    assert_eq!(version, 2);
    let total_len = u32::from_le_bytes([glb[8], glb[9], glb[10], glb[11]]) as usize;
    assert_eq!(total_len, glb.len());
    assert_eq!(glb.len() % 4, 0);
}

#[test]
fn test_two_peer_blob_exchange() {
    // Create two separate FsBlobStores in separate temp dirs
    let tmp = tempfile::tempdir().unwrap();
    let alice_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("alice_blobs")).unwrap());
    let bob_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("bob_blobs")).unwrap());

    // Alice writes a blob
    let glb_bytes = create_minimal_glb();
    let alice_hash = alice_store.add_blob(&glb_bytes).unwrap();

    // Read bytes from Alice's store, write to Bob's store
    let alice_blob_path = alice_store.get_blob_path(&alice_hash).unwrap();
    let blob_bytes = std::fs::read(&alice_blob_path).unwrap();
    let bob_hash = bob_store.add_blob(&blob_bytes).unwrap();

    // Verify same hash (content-addressable)
    assert_eq!(alice_hash, bob_hash);

    // Verify Bob has the blob
    assert!(bob_store.has_blob(&bob_hash));

    // Verify identical bytes on disk
    let bob_blob_path = bob_store.get_blob_path(&bob_hash).unwrap();
    let bob_bytes = std::fs::read(&bob_blob_path).unwrap();
    assert_eq!(blob_bytes, bob_bytes);

    // Verify same blob:// URL
    let alice_url = format!("blob://{}.glb", hash_to_hex(&alice_hash));
    let bob_url = format!("blob://{}.glb", hash_to_hex(&bob_hash));
    assert_eq!(alice_url, bob_url);
}

#[test]
fn test_two_peer_invite_join() {
    use fe_database::invite::VerseInvite;
    use fe_identity::NodeKeypair;

    // Alice generates an invite
    let alice_kp = NodeKeypair::generate();
    let alice_addr = hex::encode(alice_kp.verifying_key().to_bytes());

    let verse_id = "collab-verse-001";
    let namespace_id = hex::encode(blake3::hash(b"test-ns").as_bytes());

    let mut invite = VerseInvite {
        namespace_id: namespace_id.clone(),
        namespace_secret: Some("ns-secret-hex".into()),
        creator_node_addr: alice_addr,
        verse_name: "Collab Verse".into(),
        verse_id: verse_id.into(),
        expiry_timestamp: u64::MAX,
        signature: String::new(),
    };
    invite.sign(&alice_kp);

    // Serialize and parse (simulating transfer to Bob)
    let invite_string = invite.to_invite_string();
    let parsed = VerseInvite::from_invite_string(&invite_string).unwrap();

    // Bob verifies the invite
    let bob_kp = NodeKeypair::generate();
    let _ = bob_kp; // Bob has his own identity but verifies with Alice's public key
    assert!(parsed.verify(&alice_kp.verifying_key()));
    assert!(!parsed.is_expired());

    // Confirm verse_id matches
    assert_eq!(parsed.verse_id, verse_id);
    assert_eq!(parsed.verse_name, "Collab Verse");
    assert_eq!(parsed.namespace_id, namespace_id);

    // Confirm write cap was included
    assert_eq!(parsed.namespace_secret, Some("ns-secret-hex".into()));
}

#[test]
fn test_two_peer_sync_blobs() {
    // Verifies two separate blob stores can hold the same content-addressed blob
    // after a simulated sync (manual copy)
    let tmp = tempfile::tempdir().unwrap();
    let alice_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("alice_sync_blobs")).unwrap());
    let bob_store: BlobStoreHandle =
        Arc::new(FsBlobStore::new(tmp.path().join("bob_sync_blobs")).unwrap());

    // Alice creates a serialized verse row and stores it as a blob
    let row_data = serde_json::json!({
        "verse_id": "sync-verse-001",
        "name": "Sync Test Verse",
        "created_by": "alice",
    });
    let row_bytes = serde_json::to_vec(&row_data).unwrap();
    let alice_hash = alice_store.add_blob(&row_bytes).unwrap();

    // Simulate network fetch: read from Alice, write to Bob
    let alice_blob_path = alice_store.get_blob_path(&alice_hash).unwrap();
    let fetched_bytes = std::fs::read(&alice_blob_path).unwrap();
    let bob_hash = bob_store.add_blob(&fetched_bytes).unwrap();

    // Both stores have the blob with matching hash
    assert_eq!(alice_hash, bob_hash);
    assert!(alice_store.has_blob(&alice_hash));
    assert!(bob_store.has_blob(&bob_hash));

    // Verify the deserialized content matches
    let bob_blob_path = bob_store.get_blob_path(&bob_hash).unwrap();
    let bob_bytes = std::fs::read(&bob_blob_path).unwrap();
    let bob_row: serde_json::Value = serde_json::from_slice(&bob_bytes).unwrap();
    assert_eq!(bob_row["verse_id"], "sync-verse-001");
    assert_eq!(bob_row["name"], "Sync Test Verse");
}

// ---------------------------------------------------------------------------
// API Token tests
// ---------------------------------------------------------------------------

#[test]
fn test_api_token_mint_and_verify() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let token = mint_api_token(&kp, "VERSE#v1", "editor", 3600, "jti-test-001").unwrap();

    let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
    assert_eq!(claims.sub, kp.to_did_key());
    assert_eq!(claims.token_type, "api");
    assert_eq!(claims.scope, "VERSE#v1");
    assert_eq!(claims.max_role, "editor");
    assert_eq!(claims.jti, "jti-test-001");
    assert!(claims.exp > claims.iat);
    assert_eq!(claims.exp - claims.iat, 3600);
}

#[test]
fn test_api_token_wrong_key_rejected() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let alice = NodeKeypair::generate();
    let bob = NodeKeypair::generate();

    let token = mint_api_token(&alice, "VERSE#v1", "viewer", 3600, "jti-xkey").unwrap();

    // Verify with wrong key fails
    assert!(verify_api_token(&token, &bob.verifying_key()).is_err());
    // Verify with correct key succeeds
    assert!(verify_api_token(&token, &alice.verifying_key()).is_ok());
}

#[test]
fn test_api_token_tampered_rejected() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let mut token = mint_api_token(&kp, "VERSE#v1", "editor", 3600, "jti-tamper").unwrap();

    // Flip the last character
    let last = token.pop().unwrap();
    token.push(if last == 'a' { 'b' } else { 'a' });

    assert!(verify_api_token(&token, &kp.verifying_key()).is_err());
}

#[test]
fn test_api_token_empty_scope_rejected() {
    use fe_identity::api_token::mint_api_token;
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let result = mint_api_token(&kp, "", "viewer", 3600, "jti-empty");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("scope"));
}

#[test]
fn test_api_token_excessive_ttl_rejected() {
    use fe_identity::api_token::{mint_api_token, MAX_API_TOKEN_TTL_SECS};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let result = mint_api_token(&kp, "VERSE#v1", "viewer", MAX_API_TOKEN_TTL_SECS + 1, "jti-ttl");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("30 days"));
}

#[test]
fn test_api_token_max_ttl_accepted() {
    use fe_identity::api_token::{mint_api_token, verify_api_token, MAX_API_TOKEN_TTL_SECS};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let token = mint_api_token(&kp, "VERSE#v1", "viewer", MAX_API_TOKEN_TTL_SECS, "jti-max").unwrap();
    let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
    assert_eq!(claims.exp - claims.iat, MAX_API_TOKEN_TTL_SECS);
}

#[test]
fn test_api_token_session_token_rejected() {
    use fe_identity::api_token::verify_api_token;
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    // Mint a session token (FractalClaims, not ApiClaims)
    let session = fe_identity::jwt::mint_session_token(&kp, "petal-1", "admin", 300).unwrap();

    // Attempt to verify as API token should fail (wrong token_type)
    assert!(verify_api_token(&session, &kp.verifying_key()).is_err());
}

#[test]
fn test_api_token_multiple_scopes() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();

    // Test various scope levels
    for scope in &["VERSE#v1", "VERSE#v1-FRACTAL#f1", "VERSE#v1-FRACTAL#f1-PETAL#p1"] {
        let token = mint_api_token(&kp, scope, "viewer", 3600, &format!("jti-{}", scope)).unwrap();
        let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
        assert_eq!(claims.scope, *scope);
    }
}

#[test]
fn test_api_token_all_roles() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();

    for role in &["viewer", "editor", "manager", "owner"] {
        let token = mint_api_token(&kp, "VERSE#v1", role, 3600, &format!("jti-{}", role)).unwrap();
        let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();
        assert_eq!(claims.max_role, *role);
    }
}

#[test]
fn test_api_token_did_key_format() {
    use fe_identity::api_token::{mint_api_token, verify_api_token};
    use fe_identity::NodeKeypair;

    let kp = NodeKeypair::generate();
    let token = mint_api_token(&kp, "VERSE#v1", "viewer", 3600, "jti-did").unwrap();
    let claims = verify_api_token(&token, &kp.verifying_key()).unwrap();

    // sub should be a valid did:key
    assert!(claims.sub.starts_with("did:key:z6Mk"), "sub should be did:key format, got: {}", claims.sub);
}

#[tokio::test]
async fn test_api_token_store_roundtrip() {
    use fe_database::api_token_store::*;

    // Set up in-memory SurrealDB
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");

    apply_api_token_schema(&db).await.expect("apply schema");

    let record = ApiTokenRecord {
        jti: "test-jti-001".to_string(),
        scope: "VERSE#v1".to_string(),
        max_role: "editor".to_string(),
        label: Some("Test".to_string()),
        sub: "did:key:z6MkTest".to_string(),
        created_at: "2025-01-01T00:00:00Z".to_string(),
        expires_at: "2025-01-02T00:00:00Z".to_string(),
        revoked: false,
    };

    // Store
    store_api_token(&db, &record).await.expect("store");

    // Check not revoked
    let revoked = is_token_revoked(&db, "test-jti-001").await.expect("check revoked");
    assert!(!revoked);

    // List
    let tokens = list_active_tokens(&db, "did:key:z6MkTest").await.expect("list");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].jti, "test-jti-001");
    assert_eq!(tokens[0].scope, "VERSE#v1");

    // Revoke
    let did_revoke = revoke_api_token(&db, "test-jti-001").await.expect("revoke");
    assert!(did_revoke);

    // Check revoked
    let revoked_after = is_token_revoked(&db, "test-jti-001").await.expect("check revoked after");
    assert!(revoked_after);

    // List should be empty now (revoked tokens filtered)
    let tokens_after = list_active_tokens(&db, "did:key:z6MkTest").await.expect("list after");
    assert!(tokens_after.is_empty());
}

#[tokio::test]
async fn test_api_token_store_revoke_nonexistent() {
    use fe_database::api_token_store::*;

    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");

    apply_api_token_schema(&db).await.expect("apply schema");

    // Revoking a non-existent token returns false
    let result = revoke_api_token(&db, "non-existent-jti").await.expect("revoke");
    assert!(!result);
}

#[tokio::test]
async fn test_api_token_store_is_revoked_unknown_token() {
    use fe_database::api_token_store::*;

    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");

    apply_api_token_schema(&db).await.expect("apply schema");

    // Unknown token is treated as non-revoked
    let revoked = is_token_revoked(&db, "totally-unknown").await.expect("check");
    assert!(!revoked);
}

#[tokio::test]
async fn test_api_token_store_list_wrong_sub() {
    use fe_database::api_token_store::*;

    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");

    apply_api_token_schema(&db).await.expect("apply schema");

    let record = ApiTokenRecord {
        jti: "isolation-jti".to_string(),
        scope: "VERSE#v1".to_string(),
        max_role: "viewer".to_string(),
        label: None,
        sub: "did:key:z6MkAlice".to_string(),
        created_at: "2025-01-01T00:00:00Z".to_string(),
        expires_at: "2025-01-02T00:00:00Z".to_string(),
        revoked: false,
    };
    store_api_token(&db, &record).await.expect("store");

    // Listing with a different sub should return empty
    let tokens = list_active_tokens(&db, "did:key:z6MkBob").await.expect("list");
    assert!(tokens.is_empty(), "Bob should not see Alice's tokens");

    // Listing with correct sub should return the token
    let alice_tokens = list_active_tokens(&db, "did:key:z6MkAlice").await.expect("list");
    assert_eq!(alice_tokens.len(), 1);
}

#[tokio::test]
async fn test_api_token_store_multiple_tokens() {
    use fe_database::api_token_store::*;

    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");

    apply_api_token_schema(&db).await.expect("apply schema");

    // Create 5 tokens
    for i in 0..5 {
        let record = ApiTokenRecord {
            jti: format!("multi-jti-{}", i),
            scope: format!("VERSE#v{}", i),
            max_role: "viewer".to_string(),
            label: None,
            sub: "did:key:z6MkMulti".to_string(),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-02T00:00:00Z".to_string(),
            revoked: false,
        };
        store_api_token(&db, &record).await.expect("store");
    }

    let tokens = list_active_tokens(&db, "did:key:z6MkMulti").await.expect("list");
    assert_eq!(tokens.len(), 5);

    // Revoke 2 of them
    revoke_api_token(&db, "multi-jti-1").await.expect("revoke 1");
    revoke_api_token(&db, "multi-jti-3").await.expect("revoke 3");

    let tokens_after = list_active_tokens(&db, "did:key:z6MkMulti").await.expect("list after");
    assert_eq!(tokens_after.len(), 3);

    // Verify the right ones remain
    let remaining_jtis: Vec<&str> = tokens_after.iter().map(|t| t.jti.as_str()).collect();
    assert!(remaining_jtis.contains(&"multi-jti-0"));
    assert!(!remaining_jtis.contains(&"multi-jti-1"));
    assert!(remaining_jtis.contains(&"multi-jti-2"));
    assert!(!remaining_jtis.contains(&"multi-jti-3"));
    assert!(remaining_jtis.contains(&"multi-jti-4"));
}
