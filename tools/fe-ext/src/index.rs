//! `fe-ext index build|verify`: write and check a registry contract v1 static signed catalogue tree.
//! Layout, trust chain and determinism rules: `tools/fe-ext/AGENTS.md` section index.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use clap::{Args, Subcommand};
use ed25519_dalek::{SigningKey, VerifyingKey};
use fe_format::integrity::{self, MANIFEST_PATH, PLUGIN_COUNTERSIGNATURES_PATH, PLUGIN_EXTENSION_PATH};
use fe_format::{HexonArchive, HexonManifest, IntegrityPolicy, IntegrityStatus, SealCheck};
use fe_hexon::registry_source::{
    no_kill_list, sign_index, valid_package_id, valid_version, BytesFetcher, FileFetcher, RegistryEntry, RegistryError, RegistrySource,
    SourceTrust, StaticIndexSource, DOC_MAX_BYTES, HEADER_MAX_BYTES, INDEX_MAX_BYTES, PACKAGE_MAX_BYTES,
};
use serde_json::{json, Map, Value};

/// Where the catalogue lives under a static host root.
const CATALOG_DIR: &str = "api/v1/catalog";
/// Kind string of web bundles (every other signed kind is an extension).
const WEB_BUNDLE_KIND: &str = "web-bundle";
/// Longest `summary` the index carries.
const SUMMARY_MAX_CHARS: usize = 160;
/// Default index validity before it must be re-signed.
const DEFAULT_VALID_DAYS: i64 = 7;

/// `fe-ext index` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum IndexCmd {
    /// Write the static signed catalogue tree for a folder of sealed `.hexon` packages.
    Build(BuildArgs),
    /// Run `StaticIndexSource` end to end over a tree folder (or an http(s) endpoint) against a pinned index key.
    Verify(VerifyArgs),
}

/// Arguments of `index build`.
#[derive(Args, Debug, Clone)]
pub struct BuildArgs {
    /// Folder holding the sealed `.hexon` packages (extensions and web bundles).
    #[arg(long)]
    pub packages: PathBuf,
    /// Public root URL the tree will be served from; blobs are named `<base-url>/blobs/<content_hash>.hexon`.
    #[arg(long)]
    pub base_url: String,
    /// Index signing key file (see `keygen`); it must differ from every publisher key.
    #[arg(long)]
    pub key: PathBuf,
    /// Output folder (the static host root).
    #[arg(long)]
    pub out: PathBuf,
    /// RFC 3339 issue time; default is the newest package `updated_at`, so the output is reproducible.
    #[arg(long)]
    pub issued_at: Option<String>,
    /// Days from the issue time until `expires_at`.
    #[arg(long, default_value_t = DEFAULT_VALID_DAYS)]
    pub valid_days: i64,
    /// Publisher DIDs the operator has verified (`publisher.verified = true`); repeatable.
    #[arg(long = "verified-publisher")]
    pub verified_publishers: Vec<String>,
    /// `package_id@version` to mark yanked; repeatable.
    #[arg(long = "yank")]
    pub yanked: Vec<String>,
    /// A signed revocation list to serve at `revocations.json` (advertises the `revocations` feature).
    #[arg(long)]
    pub revocations: Option<PathBuf>,
}

/// Arguments of `index verify`.
#[derive(Args, Debug, Clone)]
pub struct VerifyArgs {
    /// A tree folder (the static host root) or the http(s) root URL of an endpoint.
    pub target: String,
    /// The pinned index key as a `did:key`.
    #[arg(long)]
    pub index_key: String,
}

/// What `index build` wrote.
#[derive(Debug, Clone)]
pub struct BuildReport {
    /// Packages listed in the index.
    pub listed: usize,
    /// Versions written across every detail document.
    pub versions: usize,
    /// Key id (the index key's `did:key`) that signed the index.
    pub index_key_id: String,
    /// Signed issue time.
    pub issued_at: String,
    /// Signed expiry.
    pub expires_at: String,
}

/// What `index verify` found; `failures` empty means the whole tree verified.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// `package_id@version` verified end to end.
    pub checked: Vec<String>,
    /// Index entries of a kind this build does not read.
    pub skipped: Vec<String>,
    /// Every break in the chain, one line each.
    pub failures: Vec<String>,
    /// True when the index is past its `expires_at` (a warning, never a failure).
    pub stale: bool,
}

