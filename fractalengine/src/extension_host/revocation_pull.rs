//! OD-8 revocation pull (governance.md §Kill switch layers, layer d): every revocation-capable root's list on an
//! interval, anonymous and https-only, the last good list per signer cached on disk, each list handed to
//! governance (newest per signer; every stopped instance audited). Rationale: src/AGENTS.md §extension-governance.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::Resource;
use fe_plugin::lifecycle::{fetch_revocation_list, REVOCATION_LIST_MAX_BYTES};
use fe_plugin::trust_roots::RevocationList;
use fe_runtime::service_lifecycle::{OwnedService, StopSignal};

use crate::extension_host::governance::ExtensionGovernance;

/// Overrides the pull interval in seconds (QA scenarios need a revocation inside one run); clamped to 10 s..24 h.
pub(crate) const PULL_INTERVAL_ENV: &str = "FE_EXT_REVOCATION_PULL_SECS";
/// OD-8 default pull interval.
pub(crate) const DEFAULT_PULL_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MIN_PULL_INTERVAL: Duration = Duration::from_secs(10);
const MAX_PULL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Redirects one pull may follow; every hop stays on https.
const MAX_REDIRECTS: usize = 5;
/// Subdirectory of the governance config dir holding the last good list per signer.
pub(crate) const CACHE_DIR: &str = "revocations";

/// The pull interval from [`PULL_INTERVAL_ENV`], else [`DEFAULT_PULL_INTERVAL`].
pub(crate) fn interval_from_env() -> Duration {
    parse_interval(std::env::var(PULL_INTERVAL_ENV).ok().as_deref())
}

fn parse_interval(value: Option<&str>) -> Duration {
    value
        .and_then(|seconds| seconds.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .map_or(DEFAULT_PULL_INTERVAL, |interval| interval.clamp(MIN_PULL_INTERVAL, MAX_PULL_INTERVAL))
}

/// Wakes the pull loop before its interval ends (QA, an operator's "check now").
#[derive(Resource, Clone, Default)]
pub(crate) struct RevocationPullHandle(Arc<tokio::sync::Notify>);

impl RevocationPullHandle {
    /// Pull every source now.
    pub(crate) fn pull_now(&self) {
        self.0.notify_one();
    }
}

/// The running pull loop, stopped with the other host services on exit.
#[derive(Resource)]
pub(crate) struct RevocationPullService(pub Mutex<Option<OwnedService>>);

/// Apply the cached lists, then pull each revocation-capable root's `revocation_url` every `interval`.
/// No service starts when no root names a URL; the cached lists still applied.
pub(crate) fn start(
    governance: ExtensionGovernance,
    cache_dir: Option<PathBuf>,
    interval: Duration,
) -> (Option<OwnedService>, RevocationPullHandle) {
    let handle = RevocationPullHandle::default();
    if let Some(dir) = cache_dir.as_deref() {
        let stopped = seed_from_cache(&governance, dir);
        if !stopped.is_empty() {
            tracing::info!(stopped = ?stopped, "cached revocation lists stopped extension instances");
        }
    }
    let sources: Vec<(String, String)> = governance
        .trust()
        .roots()
        .revocation_sources()
        .into_iter()
        .map(|(root, url)| (root.to_string(), url.to_string()))
        .collect();
    if sources.is_empty() {
        tracing::info!("no trust root names a revocation_url: the revocation pull stays off");
        return (None, handle);
    }
    let wake = handle.0.clone();
    let service = OwnedService::spawn("ext-revocation-pull", move |stop| {
        run(stop, governance, sources, cache_dir, interval, wake)
    });
    (Some(service), handle)
}

/// The loop: one pass over every source, then wait for the interval, a wake-up or the stop signal.
fn run(
    stop: StopSignal,
    governance: ExtensionGovernance,
    sources: Vec<(String, String)>,
    cache_dir: Option<PathBuf>,
    interval: Duration,
    wake: Arc<tokio::sync::Notify>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(%error, "revocation pull runtime did not start; cached lists stay in force");
            return;
        }
    };
    runtime.block_on(async move {
        let client = match pull_client() {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "revocation pull client did not build; cached lists stay in force");
                return;
            }
        };
        while !stop.is_requested() {
            for (root, url) in &sources {
                tokio::select! {
                    _ = pull_once(&client, &governance, root, url, cache_dir.as_deref()) => {}
                    _ = stop.cancelled() => return,
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = wake.notified() => {}
                _ = stop.cancelled() => return,
            }
        }
    });
}

/// Anonymous client: no credentials or cookies, bounded time, redirects only to https.
fn pull_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("a revocation list redirect must stay on https")
            } else {
                attempt.follow()
            }
        }))
        .build()
}

