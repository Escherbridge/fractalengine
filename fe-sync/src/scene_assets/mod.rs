//! Bounded native scene job ownership; see AGENTS.md.

mod admission;
mod transfer;
use crate::{
    blob_transport::IrohBlobTransport,
    event_outbox::{EventOutbox, EventSink},
    messages::SyncEvent,
    scene_asset_gate::SceneAssetGate,
};
use admission::{admit, admit_job};
use fe_runtime::{
    blob_store::{BlobHash, BlobStoreHandle},
    blob_transport::ProviderRef,
    messages::DbCommand,
    scene_asset::SceneAssetTransferPurpose,
    service_lifecycle::StopSignal,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
pub(crate) use transfer::is_scene_row;
use transfer::{fetch, finish_fetch, publish, JobLease};
const JOB_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_JOBS: usize = 68;

#[derive(Clone)]
struct OpenVerse {
    generation: u64,
    stop: StopSignal,
    providers: Vec<ProviderRef>,
    roster: Option<crate::replicator::SceneProviderRoster>,
}

impl OpenVerse {
    fn providers(&self) -> anyhow::Result<Vec<ProviderRef>> {
        anyhow::ensure!(
            !self.stop.is_requested(),
            "Scene provider generation closed"
        );
        let mut providers = self.providers.clone();
        if let Some(roster) = &self.roster {
            for peer in roster.peers()? {
                if !providers.iter().any(|provider| {
                    IrohBlobTransport::parse_provider(provider)
                        .is_ok_and(|address| address.id == peer)
                }) {
                    providers.push(IrohBlobTransport::provider_ref(&iroh::EndpointAddr::new(
                        peer,
                    ))?);
                }
            }
        }
        anyhow::ensure!(
            providers.len() <= 16,
            "More than 16 native scene providers; no prefix selected"
        );
        Ok(providers)
    }
}

type OpenVerses = Arc<RwLock<HashMap<String, OpenVerse>>>;

#[derive(Clone, Hash, PartialEq, Eq)]
enum Job {
    Fetch {
        verse: String,
        hash: BlobHash,
        generation: u64,
    },
    PublishRow {
        verse: String,
        hash: BlobHash,
        generation: u64,
    },
}

impl Job {
    fn verse(&self) -> &str {
        match self {
            Self::Fetch { verse, .. } | Self::PublishRow { verse, .. } => verse,
        }
    }
    fn hash(&self) -> BlobHash {
        match self {
            Self::Fetch { hash, .. } | Self::PublishRow { hash, .. } => *hash,
        }
    }
}

type PendingJobs = Arc<Mutex<HashMap<Job, Vec<String>>>>;

struct Pending {
    key: Job,
    pending: PendingJobs,
}
impl Pending {
    fn finish(&self) -> Vec<String> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key)
            .unwrap_or_default()
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.finish();
    }
}

pub(crate) struct SceneAssetPublication {
    pub(crate) verse: String,
    pub(crate) row_hash: BlobHash,
    pub(crate) record_id: String,
    generation: u64,
}

pub(crate) struct SceneAssetJobs {
    blobs: BlobStoreHandle,
    transport: Option<Arc<IrohBlobTransport>>,
    gate: SceneAssetGate,
    db: Option<crossbeam::channel::Sender<DbCommand>>,
    events: EventOutbox,
    publications: tokio::sync::mpsc::Sender<SceneAssetPublication>,
    open: OpenVerses,
    pending: PendingJobs,
    slots: Arc<tokio::sync::Semaphore>,
    jobs: tokio::task::JoinSet<()>,
    next_generation: u64,
}