/// CLI entry: file trees only; http(s) targets need [`run_index_with`] and a fetcher.
pub fn run_index(command: &IndexCmd) -> Result<()> {
    run_index_with(command, None)
}

/// CLI entry with an anonymous http(s) fetcher for `verify https://...`.
pub fn run_index_with(command: &IndexCmd, http: Option<Arc<dyn BytesFetcher>>) -> Result<()> {
    match command {
        IndexCmd::Build(args) => {
            let report = build_tree(args)?;
            println!(
                "index: {} package(s), {} version(s), signed by {}, issued {}, expires {}",
                report.listed, report.versions, report.index_key_id, report.issued_at, report.expires_at
            );
            Ok(())
        }
        IndexCmd::Verify(args) => {
            let report = verify_tree(&args.target, &args.index_key, http)?;
            for line in &report.checked {
                println!("verified {line}");
            }
            for line in &report.skipped {
                println!("skipped {line}");
            }
            if report.stale {
                println!("warning: index is past its expires_at (stale, not a failure)");
            }
            if report.failures.is_empty() {
                Ok(())
            } else {
                bail!("index verification failed:\n  {}", report.failures.join("\n  "))
            }
        }
    }
}

// ---- build ------------------------------------------------------------------------------------

/// Everything the catalogue says about one sealed build, read from the archive itself.
struct Facts {
    path: PathBuf,
    package_id: String,
    version: String,
    kind: &'static str,
    name: String,
    summary: String,
    description: String,
    tags: Vec<String>,
    homepage: Option<String>,
    publisher_did: String,
    publisher_name: Option<String>,
    content_hash: String,
    size_bytes: u64,
    updated_at: Option<DateTime<Utc>>,
    licensed: bool,
    requires: Value,
    manifest: Vec<u8>,
    header_members: BTreeMap<String, Vec<u8>>,
    countersignatures: Vec<Value>,
}

fn read_facts(path: &Path) -> Result<Facts> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        bail!("{} exceeds {PACKAGE_MAX_BYTES} bytes", path.display());
    }
    let status = fe_format::verify_archive(&bytes, &IntegrityPolicy::RequireSealed { pin: None })
        .map_err(|error| anyhow!("{} fails the seal check: {error}", path.display()))?;
    let IntegrityStatus::Sealed { publisher_did } = status else {
        bail!("{} is not sealed", path.display());
    };
    let cap = fe_format::limits::MAX_JSON_ENTRY_BYTES;
    let manifest = HexonArchive::read_member(&bytes, MANIFEST_PATH, cap)?.ok_or_else(|| anyhow!("{}: no manifest.json", path.display()))?;
    let parsed: HexonManifest = serde_json::from_slice(&manifest).with_context(|| format!("{}: manifest.json", path.display()))?;
    let root = parsed.integrity.clone().ok_or_else(|| anyhow!("{}: manifest has no integrity root", path.display()))?;
    let mut header_members = BTreeMap::new();
    for member in root.header.keys() {
        let data = HexonArchive::read_member(&bytes, member, cap)?.ok_or_else(|| anyhow!("{}: header member {member} is missing", path.display()))?;
        header_members.insert(member.clone(), data);
    }
    let extension: Value = header_members
        .get(PLUGIN_EXTENSION_PATH)
        .ok_or_else(|| anyhow!("{}: no {PLUGIN_EXTENSION_PATH} header member; not an extension or web-bundle package", path.display()))
        .and_then(|raw| serde_json::from_slice(raw).map_err(|error| anyhow!("{}: {PLUGIN_EXTENSION_PATH}: {error}", path.display())))?;
    let kind = if extension.get("kind").and_then(Value::as_str) == Some(WEB_BUNDLE_KIND) { WEB_BUNDLE_KIND } else { "extension" };
    if !valid_package_id(&parsed.hexon_id) {
        bail!("{}: package id {:?} breaks the id grammar", path.display(), parsed.hexon_id);
    }
    if !valid_version(&parsed.version) {
        bail!("{}: version {:?} is not SemVer without build metadata", path.display(), parsed.version);
    }
    let description = extension
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| parsed.description.clone())
        .unwrap_or_default();
    let name = [Some(parsed.name.as_str()), extension.get("name").and_then(Value::as_str)]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or(parsed.hexon_id.as_str())
        .to_owned();
    let requires = match extension.get("requires") {
        Some(Value::Object(map)) if map.values().all(Value::is_string) => Value::Object(map.clone()),
        _ if kind == "extension" => json!({ "fractalengine:plugin": "2.0" }),
        _ => json!({}),
    };
    let countersignatures = match HexonArchive::read_member(&bytes, PLUGIN_COUNTERSIGNATURES_PATH, cap)? {
        Some(raw) => {
            let doc: Value = serde_json::from_slice(&raw).with_context(|| format!("{}: {PLUGIN_COUNTERSIGNATURES_PATH}", path.display()))?;
            doc["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| Some(json!({ "key_id": entry.get("key_id")?, "signature": entry.get("signature")? })))
                .collect()
        }
        None => Vec::new(),
    };
    Ok(Facts {
        path: path.to_owned(),
        package_id: parsed.hexon_id.clone(),
        version: parsed.version.clone(),
        kind,
        summary: description.chars().take(SUMMARY_MAX_CHARS).collect(),
        description,
        tags: parsed.tags.clone(),
        homepage: parsed.homepage_url.clone(),
        publisher_did,
        publisher_name: parsed.publisher_name.clone(),
        content_hash: integrity::content_hash(&manifest)?,
        size_bytes: bytes.len() as u64,
        updated_at: DateTime::parse_from_rfc3339(&parsed.updated_at).ok().map(|at| at.with_timezone(&Utc)),
        licensed: header_members.contains_key("license.json"),
        name,
        requires,
        manifest,
        header_members,
        countersignatures,
    })
}

