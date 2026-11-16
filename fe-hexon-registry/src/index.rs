//! Lazy zip-backed package index — see `AGENTS.md` §lazy zip indexing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::limits;

/// One indexed `.hexon` package (metadata only; blobs stay on disk).
#[derive(Debug, Clone, Serialize)]
pub struct HexonIndexEntry {
    pub hexon_id: String,
    pub version: String,
    pub hexon_type: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub publisher_did: String,
    pub size_bytes: u64,
    /// BLAKE3 of the manifest with the signature blanked (store-client contract v1); empty when the manifest cannot be canonicalised.
    pub content_hash: String,
    /// `manifest.integrity` is present (cheap presence check at scan time —
    /// NOT a cryptographic verification; that happens at publish and again,
    /// per read, in the meta/chunk/member routes via `HexonArchiveIndex`).
    pub sealed: bool,
    /// Monolithic, or a signed chunk container (from `chunks/index.json`'s
    /// bundle count, read the same cheap way).
    pub layout: fe_format::Layout,
    /// `license.json` `license_type` (`free` / `attribution` / `paid`); `None` without a readable licence.
    pub license_type: Option<String>,
    /// `license.json` `spdx_id`.
    pub spdx_id: Option<String>,
    /// `license.json` `attribution`, clipped to [`ATTRIBUTION_INDEX_CHARS`].
    pub attribution: Option<String>,
    /// Card image member path: first placeable entry's `preview_image`, else `icon.png`; only when present in the archive.
    pub preview: Option<String>,
    /// Uncompressed size of `preview` from the central directory.
    pub preview_bytes: Option<u64>,
    #[serde(skip)]
    pub path: PathBuf,
}

/// Read and JSON-parse a single named file from a `.hexon` zip.
pub fn read_zip_json(path: &Path, name: &str) -> Result<serde_json::Value> {
    let bytes = read_zip_bytes(path, name)?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid JSON in {name}"))
}

/// Read a single named file from a `.hexon` zip without touching other entries
/// (decompression-capped — see `AGENTS.md` §decompression limits).
pub fn read_zip_bytes(path: &Path, name: &str) -> Result<Vec<u8>> {
    try_read_zip_bytes(path, name)?.with_context(|| format!("missing {name} in {}", path.display()))
}

/// [`read_zip_bytes`], but `Ok(None)` when `name` is simply absent from the
/// archive (as opposed to a read/decompression failure) — `index_one` uses
/// this to tell "no `chunks/index.json`" (an ordinary monolithic archive)
/// apart from a real error on an archive that does claim one.
fn try_read_zip_bytes(path: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("not a zip archive: {}", path.display()))?;
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {name} in {}", path.display())),
    };
    let declared = entry.size();
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
    limits::read_entry_capped(&mut entry, declared, limits::entry_cap(name), &mut budget)
        .map(Some)
        .with_context(|| format!("reading {name} in {}", path.display()))
}

/// [`try_read_zip_bytes`], JSON-parsed.
fn try_read_zip_json(path: &Path, name: &str) -> Result<Option<serde_json::Value>> {
    match try_read_zip_bytes(path, name)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("invalid JSON in {name}")),
        None => Ok(None),
    }
}

/// Index-time clip for `license.json` `attribution`, in characters.
pub const ATTRIBUTION_INDEX_CHARS: usize = 512;

/// Licence and preview facts of one package; every failure leaves the field empty.
#[derive(Default)]
struct LibraryFacts {
    license_type: Option<String>,
    spdx_id: Option<String>,
    attribution: Option<String>,
    preview: Option<String>,
    preview_bytes: Option<u64>,
}

/// Best-effort read of `license.json`, `entries.json` and the central directory; never fails the listing.
fn library_facts(path: &Path) -> LibraryFacts {
    let mut facts = LibraryFacts::default();
    if let Ok(Some(license)) = try_read_zip_json(path, "license.json") {
        let text = |key: &str| {
            license
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        facts.license_type = text("license_type");
        facts.spdx_id = text("spdx_id");
        facts.attribution = text("attribution")
            .map(|a| a.chars().take(ATTRIBUTION_INDEX_CHARS).collect::<String>());
    }
    let placeable_preview = match try_read_zip_json(path, "entries.json") {
        Ok(Some(entries)) => entries
            .get("entries")
            .and_then(|e| e.as_array())
            .and_then(|list| {
                list.iter()
                    .filter(|e| e.get("is_placeable").and_then(|v| v.as_bool()) == Some(true))
                    .find_map(|e| e.get("preview_image").and_then(|v| v.as_str()))
                    .map(str::to_string)
            }),
        _ => None,
    };
    let Ok(file) = std::fs::File::open(path) else {
        return facts;
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return facts;
    };
    for candidate in placeable_preview.into_iter().chain(["icon.png".to_string()]) {
        if let Ok(member) = archive.by_name(&candidate) {
            facts.preview_bytes = Some(member.size());
            facts.preview = Some(candidate);
            break;
        }
    }
    facts
}

/// Publish rule for `model` packages: `license.json` present, and `attribution` licences name an attribution.
pub fn check_model_license(bytes: &[u8]) -> std::result::Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("invalid .hexon: {e}"))?;
    let mut entry = match archive.by_name("license.json") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err("model packages require a license.json".to_string())
        }
        Err(e) => return Err(format!("cannot open license.json: {e}")),
    };
    let declared = entry.size();
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
    let raw = limits::read_entry_capped(&mut entry, declared, limits::MAX_JSON_ENTRY_BYTES, &mut budget)
        .map_err(|e| format!("cannot read license.json: {e:#}"))?;
    let license: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|e| format!("invalid license.json: {e}"))?;
    match license.get("license_type").and_then(|v| v.as_str()) {
        Some("attribution") => {
            let named = license
                .get("attribution")
                .and_then(|v| v.as_str())
                .is_some_and(fe_format::text::has_visible_text);
            if named {
                Ok(())
            } else {
                Err("an attribution license requires visible attribution text".to_string())
            }
        }
        Some(_) => Ok(()),
        None => Err("license.json has no license_type".to_string()),
    }
}

