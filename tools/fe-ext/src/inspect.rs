//! `inspect`: identity, signers, hashes and the consent digest of a sealed package.

use std::io::{Cursor, Read};

use anyhow::Result;
use fe_format::integrity::PLUGIN_COUNTERSIGNATURES_PATH;
use serde_json::{json, Value};

use crate::pack::verify_any_signer;

/// The facts `inspect` prints, as JSON (the CLI renders lines or the raw document).
/// Trust is the host's decision, so this names signers and never says "trusted".
pub fn inspect(package_bytes: &[u8]) -> Result<Value> {
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

/// `key_id`s of `plugin/countersignatures.json` (every one verified by `verify_any_signer`), comma-separated.
fn countersigners(package_bytes: &[u8]) -> Result<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    let Ok(mut file) = archive.by_name(PLUGIN_COUNTERSIGNATURES_PATH) else {
        return Ok(String::new());
    };
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    let doc: Value = serde_json::from_slice(&raw)?;
    let ids: Vec<&str> = doc["entries"].as_array().into_iter().flatten().filter_map(|entry| entry["key_id"].as_str()).collect();
    Ok(ids.join(","))
}