/// SemVer 2.0 precedence (build metadata is refused upstream, so only core and pre-release count).
fn cmp_version(left: &str, right: &str) -> Ordering {
    let split = |version: &str| match version.split_once('-') {
        Some((core, pre)) => (core.to_owned(), Some(pre.to_owned())),
        None => (version.to_owned(), None),
    };
    let numbers = |core: &str| core.split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let ((left_core, left_pre), (right_core, right_pre)) = (split(left), split(right));
    numbers(&left_core).cmp(&numbers(&right_core)).then_with(|| match (left_pre, right_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left_pre), Some(right_pre)) => cmp_prerelease(&left_pre, &right_pre),
    })
}

fn cmp_prerelease(left: &str, right: &str) -> Ordering {
    let (mut left_parts, mut right_parts) = (left.split('.'), right.split('.'));
    loop {
        let order = match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) => match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => a.cmp(b),
            },
        };
        if order != Ordering::Equal {
            return order;
        }
    }
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn pretty(value: &Value) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec_pretty(value)?;
    out.push(b'\n');
    Ok(out)
}

/// Read every `.hexon` in `dir`, sorted by file name so the output never depends on directory order.
fn read_packages(dir: &Path) -> Result<Vec<Facts>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("hexon")))
        .collect();
    paths.sort();
    if paths.is_empty() {
        bail!("no .hexon packages in {}", dir.display());
    }
    paths.iter().map(|path| read_facts(path)).collect()
}

