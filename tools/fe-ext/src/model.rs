//! `model pack` (folder of GLB files -> signed model package) and `inspect` for model packages; see `AGENTS.md` §model-pack.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use ed25519_dalek::SigningKey;
use fe_ext::key;
use fe_format::integrity::{self, MANIFEST_PATH};
use fe_format::limits::{MAX_ENTRY_UNCOMPRESSED_BYTES, MAX_JSON_ENTRY_BYTES};
use fe_format::{glb, HexonArchive, IntegrityPolicy, IntegrityStatus};
use fe_hexon::manifest::{AssetEntry, EntryKind, HexonManifest, HexonType, License, LicenseType};
use fe_hexon::package::{HexonPackage, PREVIEW_PREFIX};
use fe_hexon::registry_source::{valid_package_id, valid_version, PACKAGE_MAX_BYTES};
use fe_hexon::signature::asset_hash;
use fe_hexon::thumbnail::{self, ThumbnailError, MAX_THUMBNAIL_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};

/// The manifest file every model folder carries.
const LIBRARY_FILE: &str = "library.toml";
/// `library.toml` is a small text file; anything larger is refused unread.
const LIBRARY_MAX_BYTES: u64 = 1024 * 1024;
/// Longest SPDX identifier accepted.
const SPDX_MAX_CHARS: usize = 128;

