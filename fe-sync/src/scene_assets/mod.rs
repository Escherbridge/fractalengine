//! Bounded native scene job ownership; see AGENTS.md.

mod admission;
mod transfer;
use crate::{
    blob_transport::IrohBlobTransport,
    event_outbox::{EventOutbox, EventSink},
    messages::SyncEvent,
    scene_asset_gate::SceneAssetGate,
};
use crate::scene_asset_gate::ServeDecision;
use admission::{admit, admit_job, is_busy};
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
/// Retries after a failed row-arrival prefetch (back-off 1, 2, 4, 8, 16 s).
const PREFETCH_RETRIES: u32 = 5;
/// Row-arrival prefetches in flight at once (QA-0133): unowned work never
/// takes more than this share of the `MAX_JOBS` owned-fetch budget.
const PREFETCH_CAP: usize = 8;
/// Row-arrival prefetches waiting for a free slot; beyond this they are left
/// to the on-demand fetch at petal entry.
const PREFETCH_BACKLOG_CAP: usize = 256;
/// Blobs whose fetch failed while their Verse stayed open (QA-0118): re-armed
/// with capped back-off (15, 30, 60, 60 ... s) instead of given up.
const UNAVAILABLE_CAP: usize = 1024;
const REARM_BASE: Duration = Duration::from_secs(15);
const REARM_CAP: Duration = Duration::from_secs(60);

/// One unavailable blob's re-arm schedule.
#[derive(Clone, Copy)]
struct Rearm {
    failures: u32,
    due: Instant,
}

impl Rearm {
    fn delay(failures: u32) -> Duration {
        REARM_BASE
            .saturating_mul(1 << failures.saturating_sub(1).min(8))
            .min(REARM_CAP)
    }
}

type Unavailable = Arc<Mutex<HashMap<(String, BlobHash), Rearm>>>;

/// The wake of each fetch job sleeping between prefetch attempts (QA-0143).
type Wakes = Arc<Mutex<HashMap<(String, BlobHash), Arc<tokio::sync::Notify>>>>;

/// A fetch job's registered wake; unregistered (only if still its own) when the job ends.
struct WakeRegistration {
    wakes: Wakes,
    key: (String, BlobHash),
    wake: Arc<tokio::sync::Notify>,
}

impl WakeRegistration {
    fn new(wakes: &Wakes, verse: &str, hash: BlobHash) -> Self {
        let key = (verse.to_string(), hash);
        let wake = Arc::new(tokio::sync::Notify::new());
        wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), wake.clone());
        Self { wakes: wakes.clone(), key, wake }
    }
}

impl Drop for WakeRegistration {
    fn drop(&mut self) {
        let mut wakes = self.wakes.lock().unwrap_or_else(|e| e.into_inner());
        if wakes.get(&self.key).is_some_and(|wake| Arc::ptr_eq(wake, &self.wake)) {
            wakes.remove(&self.key);
        }
    }
}

/// Record a failed fetch of an open Verse's blob for a later re-arm.
fn note_unavailable(unavailable: &Unavailable, verse: &str, hash: BlobHash) {
    let mut map = unavailable.lock().unwrap_or_else(|e| e.into_inner());
    let key = (verse.to_string(), hash);
    if map.len() >= UNAVAILABLE_CAP && !map.contains_key(&key) {
        tracing::warn!(verse, "scene asset re-arm set full; this blob waits for petal entry");
        return;
    }
    let failures = map.get(&key).map_or(0, |rearm| rearm.failures).saturating_add(1);
    map.insert(
        key,
        Rearm {
            failures,
            due: Instant::now() + Rearm::delay(failures),
        },
    );
}

/// One in-flight row-arrival prefetch; released when its job ends.
struct PrefetchSlot(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for PrefetchSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

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
        let peers = match &self.roster {
            Some(roster) => roster.peers()?,
            None => Vec::new(),
        };
        // Sub-second clock nanos rotate the window (the replicator's jitter idiom).
        let rotation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos() as usize)
            .unwrap_or(0);
        provider_window(&self.providers, peers, rotation)
    }
}