impl SceneAssetJobs {
    pub(crate) fn new(
        blobs: BlobStoreHandle,
        transport: Option<Arc<IrohBlobTransport>>,
        gate: SceneAssetGate,
        db: Option<crossbeam::channel::Sender<DbCommand>>,
        events: EventOutbox,
        publications: tokio::sync::mpsc::Sender<SceneAssetPublication>,
    ) -> Self {
        let open: OpenVerses = Default::default();
        let serve_open = open.clone();
        let serve_db = db.clone();
        gate.set_authorizer(Arc::new(move |requester, verse, hash| {
            let open = serve_open.clone();
            let db = serve_db.clone();
            Box::pin(async move {
                let lease = open
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&verse)
                    .cloned();
                let Some(lease) = lease else { return false };
                if lease.stop.is_requested() {
                    return false;
                }
                let result = admit(
                    &db,
                    &verse,
                    hash,
                    SceneAssetTransferPurpose::Serve {
                        requester_did: requester,
                    },
                )
                .await;
                !lease.stop.is_requested() && result.is_ok()
            })
        }));
        Self {
            blobs,
            transport,
            gate,
            db,
            events,
            publications,
            open,
            pending: Default::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
            jobs: tokio::task::JoinSet::new(),
            next_generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn opened(&mut self, verse: &str, peers: &[iroh::EndpointAddr]) {
        self.opened_with_roster(verse, peers, None);
    }

    pub(crate) fn opened_with_roster(
        &mut self,
        verse: &str,
        peers: &[iroh::EndpointAddr],
        roster: Option<crate::replicator::SceneProviderRoster>,
    ) {
        self.close(verse);
        if peers.len() > 16 {
            tracing::warn!(
                verse,
                "scene assets unavailable: more than 16 native providers"
            );
            return;
        }
        let Some(generation) = self.next_generation.checked_add(1) else {
            tracing::error!("scene open generations exhausted");
            return;
        };
        self.next_generation = generation;
        let providers = peers
            .iter()
            .filter_map(|peer| IrohBlobTransport::provider_ref(peer).ok())
            .collect();
        self.open.write().unwrap_or_else(|e| e.into_inner()).insert(
            verse.into(),
            OpenVerse {
                generation,
                stop: StopSignal::default(),
                providers,
                roster,
            },
        );
    }

    pub(crate) fn publication_is_current(&self, publication: &SceneAssetPublication) -> bool {
        self.open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&publication.verse)
            .is_some_and(|lease| {
                lease.generation == publication.generation && !lease.stop.is_requested()
            })
    }

    pub(crate) fn close(&mut self, verse: &str) {
        if let Some(lease) = self
            .open
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(verse)
        {
            lease.stop.request();
        }
    }

    pub(crate) fn fetch(&mut self, verse: &str, hash: BlobHash) {
        self.fetch_owned(verse, hash, None);
    }

