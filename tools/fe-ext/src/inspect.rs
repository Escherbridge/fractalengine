//! `inspect`: identity, signers, hashes and the consent digest of a sealed package.

use std::io::{Cursor, Read};

use anyhow::{anyhow, bail, Context, Result};
use fe_format::integrity::{self, MANIFEST_PATH, PLUGIN_COUNTERSIGNATURES_PATH, PLUGIN_EXTENSION_PATH};
use fe_format::limits::MAX_JSON_ENTRY_BYTES;
use fe_format::portal_bundle::{
    validate_portal_bundle, BundleFacts, BundleMember, DEFAULT_MAX_BUNDLE_BYTES, WEB_BUNDLE_KIND,
};
use fe_format::{HexonArchive, HexonManifest, IntegrityPolicy, IntegrityStatus};
use serde_json::{json, Value};

use crate::pack::verify_any_signer;

/// The facts `inspect` prints, as JSON (the CLI renders lines or the raw document).
/// Trust is the host's decision, so this names signers and never says "trusted".
pub fn inspect(package_bytes: &[u8]) -> Result<Value> {
    if let Ok(Some(ext_raw)) = HexonArchive::read_member(package_bytes, PLUGIN_EXTENSION_PATH, MAX_JSON_ENTRY_BYTES) {
        if let Ok(doc) = serde_json::from_slice::<Value>(&ext_raw) {
            if doc.get("kind").and_then(Value::as_str) == Some(WEB_BUNDLE_KIND) {
                return inspect_web_bundle(package_bytes, &ext_raw);
            }
        }
    }
    let package = verify_any_signer(package_bytes)?;
    let validated = crate::validate::validate_manifest(&package.extension_json)?;
    Ok(json!({
        "package_id": package.package_id,
        "version": package.version,
        "kind": validated.manifest.kind,
        "publisher_did": package.publisher_did,
        "countersigners": countersigners(package_bytes)?,
        "content_hash": package.content_hash,
        "component_digest": package.component_digest,
        "capability_digest": validated.capability_digest,
        "licensed": package.license_json.is_some(),
    }))
}

/// Inspect and validate a sealed `web-bundle` archive against `fe_format::portal_bundle`.
fn inspect_web_bundle(package_bytes: &[u8], ext_raw: &[u8]) -> Result<Value> {
    let status = fe_format::verify_archive(package_bytes, &IntegrityPolicy::RequireSealed { pin: None })
        .map_err(|error| anyhow!("web-bundle fails the seal check: {error}"))?;
    let IntegrityStatus::Sealed { publisher_did } = status else {
        bail!("web-bundle archive is not sealed");
    };
    let manifest_raw = HexonArchive::read_member(package_bytes, MANIFEST_PATH, MAX_JSON_ENTRY_BYTES)?
        .ok_or_else(|| anyhow!("web-bundle has no {MANIFEST_PATH}"))?;
    let manifest: HexonManifest = serde_json::from_slice(&manifest_raw).context("parsing manifest.json")?;
    if manifest.hexon_type != fe_format::manifest::HexonType::Plugin {
        bail!("web-bundle hexon_type must be plugin, got {:?}", manifest.hexon_type);
    }
    let content_hash = integrity::content_hash(&manifest_raw)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    let mut members = Vec::with_capacity(archive.len());
    let mut has_license = false;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let path = file.name().to_string();
        if path == "license.json" {
            has_license = true;
        }
        members.push(BundleMember { path, uncompressed_size: file.size() });
    }
    let facts = BundleFacts {
        archive_bytes: package_bytes.len() as u64,
        max_bytes: DEFAULT_MAX_BUNDLE_BYTES,
        members: &members,
        integrity_failure: None,
        extension_json: Some(ext_raw),
        hexon_id: &manifest.hexon_id,
        hexon_version: &manifest.version,
    };
    let validated = validate_portal_bundle(&facts, &|origin| origin.starts_with("https://"))
        .map_err(|error| anyhow!("invalid web-bundle: {error}"))?;
    Ok(json!({
        "package_id": validated.package_id,
        "version": validated.version,
        "kind": WEB_BUNDLE_KIND,
        "publisher_did": publisher_did,
        "countersigners": countersigners(package_bytes)?,
        "content_hash": content_hash,
        "capability_digest": validated.capability_digest,
        "entry": validated.portal.entry,
        "licensed": has_license,
    }))
}