/// `fe-ext model ...`
#[derive(Subcommand)]
pub enum ModelCmd {
    /// Seal a folder of GLB files, thumbnails and `library.toml` into a signed model package.
    Pack {
        /// Folder holding `library.toml`, the listed `.glb` files and their `.png` thumbnails.
        folder: PathBuf,
        /// Signing key file (see `keygen`); never printed.
        #[arg(long)]
        key: PathBuf,
        /// Output file, or a folder (existing, or ending in `/`) that gets `<id>-<version>.hexon`; default: the current directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

/// Where the package goes.
pub enum Output {
    /// `<id>-<version>.hexon` in the current directory.
    Here,
    File(PathBuf),
    Folder(PathBuf),
}

/// What a successful pack produced.
pub struct Packed {
    pub path: PathBuf,
    pub package_id: String,
    pub version: String,
    pub content_hash: String,
    pub publisher_did: String,
    pub size_bytes: u64,
    pub entry_ids: Vec<String>,
}

/// Run a `model` subcommand and print its result lines.
pub fn run_model(command: &ModelCmd) -> Result<()> {
    match command {
        ModelCmd::Pack { folder, key: key_path, out } => {
            let signing_key = key::load(key_path)?;
            let output = match out {
                Some(out) if names_directory(out) => Output::Folder(out.clone()),
                Some(out) => Output::File(out.clone()),
                None => Output::Here,
            };
            let packed = pack(folder, &signing_key, output)?;
            println!("packed {}", packed.path.display());
            println!("package_id {}", packed.package_id);
            println!("version {}", packed.version);
            println!("content_hash {}", packed.content_hash);
            println!("publisher_did {}", packed.publisher_did);
            println!("size_bytes {}", packed.size_bytes);
            println!("models {}", packed.entry_ids.join(","));
        }
    }
    Ok(())
}

/// `--out` names a folder when it exists as one or ends in a path separator (same rule as `pack`).
fn names_directory(path: &Path) -> bool {
    path.is_dir() || path.as_os_str().to_string_lossy().ends_with(['/', '\\'])
}

// ---- library.toml ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Library {
    id: String,
    version: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    license: Option<LicenseSection>,
    #[serde(default, rename = "model")]
    models: Vec<ModelSection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenseSection {
    #[serde(rename = "type")]
    kind: String,
    spdx: Option<String>,
    attribution: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSection {
    file: String,
    thumbnail: String,
    label: String,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_placeable")]
    placeable: bool,
    #[serde(default)]
    auto_scale: bool,
}

fn default_placeable() -> bool {
    true
}

struct PreparedModel {
    entry: AssetEntry,
    glb: Vec<u8>,
    thumbnail: Vec<u8>,
}

struct Prepared {
    library: Library,
    license: License,
    models: Vec<PreparedModel>,
    total_bytes: u64,
}

// ---- pack ----

/// Validate the whole folder, build and seal, re-verify the built bytes, then write atomically.
pub fn pack(folder: &Path, signing_key: &SigningKey, output: Output) -> Result<Packed> {
    let prepared = prepare(folder)?;
    let publisher_did = key::did(signing_key);
    let bytes = build(&prepared, signing_key, &publisher_did)?;
    let content_hash = reverify(&bytes, &publisher_did, &prepared)?;

    let file_name = format!("{}-{}.hexon", prepared.library.id, prepared.library.version);
    let path = match output {
        Output::Here => PathBuf::from(&file_name),
        Output::File(path) => path,
        Output::Folder(folder) => folder.join(&file_name),
    };
    write_atomically(&path, &bytes)?;
    Ok(Packed {
        path,
        package_id: prepared.library.id,
        version: prepared.library.version,
        content_hash,
        publisher_did,
        size_bytes: bytes.len() as u64,
        entry_ids: prepared.models.into_iter().map(|model| model.entry.entry_id).collect(),
    })
}

/// Every refusal of the folder, collected rather than first-failure; nothing is written here.
fn prepare(folder: &Path) -> Result<Prepared> {
    let mut problems: Vec<String> = Vec::new();
    let files = scan_folder(folder, &mut problems);
    let library = if files.contains(LIBRARY_FILE) {
        read_library(&folder.join(LIBRARY_FILE), &mut problems)
    } else {
        problems.push(format!("{LIBRARY_FILE} is missing from {}", folder.display()));
        None
    };
    let Some(library) = library else {
        return Err(refusal(&problems));
    };

    if !valid_package_id(&library.id) || !fe_format::is_safe_path_id(&library.id) {
        problems.push(format!("id {:?} is not a valid package id (letters, digits, '.', '_' and '-'; at most 128; starts with a letter or digit)", library.id));
    }
    if !valid_version(&library.version) {
        problems.push(format!("version {:?} is not a SemVer version such as 1.0.0", library.version));
    }
    if library.name.trim().is_empty() {
        problems.push("name is empty".to_string());
    }
    let license = license_from(library.license.as_ref(), &mut problems);

    if library.models.is_empty() {
        problems.push(format!("{LIBRARY_FILE} lists no [[model]] entries"));
    }
    let mut listed_glbs: BTreeSet<&str> = BTreeSet::new();
    let mut listed_thumbnails: BTreeSet<&str> = BTreeSet::new();
    let mut entry_ids: BTreeSet<String> = BTreeSet::new();
    let mut models = Vec::new();
    let mut total_bytes = 0u64;
    for (position, model) in library.models.iter().enumerate() {
        let before = problems.len();
        let which = format!("[[model]] #{}", position + 1);
        listed_glbs.insert(model.file.as_str());
        if model.label.trim().is_empty() {
            problems.push(format!("{which}: label is empty"));
        }
        let file_listed_once = listed_thumbnails.insert(model.thumbnail.as_str());
        if !file_listed_once {
            problems.push(format!("{which}: thumbnail {} is used by another model; each model needs its own", model.thumbnail));
        }
        if model.thumbnail == model.file {
            problems.push(format!("{which}: thumbnail and file are the same file"));
        }

        let entry_id = entry_id_of(model, &files, &which, &mut entry_ids, &mut problems);
        let glb_bytes = entry_id.as_ref().and_then(|_| read_glb(folder, &model.file, &mut total_bytes, &mut problems));
        let thumbnail_bytes = if check_member_name(&model.thumbnail, "thumbnail", &files, &which, &mut problems) {
            read_thumbnail(folder, &model.thumbnail, &mut total_bytes, &mut problems)
        } else {
            None
        };
        if let (true, Some(entry_id), Some(glb), Some(thumbnail)) = (problems.len() == before, entry_id, glb_bytes, thumbnail_bytes) {
            models.push(PreparedModel { entry: entry_for(model, &entry_id, &glb), glb, thumbnail });
        }
    }
    for name in &files {
        if name.to_ascii_lowercase().ends_with(".glb") && !listed_glbs.contains(name.as_str()) {
            problems.push(format!("{name}: is not listed in {LIBRARY_FILE}; list it or remove it"));
        }
    }
    if total_bytes > PACKAGE_MAX_BYTES {
        problems.push(format!("models and thumbnails total {total_bytes} bytes; a package is at most {PACKAGE_MAX_BYTES} bytes (64 MiB)"));
    }
    match (problems.is_empty(), license) {
        (true, Some(license)) => Ok(Prepared { library, license, models, total_bytes }),
        _ => Err(refusal(&problems)),
    }
}

/// One refusal error naming every problem.
fn refusal(problems: &[String]) -> anyhow::Error {
    anyhow!("model pack refused, {} problem(s):\n  - {}", problems.len(), problems.join("\n  - "))
}

/// Top-level names of the regular files in `folder`; symlinks, subdirectories and non-UTF-8 names are problems.
fn scan_folder(folder: &Path, problems: &mut Vec<String>) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let read = match std::fs::read_dir(folder) {
        Ok(read) => read,
        Err(error) => {
            problems.push(format!("cannot read folder {}: {error}", folder.display()));
            return files;
        }
    };
    for item in read {
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                problems.push(format!("cannot list {}: {error}", folder.display()));
                continue;
            }
        };
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            problems.push(format!("{}: file name is not valid UTF-8", item.file_name().to_string_lossy()));
            continue;
        };
        match std::fs::symlink_metadata(item.path()) {
            Ok(meta) if meta.file_type().is_symlink() => problems.push(format!("{name}: is a symlink; only regular files are packed")),
            Ok(meta) if meta.is_dir() => problems.push(format!("{name}: is a subdirectory; a model folder holds only files")),
            Ok(meta) if meta.is_file() => {
                files.insert(name);
            }
            Ok(_) => problems.push(format!("{name}: is not a regular file")),
            Err(error) => problems.push(format!("{name}: {error}")),
        }
    }
    files
}

