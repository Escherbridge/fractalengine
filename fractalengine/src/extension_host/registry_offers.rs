//! Install/Update offers from configured sources: the `LocalFacts.registry` the binding resolver gates §5.5 offers on.
//! Bounded and fail closed (src/AGENTS.md §plugin-host "Registry offers").

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use fe_hexon::registry_source::{KillPredicate, RegistryEntry, RegistrySource};
use fe_plugin::bindings::resolver::RegistryBuild;
use fe_sdk::manifest_v2::{parse_and_validate, ProvidedHook};

use super::ops_drain::semver_triplet;
use super::sources::SourceSet;

/// Newest listed versions per package whose header is read.
pub(crate) const VERSIONS_PER_PACKAGE: usize = 3;
/// Header fetches one refresh may spend (the rest wait for the next refresh, unoffered).
pub(crate) const HEADER_FETCHES_PER_REFRESH: usize = 16;
/// Package ids one refresh looks up (a petal naming more is served in id order up to this).
pub(crate) const WANTED_PACKAGES_MAX: usize = 32;

/// Parsed `provides` of one header, keyed by `(package_id, content_hash)`: a hash names immutable bytes.
type ProvidesCache = HashMap<(String, String), Vec<ProvidedHook>>;

/// Registry builds the plugin worker resolves against; the ops worker refreshes them, both share one instance.
#[derive(Clone, Default)]
pub(crate) struct RegistryOffers {
    builds: Arc<RwLock<Vec<RegistryBuild>>>,
    provides_cache: Arc<Mutex<ProvidesCache>>,
}

/// One listed build chosen for a header read, with the source that listed it.
struct Candidate {
    source: Arc<dyn RegistrySource>,
    entry: RegistryEntry,
    version: (u64, u64, u64),
}

impl RegistryOffers {
    /// Empty offers (nothing is offered until the first refresh).
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The current builds, for one `LocalFacts`.
    pub(crate) fn snapshot(&self) -> Vec<RegistryBuild> {
        self.builds.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Re-read every configured source for `wanted`; true when the offered builds changed.
    pub(crate) fn refresh(&self, sources: &SourceSet, wanted: &BTreeSet<String>, is_killed: &KillPredicate) -> bool {
        let registries: Vec<Arc<dyn RegistrySource>> =
            sources.browse_sources().iter().filter_map(|source| sources.get(&source.id).cloned()).collect();
        self.refresh_from(&registries, wanted, is_killed)
    }

    /// [`Self::refresh`] over explicit sources, in configured order.
    pub(crate) fn refresh_from(
        &self,
        registries: &[Arc<dyn RegistrySource>],
        wanted: &BTreeSet<String>,
        is_killed: &KillPredicate,
    ) -> bool {
        let mut cache = self.provides_cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fetches_left = HEADER_FETCHES_PER_REFRESH;
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut builds: Vec<RegistryBuild> = Vec::new();
        for package_id in wanted.iter().take(WANTED_PACKAGES_MAX) {
            for candidate in newest_listed(registries, package_id) {
                let hash = candidate.entry.content_hash.to_ascii_lowercase();
                let key = (package_id.clone(), hash.clone());
                seen.insert(key.clone());
                if is_killed(package_id.as_str(), candidate.entry.content_hash.as_str()) {
                    // Kept as revoked so the resolver states why nothing is offered (R-X-28); no header read.
                    builds.push(RegistryBuild {
                        package_id: package_id.clone(),
                        version: candidate.entry.version.clone(),
                        provides: Vec::new(),
                        revoked: true,
                    });
                    continue;
                }
                let provides = match cache.get(&key) {
                    Some(provides) => provides.clone(),
                    None if fetches_left == 0 => continue,
                    None => {
                        fetches_left -= 1;
                        match read_provides(&candidate) {
                            Ok(provides) => {
                                cache.insert(key, provides.clone());
                                provides
                            }
                            Err(reason) => {
                                tracing::debug!(%package_id, version = %candidate.entry.version, %reason, "registry header unreadable; build not offered");
                                continue;
                            }
                        }
                    }
                };
                builds.push(RegistryBuild {
                    package_id: package_id.clone(),
                    version: candidate.entry.version.clone(),
                    provides,
                    revoked: false,
                });
            }
        }
        cache.retain(|key, _| seen.contains(key));
        drop(cache);
        let mut current = self.builds.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        if same_builds(&current, &builds) {
            return false;
        }
        *current = builds;
        true
    }
}

/// The newest [`VERSIONS_PER_PACKAGE`] distinct builds of exactly `package_id` across the sources, newest first.
/// A source whose listing fails offers nothing this refresh (fail closed); an unparsable version is never offered.
fn newest_listed(registries: &[Arc<dyn RegistrySource>], package_id: &str) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for registry in registries {
        let listing = match registry.list(Some(package_id)) {
            Ok(listing) => listing,
            Err(error) => {
                tracing::debug!(source = %registry.label(), %package_id, %error, "registry listing failed; no offer from it");
                continue;
            }
        };
        for entry in listing.entries.into_iter().filter(|entry| entry.package_id == package_id) {
            let Some(version) = semver_triplet(&entry.version) else { continue };
            let duplicate =
                candidates.iter().any(|known| known.entry.content_hash.eq_ignore_ascii_case(&entry.content_hash));
            if !duplicate {
                candidates.push(Candidate { source: registry.clone(), entry, version });
            }
        }
    }
    // Stable sort: on equal versions the first configured source wins.
    candidates.sort_by(|a, b| b.version.cmp(&a.version));
    candidates.truncate(VERSIONS_PER_PACKAGE);
    candidates
}