/// `key_id`s of `plugin/countersignatures.json` (every one verified by `verify_any_signer`), comma-separated.
fn countersigners(package_bytes: &[u8]) -> Result<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    let Ok(file) = archive.by_name(PLUGIN_COUNTERSIGNATURES_PATH) else {
        return Ok(String::new());
    };
    const MAX_COUNTERSIGNATURES_BYTES: u64 = 65_536;
    let mut raw = Vec::new();
    file.take(MAX_COUNTERSIGNATURES_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_COUNTERSIGNATURES_BYTES {
        bail!("{PLUGIN_COUNTERSIGNATURES_PATH} exceeds {MAX_COUNTERSIGNATURES_BYTES} bytes");
    }
    let doc: Value = serde_json::from_slice(&raw)?;
    let ids: Vec<&str> = doc["entries"].as_array().into_iter().flatten().filter_map(|entry| entry["key_id"].as_str()).collect();
    Ok(ids.join(","))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use ed25519_dalek::SigningKey;
    use fe_format::manifest::HexonType;
    use zip::write::FileOptions;

    use super::*;

    fn sealed_web_bundle(entry_path: &str) -> Vec<u8> {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let did = crate::key::did(&key);
        let manifest = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "com.example.qa-portal".to_string(),
            hexon_type: HexonType::Plugin,
            publisher_did: did,
            publisher_name: None,
            version: "0.1.0".to_string(),
            build_id: None,
            name: "QA Portal".to_string(),
            description: None,
            tags: Vec::new(),
            created_at: "2026-11-17T00:00:00Z".to_string(),
            updated_at: "2026-11-17T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: Vec::new(),
            platforms: Vec::new(),
            address: None,
            integrity: None,
            signature: None,
        };
        let extension = json!({
            "kind": "web-bundle",
            "id": "com.example.qa-portal",
            "version": "0.1.0",
            "portal": {
                "schema": "fe-portal-bundle/1",
                "title": "QA Portal",
                "entry": entry_path,
                "csp": {
                    "connect": [],
                    "img": [],
                    "font": [],
                    "media": [],
                    "frame": [],
                    "wasm": false,
                    "inline_styles": false
                },
                "navigation": [],
                "subscriptions": { "node": true, "readings": false, "queries": [] },
                "commands": {},
                "hooks": [],
                "streams": [],
                "ui_intents": []
            }
        });
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(MANIFEST_PATH, opts).unwrap();
        writer.write_all(&serde_json::to_vec(&manifest).unwrap()).unwrap();
        writer.start_file(PLUGIN_EXTENSION_PATH, opts).unwrap();
        writer.write_all(&serde_json::to_vec(&extension).unwrap()).unwrap();
        writer.start_file("web/index.html", opts).unwrap();
        writer.write_all(b"<!doctype html><html><body>hi</body></html>").unwrap();
        writer.start_file("web/app.js", opts).unwrap();
        writer.write_all(b"console.log('qa');").unwrap();
        let unsealed = writer.finish().unwrap().into_inner();
        fe_format::seal(&unsealed, &key).unwrap()
    }

    #[test]
    fn inspect_accepts_sealed_web_bundle() {
        let bytes = sealed_web_bundle("web/index.html");
        let facts = inspect(&bytes).expect("sealed web-bundle inspects cleanly");
        assert_eq!(facts["package_id"], "com.example.qa-portal");
        assert_eq!(facts["version"], "0.1.0");
        assert_eq!(facts["kind"], "web-bundle");
        assert_eq!(facts["entry"], "web/index.html");
        assert_eq!(facts["licensed"], false);
        assert!(facts["capability_digest"].as_str().is_some_and(|d| d.len() == 64));
    }

    #[test]
    fn inspect_rejects_web_bundle_missing_entry() {
        let bytes = sealed_web_bundle("web/missing.html");
        let err = inspect(&bytes).expect_err("missing entry html must be refused");
        assert!(err.to_string().contains("entry_missing"), "unexpected err: {err}");
    }
}