fn read_library(path: &Path, problems: &mut Vec<String>) -> Option<Library> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > LIBRARY_MAX_BYTES => {
            problems.push(format!("{LIBRARY_FILE} is larger than {LIBRARY_MAX_BYTES} bytes"));
            return None;
        }
        Err(error) => {
            problems.push(format!("{LIBRARY_FILE}: {error}"));
            return None;
        }
        Ok(_) => {}
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            problems.push(format!("{LIBRARY_FILE}: {error}"));
            return None;
        }
    };
    match toml::from_str::<Library>(&text) {
        Ok(library) => Some(library),
        Err(error) => {
            problems.push(format!("{LIBRARY_FILE} does not parse: {error}"));
            None
        }
    }
}

/// The `[license]` table as a package licence: present, never paid, attribution text under an Attribution licence.
fn license_from(section: Option<&LicenseSection>, problems: &mut Vec<String>) -> Option<License> {
    let Some(section) = section else {
        problems.push(format!("{LIBRARY_FILE} has no [license] table; a model package must state its licence"));
        return None;
    };
    let attribution = section.attribution.as_deref().map(str::trim).filter(|text| fe_format::text::has_visible_text(text));
    let license_type = match section.kind.trim().to_ascii_lowercase().as_str() {
        "free" => LicenseType::Free,
        "attribution" if attribution.is_some() => LicenseType::Attribution,
        "attribution" => {
            problems.push("[license] attribution is required for an Attribution licence".to_string());
            return None;
        }
        "paid" => {
            problems.push("[license] type paid is not supported for model packages".to_string());
            return None;
        }
        other => {
            problems.push(format!("[license] type {other:?} is not one of free, attribution"));
            return None;
        }
    };
    let spdx_id = section.spdx.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned);
    if let Some(spdx) = &spdx_id {
        let plain = spdx.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
        if !plain || spdx.chars().count() > SPDX_MAX_CHARS {
            problems.push(format!("[license] spdx {spdx:?} is not a plain SPDX identifier"));
            return None;
        }
    }
    Some(License { license_type, spdx_id, attribution: attribution.map(str::to_owned), ..License::default() })
}

