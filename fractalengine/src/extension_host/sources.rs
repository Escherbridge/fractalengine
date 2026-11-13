//! Extension registry sources the operator configured: local folders, static signed indexes, protocol-v1 registries.
//! Every fetcher is anonymous; index keys are pinned per endpoint (docs/extensions/extensions-ui.md §Registry sources).

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use fe_hexon::registry_source::{
    BytesFetcher, FetchedPackage, FileFetcher, Listing, LocalSource, RegistryEntry, RegistryError, RegistrySource,
    SourceTrust, StaticIndexSource,
};
use fe_ui::extensions::browse::BrowseSource;
use serde::Deserialize;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const USER_AGENT: &str = "fractalengine-extension-sources";
/// Folder beside the sources file holding each static source's last verified index.
const INDEX_CACHE_DIR: &str = "extension-index-cache";

/// One pinned index key of a static endpoint: the signer id the index names and its Ed25519 key (hex).
#[derive(Debug, Clone, Deserialize)]
pub struct IndexKeyPin {
    pub key_id: String,
    pub public_key: String,
}

/// What a source reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// One `.hexon` or a folder of them.
    Local { path: PathBuf },
    /// A static signed-index endpoint (folder or http(s) prefix) with its own pinned index keys.
    Static { base: String, index_keys: Vec<IndexKeyPin>, #[serde(default)] cache: Option<PathBuf> },
    /// A protocol-v1 registry reached anonymously over http(s).
    ProtocolV1 { base_url: String },
}

/// One configured source (an entry of `extension_sources.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: SourceKind,
}

/// Read a JSON array of [`SourceConfig`]; a missing file means no sources, a malformed one is an error.
/// A static source without `cache` caches beside the file, so its rollback guard survives a restart.
pub fn load_source_configs(path: &Path) -> Result<Vec<SourceConfig>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut configs: Vec<SourceConfig> =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let cache_dir = path.parent().unwrap_or(Path::new("")).join(INDEX_CACHE_DIR);
    for config in &mut configs {
        if let SourceKind::Static { cache, .. } = &mut config.kind {
            if cache.is_none() {
                *cache = Some(cache_dir.join(cache_file_name(&config.id)));
            }
        }
    }
    Ok(configs)
}

/// `index-<id>.json` with the id reduced to `[A-Za-z0-9_-]` (never a path, never a device name).
fn cache_file_name(source_id: &str) -> String {
    let safe: String = source_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("index-{safe}.json")
}

/// One built source: the picker row, the generic source, and the catalogue view when it is a contract v1 endpoint.
struct SourceEntry {
    browse: BrowseSource,
    source: Arc<dyn RegistrySource>,
    /// Search, details, attestation and licences (static signed-index endpoints only).
    catalog: Option<Arc<StaticIndexSource>>,
    /// The endpoint base the licence issuer sits under.
    base: Option<String>,
}

/// What [`build_one`] produces for a usable source.
struct Built {
    source: Arc<dyn RegistrySource>,
    catalog: Option<Arc<StaticIndexSource>>,
    base: Option<String>,
}

/// The sources Browse offers, in configured order.
pub struct SourceSet {
    entries: Vec<SourceEntry>,
}

impl SourceSet {
    /// Build every source; one that cannot be built stays in the picker and reports why when listed.
    /// Call on the ops worker thread: protocol-v1 and http sources own a private runtime.
    pub fn build(configs: &[SourceConfig], shared: &SourceTrust) -> Self {
        let mut entries: Vec<SourceEntry> = Vec::new();
        for config in configs {
            if entries.iter().any(|known| known.browse.id == config.id) {
                tracing::warn!(source = %config.id, "duplicate extension source id ignored");
                continue;
            }
            let built = build_one(config, shared).unwrap_or_else(|reason| {
                tracing::warn!(source = %config.id, %reason, "extension source unusable");
                let source = Arc::new(UnusableSource { label: config.label.clone(), reason }) as Arc<dyn RegistrySource>;
                Built { source, catalog: None, base: None }
            });
            entries.push(SourceEntry {
                browse: BrowseSource { id: config.id.clone(), label: config.label.clone() },
                source: built.source,
                catalog: built.catalog,
                base: built.base,
            });
        }
        Self { entries }
    }

