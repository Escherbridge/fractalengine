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
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("not a zip archive: {}", path.display()))?;
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("missing {name} in {}", path.display()))?;
    let declared = entry.size();
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
    limits::read_entry_capped(&mut entry, declared, limits::entry_cap(name), &mut budget)
        .with_context(|| format!("reading {name} in {}", path.display()))
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

/// Index one package by reading only `manifest.json` from its zip central directory.
pub fn index_one(path: &Path) -> Result<HexonIndexEntry> {
    let manifest = read_zip_json(path, "manifest.json")?;
    let s = |key: &str| -> Option<String> {
        manifest
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
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
    match uri.split_once('@') {
        Some((id, v)) => (id, Some(v)),
        None => (uri, None),
    }
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
