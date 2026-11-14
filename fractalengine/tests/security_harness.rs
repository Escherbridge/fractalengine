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
    use fe_format::integrity::{IntegrityError, MANIFEST_PATH, PLUGIN_COMPONENT_PATH};
    use fe_format::{verify_archive, IntegrityPolicy, IntegrityStatus};
    use fe_hexon::package::{verify_plugin_package, PluginTrust};
    use fe_hexon::publisher::PluginHexonBuilder;
    use std::io::{Cursor, Read, Write};

    // A real manifest v2 sealed by the production builder (what `fe-ext pack` and a registry ship).
    let key = SigningKey::generate(&mut OsRng);
    let publisher = fe_format::verifying_key_to_did(&key.verifying_key());
    let is_publisher = |did: &str| did == publisher;
    let trust = PluginTrust { is_trust_root: &is_publisher, allow_unsigned_dev: false };
    let extension_json = include_bytes!("../../fe-sdk/schema/examples/bi-push.extension.json");
    let package_id = serde_json::from_slice::<serde_json::Value>(extension_json).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Admission never compiles the component (the plugin host's job) but checks the component preamble.
    let stub_component = [&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00][..], b"stub"].concat();
    let sealed = PluginHexonBuilder::new(extension_json.to_vec(), stub_component)
        .build(&key)
        .expect("plugin seals");
    let strict = IntegrityPolicy::RequireSealed { pin: None };
    assert!(matches!(verify_archive(&sealed, &strict), Ok(IntegrityStatus::Sealed { .. })));
    assert!(
        verify_plugin_package(&sealed, Some(&package_id), &trust).is_ok(),
        "the untampered package passes the install verifier"
    );

    // An attacker's re-zip: well-formed, valid CRCs, one member replaced (so only the seal can catch it).
    let rewrite = |target: &str, edit: &dyn Fn(Vec<u8>) -> Vec<u8>| -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(Cursor::new(sealed.as_slice())).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let name = file.name().to_string();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            out.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(&if name == target { edit(bytes) } else { bytes }).unwrap();
        }
        out.finish().unwrap().into_inner()
    };

    // Flipped manifest signature: refused by the seal and by the install verifier, never a silent pass.
    let bad_signature = rewrite(MANIFEST_PATH, &|bytes| {
        let mut doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let signature = doc["signature"].as_str().unwrap().to_string();
        let flipped = if signature.starts_with('A') { 'B' } else { 'A' };
        doc["signature"] = format!("{flipped}{}", &signature[1..]).into();
        serde_json::to_vec(&doc).unwrap()
    });
    assert!(verify_archive(&bad_signature, &strict).is_err());
    assert!(verify_plugin_package(&bad_signature, Some(&package_id), &trust).is_err());

    // Swapped component: the signed members list commits its digest, so the swap is named precisely.
    let swapped = rewrite(PLUGIN_COMPONENT_PATH, &|_| b"\0asm-evil-component".to_vec());
    assert_eq!(
        verify_archive(&swapped, &strict),
        Err(IntegrityError::DigestMismatch { path: PLUGIN_COMPONENT_PATH.into() })
    );
    assert!(verify_plugin_package(&swapped, Some(&package_id), &trust).is_err());
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