/// Publish-time validation: parse `manifest.json` and decompress every entry
/// into a sink under the per-entry and whole-archive caps, so a zip bomb is
/// refused before it is stored. Returns the raw manifest.
pub fn validate_package(bytes: &[u8]) -> Result<serde_json::Value> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    limits::check_entry_count(archive.len())?;
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
    let manifest = {
        let mut file = archive.by_name("manifest.json")?;
        let declared = file.size();
        let raw = limits::read_entry_capped(
            &mut file,
            declared,
            limits::MAX_JSON_ENTRY_BYTES,
            &mut budget,
        )
        .context("reading manifest.json")?;
        serde_json::from_slice(&raw).context("invalid manifest.json")?
    };
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.name() == "manifest.json" {
            continue;
        }
        let name = file.name().to_string();
        let declared = file.size();
        limits::drain_entry_capped(&mut file, declared, limits::entry_cap(&name), &mut budget)
            .with_context(|| format!("reading {name}"))?;
    }
    Ok(manifest)
}

/// Index one package by reading only `manifest.json` from its zip central directory
/// (plus, cheaply, `chunks/index.json`'s presence/bundle-count for `layout` — a
/// container's chunk table is small, header-phase JSON, same cap as the manifest).
pub fn index_one(path: &Path) -> Result<HexonIndexEntry> {
    let manifest_raw = read_zip_bytes(path, "manifest.json")?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_raw).context("invalid JSON in manifest.json")?;
    let content_hash = fe_format::integrity::content_hash(&manifest_raw).unwrap_or_default();
    let s = |key: &str| -> Option<String> {
        manifest
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let sealed = manifest.get("integrity").is_some_and(|v| !v.is_null());
    let layout = match try_read_zip_json(path, fe_format::integrity::CHUNK_INDEX_PATH)? {
        Some(index) => fe_format::Layout::Container {
            chunk_count: index
                .get("bundles")
                .and_then(|b| b.as_array())
                .map(|a| a.len() as u32)
                .unwrap_or_default(),
        },
        None => fe_format::Layout::Monolithic,
    };
    let facts = library_facts(path);
    Ok(HexonIndexEntry {
        hexon_id: s("hexon_id").context("manifest missing hexon_id")?,
        version: s("version").context("manifest missing version")?,
        hexon_type: s("hexon_type").context("manifest missing hexon_type")?,
        name: s("name").unwrap_or_default(),
        description: s("description"),
        tags: manifest
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        publisher_did: s("publisher_did").unwrap_or_default(),
        size_bytes: std::fs::metadata(path)?.len(),
        content_hash,
        sealed,
        layout,
        license_type: facts.license_type,
        spdx_id: facts.spdx_id,
        attribution: facts.attribution,
        preview: facts.preview,
        preview_bytes: facts.preview_bytes,
        path: path.to_path_buf(),
    })
}

/// Scan `dir` (non-recursive) for `*.hexon` files; unreadable packages are skipped with a warning.
pub fn scan_dir(dir: &Path) -> Vec<HexonIndexEntry> {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "registry dir not readable");
            return Vec::new();
        }
    };
    let mut index = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("hexon") || !path.is_file() {
            continue;
        }
        match index_one(&path) {
            Ok(ie) => index.push(ie),
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "skipping unindexable hexon")
            }
        }
    }
    index
}

/// Split a hexon URI into `(hexon_id, Option<version>)`.
pub fn split_uri(uri: &str) -> (&str, Option<&str>) {
    fe_format::manifest::split_hexon_uri(uri)
}

/// Resolve a URI against the index; unversioned URIs pick the latest version.
pub fn resolve<'a>(index: &'a [HexonIndexEntry], uri: &str) -> Option<&'a HexonIndexEntry> {
    let (id, version) = split_uri(uri);
    match version {
        Some(v) => index.iter().find(|e| e.hexon_id == id && e.version == v),
        None => index
            .iter()
            .filter(|e| e.hexon_id == id)
            .max_by(|a, b| cmp_version(&a.version, &b.version)),
    }
}

/// SemVer-aware version ordering; unparseable versions sort below valid ones.
pub fn cmp_version(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => Ordering::Greater,
        (Err(_), Ok(_)) => Ordering::Less,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// A releasable version per store-client contract v1: SemVer 2.0.0 without build metadata.
pub fn valid_release_version(version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|v| v.build.is_empty())
}