/// Write the contract v1 tree (`config.json`, `index.json`, details, attestations, blobs), then verify it.
pub fn build_tree(args: &BuildArgs) -> Result<BuildReport> {
    let base_url = args.base_url.trim_end_matches('/').to_owned();
    let lower = base_url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        bail!("--base-url must be an http(s) URL");
    }
    let key: SigningKey = crate::key::load(&args.key)?;
    let index_key_id = crate::key::did(&key);

    let mut grouped: BTreeMap<String, Vec<Facts>> = BTreeMap::new();
    for facts in read_packages(&args.packages)? {
        if facts.publisher_did == index_key_id {
            bail!("the index key {index_key_id} also publishes {}; an index key never signs packages", facts.package_id);
        }
        grouped.entry(facts.package_id.clone()).or_default().push(facts);
    }
    let mut lowered = BTreeSet::new();
    for id in grouped.keys() {
        if !lowered.insert(id.to_ascii_lowercase()) {
            bail!("package ids differ only in ASCII case: {id}");
        }
    }

    let mut yanked: BTreeSet<(String, String)> = BTreeSet::new();
    for spec in &args.yanked {
        let (id, version) = spec.split_once('@').ok_or_else(|| anyhow!("--yank expects package_id@version, got {spec:?}"))?;
        yanked.insert((id.to_owned(), version.to_owned()));
    }
    let known: BTreeSet<(String, String)> =
        grouped.iter().flat_map(|(id, builds)| builds.iter().map(move |facts| (id.clone(), facts.version.clone()))).collect();
    if let Some(missing) = yanked.difference(&known).next() {
        bail!("--yank {}@{} names no package in {}", missing.0, missing.1, args.packages.display());
    }

    let newest_update = grouped.values().flatten().filter_map(|facts| facts.updated_at).max();
    let issued_at = match &args.issued_at {
        Some(text) => DateTime::parse_from_rfc3339(text).map_err(|error| anyhow!("--issued-at: {error}"))?.with_timezone(&Utc),
        None => newest_update.ok_or_else(|| anyhow!("no package carries an RFC 3339 updated_at; pass --issued-at"))?,
    };
    let expires_at = issued_at + Duration::days(args.valid_days.max(1));

    let catalog = args.out.join(CATALOG_DIR);
    let packages_dir = catalog.join("packages");
    if packages_dir.exists() {
        std::fs::remove_dir_all(&packages_dir).with_context(|| format!("clearing {}", packages_dir.display()))?;
    }

    let mut entries = Vec::new();
    let mut version_count = 0;
    for (package_id, mut builds) in grouped {
        builds.sort_by(|a, b| cmp_version(&b.version, &a.version).then_with(|| a.content_hash.cmp(&b.content_hash)));
        builds.dedup_by(|a, b| a.version == b.version && a.content_hash == b.content_hash);
        if let Some(pair) = builds.windows(2).find(|pair| pair[0].version == pair[1].version) {
            bail!("{package_id} {} is built twice with different content", pair[0].version);
        }
        if builds.iter().any(|facts| facts.kind != builds[0].kind || facts.publisher_did != builds[0].publisher_did) {
            bail!("{package_id}: versions disagree on kind or publisher");
        }
        let representative = builds.iter().find(|facts| !yanked.contains(&(package_id.clone(), facts.version.clone()))).unwrap_or(&builds[0]);
        let publisher_name = representative.publisher_name.clone().unwrap_or_else(|| representative.publisher_did.clone());
        let publisher = json!({
            "did": representative.publisher_did,
            "name": publisher_name,
            "verified": args.verified_publishers.contains(&representative.publisher_did),
        });
        let published = |facts: &Facts| rfc3339(facts.updated_at.unwrap_or(issued_at));

        let mut versions = Vec::new();
        for facts in &builds {
            let is_yanked = yanked.contains(&(package_id.clone(), facts.version.clone()));
            versions.push(json!({
                "version": facts.version,
                "content_hash": facts.content_hash,
                "size_bytes": facts.size_bytes,
                "published_at": published(facts),
                "requires": facts.requires,
                "download_url": format!("{base_url}/blobs/{}.hexon", facts.content_hash),
                "yanked": is_yanked,
            }));
            let engine = base64::engine::general_purpose::STANDARD;
            let members: BTreeMap<&String, String> = facts.header_members.iter().map(|(path, data)| (path, engine.encode(data))).collect();
            let attestation = json!({
                "schema_version": 1,
                "package_id": package_id,
                "version": facts.version,
                "content_hash": facts.content_hash,
                "manifest_b64": engine.encode(&facts.manifest),
                "header_members_b64": members,
                "signatures": facts.countersignatures,
            });
            let bytes = pretty(&attestation)?;
            if bytes.len() as u64 > HEADER_MAX_BYTES {
                bail!("{package_id} {}: attestation exceeds {HEADER_MAX_BYTES} bytes", facts.version);
            }
            write_file(&packages_dir.join(&package_id).join(&facts.version).join("attestation.json"), &bytes)?;
            let blob = args.out.join("blobs").join(format!("{}.hexon", facts.content_hash));
            if let Some(parent) = blob.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&facts.path, &blob).with_context(|| format!("copying {} to {}", facts.path.display(), blob.display()))?;
            version_count += 1;
        }

        let access = if builds.iter().any(|facts| facts.licensed) { "licensed" } else { "free" };
        let detail = json!({
            "schema_version": 1,
            "package_id": package_id,
            "kind": representative.kind,
            "name": representative.name,
            "description": representative.description,
            "publisher": publisher,
            "access": access,
            "acquire_url": null,
            "categories": [],
            "tags": representative.tags,
            "links": { "homepage": representative.homepage, "source": null, "issues": null },
            "versions": versions,
        });
        let detail_bytes = pretty(&detail)?;
        if detail_bytes.len() as u64 > DOC_MAX_BYTES {
            bail!("{package_id}: detail document exceeds {DOC_MAX_BYTES} bytes");
        }
        write_file(&packages_dir.join(format!("{package_id}.json")), &detail_bytes)?;

        // A package is listed while it has a non-yanked version; `representative` is that newest build.
        if !yanked.contains(&(package_id.clone(), representative.version.clone())) {
            entries.push(json!({
                "package_id": package_id,
                "kind": representative.kind,
                "name": representative.name,
                "summary": representative.summary,
                "publisher": publisher,
                "latest_version": representative.version,
                "latest_content_hash": representative.content_hash,
                "detail_hash": integrity::digest_hex(&detail_bytes),
                "access": access,
                "categories": [],
                "tags": representative.tags,
                "icon_url": null,
                "updated_at": published(representative),
            }));
        }
    }

    let mut features = vec!["index", "detail", "attestations"];
    if let Some(list) = &args.revocations {
        let bytes = std::fs::read(list).with_context(|| format!("reading {}", list.display()))?;
        serde_json::from_slice::<Value>(&bytes).context("--revocations is not JSON")?;
        write_file(&catalog.join("revocations.json"), &bytes)?;
        features.push("revocations");
    }
    let config = json!({
        "schema_version": 1,
        "registry_protocol": 1,
        "kinds": ["extension", "web-bundle"],
        "features": features,
        "index_key_id": index_key_id,
    });
    write_file(&catalog.join("config.json"), &pretty(&config)?)?;

    let listed = entries.len();
    let document = json!({
        "schema_version": 1,
        "issued_at": rfc3339(issued_at),
        "expires_at": rfc3339(expires_at),
        "entries": entries,
    });
    let signed = sign_index(document, &index_key_id, &key).map_err(|error| anyhow!("signing the index: {error}"))?;
    write_file(&catalog.join("index.json"), &pretty(&signed)?)?;

    let out = args.out.to_string_lossy().into_owned();
    let report = verify_tree(&out, &index_key_id, None)?;
    if !report.failures.is_empty() {
        bail!("the tree written to {out} does not verify:\n  {}", report.failures.join("\n  "));
    }
    Ok(BuildReport { listed, versions: version_count, index_key_id, issued_at: rfc3339(issued_at), expires_at: rfc3339(expires_at) })
}