/// A listed name is one plain file name that exists as a regular file in the folder (no path, `..`, drive or separator).
fn check_member_name(name: &str, role: &str, files: &BTreeSet<String>, which: &str, problems: &mut Vec<String>) -> bool {
    let mut parts = Path::new(name).components();
    let plain = matches!((parts.next(), parts.next()), (Some(Component::Normal(part)), None) if part.to_str() == Some(name))
        && !name.contains(['/', '\\', ':']);
    if !plain {
        problems.push(format!("{which}: {role} {name:?} must be a plain file name inside the folder (no path, no '..', not absolute)"));
        return false;
    }
    if !files.contains(name) {
        problems.push(format!("{which}: {role} {name} does not exist as a regular file in the folder"));
        return false;
    }
    true
}

/// The entry id (file stem) of a model whose file is listed correctly; unique and path-safe.
fn entry_id_of(model: &ModelSection, files: &BTreeSet<String>, which: &str, seen: &mut BTreeSet<String>, problems: &mut Vec<String>) -> Option<String> {
    if !check_member_name(&model.file, "file", files, which, problems) {
        return None;
    }
    if !model.file.to_ascii_lowercase().ends_with(".glb") {
        problems.push(format!("{which}: file {} is not a .glb", model.file));
        return None;
    }
    let stem = Path::new(&model.file).file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    if !fe_format::is_safe_path_id(stem) {
        problems.push(format!("{which}: file name {} does not give a safe entry id (letters, digits, '.', '_' and '-')", model.file));
        return None;
    }
    if !seen.insert(stem.to_ascii_lowercase()) {
        problems.push(format!("{which}: entry id {stem} is already used by another model"));
        return None;
    }
    Some(stem.to_string())
}

/// Read one GLB under the per-entry cap and run the structural check; size is checked before any byte is read.
fn read_glb(folder: &Path, name: &str, total_bytes: &mut u64, problems: &mut Vec<String>) -> Option<Vec<u8>> {
    let path = folder.join(name);
    let length = match std::fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(error) => {
            problems.push(format!("{name}: {error}"));
            return None;
        }
    };
    *total_bytes = total_bytes.saturating_add(length);
    if length > MAX_ENTRY_UNCOMPRESSED_BYTES {
        problems.push(format!("{name}: {length} bytes exceeds the 64 MiB ({MAX_ENTRY_UNCOMPRESSED_BYTES} bytes) limit per model"));
        return None;
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            problems.push(format!("{name}: {error}"));
            return None;
        }
    };
    match glb::check(&bytes, MAX_ENTRY_UNCOMPRESSED_BYTES as usize) {
        Ok(facts) if facts.external_uri => {
            problems.push(format!("{name}: depends on files outside itself (an image or buffer has an external uri); embed them"));
            None
        }
        Ok(_) => Some(bytes),
        Err(error) => {
            problems.push(format!("{name}: is not a valid GLB: {error}"));
            None
        }
    }
}

