//! `validate` for an unsealed portal (`web-bundle`) project; see `tools/fe-ext/AGENTS.md` §portal-validate.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use fe_format::portal_bundle::{
    validate_portal_bundle, BundleFacts, BundleMember, ValidatedBundle, DEFAULT_MAX_BUNDLE_BYTES, WEB_BUNDLE_KIND,
};
use serde_json::{json, Value};

/// The origin rule `inspect` applies to a sealed bundle's `csp`/`navigation` origins.
pub fn origin_allowed(origin: &str) -> bool {
    origin.starts_with("https://")
}

/// Which source file describes the portal: `extension.json` (kind `web-bundle`) or the QA layout's section-only `portal.json`.
pub enum PortalSource {
    Extension(PathBuf),
    SectionOnly(PathBuf),
}

/// `Some` when `dir` is a portal project: an `extension.json` with `kind: "web-bundle"`, or (no `extension.json`) a `portal.json`.
pub fn detect(dir: &Path) -> Option<PortalSource> {
    let extension = dir.join("extension.json");
    if extension.is_file() {
        let document = std::fs::read(&extension).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())?;
        return (document.get("kind").and_then(Value::as_str) == Some(WEB_BUNDLE_KIND)).then_some(PortalSource::Extension(extension));
    }
    let section = dir.join("portal.json");
    section.is_file().then_some(PortalSource::SectionOnly(section))
}

/// Run the host's `validate_portal_bundle` over the project's `web/` tree; `identity` is `(id, version)` for a section-only layout.
pub fn validate_project(dir: &Path, source: &PortalSource, identity: Option<(&str, &str)>) -> Result<ValidatedBundle> {
    let extension_json = match source {
        PortalSource::Extension(path) => std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
        PortalSource::SectionOnly(path) => {
            let Some((id, version)) = identity else {
                bail!("{} is a section-only portal.json: pass --package-id and --package-version", path.display());
            };
            let section: Value = serde_json::from_slice(&std::fs::read(path)?).with_context(|| format!("parsing {}", path.display()))?;
            serde_json::to_vec(&json!({ "kind": WEB_BUNDLE_KIND, "id": id, "version": version, "portal": section }))?
        }
    };
    let ids: Value = serde_json::from_slice(&extension_json).map_err(|error| anyhow!("extension.json is not JSON: {error}"))?;
    let hexon_id = ids.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let hexon_version = ids.get("version").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut members = Vec::new();
    collect_web_members(&dir.join("web"), "web", &mut members)?;
    members.sort_by(|left, right| left.path.cmp(&right.path));
    let facts = BundleFacts {
        // The uncompressed total bounds the archive size from above (the sealed archive is deflated).
        archive_bytes: members.iter().map(|member| member.uncompressed_size).sum(),
        max_bytes: DEFAULT_MAX_BUNDLE_BYTES,
        members: &members,
        integrity_failure: None,
        extension_json: Some(&extension_json),
        hexon_id: &hexon_id,
        hexon_version: &hexon_version,
    };
    validate_portal_bundle(&facts, &origin_allowed).map_err(|error| anyhow!("invalid web-bundle: {error}"))
}

/// Every regular file under `folder` as `web/...` members; a symlink is refused (it could point out of the project).
fn collect_web_members(folder: &Path, archive_prefix: &str, out: &mut Vec<BundleMember>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let archive_path = format!("{archive_prefix}/{name}");
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("invalid web-bundle: {archive_path} is a symlink");
        }
        if kind.is_dir() {
            collect_web_members(&entry.path(), &archive_path, out)?;
        } else {
            out.push(BundleMember { path: archive_path, uncompressed_size: entry.metadata()?.len() });
        }
    }
    Ok(())
}

/// The lines `validate` prints for a bundle that passed.
pub fn report(validated: &ValidatedBundle) -> Vec<String> {
    let mut lines = vec![
        format!(
            "ok {}@{} kind={} capability_digest={}",
            validated.package_id, validated.version, WEB_BUNDLE_KIND, validated.capability_digest
        ),
        format!("entry {}", validated.portal.entry),
        format!("members {}", validated.web_members.len()),
    ];
    lines.extend(validated.web_members.iter().map(|(path, mime)| format!("  {path} {mime}")));
    lines
}
