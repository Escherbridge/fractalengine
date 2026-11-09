//! `blob.source` and `sync.tileset-source`: extension bytes asked only after the peers missed, kept only when BLAKE3
//! and size match (bindings.md §6, BND-10d); rationale in `fe-plugin/src/bindings/AGENTS.md` §blob-source.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use crossbeam::channel::{Receiver, Sender, TrySendError};
use fe_runtime::bevy_blob_reader::{
    verify_fallback_bytes, BlobFallback, BlobSourceRequest, FallbackFuture, TilesetChildRequest, BLOB_SOURCE_MAX_BYTES,
};
use fe_runtime::blob_store::{hash_to_hex, BlobHash};
use fe_runtime::extension_binding::BindingPoint;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::activation::{BindingActivator, BindingLauncher, CallTarget};
use crate::lifecycle::PluginManager;
use crate::wasm::component::hooks::{HookCallError, HookReply, HookRequest};

/// Catalogue id of the blob source hook.
pub const BLOB_SOURCE_HOOK: &str = "blob.source";
/// Catalogue id of the tileset child source hook.
pub const TILESET_SOURCE_HOOK: &str = "sync.tileset-source";
/// Asks waiting for the plugin worker; past it an ask answers `None` at once (counted).
pub const SOURCE_QUEUE_CAPACITY: usize = 64;
/// Source calls running off the worker at once; further asks wait in the queue.
pub const MAX_SOURCE_CALLS_IN_FLIGHT: usize = 4;
/// One ask's whole budget, every bound source included: the hooks' 30 s deadline.
pub const SOURCE_BUDGET: Duration = Duration::from_secs(30);

/// Delivers one hook call to the running instance `target.plugin_id`. The binary implements it over `HooksGuest`,
/// taking that instance's lock, refusing while it is degraded or once its lease is revoked (a stopped instance takes no
/// new call, OD-E1) and degrading it on `HookCallError::Fault`. It runs on a call thread: never the plugin worker, the
/// DB thread or the render loop.
pub trait HookCaller: Send + Sync {
    fn call(&self, target: &CallTarget, request: &HookRequest) -> Result<HookReply, HookCallError>;
}

/// Off-worker call slots shared by one service's call threads.
#[derive(Clone, Default)]
pub(crate) struct CallSlots(Arc<AtomicUsize>);

/// One taken slot; dropping it frees the slot.
pub(crate) struct CallSlot(Arc<AtomicUsize>);

impl Drop for CallSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl CallSlots {
    /// A slot while fewer than `cap` are taken.
    pub(crate) fn try_take(&self, cap: usize) -> Option<CallSlot> {
        let mut taken = self.0.load(Ordering::Acquire);
        loop {
            if taken >= cap {
                return None;
            }
            match self.0.compare_exchange(taken, taken + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(CallSlot(Arc::clone(&self.0))),
                Err(seen) => taken = seen,
            }
        }
    }
}

/// Run `work` on a named call thread that holds `slot`; `false` when the thread could not start (`work` is dropped).
pub(crate) fn spawn_call(name: &str, slot: CallSlot, work: impl FnOnce() + Send + 'static) -> bool {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _slot = slot;
            work();
        })
        .is_ok()
}

/// Points of `hook` that resolve to an extension at the petal, in key order; nothing starts here.
pub(crate) fn resolved_points(activator: &BindingActivator, petal_id: &str, hook: &str) -> Vec<BindingPoint> {
    activator
        .resolutions_at(petal_id)
        .into_iter()
        .filter(|(point, resolved)| point.hook == hook && resolved.resolution.is_extension())
        .map(|(point, _)| point)
        .collect()
}

/// Each point's call target, lazily starting its instance (bindings.md §5.6); a refused start serves nothing.
pub(crate) fn call_targets(
    activator: &mut BindingActivator,
    manager: &Mutex<PluginManager>,
    launcher: &mut dyn BindingLauncher,
    petal_id: &str,
    points: Vec<BindingPoint>,
) -> Vec<(BindingPoint, CallTarget)> {
    let mut targets = Vec::with_capacity(points.len());
    for point in points {
        if let Ok(target) = activator.ensure_call_target(petal_id, &point, manager, launcher) {
            targets.push((point, target));
        }
    }
    targets
}

