//! `validate`: the R3 manifest table plus the host ceilings, then the component admission checks.

use anyhow::{bail, Result};
use fe_hexon::package::PLUGIN_EXTENSION_MAX_BYTES;
use fe_plugin::wasm::component::ComponentHost;
use fe_sdk::manifest_v2::{parse_and_validate, ExtensionManifest, ValidatedManifest};

/// Manifest bytes through the same entry REST install uses, plus the header size ceiling.
pub fn validate_manifest(bytes: &[u8]) -> Result<ValidatedManifest> {
    if bytes.len() as u64 > PLUGIN_EXTENSION_MAX_BYTES {
        bail!("over-host-ceiling: extension.json is {} bytes, the header ceiling is {PLUGIN_EXTENSION_MAX_BYTES}", bytes.len());
    }
    parse_and_validate(bytes).map_err(|error| anyhow::anyhow!("{error}"))
}

/// Component bytes through the host's install admission (imports, exports, pre-link).
pub fn validate_component(bytes: &[u8]) -> Result<String> {
    let host = ComponentHost::new().map_err(|error| anyhow::anyhow!("{error}"))?;
    let component = host.compile(bytes).map_err(|error| anyhow::anyhow!("{} ({error})", error.code()))?;
    Ok(component.digest().to_string())
}

/// Component admission plus `check_provides`: a manifest that declares `provides` needs the hooks export (`missing-export`).
pub fn validate_component_for(bytes: &[u8], manifest: &ExtensionManifest) -> Result<String> {
    let host = ComponentHost::new().map_err(|error| anyhow::anyhow!("{error}"))?;
    let component = host.compile(bytes).map_err(|error| anyhow::anyhow!("{} ({error})", error.code()))?;
    component.check_provides(manifest).map_err(|error| anyhow::anyhow!("{} ({error})", error.code()))?;
    Ok(component.digest().to_string())
}