/// The header's validated `provides`; the manifest must name the listed id and version.
fn read_provides(candidate: &Candidate) -> Result<Vec<ProvidedHook>, String> {
    let bytes = candidate.source.header(&candidate.entry).map_err(|error| error.to_string())?;
    let validated = parse_and_validate(&bytes).map_err(|error| format!("{:?}: {}", error.reason, error.detail))?;
    let manifest = validated.manifest;
    if manifest.id != candidate.entry.package_id || manifest.version != candidate.entry.version {
        return Err(format!(
            "header names {}@{}, the listing {}@{}",
            manifest.id, manifest.version, candidate.entry.package_id, candidate.entry.version
        ));
    }
    Ok(manifest.provides)
}

fn same_builds(a: &[RegistryBuild], b: &[RegistryBuild]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.package_id == b.package_id && a.version == b.version && a.revoked == b.revoked && a.provides == b.provides
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use fe_hexon::registry_source::{FetchedPackage, Listing, RegistryError};
    use serde_json::{json, Value};

    const FLEET: &str = include_str!("../../../fe-sdk/schema/examples/fleet-sim.extension.json");

    fn header(package_id: &str, version: &str, provides: Value) -> Vec<u8> {
        let mut manifest: Value = serde_json::from_str(FLEET).expect("example manifest parses");
        manifest["id"] = json!(package_id);
        manifest["version"] = json!(version);
        manifest["provides"] = provides;
        serde_json::to_vec(&manifest).expect("manifest serializes")
    }

    fn entry(package_id: &str, version: &str, hash: &str) -> RegistryEntry {
        RegistryEntry {
            package_id: package_id.into(),
            version: version.into(),
            content_hash: hash.into(),
            url: format!("{package_id}-{version}.hexon"),
            size: 1,
            name: package_id.into(),
            header_url: None,
        }
    }

    /// A source over fixed entries; a header is served only where one is stored, and every header read is counted.
    struct FakeSource {
        entries: Vec<RegistryEntry>,
        headers: HashMap<String, Vec<u8>>,
        header_reads: AtomicUsize,
    }

    impl RegistrySource for FakeSource {
        fn label(&self) -> String {
            "fake".into()
        }
        fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
            let needle = query.unwrap_or_default();
            let entries = self.entries.iter().filter(|entry| entry.package_id.contains(needle)).cloned().collect();
            Ok(Listing { entries, refused: Vec::new(), from_cache: false, stale: false })
        }
        fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
            self.header_reads.fetch_add(1, Ordering::SeqCst);
            self.headers.get(&entry.content_hash).cloned().ok_or(RegistryError::HeaderUnavailable)
        }
        fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
            Err(RegistryError::NotFound(entry.package_id.clone()))
        }
    }

    fn offered(offers: &RegistryOffers, package_id: &str) -> Vec<(String, bool, Vec<String>)> {
        offers
            .snapshot()
            .into_iter()
            .filter(|build| build.package_id == package_id)
            .map(|build| (build.version, build.revoked, build.provides.into_iter().map(|hook| hook.hook).collect()))
            .collect()
    }

    /// Journey: offered with provides, newest three only, not listed, killed, header refused or lying, then a cache hit.
    #[test]
    fn registry_offers_journey_offered_unlisted_killed_refused_cached() {
        let map = json!([{ "hook": "petal.map" }]);
        let mut headers = HashMap::new();
        for (version, hash) in [("1.0.0", "h100"), ("1.1.0", "h110"), ("1.2.0", "h120"), ("2.0.0", "h200")] {
            headers.insert(hash.to_owned(), header("org.example.maps", version, map.clone()));
        }
        // The lying header names another package: never offered.
        headers.insert("hlie".into(), header("org.example.other", "3.0.0", map.clone()));
        let source = Arc::new(FakeSource {
            entries: vec![
                entry("org.example.maps", "1.0.0", "h100"),
                entry("org.example.maps", "1.1.0", "h110"),
                entry("org.example.maps", "1.2.0", "h120"),
                entry("org.example.maps", "2.0.0", "h200"),
                entry("org.example.maps.extra", "9.0.0", "hextra"),
                entry("org.example.killed", "1.0.0", "hkill"),
                entry("org.example.noheader", "1.0.0", "hnone"),
                entry("org.example.liar", "3.0.0", "hlie"),
            ],
            headers,
            header_reads: AtomicUsize::new(0),
        });
        let registries: Vec<Arc<dyn RegistrySource>> = vec![source.clone() as Arc<dyn RegistrySource>];
        let is_killed: KillPredicate = Arc::new(|_: &str, hash: &str| hash == "hkill");
        let offers = RegistryOffers::new();
        let plugin_side = offers.clone();
        let wanted: BTreeSet<String> =
            ["org.example.maps", "org.example.absent", "org.example.killed", "org.example.noheader", "org.example.liar"]
                .into_iter()
                .map(String::from)
                .collect();

        assert!(offers.refresh_from(&registries, &wanted, &is_killed), "the first refresh offers builds");
        // Offered: the newest three exact-id builds, each with the header's provides; the prefix match is not one.
        assert_eq!(
            offered(&plugin_side, "org.example.maps"),
            vec![
                ("2.0.0".to_owned(), false, vec!["petal.map".to_owned()]),
                ("1.2.0".to_owned(), false, vec!["petal.map".to_owned()]),
                ("1.1.0".to_owned(), false, vec!["petal.map".to_owned()]),
            ]
        );
        assert!(offered(&plugin_side, "org.example.maps.extra").is_empty(), "not wanted, not offered");
        assert!(offered(&plugin_side, "org.example.absent").is_empty(), "nothing listed, nothing offered");
        assert_eq!(offered(&plugin_side, "org.example.killed"), vec![("1.0.0".to_owned(), true, Vec::new())]);
        assert!(offered(&plugin_side, "org.example.noheader").is_empty(), "a refused header fails closed");
        assert!(offered(&plugin_side, "org.example.liar").is_empty(), "a header naming another package fails closed");
        // Three maps headers, the refused one and the liar; the killed build is never read.
        assert_eq!(source.header_reads.load(Ordering::SeqCst), 5);

        // Cache hit: the same listing reads only the headers that failed, and nothing changes.
        assert!(!offers.refresh_from(&registries, &wanted, &is_killed), "an unchanged listing changes nothing");
        assert_eq!(source.header_reads.load(Ordering::SeqCst), 7);

        // The kill list growing turns the offer into a revoked build without a header read.
        let all_killed: KillPredicate = Arc::new(|_: &str, hash: &str| hash == "hkill" || hash == "h200");
        assert!(offers.refresh_from(&registries, &wanted, &all_killed));
        assert_eq!(offered(&plugin_side, "org.example.maps")[0], ("2.0.0".to_owned(), true, Vec::new()));

        // Nothing wanted any more: every offer goes.
        assert!(offers.refresh_from(&registries, &BTreeSet::new(), &is_killed));
        assert!(plugin_side.snapshot().is_empty());
    }
}