/// The request the host builds for one bound point; `config` was validated by the resolver.
pub(crate) fn hook_request(point: &BindingPoint, target: &CallTarget, input: &str, body: Vec<u8>) -> HookRequest {
    HookRequest {
        hook: point.hook.clone(),
        key: point.key.clone(),
        binding_scope: target.binding_scope.clone(),
        config: target.config.to_string(),
        input: input.to_string(),
        body,
    }
}

/// One ask for bytes the peers missed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceAsk {
    Blob(BlobSourceRequest),
    TilesetChild(TilesetChildRequest),
}

impl SourceAsk {
    /// The hook that serves this ask.
    pub fn hook(&self) -> &'static str {
        match self {
            Self::Blob(_) => BLOB_SOURCE_HOOK,
            Self::TilesetChild(_) => TILESET_SOURCE_HOOK,
        }
    }

    /// The hash the answer must have.
    pub fn hash(&self) -> BlobHash {
        match self {
            Self::Blob(request) => request.hash,
            Self::TilesetChild(request) => request.child_hash,
        }
    }

    fn expected_size(&self) -> Option<u64> {
        match self {
            Self::Blob(request) => request.expected_size,
            Self::TilesetChild(request) => request.size,
        }
    }

    fn max_bytes(&self) -> u64 {
        match self {
            Self::Blob(_) => BLOB_SOURCE_MAX_BYTES,
            Self::TilesetChild(request) => request.max_bytes,
        }
    }

    /// The petal whose bindings pick the source; tileset children carry none (the active petal decides).
    fn petal_id(&self) -> Option<&str> {
        match self {
            Self::Blob(request) => request.petal_id.as_deref(),
            Self::TilesetChild(_) => None,
        }
    }

    /// The hook input (bindings.md §6); absent facts are `null`.
    pub fn input(&self) -> Value {
        match self {
            Self::Blob(request) => json!({
                "hash": hash_to_hex(&request.hash),
                "expected_size": request.expected_size,
                "asset_id": request.asset_id,
            }),
            Self::TilesetChild(request) => json!({
                "tileset_id": request.tileset_id,
                "root": hash_to_hex(&request.root),
                "child_hash": hash_to_hex(&request.child_hash),
                "size": request.size,
                "max_size": request.max_bytes,
            }),
        }
    }
}

/// Counters of one source hook (extension status, R11).
#[derive(Debug, Default)]
struct SourceTally {
    asked: AtomicU64,
    served: AtomicU64,
    /// A guest declined or answered no bytes.
    declined: AtomicU64,
    /// Bytes that failed the size or BLAKE3 check: never used.
    refused: AtomicU64,
    /// Faults, timeouts, bad output, or a call thread that never started.
    failed: AtomicU64,
    /// Nothing resolved at the petal: the built-in (peers only) stands.
    unbound: AtomicU64,
    /// Queue full, worker gone, or the ask outlived its budget before it was served.
    dropped: AtomicU64,
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Default)]
struct SourceTallies {
    blob: SourceTally,
    tileset: SourceTally,
}

impl SourceTallies {
    fn of(&self, hook: &str) -> &SourceTally {
        if hook == TILESET_SOURCE_HOOK {
            &self.tileset
        } else {
            &self.blob
        }
    }

    fn rows(&self) -> Vec<(String, u64)> {
        let mut rows = Vec::with_capacity(14);
        for (hook, tally) in [(BLOB_SOURCE_HOOK, &self.blob), (TILESET_SOURCE_HOOK, &self.tileset)] {
            for (name, counter) in [
                ("asked", &tally.asked),
                ("served", &tally.served),
                ("declined", &tally.declined),
                ("refused", &tally.refused),
                ("failed", &tally.failed),
                ("unbound", &tally.unbound),
                ("dropped", &tally.dropped),
            ] {
                rows.push((format!("{hook}.{name}"), counter.load(Ordering::Relaxed)));
            }
        }
        rows
    }
}

/// One ask waiting for the plugin worker.
struct SourceJob {
    ask: SourceAsk,
    queued_at: Instant,
    reply: oneshot::Sender<Option<Bytes>>,
}

/// The `BlobFallback` the sync thread and the blob reader hold in their slot. An ask waits for the worker's answer
/// and resolves `None` when nothing bound serves; enqueueing never blocks the caller.
#[derive(Clone)]
pub struct BlobSourceQueue {
    jobs: Sender<SourceJob>,
    tallies: Arc<SourceTallies>,
}