/// Fetch one source and hand the list to governance; a failure keeps the last good list in force.
async fn pull_once(
    client: &reqwest::Client,
    governance: &ExtensionGovernance,
    root: &str,
    url: &str,
    cache_dir: Option<&Path>,
) {
    let list = match fetch_revocation_list(client, url).await {
        Ok(list) => list,
        Err(error) => {
            tracing::warn!(root, url, %error, "revocation pull failed; the last good list stays in force");
            return;
        }
    };
    if list.signer_key_id != root {
        // The signature, not the transport, decides: another revocation root's list is still that root's word.
        tracing::debug!(root, signer = %list.signer_key_id, "revocation list served for another root");
    }
    apply(governance, &list, cache_dir, "pull");
}

/// Accept one list; the cache is rewritten only for a list governance accepted.
fn apply(governance: &ExtensionGovernance, list: &RevocationList, cache_dir: Option<&Path>, origin: &str) -> Vec<String> {
    match governance.accept_revocation_list(list) {
        Ok(stopped) => {
            tracing::info!(
                signer = %list.signer_key_id,
                issued_at = %list.issued_at,
                entries = list.entries.len(),
                stopped = stopped.len(),
                origin,
                "revocation list accepted"
            );
            if let Some(dir) = cache_dir {
                if let Err(error) = write_cache(dir, list) {
                    tracing::warn!(%error, "revocation list accepted but not cached");
                }
            }
            stopped
        }
        Err(reason) => {
            tracing::warn!(signer = %list.signer_key_id, issued_at = %list.issued_at, origin, %reason, "revocation list refused");
            Vec::new()
        }
    }
}

/// `<dir>/<hex(signer key id)>.json`: one file per signer, safe whatever the label holds.
fn cache_path(dir: &Path, signer_key_id: &str) -> PathBuf {
    dir.join(format!("{}.json", hex::encode(signer_key_id.as_bytes())))
}

/// Write the list as received, staged then renamed so a crash never leaves half a file.
pub(crate) fn write_cache(dir: &Path, list: &RevocationList) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(list).map_err(std::io::Error::other)?;
    let path = cache_path(dir, &list.signer_key_id);
    let staging = path.with_extension("json.part");
    std::fs::write(&staging, bytes)?;
    std::fs::rename(&staging, &path)
}

/// Re-apply every cached list (each verified again, oldest first); the instances they stopped.
pub(crate) fn seed_from_cache(governance: &ExtensionGovernance, dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut lists: Vec<RevocationList> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| read_cached(&path))
        .collect();
    lists.sort_by(|a, b| a.issued_at.cmp(&b.issued_at));
    lists.iter().flat_map(|list| apply(governance, list, None, "cache")).collect()
}

fn read_cached(path: &Path) -> Option<RevocationList> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > REVOCATION_LIST_MAX_BYTES as u64 {
        tracing::warn!(path = %path.display(), size, "cached revocation list over the size cap; ignored");
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .map_err(|error| tracing::warn!(path = %path.display(), %error, "cached revocation list does not parse"))
        .ok()
}

#[cfg(test)]
mod tests {
    //! Journey: a cached list stops the build it names on restart, a replayed older list never un-revokes,
    //! a newer list is the signer's word. The plugin manager is a stand-in that keeps fe-plugin's real
    //! newest-per-signer verification (`TrustRoots::verify_revocation_list_fresh`).
    use super::*;
    use crate::extension_host::governance::{GovernanceConfig, ManagerControl, NoEntitlementVerifier, TRUST_ROOTS_FILE};
    use ed25519_dalek::{Signer, SigningKey};
    use fe_plugin::lifecycle::ConsentRecord;
    use fe_plugin::trust_roots::{RevocationEntry, TrustRoots};
    use fe_runtime::messages::DbCommand;
    use std::collections::HashMap;

    const COMPONENT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMPONENT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Running builds keyed by instance id; a list stops the ones whose digest it names.
    struct RunningBuilds {
        roots: TrustRoots,
        newest: Mutex<HashMap<String, String>>,
        running: Mutex<Vec<(String, String)>>,
    }

    impl ManagerControl for RunningBuilds {
        fn record_consent(&self, _record: ConsentRecord) {}
        fn add_kill_list_override(&self, _verse_id: &str, _content_hash: &str) {}
        fn reevaluate_verse_policy(&self, _verse_id: &str) -> Vec<String> {
            Vec::new()
        }
        fn accept_revocation_list(&self, list: &RevocationList) -> Result<Vec<String>, String> {
            let set = self
                .roots
                .verify_revocation_list_fresh(list, &mut self.newest.lock().unwrap())
                .map_err(|error| error.to_string())?;
            let mut running = self.running.lock().unwrap();
            let stopped: Vec<String> =
                running.iter().filter(|(_, digest)| set.is_revoked(digest)).map(|(id, _)| id.clone()).collect();
            running.retain(|(id, _)| !stopped.contains(id));
            Ok(stopped)
        }
    }