/// Read one thumbnail and decode it with the decoder the app uses; an oversize file is refused unread.
fn read_thumbnail(folder: &Path, name: &str, total_bytes: &mut u64, problems: &mut Vec<String>) -> Option<Vec<u8>> {
    let path = folder.join(name);
    let length = match std::fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(error) => {
            problems.push(format!("{name}: {error}"));
            return None;
        }
    };
    *total_bytes = total_bytes.saturating_add(length);
    if length > MAX_THUMBNAIL_BYTES as u64 {
        problems.push(format!("{name}: {}", ThumbnailError::TooLarge));
        return None;
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            problems.push(format!("{name}: {error}"));
            return None;
        }
    };
    match thumbnail::decode(&bytes) {
        Ok(_) => Some(bytes),
        Err(error) => {
            problems.push(format!("{name}: {error}"));
            None
        }
    }
}

fn clean_text(text: &Option<String>) -> Option<String> {
    text.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter().map(|tag| tag.trim().to_owned()).filter(|tag| !tag.is_empty()).collect()
}

/// The catalogue entry of one model; `preview_image` names the sealed `preview/<entry_id>.png` member.
fn entry_for(model: &ModelSection, entry_id: &str, glb: &[u8]) -> AssetEntry {
    AssetEntry {
        entry_id: entry_id.to_string(),
        kind: EntryKind::Model,
        asset_hash: asset_hash(glb),
        format: "glb".to_string(),
        label: model.label.trim().to_owned(),
        tags: clean_tags(&model.tags),
        description: clean_text(&model.description),
        is_placeable: model.placeable,
        is_private: false,
        auto_scale: model.auto_scale,
        preview_image: Some(preview_member(entry_id)),
        center: None,
        extents: None,
        sub_assets: None,
        address: None,
        metadata: None,
    }
}

fn preview_name(entry_id: &str) -> String {
    format!("{entry_id}.png")
}

fn preview_member(entry_id: &str) -> String {
    format!("{PREVIEW_PREFIX}{}", preview_name(entry_id))
}

/// Seal the package last (`HexonPackage::build`); the licence goes through directly because the builder has no SPDX setter.
fn build(prepared: &Prepared, signing_key: &SigningKey, publisher_did: &str) -> Result<Vec<u8>> {
    let library = &prepared.library;
    let now = chrono::Utc::now().to_rfc3339();
    let manifest = HexonManifest {
        schema_version: "1.0.0".to_string(),
        hexon_id: library.id.clone(),
        hexon_type: HexonType::Model,
        publisher_did: publisher_did.to_string(),
        publisher_name: None,
        version: library.version.clone(),
        build_id: None,
        name: library.name.trim().to_owned(),
        description: clean_text(&library.description),
        tags: clean_tags(&library.tags),
        created_at: now.clone(),
        updated_at: now,
        source_peer_did: None,
        approx_size_bytes: Some(prepared.total_bytes),
        min_engine_version: None,
        homepage_url: None,
        dependencies: Vec::new(),
        platforms: Vec::new(),
        address: None,
        integrity: None,
        signature: None,
    };
    let mut entries = Vec::new();
    let mut assets: HashMap<String, Vec<u8>> = HashMap::new();
    let mut previews: HashMap<String, Vec<u8>> = HashMap::new();
    for model in &prepared.models {
        assets.insert(model.entry.asset_hash.clone(), model.glb.clone());
        previews.insert(preview_name(&model.entry.entry_id), model.thumbnail.clone());
        entries.push(model.entry.clone());
    }
    HexonPackage::build(manifest, entries, Some(prepared.license.clone()), assets, None, previews, None, None, signing_key)
        .context("sealing the package")
}