impl BlobSourceQueue {
    fn ask(&self, ask: SourceAsk) -> FallbackFuture<'_> {
        let tally = self.tallies.of(ask.hook());
        bump(&tally.asked);
        let (reply, answer) = oneshot::channel();
        match self.jobs.try_send(SourceJob { ask, queued_at: Instant::now(), reply }) {
            Ok(()) => Box::pin(async move { answer.await.ok().flatten() }),
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                bump(&tally.dropped);
                Box::pin(async { None })
            }
        }
    }
}

impl BlobFallback for BlobSourceQueue {
    fn source_blob(&self, request: BlobSourceRequest) -> FallbackFuture<'_> {
        self.ask(SourceAsk::Blob(request))
    }

    fn source_tileset_child(&self, request: TilesetChildRequest) -> FallbackFuture<'_> {
        self.ask(SourceAsk::TilesetChild(request))
    }
}

/// The plugin-worker half: resolves each ask on the worker, then asks the bound sources on a call thread.
pub struct BlobSourceService {
    jobs: Receiver<SourceJob>,
    slots: CallSlots,
    tallies: Arc<SourceTallies>,
}

/// The queue to install in the hosts' `BlobFallbackSlot`, and the service the plugin worker pumps.
pub fn blob_source_channel() -> (BlobSourceQueue, BlobSourceService) {
    let (jobs, waiting) = crossbeam::channel::bounded(SOURCE_QUEUE_CAPACITY);
    let tallies = Arc::new(SourceTallies::default());
    (
        BlobSourceQueue { jobs, tallies: Arc::clone(&tallies) },
        BlobSourceService { jobs: waiting, slots: CallSlots::default(), tallies },
    )
}

impl BlobSourceService {
    /// Serve waiting asks while call slots are free; an ask naming no petal resolves at `active_petal`. Returns the
    /// asks taken off the queue.
    pub fn pump(
        &self,
        activator: &mut BindingActivator,
        manager: &Mutex<PluginManager>,
        launcher: &mut dyn BindingLauncher,
        caller: &Arc<dyn HookCaller>,
        active_petal: Option<&str>,
    ) -> usize {
        let mut taken = 0;
        loop {
            let Some(slot) = self.slots.try_take(MAX_SOURCE_CALLS_IN_FLIGHT) else { break };
            let Ok(job) = self.jobs.try_recv() else { break };
            taken += 1;
            let SourceJob { ask, queued_at, reply } = job;
            let tally = self.tallies.of(ask.hook());
            let deadline = queued_at + SOURCE_BUDGET;
            if Instant::now() >= deadline || reply.is_closed() {
                bump(&tally.dropped);
                let _ = reply.send(None);
                continue;
            }
            let Some(petal_id) = ask.petal_id().or(active_petal).map(str::to_string) else {
                bump(&tally.unbound);
                let _ = reply.send(None);
                continue;
            };
            let points = resolved_points(activator, &petal_id, ask.hook());
            let targets = call_targets(activator, manager, launcher, &petal_id, points);
            if targets.is_empty() {
                bump(&tally.unbound);
                let _ = reply.send(None);
                continue;
            }
            let caller = Arc::clone(caller);
            let tallies = Arc::clone(&self.tallies);
            let started = spawn_call("fe-blob-source", slot, move || {
                let answer = serve_ask(caller.as_ref(), &targets, &ask, deadline, tallies.of(ask.hook()));
                let _ = reply.send(answer);
            });
            if !started {
                bump(&tally.failed);
            }
        }
        taken
    }

    /// `<hook>.asked|served|declined|refused|failed|unbound|dropped` for both source hooks.
    pub fn status_rows(&self) -> Vec<(String, u64)> {
        self.tallies.rows()
    }
}