    fn signed(key: &SigningKey, issued_at: &str, revoked: &[&str]) -> RevocationList {
        let mut list = RevocationList {
            schema_version: 1,
            issued_at: issued_at.into(),
            entries: revoked
                .iter()
                .map(|hash| RevocationEntry {
                    package_id: "org.example.renamed".into(),
                    content_hash: (*hash).into(),
                    reason: "malicious".into(),
                    revoked_at: issued_at.into(),
                })
                .collect(),
            signer_key_id: "rev".into(),
            signature: String::new(),
        };
        list.signature = hex::encode(key.sign(&list.signed_bytes()).to_bytes());
        list
    }

    #[test]
    fn revocation_journey_cache_seed_audited_stop_and_no_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let revoker = SigningKey::from_bytes(&[21; 32]);
        let roots = format!(
            "[[publisher]]\nkey_id = \"rev\"\npublic_key = \"{}\"\nrevocation = true\nrevocation_url = \"http://127.0.0.1:9/revoked.json\"\n",
            hex::encode(revoker.verifying_key().to_bytes())
        );
        std::fs::write(dir.path().join(TRUST_ROOTS_FILE), &roots).unwrap();
        let (db_tx, db_rx) = crossbeam::channel::unbounded();
        let governance = ExtensionGovernance::new(
            GovernanceConfig {
                local_did: "did:key:zLocalNode".into(),
                config_dir: dir.path().to_path_buf(),
                dev_flag: false,
                entitlement: Arc::new(NoEntitlementVerifier),
            },
            db_tx,
            None,
            None,
        );
        assert_eq!(governance.trust().roots().revocation_sources().len(), 1);
        governance.attach_manager(Arc::new(RunningBuilds {
            roots: TrustRoots::from_toml_str(&roots).unwrap(),
            newest: Mutex::new(HashMap::new()),
            running: Mutex::new(vec![("inst-a".into(), COMPONENT_A.into()), ("inst-b".into(), COMPONENT_B.into())]),
        }));

        // A list cached by an earlier run applies on restart: the build revoked by its component digest
        // (under a renamed package id) stops, and the stop is audited once.
        let cache = dir.path().join(CACHE_DIR);
        write_cache(&cache, &signed(&revoker, "2026-11-08T00:00:00Z", &[COMPONENT_A])).unwrap();
        assert_eq!(seed_from_cache(&governance, &cache), ["inst-a"]);
        assert!(governance.is_revoked_any(&["unrelated", COMPONENT_A]));
        let audit = loop {
            match db_rx.recv_timeout(Duration::from_secs(5)).expect("the stop is audited") {
                DbCommand::AppendExtensionAudit { entry, .. } => break entry,
                _ => continue,
            }
        };
        assert_eq!((audit["action"].as_str(), audit["target"].as_str()), (Some("revocation"), Some("inst-a")));

        // A replayed older list is refused: nothing is un-revoked and the cache keeps the newer list.
        let cached = std::fs::read(cache_path(&cache, "rev")).unwrap();
        let older = signed(&revoker, "2026-11-01T00:00:00Z", &[]);
        assert!(governance.accept_revocation_list(&older).unwrap_err().contains("older"));
        assert!(apply(&governance, &older, Some(&cache), "pull").is_empty());
        assert!(governance.is_revoked_any(&[COMPONENT_A]));
        assert_eq!(std::fs::read(cache_path(&cache, "rev")).unwrap(), cached);

        // A newer list is the signer's word now: it names B, which stops; A is no longer listed (its rows stay
        // and nothing restarts by itself).
        let newer = signed(&revoker, "2026-11-09T00:00:00Z", &[COMPONENT_B]);
        assert_eq!(apply(&governance, &newer, Some(&cache), "pull"), ["inst-b"]);
        assert!(!governance.is_revoked_any(&[COMPONENT_A]));
        assert!(governance.is_revoked_any(&[COMPONENT_B]));

        // A forged list (not signed by a revocation root) never applies.
        let mut forged = signed(&SigningKey::from_bytes(&[22; 32]), "2026-11-10T00:00:00Z", &[COMPONENT_A]);
        forged.signer_key_id = "rev".into();
        assert!(governance.accept_revocation_list(&forged).is_err());
        assert!(!governance.is_revoked_any(&[COMPONENT_A]));

        assert_eq!(parse_interval(Some("2")), MIN_PULL_INTERVAL, "the interval never drops under 10 s");
        assert_eq!(parse_interval(Some("junk")), DEFAULT_PULL_INTERVAL);
    }
}
