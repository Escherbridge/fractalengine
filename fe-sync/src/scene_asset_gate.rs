//! Durable scene-content classification and fresh Serve admission; see AGENTS.md.

use fe_runtime::blob_store::{hash_to_hex, BlobHash};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};

pub(crate) type SceneBlobAuthorizer = Arc<
    dyn Fn(String, String, BlobHash) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync,
>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Binding {
    verse_id: String,
    asset_hash: BlobHash,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Classification {
    version: u32,
    deny_unknown: bool,
    hashes: BTreeMap<String, Vec<Binding>>,
}
impl Classification {
    fn valid(&self) -> bool {
        self.version == 1
            && !self.hashes.is_empty()
            && self.hashes.iter().all(|(key, bindings)| {
                !bindings.is_empty()
                    && bindings.len() <= 16
                    && bindings.iter().all(|binding| {
                        !binding.verse_id.is_empty()
                            && binding.verse_id.len() <= 1024
                            && (*key == hash_to_hex(&binding.asset_hash)
                                || *key
                                    == hash_to_hex(&fe_runtime::blob_transport::hash_seq_root(&[
                                        binding.asset_hash,
                                    ])))
                    })
            })
    }
}

impl Default for Classification {
    fn default() -> Self {
        Self {
            version: 1,
            deny_unknown: false,
            hashes: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
struct State {
    class: Mutex<Classification>,
    authorizer: RwLock<Option<SceneBlobAuthorizer>>,
    path: Option<PathBuf>,
    persist_lock: Mutex<()>,
    pending: Mutex<HashMap<String, usize>>,
    failed: AtomicBool,
}

struct ClassificationWrite {
    state: Arc<State>,
    keys: Vec<String>,
}
impl Drop for ClassificationWrite {
    fn drop(&mut self) {
        let mut pending = self.state.pending.lock().unwrap_or_else(|e| e.into_inner());
        for key in &self.keys {
            if let Some(count) = pending.get_mut(key) {
                *count -= 1;
                if *count == 0 {
                    pending.remove(key);
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SceneAssetGate(Arc<State>);

impl SceneAssetGate {
    pub(crate) fn load(path: PathBuf) -> Self {
        let marker = path.with_extension("enabled");
        let read = std::fs::metadata(&path).and_then(|metadata| {
            if metadata.len() > 8 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "scene classification exceeds 8 MiB",
                ));
            }
            std::fs::read(&path)
        });
        let class = match read {
            Ok(bytes) => match serde_json::from_slice::<Classification>(&bytes) {
                Ok(class) if class.valid() => class,
                Ok(_) => {
                    tracing::error!("scene blob classification has invalid format/state; denying unknown native content");
                    Classification {
                        deny_unknown: true,
                        ..Default::default()
                    }
                }
                Err(error) => {
                    tracing::error!(%error,"scene blob classification corrupt; denying unknown native content");
                    Classification {
                        deny_unknown: true,
                        ..Default::default()
                    }
                }
            },
            Err(error) if marker.exists() || path.exists() => {
                tracing::error!(%error,"scene blob classification missing/unreadable after activation; denying unknown native content");
                Classification {
                    deny_unknown: true,
                    ..Default::default()
                }
            }
            Err(_) => Classification::default(),
        };
        Self(Arc::new(State {
            class: Mutex::new(class),
            path: Some(path),
            ..Default::default()
        }))
    }

    pub(crate) fn set_authorizer(&self, authorizer: SceneBlobAuthorizer) {
        *self.0.authorizer.write().unwrap_or_else(|e| e.into_inner()) = Some(authorizer);
    }

    /// Persist both raw child and retention root before either enters the native store.
    pub(crate) async fn classify(
        &self,
        verse: &str,
        asset_hash: BlobHash,
        io_owner: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !verse.is_empty() && verse.len() <= 1024,
            "invalid scene classification"
        );
        let hashes = [
            asset_hash,
            fe_runtime::blob_transport::hash_seq_root(&[asset_hash]),
        ];
        let state = self.0.clone();
        let verse = verse.to_owned();
        let hashes = hashes.to_vec();
        tokio::task::spawn_blocking(move || {
            register_classification(state, verse, asset_hash, hashes, io_owner)
        })
        .await??;
        Ok(())
    }

    pub(crate) async fn admits(&self, did: &str, hashes: &[BlobHash]) -> bool {
        if hashes.len() > 64 {
            return false;
        }
        let contexts = {
            let pending = self.0.pending.lock().unwrap_or_else(|e| e.into_inner());
            if self.0.failed.load(Ordering::Acquire)
                || hashes
                    .iter()
                    .any(|hash| pending.contains_key(&hash_to_hex(hash)))
            {
                return false;
            }
            let state = self.0.class.lock().unwrap_or_else(|e| e.into_inner());
            if state.deny_unknown {
                return false;
            }
            hashes
                .iter()
                .filter_map(|hash| state.hashes.get(&hash_to_hex(hash)).cloned())
                .collect::<Vec<_>>()
        };
        if contexts.is_empty() {
            return true;
        }
        let Some(authorize) = self
            .0
            .authorizer
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return false;
        };
        for bindings in contexts {
            let mut allowed = false;
            for binding in bindings {
                if authorize(did.to_owned(), binding.verse_id, binding.asset_hash).await {
                    allowed = true;
                    break;
                }
            }
            if !allowed {
                return false;
            }
        }
        true
    }
}

fn register_classification(
    state: Arc<State>,
    verse: String,
    asset_hash: BlobHash,
    hashes: Vec<BlobHash>,
    io_owner: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> anyhow::Result<()> {
    let _io_owner = io_owner;
    anyhow::ensure!(
        !state.failed.load(Ordering::Acquire),
        "Scene classification storage failed; restart/recover before retrying"
    );
    let keys = hashes.iter().map(hash_to_hex).collect::<Vec<_>>();
    {
        let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        for key in &keys {
            *pending.entry(key.clone()).or_default() += 1;
        }
    }
    let _write_owner = ClassificationWrite {
        state: state.clone(),
        keys,
    };
    {
        let mut current = state.class.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(!current.deny_unknown,"Scene asset classification needs recovery before native content can be published or fetched");
        let mut updated = current.clone();
        for hash in hashes {
            let bindings = updated.hashes.entry(hash_to_hex(&hash)).or_default();
            let binding = Binding {
                verse_id: verse.clone(),
                asset_hash,
            };
            if !bindings.contains(&binding) {
                anyhow::ensure!(bindings.len() < 16, "scene hash has more than 16 contexts");
                bindings.push(binding);
            }
        }
        anyhow::ensure!(
            serde_json::to_vec(&updated)?.len() <= 8 * 1024 * 1024,
            "scene classification exceeds 8 MiB; old classifications are never evicted"
        );
        *current = updated;
    }
    let _write = state.persist_lock.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = serde_json::to_vec(&*state.class.lock().unwrap_or_else(|e| e.into_inner()))?;
    if let Some(path) = &state.path {
        let result = (|| {
            use std::io::Write;
            let temporary = path.with_extension("tmp");
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)?;
            let mut marker = std::fs::File::create(path.with_extension("enabled"))?;
            marker.write_all(b"scene-assets-v1")?;
            marker.sync_all()?;
            #[cfg(unix)]
            std::fs::File::open(path.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "classification has no parent",
                )
            })?)?
            .sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            state.failed.store(true, Ordering::Release);
        }
        result?;
    }
    Ok::<_, anyhow::Error>(())
}