// ---- verify -----------------------------------------------------------------------------------

/// Serves a local tree; an absolute catalogue or blob URL maps onto the folder by its `/api/v1/catalog/` or `/blobs/` tail.
struct TreeFetcher;

impl BytesFetcher for TreeFetcher {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
        FileFetcher.get(location, max_bytes)
    }
}

/// Maps `http(s)` locations onto `root`; every other location is already a path.
struct MappedTreeFetcher {
    root: PathBuf,
}

impl BytesFetcher for MappedTreeFetcher {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
        let lower = location.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) {
            return TreeFetcher.get(location, max_bytes);
        }
        let markers = [format!("/{CATALOG_DIR}/"), "/blobs/".to_owned()];
        let at = markers
            .iter()
            .filter_map(|marker| location.find(marker.as_str()))
            .min()
            .ok_or_else(|| RegistryError::Transport(format!("{location} is outside the tree")))?;
        let relative = location[at + 1..].split(['?', '#']).next().unwrap_or_default();
        if relative.split('/').any(|part| part == ".." || part.contains(['\\', ':'])) {
            return Err(RegistryError::Transport(format!("{location} climbs out of the tree")));
        }
        TreeFetcher.get(&self.root.join(relative).to_string_lossy(), max_bytes)
    }
}

/// An ed25519 `did:key` as a verifying key.
fn did_to_key(did: &str) -> Result<VerifyingKey> {
    let encoded = did.strip_prefix("did:key:z").ok_or_else(|| anyhow!("--index-key must be a did:key:z... identifier"))?;
    let raw = bs58::decode(encoded).into_vec().map_err(|error| anyhow!("--index-key: {error}"))?;
    if raw.len() != 34 || raw[..2] != [0xed, 0x01] {
        bail!("--index-key is not an ed25519 did:key");
    }
    let bytes: [u8; 32] = raw[2..].try_into().map_err(|_| anyhow!("--index-key has the wrong length"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|error| anyhow!("--index-key: {error}"))
}

fn catalog_base(target: &str) -> String {
    let trimmed = target.trim_end_matches(['/', '\\']);
    if trimmed.replace('\\', "/").ends_with(CATALOG_DIR) {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/{CATALOG_DIR}")
    }
}

/// `base/location` for a document-named location; http(s) passes, absolute, drive or climbing paths are refused.
fn locate(base: &str, location: &str) -> Result<String> {
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Ok(location.to_owned());
    }
    if location.contains(':') || location.starts_with(['/', '\\']) || location.split(['/', '\\']).any(|part| part == "..") {
        bail!("location {location:?} is not a path inside the source");
    }
    Ok(format!("{base}/{location}"))
}

