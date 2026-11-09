//! `egress.connector` and `blob.mirror`, the one-peer egress hooks (bindings.md §5.4, §5.6, §6, BND-10d); rationale
//! in `fe-plugin/src/bindings/AGENTS.md` §egress.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam::channel::{Receiver, Sender};
use fe_runtime::bevy_blob_reader::verify_fallback_bytes;
use fe_runtime::blob_store::{hash_to_hex, BlobHash, BlobStoreHandle};
use fe_runtime::extension_binding::{BindingOffer, BindingPoint, BindingResolution, UnresolvedReason};
use serde_json::json;

use super::activation::{BindingActivator, BindingLauncher, CallTarget, SharedEgressBuckets};
use super::blob_source::{call_targets, hook_request, resolved_points, spawn_call, CallSlots, HookCaller};
use crate::bus::rate::TokenBucket;
use crate::bus::InstanceKey;
use crate::lifecycle::PluginManager;
use crate::wasm::component::hooks::HookCallError;

/// Catalogue id of the push connector hook.
pub const EGRESS_CONNECTOR_HOOK: &str = "egress.connector";
/// Catalogue id of the blob mirror hook.
pub const BLOB_MIRROR_HOOK: &str = "blob.mirror";
/// Largest blob handed to a mirror (the hook's 8 MiB body cap); larger blobs are skipped and counted.
pub const MAX_MIRROR_BYTES: u64 = 8 * 1024 * 1024;
/// Committed blobs waiting for the plugin worker; past it a commit is dropped (counted).
pub const MIRROR_QUEUE_CAPACITY: usize = 256;
/// Mirror calls running off the worker at once.
pub const MAX_MIRROR_CALLS_IN_FLIGHT: usize = 2;

/// The egress rate gate: an instance a binding keeps draws from its installation's shared bucket for `endpoint`, so
/// one budget covers every petal the binding reaches; any other instance draws from `own`. `rate_hz` is the
/// endpoint's host-clamped rate.
pub fn take_egress_token(
    shared: &SharedEgressBuckets,
    key: &InstanceKey,
    endpoint: &str,
    rate_hz: f64,
    now_ms: u64,
    own: &mut TokenBucket,
) -> bool {
    shared.try_take(key, endpoint, rate_hz, now_ms).unwrap_or_else(|| own.try_take(now_ms))
}

/// What one bound connector does at a petal on this host (the egress card row `binding:egress.connector:<key>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorState {
    /// This host pushes the petal's batch; `sharing` instances of the installation draw from one bucket.
    Pushing { package_id: String, scope: String, sharing: usize },
    /// Another peer pushes (one-peer placement); nothing is pushed from here.
    Elsewhere { runs_on: String },
    /// A `builtin` row masks outer connectors at this petal.
    Masked { scope: String },
    /// The binding cannot run here; no push, at most one offer.
    Unresolved { package_id: String, scope: String, reason: UnresolvedReason, offer: Option<BindingOffer> },
}

/// One connector point at a petal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRow {
    pub key: String,
    pub state: ConnectorState,
}

/// Every bound connector at the petal, by key. The pull API (query, exports, share URLs) is core and never a row (R33).
pub fn connector_rows(activator: &BindingActivator, petal_id: &str) -> Vec<ConnectorRow> {
    let buckets = activator.egress_buckets();
    activator
        .resolutions_at(petal_id)
        .into_iter()
        .filter(|(point, _)| point.hook == EGRESS_CONNECTOR_HOOK)
        .filter_map(|(point, resolved)| {
            let state = match resolved.resolution {
                BindingResolution::Resolved { package_id, scope } => {
                    let sharing = resolved.detail.installation_id.as_deref().map_or(0, |id| buckets.members(id));
                    ConnectorState::Pushing { package_id, scope, sharing }
                }
                BindingResolution::Elsewhere { runs_on } => ConnectorState::Elsewhere { runs_on },
                BindingResolution::Builtin { scope: Some(scope) } => ConnectorState::Masked { scope },
                BindingResolution::Builtin { scope: None } => return None,
                BindingResolution::Unresolved { package_id, scope, reason, offer } => {
                    ConnectorState::Unresolved { package_id, scope, reason, offer }
                }
            };
            Some(ConnectorRow { key: point.key, state })
        })
        .collect()
}