    fn entry(&self, id: &str) -> Option<&SourceEntry> {
        self.entries.iter().find(|entry| entry.browse.id == id)
    }

    /// The picker rows.
    pub fn browse_sources(&self) -> Vec<BrowseSource> {
        self.entries.iter().map(|entry| entry.browse.clone()).collect()
    }

    /// The source behind a picker id.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn RegistrySource>> {
        self.entry(id).map(|entry| &entry.source)
    }

    /// The catalogue view (search, details, attestation) of a contract v1 endpoint; `None` for local and v1 registries.
    pub fn catalog(&self, id: &str) -> Option<&Arc<StaticIndexSource>> {
        self.entry(id).and_then(|entry| entry.catalog.as_ref())
    }

    /// The endpoint base of a contract v1 source (where its licence issuer is reached).
    pub fn base(&self, id: &str) -> Option<&str> {
        self.entry(id).and_then(|entry| entry.base.as_deref())
    }
}

/// A configured source that could not be built: every operation answers with the build failure.
struct UnusableSource {
    label: String,
    reason: String,
}

impl UnusableSource {
    fn error(&self) -> RegistryError {
        RegistryError::Transport(format!("source not usable: {}", self.reason))
    }
}

impl RegistrySource for UnusableSource {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn list(&self, _query: Option<&str>) -> Result<Listing, RegistryError> {
        Err(self.error())
    }
    fn header(&self, _entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        Err(self.error())
    }
    fn fetch(&self, _entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        Err(self.error())
    }
}

fn build_one(config: &SourceConfig, shared: &SourceTrust) -> Result<Built, String> {
    fn plain(source: Arc<dyn RegistrySource>) -> Built {
        Built { source, catalog: None, base: None }
    }
    match &config.kind {
        SourceKind::Local { path } => Ok(plain(Arc::new(LocalSource::new(path.clone(), shared.clone())))),
        SourceKind::Static { base, index_keys, cache } => {
            let base = base.trim();
            if base.is_empty() {
                return Err("base is empty".into());
            }
            let pins = parse_pins(index_keys)?;
            // An index key never signs packages or revocation lists (store-client contract v1); verify_index re-checks.
            if let Some((key_id, _)) =
                pins.iter().find(|(_, key)| (shared.is_trust_root)(&fe_format::verifying_key_to_did(key)))
            {
                return Err(format!("index key {key_id} is a trust root; an index key never signs packages"));
            }
            let trust = endpoint_trust(shared, pins);
            let fetcher: Arc<dyn BytesFetcher> = if is_http(base) {
                Arc::new(AnonymousHttpFetcher::for_base(base)?)
            } else {
                Arc::new(FileFetcher)
            };
            let source = StaticIndexSource::new(base, fetcher, trust);
            let catalog = Arc::new(match cache {
                Some(path) => source.with_cache(path.clone()),
                None => source,
            });
            Ok(Built { source: catalog.clone(), catalog: Some(catalog), base: Some(base.to_owned()) })
        }
        SourceKind::ProtocolV1 { base_url } => {
            let url = reqwest::Url::parse(base_url.trim()).map_err(|error| format!("base_url: {error}"))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err("base_url must be http or https".into());
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err("base_url must not carry credentials".into());
            }
            // Anonymous by construction (no token parameter), listing plugin packages only.
            let source = fe_hexon::remote::remote_extension_source(config.label.clone(), url.as_str(), shared.clone())
                .map_err(|error| format!("registry client: {error}"))?;
            Ok(plain(Arc::new(source)))
        }
    }
}

fn is_http(location: &str) -> bool {
    let lower = location.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn parse_pins(pins: &[IndexKeyPin]) -> Result<Vec<(String, VerifyingKey)>, String> {
    let mut parsed: Vec<(String, VerifyingKey)> = Vec::with_capacity(pins.len());
    for pin in pins {
        if parsed.iter().any(|(key_id, _)| *key_id == pin.key_id) {
            return Err(format!("index key {} is pinned twice", pin.key_id));
        }
        let raw = hex::decode(pin.public_key.trim()).map_err(|error| format!("index key {}: {error}", pin.key_id))?;
        let bytes: [u8; 32] = raw.try_into().map_err(|_| format!("index key {} must be 32 bytes of hex", pin.key_id))?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|error| format!("index key {}: {error}", pin.key_id))?;
        if key.is_weak() {
            return Err(format!("index key {} is a weak key", pin.key_id));
        }
        parsed.push((pin.key_id.clone(), key));
    }
    Ok(parsed)
}