    pub(crate) fn fetch_owned(&mut self, verse: &str, hash: BlobHash, request_id: Option<String>) {
        if request_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            self.failed(verse, hash, "Invalid scene asset request owner");
            return;
        }
        let selected = if verse.is_empty() {
            let verses = self
                .open
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            if verses.len() != 1 {
                self.failed_owned(
                    verse,
                    hash,
                    request_id,
                    "Asset reader miss needs one currently opened Verse",
                );
                return;
            }
            verses[0].clone()
        } else {
            verse.into()
        };
        let generation = self
            .open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&selected)
            .map(|lease| lease.generation)
            .unwrap_or(0);
        self.enqueue(
            Job::Fetch {
                verse: selected,
                hash,
                generation,
            },
            request_id,
        );
    }

    pub(crate) fn publish_row(&mut self, verse: &str, row_hash: BlobHash) {
        let generation = self
            .open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(verse)
            .map(|lease| lease.generation)
            .unwrap_or(0);
        self.enqueue(
            Job::PublishRow {
                verse: verse.into(),
                hash: row_hash,
                generation,
            },
            None,
        );
    }

    fn failed_owned(&self, verse: &str, hash: BlobHash, request_id: Option<String>, reason: &str) {
        if let Some(request_id) = request_id {
            self.events.emit(SyncEvent::SceneAssetFetchSettled {
                verse_id: verse.into(),
                hash,
                request_ids: vec![request_id],
                result: Err(reason.into()),
            });
        }
        self.failed(verse, hash, reason);
    }

    fn failed(&self, verse: &str, hash: BlobHash, reason: &str) {
        self.events.emit(SyncEvent::SceneAssetFetchFailed {
            verse_id: verse.into(),
            hash,
            reason: reason.into(),
        });
    }

    fn enqueue(&mut self, key: Job, request_id: Option<String>) {
        while self.jobs.try_join_next().is_some() {}
        let lease = self
            .open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key.verse())
            .cloned();
        let Some(lease) = lease else {
            if matches!(key, Job::Fetch { .. }) {
                self.failed_owned(
                    key.verse(),
                    key.hash(),
                    request_id,
                    "Open the Verse online before fetching scene assets",
                );
            }
            return;
        };
        let Some(transport) = self.transport.clone() else {
            if matches!(key, Job::Fetch { .. }) {
                self.failed_owned(
                    key.verse(),
                    key.hash(),
                    request_id,
                    "Native scene blob transport unavailable",
                );
            }
            return;
        };
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(owners) = pending.get_mut(&key) {
                if let Some(request_id) = request_id {
                    if owners.len() >= 256 {
                        drop(pending);
                        self.failed_owned(
                            key.verse(),
                            key.hash(),
                            Some(request_id),
                            "Scene asset request-owner limit reached",
                        );
                        return;
                    }
                    if !owners.contains(&request_id) {
                        owners.push(request_id);
                    }
                }
                return;
            }
            if pending.len() >= MAX_JOBS {
                drop(pending);
                if matches!(key, Job::Fetch { .. }) {
                    self.failed_owned(
                        key.verse(),
                        key.hash(),
                        request_id,
                        "Scene asset queue is full; retry after current transfers finish",
                    );
                }
                return;
            }
            pending.insert(key.clone(), request_id.into_iter().collect());
        }
        let pending = Pending {
            key: key.clone(),
            pending: self.pending.clone(),
        };
        let publications = self.publications.clone();
        let blobs = self.blobs.clone();
        let db = self.db.clone();
        let events = self.events.clone();
        let gate = self.gate.clone();
        let slots = self.slots.clone();
        self.jobs.spawn(async move {
            let deadline = Instant::now() + JOB_TIMEOUT;
            let future = async {
                let job_lease = JobLease {
                    stop: lease.stop.clone(),
                    io_owner: Arc::new(slots.acquire_owned().await?),
                };
                let work = match &key {
                    Job::PublishRow { verse, hash, generation } => {
                        let record_id = publish(&blobs, &transport, &gate, &db, verse, *hash, &job_lease).await?;
                        publications.try_send(SceneAssetPublication {
                            verse: verse.clone(), row_hash: *hash, record_id, generation: *generation,
                        }).map_err(|error| anyhow::anyhow!("Scene asset metadata publication queue unavailable: {error}"))?;
                        None
                    }
                    Job::Fetch { verse, hash, .. } => {
                        let permit = admit_job(&db, verse, *hash, SceneAssetTransferPurpose::Fetch).await?;
                        // Subscription startup and a newly joined peer may settle after metadata arrives.
                        for attempt in 0..20 {
                            if lease.providers().is_ok_and(|providers| !providers.is_empty()) { break; }
                            if attempt < 19 { tokio::time::sleep(Duration::from_millis(100)).await; }
                        }
                        let providers = lease.providers()?;
                        let bytes = fetch(&transport, &gate, verse, *hash, &permit, &providers, &job_lease).await?;
                        Some((permit, bytes))
                    }
                };
                Ok::<_, anyhow::Error>((job_lease, work))
            };
            let result = tokio::select! {
                _ = lease.stop.cancelled() => Err(anyhow::anyhow!("Scene asset transfer cancelled because the Verse closed or changed")),
                result = tokio::time::timeout(JOB_TIMEOUT, future) => result
                    .map_err(|_| anyhow::anyhow!("Scene asset transfer timed out"))
                    .and_then(|result| result),
            };
            let result = match result {
                Ok((job_lease, Some((permit, bytes)))) => {
                    finish_fetch(&blobs, &db, &job_lease, deadline, permit, bytes).await
                }
                Ok((_job_lease, None)) => Ok(()),
                Err(error) => Err(error),
            };
            let owners = pending.finish();
            if matches!(key, Job::Fetch { .. }) {
                let final_result = match &result {
                    Ok(()) if !lease.stop.is_requested() && Instant::now() < deadline => Ok(()),
                    Ok(()) => Err("Scene asset transfer owner expired".into()),
                    Err(error) => Err(error.to_string()),
                };
                if !owners.is_empty() {
                    events.emit(SyncEvent::SceneAssetFetchSettled {
                        verse_id: key.verse().into(), hash: key.hash(), request_ids: owners,
                        result: final_result,
                    });
                }
                match result {
                    Ok(()) if !lease.stop.is_requested() && Instant::now() < deadline => {
                        events.emit(SyncEvent::BlobReady { hash: key.hash() });
                    }
                    Ok(()) => events.emit(SyncEvent::SceneAssetFetchFailed {
                        verse_id: key.verse().into(), hash: key.hash(),
                        reason: "Scene asset transfer owner expired".into(),
                    }),
                    Err(error) => events.emit(SyncEvent::SceneAssetFetchFailed {
                        verse_id: key.verse().into(), hash: key.hash(), reason: error.to_string(),
                    }),
                }
            } else if let Err(error) = result {
                tracing::warn!(verse = key.verse(), "scene asset publication deferred: {error:#}");
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn pending_job_count(&self) -> usize {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub(crate) async fn shutdown(mut self) {
        for (_, lease) in self.open.write().unwrap_or_else(|e| e.into_inner()).drain() {
            lease.stop.request();
        }
        while let Some(result) = self.jobs.join_next().await {
            if let Err(error) = result {
                tracing::warn!(%error,"scene asset worker failed during shutdown");
            }
        }
    }
}