/// Re-verify the built bytes the way an installer would; returns the package `content_hash`.
fn reverify(bytes: &[u8], publisher_did: &str, prepared: &Prepared) -> Result<String> {
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        bail!("the packed archive is {} bytes; a package is at most {PACKAGE_MAX_BYTES} bytes (64 MiB)", bytes.len());
    }
    let status = fe_format::verify_archive(bytes, &IntegrityPolicy::RequireSealed { pin: Some(publisher_did.to_string()) })
        .map_err(|error| anyhow!("the sealed package failed verification: {error}"))?;
    let IntegrityStatus::Sealed { publisher_did: sealed_by } = status else {
        bail!("the packed archive is not sealed");
    };
    if sealed_by != publisher_did {
        bail!("the packed archive is sealed by {sealed_by}, not by the signing key {publisher_did}");
    }
    let package = HexonPackage::open(bytes).context("the packed archive does not open as a hexon package")?;
    if !package.signature_valid() {
        bail!("the packed archive is not signed by its publisher");
    }
    if package.manifest.hexon_type != HexonType::Model {
        bail!("the packed archive is not a model package");
    }
    if package.license.is_none() {
        bail!("the packed archive carries no license.json");
    }
    if package.entries.len() != prepared.models.len() {
        bail!("the packed archive lists {} entries for {} models", package.entries.len(), prepared.models.len());
    }
    for entry in &package.entries {
        if !package.assets.contains_key(&entry.asset_hash) {
            bail!("entry {} has no asset in the packed archive", entry.entry_id);
        }
        let expected_member = preview_member(&entry.entry_id);
        if entry.preview_image.as_deref() != Some(expected_member.as_str()) || !package.previews.contains_key(&preview_name(&entry.entry_id)) {
            bail!("entry {} has no preview member in the packed archive", entry.entry_id);
        }
    }
    let manifest_raw = HexonArchive::read_member(bytes, MANIFEST_PATH, MAX_JSON_ENTRY_BYTES)?.context("the packed archive has no manifest.json")?;
    integrity::content_hash(&manifest_raw)
}

/// Temp file in the output directory, flushed and synced, then renamed; the temp is removed on every failure path.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let name = path.file_name().ok_or_else(|| anyhow!("output path {} has no file name", path.display()))?.to_string_lossy().into_owned();
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        bail!("writing {}: {error}", path.display());
    }
    Ok(())
}

// ---- inspect ----

/// Facts of a verified model package; `None` when the archive is not a model package (the plugin `inspect` then runs).
pub fn inspect(bytes: &[u8]) -> Result<Option<Value>> {
    let Ok(Some(manifest_raw)) = HexonArchive::read_member(bytes, MANIFEST_PATH, MAX_JSON_ENTRY_BYTES) else {
        return Ok(None);
    };
    let Ok(document) = serde_json::from_slice::<Value>(&manifest_raw) else {
        return Ok(None);
    };
    if document.get("hexon_type").and_then(Value::as_str) != Some("model") {
        return Ok(None);
    }
    let status = fe_format::verify_archive(bytes, &IntegrityPolicy::RequireSealed { pin: None })
        .map_err(|error| anyhow!("the model package fails the seal check: {error}"))?;
    let IntegrityStatus::Sealed { publisher_did } = status else {
        bail!("the model package is not sealed");
    };
    let package = HexonPackage::open(bytes).context("the model package does not open")?;
    let (license_type, spdx, attribution) = match &package.license {
        Some(license) => (
            match license.license_type {
                LicenseType::Free => "free",
                LicenseType::Attribution => "attribution",
                LicenseType::Paid => "paid",
            },
            license.spdx_id.clone(),
            license.attribution.clone(),
        ),
        None => ("none", None, None),
    };
    let models: Vec<&str> = package.entries.iter().filter(|entry| entry.kind == EntryKind::Model).map(|entry| entry.entry_id.as_str()).collect();
    Ok(Some(json!({
        "package_id": package.manifest.hexon_id,
        "version": package.manifest.version,
        "kind": "model",
        "name": package.manifest.name,
        "publisher_did": publisher_did,
        "content_hash": integrity::content_hash(&manifest_raw)?,
        "license": license_type,
        "spdx": spdx,
        "attribution": attribution,
        "models": models,
        "signature_valid": package.signature_valid(),
    })))
}
