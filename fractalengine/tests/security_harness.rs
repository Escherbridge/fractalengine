use ed25519_dalek::{Signer, SigningKey};
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
fn test_tampered_sealed_plugin_rejected() {
    use fe_format::integrity::{seal, verify_archive, IntegrityError, IntegrityPolicy, IntegrityStatus};
    use std::io::{Cursor, Read, Write};

    let key = SigningKey::generate(&mut OsRng);
    let manifest = serde_json::json!({
        "schema_version": "1.0.0", "hexon_id": "org.example.sealed", "hexon_type": "plugin",
        "publisher_did": "did:key:z6Mkplaceholder", "version": "0.1.0", "name": "Sealed",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    });
    let build = |members: &[(&str, Vec<u8>)]| -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in members {
            out.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(bytes).unwrap();
        }
        out.finish().unwrap().into_inner()
    };
    let unsealed = build(&[
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("plugin/extension.json", br#"{"id":"org.example.sealed"}"#.to_vec()),
        ("plugin/component.wasm", b"wasm-bytes".to_vec()),
    ]);
    let sealed = seal(&unsealed, &key).expect("plugin seals");
    let strict = IntegrityPolicy::RequireSealed { pin: None };
    assert!(matches!(
        verify_archive(&sealed, &strict),
        Ok(IntegrityStatus::Sealed { .. })
    ));

    // Rewrite the sealed archive with one member replaced.
    let rewrite = |name: &str, edit: &dyn Fn(Vec<u8>) -> Vec<u8>| -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(Cursor::new(sealed.clone())).unwrap();
        let mut members: Vec<(String, Vec<u8>)> = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            let member = file.name().to_string();
            members.push((member.clone(), if member == name { edit(bytes) } else { bytes }));
        }
        let borrowed: Vec<(&str, Vec<u8>)> =
            members.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
        build(&borrowed)
    };

    // Flipped manifest signature: bad signature, never a silent pass.
    let bad_signature = rewrite("manifest.json", &|bytes| {
        let mut doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let signature = doc["signature"].as_str().unwrap().to_string();
        let flipped = if signature.starts_with('A') { 'B' } else { 'A' };
        doc["signature"] = format!("{flipped}{}", &signature[1..]).into();
        serde_json::to_vec(&doc).unwrap()
    });
    assert!(verify_archive(&bad_signature, &strict).is_err());

    // Swapped component bytes: digest mismatch on the committed body member.
    let swapped = rewrite("plugin/component.wasm", &|_| b"evil-wasm".to_vec());
    assert_eq!(
        verify_archive(&swapped, &strict),
        Err(IntegrityError::DigestMismatch { path: "plugin/component.wasm".into() })
    );
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
    use fe_database::session_cache::SessionCache;
    use fe_database::RoleId;
    let mut cache = SessionCache::new();
    let key = [42u8; 32];
    cache.insert(key, RoleId("admin".to_string()));
    assert!(cache.get(&key).is_some());
    cache.revoke(&key);
    assert!(cache.get(&key).is_none());
}