/// Ask each bound source in key order until one answers bytes that pass the size and BLAKE3 check.
fn serve_ask(
    caller: &dyn HookCaller,
    targets: &[(BindingPoint, CallTarget)],
    ask: &SourceAsk,
    deadline: Instant,
    tally: &SourceTally,
) -> Option<Bytes> {
    let input = ask.input().to_string();
    let hash = ask.hash();
    for (point, target) in targets {
        if Instant::now() >= deadline {
            bump(&tally.failed);
            break;
        }
        match caller.call(target, &hook_request(point, target, &input, Vec::new())) {
            Ok(reply) if reply.body.is_empty() => bump(&tally.declined),
            Ok(reply) => match verify_fallback_bytes(&hash, ask.expected_size(), ask.max_bytes(), &reply.body) {
                Ok(()) => {
                    bump(&tally.served);
                    return Some(Bytes::from(reply.body));
                }
                Err(rejection) => {
                    bump(&tally.refused);
                    tracing::warn!(
                        hook = %point.hook, key = %point.key, package = %target.key.package_id,
                        hash = %hash_to_hex(&hash), %rejection, "source bytes refused"
                    );
                }
            },
            Err(HookCallError::Guest(_)) => bump(&tally.declined),
            Err(error) => {
                bump(&tally.failed);
                tracing::warn!(
                    hook = %point.hook, key = %point.key, package = %target.key.package_id, %error,
                    "source call failed"
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::component::hooks::HookRefusal;

    /// Each source answers by its key: forged bytes, nothing, a refusal, or the genuine blob.
    struct Sources {
        genuine: Vec<u8>,
        calls: Mutex<Vec<String>>,
    }

    impl HookCaller for Sources {
        fn call(&self, _target: &CallTarget, request: &HookRequest) -> Result<HookReply, HookCallError> {
            self.calls.lock().unwrap().push(request.key.clone());
            let body = match request.key.as_str() {
                "a-forged" => b"not the requested blob".to_vec(),
                "b-empty" => Vec::new(),
                "c-refuses" => return Err(HookCallError::Guest(HookRefusal::Refused("not here".into()))),
                _ => self.genuine.clone(),
            };
            Ok(HookReply { output: "{}".into(), body })
        }
    }

    fn bound(key: &str) -> (BindingPoint, CallTarget) {
        let target = CallTarget {
            plugin_id: format!("inst@{key}"),
            installation_id: "inst".into(),
            key: crate::bus::InstanceKey::new("petal".to_string(), "org.example.cdn".to_string()),
            binding_scope: "VERSE#v".into(),
            config: json!({}),
        };
        (BindingPoint::new(BLOB_SOURCE_HOOK, key), target)
    }

    /// Sources are asked in key order after the peers missed; forged, empty and refused answers fall through and
    /// are counted, and only bytes hashing to the request (at its size) come back.
    #[test]
    fn only_bytes_that_hash_to_the_request_are_kept() {
        let genuine = b"genuine scene blob".to_vec();
        let hash: BlobHash = *blake3::hash(&genuine).as_bytes();
        let sources = Sources { genuine: genuine.clone(), calls: Mutex::new(Vec::new()) };
        let targets: Vec<_> = ["a-forged", "b-empty", "c-refuses", "d-mirror"].into_iter().map(bound).collect();
        let tally = SourceTally::default();
        let ask = |expected_size| {
            SourceAsk::Blob(BlobSourceRequest { hash, expected_size, asset_id: Some("asset-1".into()), petal_id: None })
        };
        let far = Instant::now() + SOURCE_BUDGET;

        let served = serve_ask(&sources, &targets, &ask(Some(genuine.len() as u64)), far, &tally);
        assert_eq!(served.as_deref(), Some(genuine.as_slice()));
        assert_eq!(*sources.calls.lock().unwrap(), ["a-forged", "b-empty", "c-refuses", "d-mirror"]);
        let counts = |tally: &SourceTally| {
            [&tally.served, &tally.declined, &tally.refused, &tally.failed].map(|counter| counter.load(Ordering::Relaxed))
        };
        assert_eq!(counts(&tally), [1, 2, 1, 0]);

        // The genuine bytes at the wrong size, and an ask already past its budget, both answer nothing.
        assert_eq!(serve_ask(&sources, &targets[3..], &ask(Some(1)), far, &tally), None);
        assert_eq!(serve_ask(&sources, &targets[3..], &ask(None), Instant::now(), &tally), None);
        assert_eq!(counts(&tally), [1, 2, 2, 1]);
        assert_eq!(ask(None).input()["hash"], json!(hash_to_hex(&hash)));
        assert_eq!(ask(None).input()["expected_size"], Value::Null);
    }
}