/// Run `StaticIndexSource` over a tree: every extension entry's header and sealed bytes, then the web-bundle entries by the same chain.
pub fn verify_tree(target: &str, index_key_did: &str, http: Option<Arc<dyn BytesFetcher>>) -> Result<VerifyReport> {
    let lower = target.to_ascii_lowercase();
    let remote = lower.starts_with("https://") || lower.starts_with("http://");
    let (base, fetcher): (String, Arc<dyn BytesFetcher>) = if remote {
        let fetcher = http.ok_or_else(|| anyhow!("verifying an http(s) endpoint needs an anonymous http fetcher; verify the served folder instead"))?;
        (catalog_base(target), fetcher)
    } else {
        let root = PathBuf::from(target);
        let base = catalog_base(&root.to_string_lossy());
        (base, Arc::new(MappedTreeFetcher { root }))
    };

    let index_key = did_to_key(index_key_did)?;
    let pinned = index_key_did.to_owned();
    let trust = SourceTrust {
        // Every publisher is trusted for the check except the index key, which must never sign packages.
        is_trust_root: Arc::new({
            let pinned = pinned.clone();
            move |did: &str| did != pinned
        }),
        index_signer: Arc::new(move |key_id: &str| (key_id == pinned).then_some(index_key)),
        // Publisher tooling checks the tree's own chain; the host applies the kill list at install.
        is_killed: no_kill_list(),
        allow_unsigned_dev: false,
    };
    let source = StaticIndexSource::new(base.clone(), fetcher.clone(), trust.clone());
    let listing = source.list(None).map_err(|error| anyhow!("index: {error}"))?;

    let mut report = VerifyReport { stale: listing.stale, ..VerifyReport::default() };
    for refused in &listing.refused {
        report.failures.push(format!("{}: {}", refused.what, refused.reason));
    }
    for entry in &listing.entries {
        let label = format!("{}@{}", entry.package_id, entry.version);
        match verify_extension_entry(&source, entry) {
            Ok(()) => report.checked.push(label),
            Err(reason) => report.failures.push(format!("{label}: {reason}")),
        }
    }

    // Entries of other kinds never reach `StaticIndexSource::list`; the verified index bytes name them.
    let index_bytes = fetcher.get(&locate(&base, "index.json")?, INDEX_MAX_BYTES).map_err(|error| anyhow!("index: {error}"))?;
    let index: Value = serde_json::from_slice(&index_bytes).context("index.json")?;
    for raw in index["entries"].as_array().into_iter().flatten() {
        let kind = raw.get("kind").and_then(Value::as_str).unwrap_or_default();
        let id = raw.get("package_id").and_then(Value::as_str).unwrap_or_default();
        if kind == "extension" || !valid_package_id(id) {
            continue;
        }
        if kind != WEB_BUNDLE_KIND {
            report.skipped.push(format!("{id} (kind {kind:?} is not read by this client)"));
            continue;
        }
        match verify_web_bundle_entry(&base, fetcher.as_ref(), raw, &trust) {
            Ok(label) => report.checked.push(label),
            Err(reason) => report.failures.push(format!("{id}: {reason}")),
        }
    }
    Ok(report)
}