/// A blob that became durable on this host, as `blob.mirror` sees it (after commit, bindings.md §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorJob {
    /// The asset's petal: its bindings pick the mirrors.
    pub petal_id: String,
    pub asset_id: String,
    pub hash: BlobHash,
    pub size: u64,
    pub content_type: String,
}

/// Counters of `blob.mirror` (extension status, R11).
#[derive(Debug, Default)]
struct MirrorTally {
    /// Calls a mirror accepted.
    mirrored: AtomicU64,
    /// Bound blobs over [`MAX_MIRROR_BYTES`], never handed over.
    skipped_large: AtomicU64,
    /// Local bytes missing or not matching the commit's hash and size, never handed over.
    unreadable: AtomicU64,
    /// Calls the guest declined.
    declined: AtomicU64,
    /// Faults, timeouts, bad output, or a call thread that never started.
    failed: AtomicU64,
    /// Commits dropped on a full queue or a stopped worker.
    dropped: AtomicU64,
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// The binary's after-commit tap; it never blocks the committing thread.
#[derive(Clone)]
pub struct MirrorQueue {
    jobs: Sender<MirrorJob>,
    tally: Arc<MirrorTally>,
}

impl MirrorQueue {
    /// Queue one committed blob; `false` when it was dropped (counted).
    pub fn after_commit(&self, job: MirrorJob) -> bool {
        let queued = self.jobs.try_send(job).is_ok();
        if !queued {
            bump(&self.tally.dropped);
        }
        queued
    }
}

/// The plugin-worker half of `blob.mirror`: resolves on the worker, reads and calls on a call thread.
pub struct BlobMirror {
    jobs: Receiver<MirrorJob>,
    blobs: BlobStoreHandle,
    slots: CallSlots,
    tally: Arc<MirrorTally>,
}

/// The binary's after-commit queue and the mirror service the plugin worker pumps.
pub fn blob_mirror_channel(blobs: BlobStoreHandle) -> (MirrorQueue, BlobMirror) {
    let (jobs, waiting) = crossbeam::channel::bounded(MIRROR_QUEUE_CAPACITY);
    let tally = Arc::new(MirrorTally::default());
    (
        MirrorQueue { jobs, tally: Arc::clone(&tally) },
        BlobMirror { jobs: waiting, blobs, slots: CallSlots::default(), tally },
    )
}

impl BlobMirror {
    /// Hand committed blobs to the mirrors bound at their petal while call slots are free. An unbound petal costs
    /// nothing and starts nothing; a bound blob over 8 MiB is skipped and counted. Returns the jobs taken.
    pub fn pump(
        &self,
        activator: &mut BindingActivator,
        manager: &Mutex<PluginManager>,
        launcher: &mut dyn BindingLauncher,
        caller: &Arc<dyn HookCaller>,
    ) -> usize {
        let mut taken = 0;
        loop {
            let Some(slot) = self.slots.try_take(MAX_MIRROR_CALLS_IN_FLIGHT) else { break };
            let Ok(job) = self.jobs.try_recv() else { break };
            taken += 1;
            let points = resolved_points(activator, &job.petal_id, BLOB_MIRROR_HOOK);
            if points.is_empty() {
                continue;
            }
            if job.size > MAX_MIRROR_BYTES {
                bump(&self.tally.skipped_large);
                continue;
            }
            let targets = call_targets(activator, manager, launcher, &job.petal_id, points);
            if targets.is_empty() {
                continue;
            }
            let (blobs, caller, tally) = (self.blobs.clone(), Arc::clone(caller), Arc::clone(&self.tally));
            let started = spawn_call("fe-blob-mirror", slot, move || {
                mirror_blob(&blobs, caller.as_ref(), &targets, &job, &tally);
            });
            if !started {
                bump(&self.tally.failed);
            }
        }
        taken
    }