/// The shared package trust and kill list plus this endpoint's own pinned index keys (an index key never signs packages).
fn endpoint_trust(shared: &SourceTrust, pins: Vec<(String, VerifyingKey)>) -> SourceTrust {
    shared.clone().with_index_pin(pins)
}

/// Not a globally routable unicast address: the one predicate extension egress and dial-outs use
/// (`fe_plugin::net::is_public_unicast`, an allow-list; steer 2026-11-09 01:50 item 2).
fn is_internal(ip: IpAddr) -> bool {
    !fe_plugin::net::is_public_unicast(ip)
}

/// Anonymous http(s) reader: no credentials, no cookies, no proxy, no redirects; every target address is
/// checked and pinned, so a document cannot steer a read at an internal host unless the endpoint itself is local.
pub struct AnonymousHttpFetcher {
    allow_local: bool,
    runtime: tokio::runtime::Runtime,
}

impl AnonymousHttpFetcher {
    /// Fetcher for documents of the endpoint `base`; internal targets are allowed only when `base` is itself internal.
    pub fn for_base(base: &str) -> Result<Self, String> {
        let url = reqwest::Url::parse(base).map_err(|error| format!("base: {error}"))?;
        let allow_local = base_is_internal(&url);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("http runtime: {error}"))?;
        Ok(Self { allow_local, runtime })
    }

    async fn get_async(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
        let transport = |reason: String| RegistryError::Transport(reason);
        let url = reqwest::Url::parse(location).map_err(|error| transport(format!("bad location {location:?}: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(transport(format!("{location:?}: only http and https locations are read")));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(transport(format!("{location:?}: credentials in a location are refused")));
        }
        let port = url.port_or_known_default().ok_or_else(|| transport(format!("{location:?}: no port")))?;
        let host = url.host_str().ok_or_else(|| transport(format!("{location:?}: no host")))?;
        // `host_str` keeps the brackets of an IPv6 literal.
        let (domain, addrs): (Option<String>, Vec<SocketAddr>) = match host.trim_matches(['[', ']']).parse::<IpAddr>() {
            Ok(ip) => (None, vec![SocketAddr::new(ip, port)]),
            Err(_) => {
                let resolved = tokio::net::lookup_host((host, port))
                    .await
                    .map_err(|error| transport(format!("{host}: {error}")))?;
                (Some(host.to_owned()), resolved.collect())
            }
        };
        if addrs.is_empty() {
            return Err(transport(format!("{location:?}: the host does not resolve")));
        }
        if !self.allow_local && addrs.iter().any(|addr| is_internal(addr.ip())) {
            return Err(transport(format!("{location:?}: refusing a local or private address")));
        }
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .no_proxy();
        if let Some(domain) = &domain {
            // The connection goes to the addresses just checked, not to a second lookup.
            builder = builder.resolve_to_addrs(domain, &addrs);
        }
        let client = builder.build().map_err(|error| transport(error.to_string()))?;
        let mut response = client.get(url).send().await.map_err(|error| transport(error.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError::NotFound(location.to_owned()));
        }
        if status.is_redirection() {
            return Err(transport(format!("{location:?}: redirects are not followed ({status})")));
        }
        if !status.is_success() {
            return Err(transport(format!("{location:?}: status {status}")));
        }
        let too_large = || RegistryError::TooLarge { what: location.to_owned(), max: max_bytes };
        if response.content_length().is_some_and(|length| length > max_bytes) {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| transport(error.to_string()))? {
            if body.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// Whether the endpoint itself sits on an internal address (a loopback fixture or an intranet mirror).
fn base_is_internal(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return is_internal(ip);
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    use std::net::ToSocketAddrs as _;
    (host, url.port_or_known_default().unwrap_or(443))
        .to_socket_addrs()
        .map(|addrs| {
            let addrs: Vec<SocketAddr> = addrs.collect();
            !addrs.is_empty() && addrs.iter().all(|addr| is_internal(addr.ip()))
        })
        .unwrap_or(false)
}

impl BytesFetcher for AnonymousHttpFetcher {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
        self.runtime.block_on(self.get_async(location, max_bytes))
    }
}