fn verify_extension_entry(source: &StaticIndexSource, entry: &RegistryEntry) -> Result<(), String> {
    let header = source.header(entry).map_err(|error| format!("header: {error}"))?;
    let fetched = source.fetch(entry).map_err(|error| format!("download: {error}"))?;
    if header != fetched.package.extension_json {
        return Err("the attested header differs from the downloaded package".into());
    }
    Ok(())
}

/// One web-bundle entry through the same chain: pinned detail, attestation recomputing to the listed hash, sealed download.
fn verify_web_bundle_entry(base: &str, fetcher: &dyn BytesFetcher, raw: &Value, trust: &SourceTrust) -> Result<String> {
    let text = |field: &str| raw.get(field).and_then(Value::as_str).ok_or_else(|| anyhow!("index entry lacks {field}"));
    let (id, version, hash, detail_hash) = (text("package_id")?, text("latest_version")?, text("latest_content_hash")?, text("detail_hash")?);
    let get = |location: &str, max: u64| -> Result<Vec<u8>> {
        fetcher.get(&locate(base, location)?, max).map_err(|error| anyhow!("{location}: {error}"))
    };
    let detail_bytes = get(&format!("packages/{id}.json"), DOC_MAX_BYTES)?;
    if !integrity::digest_hex(&detail_bytes).eq_ignore_ascii_case(detail_hash) {
        bail!("detail document hash differs from the index");
    }
    let detail: Value = serde_json::from_slice(&detail_bytes)?;
    if detail["package_id"] != id || detail["kind"] != WEB_BUNDLE_KIND {
        bail!("detail names another package or kind");
    }
    let listed = detail["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|candidate| candidate["version"] == version && candidate["content_hash"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(hash)))
        .ok_or_else(|| anyhow!("detail lacks {version} with the indexed content hash"))?;

    let attestation: Value = serde_json::from_slice(&get(&format!("packages/{id}/{version}/attestation.json"), HEADER_MAX_BYTES)?)?;
    let engine = base64::engine::general_purpose::STANDARD;
    let decode = |value: &Value| -> Result<Vec<u8>> { engine.decode(value.as_str().unwrap_or_default()).map_err(|error| anyhow!("attestation: {error}")) };
    let manifest = decode(&attestation["manifest_b64"])?;
    let mut members = BTreeMap::new();
    for (path, encoded) in attestation["header_members_b64"].as_object().unwrap_or(&Map::new()) {
        members.insert(path.clone(), decode(encoded)?);
    }
    let check = fe_format::verify_sealed_header(&manifest, &members, &IntegrityPolicy::RequireSealed { pin: None })
        .map_err(|error| anyhow!("attestation: {error}"))?;
    let SealCheck::Sealed(coverage) = check else { bail!("attestation is not sealed") };
    let parsed: HexonManifest = serde_json::from_slice(&manifest)?;
    if parsed.hexon_id != id || parsed.version != version {
        bail!("attestation manifest names another build");
    }
    if !integrity::content_hash(&manifest)?.eq_ignore_ascii_case(hash) {
        bail!("attestation does not recompute to the indexed content hash");
    }
    if !(trust.is_trust_root)(coverage.publisher_did()) {
        bail!("publisher {} is not trusted", coverage.publisher_did());
    }

    let size = listed["size_bytes"].as_u64().ok_or_else(|| anyhow!("detail lacks size_bytes"))?.min(PACKAGE_MAX_BYTES);
    let bytes = get(listed["download_url"].as_str().ok_or_else(|| anyhow!("detail lacks download_url"))?, size)?;
    let pin = Some(parsed.publisher_did.clone());
    fe_format::verify_archive(&bytes, &IntegrityPolicy::RequireSealed { pin }).map_err(|error| anyhow!("download: {error}"))?;
    let served = HexonArchive::read_member(&bytes, MANIFEST_PATH, fe_format::limits::MAX_JSON_ENTRY_BYTES)?.ok_or_else(|| anyhow!("download has no manifest"))?;
    if !integrity::content_hash(&served)?.eq_ignore_ascii_case(hash) {
        bail!("download recomputes to another content hash");
    }
    Ok(format!("{id}@{version}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WASM_COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    fn sealed(id: &str, version: &str, key: &SigningKey) -> Vec<u8> {
        let extension = json!({ "id": id, "version": version, "name": "Fleet Sim", "description": "Simulated fleet.", "contract": "fractalengine:plugin@2.0.0", "kind": "simulation" });
        let mut component = WASM_COMPONENT_PREAMBLE.to_vec();
        component.extend_from_slice(&[7u8; 64]);
        fe_hexon::publisher::PluginHexonBuilder::new(serde_json::to_vec(&extension).unwrap(), component).build(key).expect("seal")
    }

    /// Journey: build a two-version tree, rebuild byte-identically, verify against the pin, then break the chain three ways.
    #[test]
    fn index_build_then_verify_journey() {
        let work = tempfile::tempdir().unwrap();
        let publisher = SigningKey::generate(&mut rand::rngs::OsRng);
        let packages = work.path().join("packages");
        std::fs::create_dir_all(&packages).unwrap();
        std::fs::write(packages.join("a-1.hexon"), sealed("org.example.fleet-sim", "1.0.0", &publisher)).unwrap();
        std::fs::write(packages.join("a-2.hexon"), sealed("org.example.fleet-sim", "1.1.0", &publisher)).unwrap();
        let key_path = work.path().join("index.key");
        let index_did = crate::key::generate(&key_path).unwrap();

        let args = |out: &Path| BuildArgs {
            packages: packages.clone(),
            base_url: "https://store.example/".into(),
            key: key_path.clone(),
            out: out.to_owned(),
            issued_at: Some("2026-11-07T00:00:00Z".into()),
            valid_days: 30,
            verified_publishers: Vec::new(),
            yanked: Vec::new(),
            revocations: None,
        };
        let (first, second) = (work.path().join("out1"), work.path().join("out2"));
        let report = build_tree(&args(&first)).expect("build verifies itself");
        assert_eq!((report.listed, report.versions), (1, 2));
        build_tree(&args(&second)).unwrap();
        let index_file = format!("{CATALOG_DIR}/index.json");
        assert_eq!(std::fs::read(first.join(&index_file)).unwrap(), std::fs::read(second.join(&index_file)).unwrap(), "deterministic");

        let verified = verify_tree(&first.to_string_lossy(), &index_did, None).unwrap();
        assert!(verified.failures.is_empty(), "{:?}", verified.failures);
        assert_eq!(verified.checked, vec!["org.example.fleet-sim@1.1.0".to_string()]);

        let other_pin = crate::key::did(&SigningKey::generate(&mut rand::rngs::OsRng));
        assert!(verify_tree(&first.to_string_lossy(), &other_pin, None).is_err(), "an unpinned index signer is refused");

        let detail = first.join(format!("{CATALOG_DIR}/packages/org.example.fleet-sim.json"));
        std::fs::write(&detail, [std::fs::read(&detail).unwrap(), b" ".to_vec()].concat()).unwrap();
        assert!(!verify_tree(&first.to_string_lossy(), &index_did, None).unwrap().failures.is_empty(), "detail hash pin");

        let publisher_as_index = packages.join("../publisher.key");
        std::fs::write(&publisher_as_index, format!("{}\n", hex::encode(publisher.to_bytes()))).unwrap();
        let mut misuse = args(&work.path().join("out3"));
        misuse.key = publisher_as_index;
        assert!(build_tree(&misuse).is_err(), "an index key never signs packages");
    }
}