    /// `blob.mirror.mirrored|skipped_large|unreadable|declined|failed|dropped`.
    pub fn status_rows(&self) -> Vec<(String, u64)> {
        let tally = &self.tally;
        [
            ("mirrored", &tally.mirrored),
            ("skipped_large", &tally.skipped_large),
            ("unreadable", &tally.unreadable),
            ("declined", &tally.declined),
            ("failed", &tally.failed),
            ("dropped", &tally.dropped),
        ]
        .into_iter()
        .map(|(name, counter)| (format!("{BLOB_MIRROR_HOOK}.{name}"), counter.load(Ordering::Relaxed)))
        .collect()
    }
}

/// Read the committed bytes (checked against the commit's hash and size), then call each bound mirror once.
fn mirror_blob(
    blobs: &BlobStoreHandle,
    caller: &dyn HookCaller,
    targets: &[(BindingPoint, CallTarget)],
    job: &MirrorJob,
    tally: &MirrorTally,
) {
    let bytes = match read_committed(blobs, &job.hash, job.size) {
        Ok(bytes) => bytes,
        Err(reason) => {
            bump(&tally.unreadable);
            tracing::warn!(asset_id = %job.asset_id, hash = %hash_to_hex(&job.hash), %reason, "blob not mirrored");
            return;
        }
    };
    let input = json!({
        "hash": hash_to_hex(&job.hash),
        "size": job.size,
        "content_type": job.content_type,
        "asset_id": job.asset_id,
    })
    .to_string();
    for (point, target) in targets {
        match caller.call(target, &hook_request(point, target, &input, bytes.clone())) {
            Ok(_) => bump(&tally.mirrored),
            Err(HookCallError::Guest(_)) => bump(&tally.declined),
            Err(error) => {
                bump(&tally.failed);
                tracing::warn!(key = %point.key, package = %target.key.package_id, %error, "blob mirror call failed");
            }
        }
    }
}

/// The local bytes of a committed blob, read with a cap and required to hash to it at its committed size.
fn read_committed(blobs: &BlobStoreHandle, hash: &BlobHash, size: u64) -> Result<Vec<u8>, String> {
    let path = blobs.get_blob_path(hash).ok_or_else(|| "the blob is not in the local store".to_string())?;
    let file = std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(size.min(MAX_MIRROR_BYTES) as usize);
    file.take(size.min(MAX_MIRROR_BYTES) + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    verify_fallback_bytes(hash, Some(size), MAX_MIRROR_BYTES, &bytes).map_err(|rejection| rejection.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::component::hooks::{HookReply, HookRequest};
    use serde_json::Value;

    /// A canonical on-disk store under a temp dir.
    struct DirStore(std::path::PathBuf);

    impl fe_runtime::blob_store::BlobStore for DirStore {
        fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
            let hash: BlobHash = *blake3::hash(bytes).as_bytes();
            let path = fe_runtime::blob_store::canonical_blob_path(&self.0, &hash);
            std::fs::create_dir_all(path.parent().expect("sharded path"))?;
            std::fs::write(path, bytes)?;
            Ok(hash)
        }
        fn get_blob_path(&self, hash: &BlobHash) -> Option<std::path::PathBuf> {
            let path = fe_runtime::blob_store::canonical_blob_path(&self.0, hash);
            path.exists().then_some(path)
        }
        fn has_blob(&self, hash: &BlobHash) -> bool {
            self.get_blob_path(hash).is_some()
        }
        fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Records every mirror call: its key, input and body length.
    #[derive(Default)]
    struct Mirrors(Mutex<Vec<(String, Value, usize)>>);

    impl HookCaller for Mirrors {
        fn call(&self, _target: &CallTarget, request: &HookRequest) -> Result<HookReply, HookCallError> {
            let input = serde_json::from_str(&request.input).unwrap_or(Value::Null);
            self.0.lock().unwrap().push((request.key.clone(), input, request.body.len()));
            Ok(HookReply { output: "{}".into(), body: Vec::new() })
        }
    }

    fn bound(key: &str) -> (BindingPoint, CallTarget) {
        let target = CallTarget {
            plugin_id: format!("inst@{key}"),
            installation_id: "inst".into(),
            key: InstanceKey::new("petal".to_string(), "org.example.bi-push".to_string()),
            binding_scope: "VERSE#v".into(),
            config: json!({}),
        };
        (BindingPoint::new(BLOB_MIRROR_HOOK, key), target)
    }

    /// A verse connector's petal instances share one token per endpoint while an unbound instance keeps its own; a
    /// committed blob reaches each bound mirror once with its bytes, and local bytes that do not match the commit
    /// are never handed over.
    #[test]
    fn connectors_share_one_budget_and_mirrors_get_only_committed_bytes() {
        let shared = SharedEgressBuckets::default();
        let (p1, p2, own_key) = (
            InstanceKey::new("p1", "org.example.bi-push"),
            InstanceKey::new("p2", "org.example.bi-push"),
            InstanceKey::new("p1", "org.example.local"),
        );
        shared.join(&p1, "inst");
        shared.join(&p2, "inst");
        let mut p1_own = TokenBucket::new(1.0, 0);
        let mut p2_own = TokenBucket::new(1.0, 0);
        let mut unbound_own = TokenBucket::new(1.0, 0);
        assert!(take_egress_token(&shared, &p1, "sink", 1.0, 0, &mut p1_own));
        assert!(!take_egress_token(&shared, &p2, "sink", 1.0, 0, &mut p2_own), "one budget for every petal");
        assert!(take_egress_token(&shared, &own_key, "sink", 1.0, 0, &mut unbound_own), "not a member: its own bucket");

        let root = tempfile::tempdir().unwrap();
        let blobs: BlobStoreHandle = Arc::new(DirStore(root.path().to_path_buf()));
        let committed = b"committed asset bytes".to_vec();
        let hash = blobs.add_blob(&committed).unwrap();
        let job = |size| MirrorJob {
            petal_id: "petal".into(),
            asset_id: "asset-1".into(),
            hash,
            size,
            content_type: "model/gltf-binary".into(),
        };
        let mirrors = Mirrors::default();
        let tally = MirrorTally::default();
        let targets = vec![bound("lake"), bound("warehouse")];

        mirror_blob(&blobs, &mirrors, &targets, &job(committed.len() as u64), &tally);
        // A commit naming another size, and a blob gone from the store, reach no mirror.
        mirror_blob(&blobs, &mirrors, &targets, &job(committed.len() as u64 + 1), &tally);
        mirror_blob(&blobs, &mirrors, &targets, &MirrorJob { hash: [7u8; 32], ..job(3) }, &tally);

        let calls = mirrors.0.lock().unwrap();
        let keys: Vec<&str> = calls.iter().map(|(key, _, _)| key.as_str()).collect();
        assert_eq!(keys, ["lake", "warehouse"]);
        assert!(calls.iter().all(|(_, _, body)| *body == committed.len()));
        assert_eq!(calls[0].1["hash"], json!(hash_to_hex(&hash)));
        assert_eq!(calls[0].1["asset_id"], json!("asset-1"));
        let counts = [&tally.mirrored, &tally.unreadable, &tally.failed].map(|counter| counter.load(Ordering::Relaxed));
        assert_eq!(counts, [2, 2, 0]);
    }
}
