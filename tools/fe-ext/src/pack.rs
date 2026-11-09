//! `pack`, `sign` and `countersign`: sealed plugin hexon bytes in, sealed plugin hexon bytes out.

use std::io::{Cursor, Write};

use anyhow::{anyhow, bail, Result};
use ed25519_dalek::SigningKey;
use fe_format::integrity::PLUGIN_COUNTERSIGNATURES_PATH;
use fe_hexon::package::{verify_plugin_package, PluginPackage, PluginTrust, PLUGIN_COUNTERSIGNATURES_MAX_ENTRIES};
use fe_hexon::publisher::PluginHexonBuilder;

use crate::{key, validate};

/// Seal `extension_json` + `component` under `signing_key` and verify the result end to end.
pub fn pack(extension_json: &[u8], component: &[u8], signing_key: &SigningKey, publisher_name: Option<&str>) -> Result<(Vec<u8>, PluginPackage)> {
    validate::validate_manifest(extension_json)?;
    validate::validate_component(component)?;
    let mut builder = PluginHexonBuilder::new(extension_json, component);
    if let Some(name) = publisher_name {
        builder = builder.publisher_name(name);
    }
    let bytes = builder.build(signing_key).map_err(|error| anyhow!("sealing failed: {error}"))?;
    let package = verify_any_signer(&bytes)?;
    Ok((bytes, package))
}

/// Verify seal, closed member set and identity, accepting any signer (trust is the host's decision).
pub fn verify_any_signer(bytes: &[u8]) -> Result<PluginPackage> {
    let trust = PluginTrust { is_trust_root: &|_| true, allow_unsigned_dev: false };
    verify_plugin_package(bytes, None, &trust).map_err(|error| anyhow!("{error}"))
}

/// Re-seal an existing package's members under a different publisher key.
pub fn resign(package_bytes: &[u8], signing_key: &SigningKey) -> Result<(Vec<u8>, PluginPackage)> {
    let existing = verify_any_signer(package_bytes)?;
    if existing.license_json.is_some() {
        bail!("licensed packages cannot be re-signed here: the component is encrypted to the original publisher");
    }
    pack(&existing.extension_json, &existing.component, signing_key, None)
}

/// Add (or replace) a counter-signature without touching any committed member.
pub fn countersign(package_bytes: &[u8], signing_key: &SigningKey) -> Result<Vec<u8>> {
    verify_any_signer(package_bytes)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    let manifest_raw = {
        use std::io::Read;
        let mut raw = Vec::new();
        archive.by_name(fe_format::integrity::MANIFEST_PATH)?.read_to_end(&mut raw)?;
        raw
    };
    let content_hash = fe_format::integrity::content_hash(&manifest_raw)?;
    let signature = fe_format::sign_manifest(&manifest_raw, signing_key)?;
    let key_id = key::did(signing_key);

    let mut entries: Vec<serde_json::Value> = match archive.by_name(PLUGIN_COUNTERSIGNATURES_PATH) {
        Ok(mut file) => {
            use std::io::Read;
            let mut raw = Vec::new();
            file.read_to_end(&mut raw)?;
            let doc: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| anyhow!("existing countersignatures.json is malformed"))?;
            doc.get("entries").and_then(|entries| entries.as_array().cloned()).unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };
    entries.retain(|entry| entry.get("key_id").and_then(|id| id.as_str()) != Some(key_id.as_str()));
    entries.push(serde_json::json!({ "key_id": key_id, "signature": signature }));
    if entries.len() > PLUGIN_COUNTERSIGNATURES_MAX_ENTRIES {
        bail!("a package carries at most {PLUGIN_COUNTERSIGNATURES_MAX_ENTRIES} counter-signatures");
    }
    let doc = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "content_hash": content_hash,
        "entries": entries,
    }))?;

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index)?;
        if file.name() == PLUGIN_COUNTERSIGNATURES_PATH {
            continue;
        }
        writer.raw_copy_file(file)?;
    }
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer.start_file(PLUGIN_COUNTERSIGNATURES_PATH, options)?;
    writer.write_all(&doc)?;
    let out = writer.finish()?.into_inner();
    verify_any_signer(&out)?;
    Ok(out)
}