/// The explicit providers, then a window of roster peers rotated by `rotation`,
/// at most 16 in all (QA-0136): more than 16 collaborators never disables
/// fetching, and successive fetches (prefetch retries, petal entry) sample
/// different windows, so every confirmed peer is eventually asked.
pub(crate) fn provider_window(
    explicit: &[ProviderRef],
    mut roster: Vec<iroh::EndpointId>,
    rotation: usize,
) -> anyhow::Result<Vec<ProviderRef>> {
    anyhow::ensure!(
        explicit.len() <= 16,
        "More than 16 explicit native scene providers"
    );
    let mut providers = explicit.to_vec();
    if !roster.is_empty() {
        let offset = rotation % roster.len();
        roster.rotate_left(offset);
    }
    for peer in roster {
        if providers.len() >= 16 {
            break;
        }
        if !providers.iter().any(|provider| {
            IrohBlobTransport::parse_provider(provider).is_ok_and(|address| address.id == peer)
        }) {
            providers.push(IrohBlobTransport::provider_ref(&iroh::EndpointAddr::new(peer))?);
        }
    }
    Ok(providers)
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
    prefetches: Arc<std::sync::atomic::AtomicUsize>,
    prefetch_backlog: std::collections::VecDeque<(String, BlobHash)>,
    unavailable: Unavailable,
    wakes: Wakes,
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
                let Some(lease) = lease else { return ServeDecision::Deny };
                if lease.stop.is_requested() {
                    return ServeDecision::Deny;
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
                match result {
                    _ if lease.stop.is_requested() => ServeDecision::Deny,
                    Ok(_) => ServeDecision::Admit,
                    Err(error) if is_busy(&error) => ServeDecision::Unavailable,
                    Err(_) => ServeDecision::Deny,
                }
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
            prefetches: Default::default(),
            prefetch_backlog: Default::default(),
            unavailable: Default::default(),
            wakes: Default::default(),
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
        self.prefetch_backlog.retain(|(waiting, _)| waiting != verse);
        self.unavailable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(waiting, _), _| waiting != verse);
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

    /// Start backlogged and due re-armed prefetches while slots are free;
    /// true while any wait (the sync loop then wakes on its kept-work timer).
    pub(crate) fn pump_prefetches(&mut self) -> bool {
        let now = Instant::now();
        let due: Vec<(String, BlobHash)> = {
            let mut map = self.unavailable.lock().unwrap_or_else(|e| e.into_inner());
            let due: Vec<_> = map
                .iter()
                .filter(|(_, rearm)| rearm.due <= now)
                .map(|(key, _)| key.clone())
                .collect();
            // Not due again until this attempt fails (which reschedules it).
            for key in &due {
                if let Some(rearm) = map.get_mut(key) {
                    rearm.due = now + Rearm::delay(rearm.failures.saturating_add(1));
                }
            }
            due
        };
        for (verse, hash) in due {
            if self.blobs.has_blob(&hash) {
                self.unavailable
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(verse, hash));
            } else {
                self.prefetch(&verse, hash);
            }
        }
        while self.prefetches.load(std::sync::atomic::Ordering::Relaxed) < PREFETCH_CAP {
            let Some((verse, hash)) = self.prefetch_backlog.pop_front() else {
                break;
            };
            self.prefetch(&verse, hash);
        }
        !self.prefetch_backlog.is_empty()
            || !self
                .unavailable
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    /// QA-0118: a new provider may now serve `verse`'s unavailable blobs (a
    /// neighbor joined, or the role view changed): retry them on the next pump.
    pub(crate) fn rearm(&mut self, verse: &str) {
        let now = Instant::now();
        for ((waiting, _), rearm) in self
            .unavailable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
        {
            if waiting == verse {
                rearm.due = now;
            }
        }
        for ((waiting, _), wake) in self.wakes.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if waiting == verse {
                wake.notify_one();
            }
        }
    }

    /// QA-0143: a fetch sleeping between attempts retries now — its row was
    /// written again or someone asked for the blob — instead of after its back-off.
    fn wake(&self, verse: &str, hash: BlobHash) {
        if let Some(wake) = self
            .wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(verse.to_string(), hash))
        {
            wake.notify_one();
        }
    }

    /// QA-0118: fetch a scene blob as soon as its replicated metadata row
    /// arrives, before any petal is opened. Unowned; retried with bounded
    /// back-off while the Verse stays open. Bytes already held are skipped,
    /// and at most `PREFETCH_CAP` run at once (QA-0133): the rest wait in a
    /// bounded backlog drained by [`Self::pump_prefetches`].
    pub(crate) fn prefetch(&mut self, verse: &str, hash: BlobHash) {
        if self.blobs.has_blob(&hash) {
            return;
        }
        self.wake(verse, hash);
        if self.prefetches.load(std::sync::atomic::Ordering::Relaxed) >= PREFETCH_CAP {
            let waiting = (verse.to_string(), hash);
            if self.prefetch_backlog.len() < PREFETCH_BACKLOG_CAP
                && !self.prefetch_backlog.contains(&waiting)
            {
                self.prefetch_backlog.push_back(waiting);
            }
            return;
        }
        let Some(generation) = self
            .open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(verse)
            .map(|lease| lease.generation)
        else {
            return;
        };
        tracing::info!(
            verse,
            hash = %fe_runtime::blob_store::hash_to_hex(&hash),
            "scene asset row arrived; prefetching its blob"
        );
        self.enqueue_with(
            Job::Fetch {
                verse: verse.into(),
                hash,
                generation,
            },
            None,
            true,
        );
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
        self.enqueue_with(key, request_id, false);
    }

    fn enqueue_with(&mut self, key: Job, request_id: Option<String>, prefetch: bool) {
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
                if let Job::Fetch { verse, hash, .. } = &key {
                    self.wake(verse, *hash);
                }
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
        let prefetch_slot = prefetch.then(|| {
            self.prefetches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            PrefetchSlot(self.prefetches.clone())
        });
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
        let unavailable = self.unavailable.clone();
        let wake = match &key {
            Job::Fetch { verse, hash, .. } => Some(WakeRegistration::new(&self.wakes, verse, *hash)),
            Job::PublishRow { .. } => None,
        };
        self.jobs.spawn(async move {
            let _prefetch_slot = prefetch_slot;
            let deadline = Instant::now() + JOB_TIMEOUT;
            let future = async {
                match &key {
                    Job::PublishRow { verse, hash, generation } => {
                        let job_lease = JobLease {
                            stop: lease.stop.clone(),
                            io_owner: Arc::new(slots.clone().acquire_owned().await?),
                        };
                        let record_id = publish(&blobs, &transport, &gate, &db, verse, *hash, &job_lease).await?;
                        publications.try_send(SceneAssetPublication {
                            verse: verse.clone(), row_hash: *hash, record_id, generation: *generation,
                        }).map_err(|error| anyhow::anyhow!("Scene asset metadata publication queue unavailable: {error}"))?;
                        Ok::<_, anyhow::Error>((job_lease, None))
                    }
                    Job::Fetch { verse, hash, .. } => {
                        // A prefetch retries without holding one of the four IO slots while it waits.
                        let mut attempt = 0;
                        loop {
                            let job_lease = JobLease {
                                stop: lease.stop.clone(),
                                io_owner: Arc::new(slots.clone().acquire_owned().await?),
                            };
                            let tried = async {
                                let permit = admit_job(&db, verse, *hash, SceneAssetTransferPurpose::Fetch).await?;
                                // Subscription startup and a newly joined peer may settle after metadata arrives.
                                for attempt in 0..20 {
                                    if lease.providers().is_ok_and(|providers| !providers.is_empty()) { break; }
                                    if attempt < 19 { tokio::time::sleep(Duration::from_millis(100)).await; }
                                }
                                let providers = lease.providers()?;
                                let bytes = fetch(&transport, &gate, verse, *hash, &permit, &providers, &job_lease).await?;
                                Ok::<_, anyhow::Error>((permit, bytes))
                            }.await;
                            match tried {
                                Ok(work) => break Ok((job_lease, Some(work))),
                                Err(error) if prefetch && attempt < PREFETCH_RETRIES => {
                                    drop(job_lease);
                                    let delay = Duration::from_secs(1 << attempt);
                                    attempt += 1;
                                    tracing::info!(verse, hash = %fe_runtime::blob_store::hash_to_hex(hash), attempt, ?delay,
                                        "scene asset prefetch failed: {error:#}; retrying");
                                    match &wake {
                                        Some(registration) => tokio::select! {
                                            _ = tokio::time::sleep(delay) => {}
                                            _ = registration.wake.notified() => {}
                                        },
                                        None => tokio::time::sleep(delay).await,
                                    }
                                }
                                Err(error) => break Err(error),
                            }
                        }
                    }
                }
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
                if !lease.stop.is_requested() {
                    match &result {
                        Ok(()) => {
                            unavailable
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&(key.verse().to_string(), key.hash()));
                        }
                        // QA-0118: never a terminal state while the Verse stays open.
                        Err(_) => note_unavailable(&unavailable, key.verse(), key.hash()),
                    }
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
