use super::*;

// ---------------------------------------------------------------------------
// The binding worker: resolution, activation, decode, snippets and governance work
// ---------------------------------------------------------------------------

/// Plugin-manager work governance hands off (never run on the Bevy thread).
pub(super) enum ManagerWork {
    Consent(ConsentRecord),
    Override {
        verse_id: String,
        content_hash: String,
    },
    Reevaluate(String),
}

/// A host-initiated hook call's answer (e.g. `petal.map` from the terrain bridge).
#[derive(Debug, Clone)]
pub(crate) enum HookAnswer {
    /// The bound instance answered with this output JSON.
    Output { package_id: String, output: String },
    /// No instance serves the point here; the built-in serves.
    Builtin,
    /// The call failed (fail open: the built-in serves).
    Failed(String),
}

pub(super) enum BindingSignal {
    Dirty(String, Trigger),
    AllDirty(Trigger),
    Navigate(Option<PetalAddress>),
    Forget(String),
    Payload(DecodeJob),
    Snippets {
        petal_id: String,
        requests: Vec<SnippetRequest>,
    },
    Call {
        petal_id: String,
        point: BindingPoint,
        input: Value,
        reply: Sender<HookAnswer>,
    },
    /// One `scene.*` gate call; the gate stops waiting at the hook's deadline and refuses (fail closed).
    Scene {
        call: SceneHookCall,
        reply: SceneHookReplySender,
    },
    /// Hold a petal an import is about to create and resolve its verse; answered after the next publication.
    Provision {
        petal: PetalAddress,
        hold: u64,
        reply: ProvisionReplySender,
    },
    /// Drop one provision hold (the import's judgment is over).
    Release { petal_id: String, hold: u64 },
    /// One `asset.import` call carrying the uploaded bytes; the GLB comes back in the reply body.
    Import {
        petal_id: String,
        mime: String,
        input: Value,
        body: Vec<u8>,
        reply: Sender<WorkerAnswer>,
        /// Held until the call thread ends; caps concurrent conversions (each carries up to 16 MiB).
        slot: ImportSlot,
    },
    Manager(ManagerWork),
}

/// Concurrent `asset.import` conversions in flight on this host; past it an upload is answered `Failed` at once.
pub(super) const MAX_IMPORTS_IN_FLIGHT: usize = 4;

/// One taken import slot; dropping it frees the slot.
pub(super) struct ImportSlot(pub(super) Arc<AtomicUsize>);

impl ImportSlot {
    /// A slot when fewer than [`MAX_IMPORTS_IN_FLIGHT`] are taken.
    pub(super) fn take(in_flight: &Arc<AtomicUsize>) -> Option<Self> {
        in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
                (taken < MAX_IMPORTS_IN_FLIGHT).then_some(taken + 1)
            })
            .ok()?;
        Some(Self(Arc::clone(in_flight)))
    }
}

impl Drop for ImportSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A provider answer for the bound snippet cache, keyed by the request args it answers (R-UI-27).
pub(super) struct SnippetAnswer {
    pub(super) target: SnippetTarget,
    pub(super) args: SnippetCallArgs,
    pub(super) label: String,
    pub(super) text: String,
}

/// Queue into the binding worker; never blocks (DB-thread and Bevy callers).
#[derive(Clone)]
pub(super) struct BindingSignals {
    pub(super) sender: Sender<BindingSignal>,
    pub(super) backlog: Arc<AtomicUsize>,
}

impl BindingSignals {
    pub(super) fn send(&self, signal: BindingSignal) {
        if self.sender.send(signal).is_err() {
            // Warn once per process; every later dropped signal would repeat it per DB event.
            static WORKER_GONE_REPORTED: AtomicBool = AtomicBool::new(false);
            if WORKER_GONE_REPORTED.swap(true, Ordering::Relaxed) {
                tracing::debug!("extension binding worker is gone; signal dropped");
            } else {
                tracing::warn!("extension binding worker is gone; later binding signals are dropped");
            }
        }
    }

    pub(super) fn dirty(&self, verse_id: &str, trigger: Trigger) {
        self.send(BindingSignal::Dirty(verse_id.to_string(), trigger));
    }

    /// Queue one accepted payload; past the backlog cap it stays opaque (counted by the warning).
    pub(super) fn payload(&self, job: DecodeJob) {
        if self.backlog.fetch_add(1, Ordering::AcqRel) >= MAX_PAYLOAD_BACKLOG {
            self.backlog.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(payload_id = %job.payload_id, "decode backlog full; payload stays opaque");
            return;
        }
        self.send(BindingSignal::Payload(job));
    }
}

/// Governance's hand into the plugin manager: revocation lists apply inline (pull thread, startup), every other
/// change queues to the binding worker so the Bevy thread never takes the manager lock.
pub(super) struct ManagerControlHandle {
    pub(super) manager: Arc<Mutex<PluginManager>>,
    pub(super) signals: BindingSignals,
}

impl ManagerControl for ManagerControlHandle {
    fn record_consent(&self, record: ConsentRecord) {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Consent(record)));
    }

    fn add_kill_list_override(&self, verse_id: &str, content_hash: &str) {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Override {
                verse_id: verse_id.to_string(),
                content_hash: content_hash.to_string(),
            }));
    }

    fn reevaluate_verse_policy(&self, verse_id: &str) -> Vec<String> {
        self.signals
            .send(BindingSignal::Manager(ManagerWork::Reevaluate(
                verse_id.to_string(),
            )));
        Vec::new()
    }

    fn accept_revocation_list(
        &self,
        list: &fe_plugin::trust_roots::RevocationList,
    ) -> Result<Vec<String>, String> {
        let stopped = lock(&self.manager)
            .accept_revocation_list(list)
            .map_err(|error| error.to_string());
        self.signals
            .send(BindingSignal::AllDirty(Trigger::KillList));
        stopped
    }
}

pub(super) struct BindingWorker {
    pub(super) manager: Arc<Mutex<PluginManager>>,
    pub(super) governance: ExtensionGovernance,
    pub(super) activator: BindingActivator,
    pub(super) catalog: InstalledCatalog,
    pub(super) stager: InstanceStager,
    pub(super) live: LiveInstances,
    pub(super) petals: PetalDirectory,
    pub(super) db: Sender<DbCommand>,
    pub(super) view: BindingView,
    pub(super) index: InstallationIndex,
    pub(super) reader: PayloadReader,
    pub(super) owners: VerseOwners,
    pub(super) answers: Sender<SnippetAnswer>,
    pub(super) backlog: Arc<AtomicUsize>,
    pub(super) admission: bool,
    pub(super) local_did: String,
    pub(super) snapshots: HashMap<String, VerseBindings>,
    pub(super) retry: HashMap<String, Instant>,
    /// When "Allow in <verse>" hydration runs next; `None` once it landed.
    pub(super) acceptance_retry: Option<Instant>,
    pub(super) last_sweep: Instant,
    pub(super) changed: bool,
    /// `(instance id, degraded)` of every governed instance at the last publish.
    pub(super) running: Vec<(String, bool)>,
    /// The active petal (an ask naming none resolves there).
    pub(super) active_petal: Option<String>,
    /// `blob.source`: the service half of the queue installed in `blob_slot`.
    pub(super) blob_source: Option<BlobSourceService>,
    /// The slot the sync thread and the `blob://` reader ask; cleared when this worker stops.
    pub(super) blob_slot: Option<BlobFallbackSlot>,
    /// `blob.mirror`: built once the blob store is bound; its queue is published in `mirror_queue`.
    pub(super) blob_mirror: Option<BlobMirror>,
    pub(super) mirror_queue: ExtensionBlobMirror,
    /// Off-worker calls the blob services make on the serving builds.
    pub(super) hook_caller: Arc<dyn HookCaller>,
    /// Install/Update offers the ops worker refreshes from configured sources (`LocalFacts.registry`).
    pub(super) registry_offers: RegistryOffers,
    /// Provisions waiting for their verse to resolve and publish.
    pub(super) provisions: Vec<PendingProvision>,
    /// `scene.*` calls waiting their turn, served round-robin per (petal, package, caller).
    pub(super) scene_calls: SceneCallQueue,
    pub(super) trace: fe_runtime::extension_trace::ExtensionTraceBuffer,
}

/// One FIFO of `scene.*` calls per (petal, package, caller DID), served round-robin so one caller's backlog delays
/// another's call by at most one call per busy queue (src/AGENTS.md §scene-hook-queue).
#[derive(Default)]
pub(super) struct SceneCallQueue {
    pub(super) queues: HashMap<(String, String, String), VecDeque<(SceneHookCall, SceneHookReplySender)>>,
    /// Keys with a non-empty queue, in serving order.
    pub(super) order: VecDeque<(String, String, String)>,
}

impl SceneCallQueue {
    pub(super) fn push(&mut self, call: SceneHookCall, reply: SceneHookReplySender) {
        let caller = call.input["caller"]["did"].as_str().unwrap_or_default().to_string();
        let key = (call.petal_id.clone(), call.package_id.clone(), caller);
        let queue = self.queues.entry(key.clone()).or_default();
        if queue.is_empty() {
            self.order.push_back(key);
        }
        queue.push_back((call, reply));
    }

    /// The head of the queue whose turn it is; that queue goes to the back of the order.
    pub(super) fn pop(&mut self) -> Option<(SceneHookCall, SceneHookReplySender)> {
        while let Some(key) = self.order.pop_front() {
            let Some(queue) = self.queues.get_mut(&key) else { continue };
            let next = queue.pop_front();
            if queue.is_empty() {
                self.queues.remove(&key);
            } else {
                self.order.push_back(key);
            }
            if next.is_some() {
                return next;
            }
        }
        None
    }

    pub(super) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// One provision waiting on its verse; `resolved` is set by the turn that (re)loaded the verse's bindings.
pub(super) struct PendingProvision {
    pub(super) petal: PetalAddress,
    pub(super) reply: ProvisionReplySender,
    pub(super) resolved: Option<Result<(), String>>,
}

/// One `LoadBindingSnapshot` round trip (pinned I-DB command).
pub(super) fn load_binding_snapshot(db: &Sender<DbCommand>, verse_id: &str) -> Result<VerseBindings, String> {
    let (reply, receiver) = crossbeam::channel::bounded(1);
    db.send_timeout(
        DbCommand::LoadBindingSnapshot {
            verse_id: verse_id.to_string(),
            reply,
        },
        DB_SEND_TIMEOUT,
    )
    .map_err(|_| "the database is not accepting commands".to_string())?;
    receiver
        .recv_timeout(DB_REPLY_TIMEOUT)
        .map_err(|_| "the database did not answer in time".to_string())?
        .map_err(|refusal: BindingRefusal| format!("{}: {}", refusal.code, refusal.message))
}

/// The stored versions as the resolver reads them.
pub(super) fn stored_rows(bindings: &VerseBindings) -> Vec<StoredRow> {
    bindings
        .versions
        .iter()
        .map(|version: &StoredBinding| StoredRow {
            record: version.record.clone(),
            written_authorized: version.written_authorized,
        })
        .collect()
}

impl BindingWorker {
    pub(super) fn run(mut self, signals: Receiver<BindingSignal>, stop: StopSignal) {
        loop {
            if stop.is_requested() {
                break;
            }
            // Queued scene calls are served in slices between drains, so a call arriving meanwhile joins the rotation.
            let wait = if self.scene_calls.is_empty() { WORKER_TURN } else { Duration::ZERO };
            match signals.recv_timeout(wait) {
                Ok(signal) => self.absorb(signal),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(signal) = signals.try_recv() {
                self.absorb(signal);
            }
            let slice = Instant::now() + SCENE_SERVE_SLICE;
            while Instant::now() < slice {
                let Some((call, reply)) = self.scene_calls.pop() else { break };
                self.scene_call(call, reply);
            }
            self.turn();
        }
        // Back to peers only: no ask may wait on a worker that is gone.
        if let Some(slot) = &self.blob_slot {
            slot.clear();
        }
        tracing::info!("extension binding worker stopped");
    }

    /// `blob.source` and `blob.mirror`: hand waiting asks and committed blobs to their bound builds (off-worker calls).
    pub(super) fn pump_blob_services(&mut self) {
        if self.blob_mirror.is_none() {
            if let Some(store) = self.reader.blobs.0.get() {
                let (queue, mirror) = blob_mirror_channel(store.clone());
                if self.mirror_queue.publish(queue) {
                    self.blob_mirror = Some(mirror);
                }
            }
        }
        let Self {
            activator,
            manager,
            catalog,
            stager,
            live,
            blob_source,
            blob_mirror,
            hook_caller,
            active_petal,
            ..
        } = self;
        let mut launcher = BridgeLauncher {
            catalog: &*catalog,
            stager: &*stager,
            live: &*live,
        };
        let mut taken = 0;
        if let Some(source) = blob_source.as_ref() {
            taken += source.pump(activator, &**manager, &mut launcher, hook_caller, active_petal.as_deref());
        }
        if let Some(mirror) = blob_mirror.as_ref() {
            taken += mirror.pump(activator, &**manager, &mut launcher, hook_caller);
        }
        // A lazily started instance changes what the resolution cache publishes.
        if taken > 0 {
            self.changed = true;
        }
    }

    /// Resolve the `asset.import` target on the worker, then convert on a call thread so a 30 s guest never stalls it.
    pub(super) fn import_call(
        &mut self,
        petal_id: &str,
        mime: &str,
        input: Value,
        body: Vec<u8>,
        reply: Sender<WorkerAnswer>,
        slot: ImportSlot,
    ) {
        let point = BindingPoint::new(IMPORT_HOOK, mime);
        let target = {
            let Self {
                activator,
                manager,
                catalog,
                stager,
                live,
                ..
            } = self;
            let mut launcher = BridgeLauncher {
                catalog: &*catalog,
                stager: &*stager,
                live: &*live,
            };
            activator.ensure_call_target(petal_id, &point, &**manager, &mut launcher)
        };
        self.changed = true;
        let Ok(target) = target else {
            let _ = reply.send(WorkerAnswer::Unbound);
            return;
        };
        let request = HookRequest {
            hook: IMPORT_HOOK.to_string(),
            key: mime.to_string(),
            binding_scope: target.binding_scope.clone(),
            config: target.config.to_string(),
            input: input.to_string(),
            body,
        };
        let live = self.live.clone();
        let answer_tx = reply.clone();
        let spawned = std::thread::Builder::new()
            .name("fe-ext-import".into())
            .spawn(move || {
                let _slot = slot;
                let answer = match live.call(&target.plugin_id, &request, None) {
                    Ok(reply) => WorkerAnswer::Output {
                        output: reply.output,
                        body: reply.body,
                    },
                    Err(HookFailure::Refused(reason) | HookFailure::Declined(reason)) => {
                        WorkerAnswer::Refused(reason)
                    }
                    Err(HookFailure::Fault(reason)) => {
                        tracing::warn!(%reason, "asset.import call failed");
                        WorkerAnswer::Failed
                    }
                };
                let _ = answer_tx.send(answer);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "asset.import call thread did not start");
            let _ = reply.send(WorkerAnswer::Failed);
        }
    }

    /// Periodic work: store scan, acceptance hydration, due retries, dirty verses, idle sweep, publication.
    pub(super) fn turn(&mut self) {
        self.catalog.refresh_if_due();
        if self.acceptance_retry.is_some_and(|at| at <= Instant::now()) {
            self.hydrate_acceptances();
        }
        let now = Instant::now();
        let due: Vec<String> = self
            .retry
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(verse, _)| verse.clone())
            .collect();
        for verse_id in due {
            self.retry.remove(&verse_id);
            self.activator.mark_dirty(&verse_id, Trigger::BindingRow);
        }
        self.resolve_dirty();
        if self.last_sweep.elapsed() >= IDLE_SWEEP {
            self.last_sweep = Instant::now();
            let plan = self.activator.sweep_idle();
            self.apply(plan);
        }
        self.pump_blob_services();
        self.live.prune();
        self.publish();
        self.answer_provisions();
    }

    /// Hold a provisional petal and mark its verse dirty; the answer waits for the turn that resolves it.
    pub(super) fn provision(&mut self, petal: PetalAddress, hold: u64, reply: ProvisionReplySender) {
        if let Err(failure) = self.petals.hold_provisional(&petal, hold) {
            let _ = reply.send(Err(failure));
            return;
        }
        self.activator.mark_dirty(&petal.verse_id, Trigger::Navigation);
        self.provisions.push(PendingProvision {
            petal,
            reply,
            resolved: None,
        });
    }

    /// Record a verse's resolve outcome on the provisions waiting for it.
    pub(super) fn settle_provisions(&mut self, verse_id: &str, outcome: &Result<(), String>) {
        for pending in self
            .provisions
            .iter_mut()
            .filter(|pending| pending.petal.verse_id == verse_id)
        {
            pending.resolved = Some(outcome.clone());
        }
    }

    /// After publication: answer each settled provision from the published cache (what `bound` will read).
    pub(super) fn answer_provisions(&mut self) {
        if self.provisions.iter().all(|pending| pending.resolved.is_none()) {
            return;
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.provisions)
            .into_iter()
            .partition(|pending| pending.resolved.is_some());
        self.provisions = waiting;
        for pending in ready {
            let answer = match pending.resolved {
                Some(Ok(())) => self.view.scene_judgeable(&pending.petal.petal_id),
                Some(Err(reason)) => Err(ProvisionFailure::Unavailable(reason)),
                None => continue,
            };
            let _ = pending.reply.send(answer);
        }
    }

    pub(super) fn absorb(&mut self, signal: BindingSignal) {
        match signal {
            BindingSignal::Dirty(verse_id, trigger) => {
                self.activator.mark_dirty(&verse_id, trigger)
            }
            BindingSignal::AllDirty(trigger) => self.activator.mark_all_dirty(trigger),
            BindingSignal::Navigate(active) => {
                self.active_petal = active.as_ref().map(|address| address.petal_id.clone());
                let plan = self.activator.navigate(active);
                self.apply(plan);
            }
            BindingSignal::Forget(installation_id) => {
                self.activator.forget_installation(&installation_id);
                self.changed = true;
            }
            BindingSignal::Payload(job) => {
                self.backlog.fetch_sub(1, Ordering::AcqRel);
                self.decode(&job);
            }
            BindingSignal::Snippets { petal_id, requests } => {
                self.answer_snippets(&petal_id, requests)
            }
            BindingSignal::Call {
                petal_id,
                point,
                input,
                reply,
            } => {
                let answer = self.call_point(&petal_id, &point, &input);
                let _ = reply.send(answer);
            }
            BindingSignal::Scene { call, reply } => self.scene_calls.push(call, reply),
            BindingSignal::Provision { petal, hold, reply } => self.provision(petal, hold, reply),
            BindingSignal::Release { petal_id, hold } => {
                // The last hold gone: re-resolve so instances started at the provisional petal stop.
                if let Some(verse_id) = self.petals.release_provisional(&petal_id, hold) {
                    self.activator.mark_dirty(&verse_id, Trigger::Navigation);
                }
            }
            BindingSignal::Import {
                petal_id,
                mime,
                input,
                body,
                reply,
                slot,
            } => self.import_call(&petal_id, &mime, input, body, reply, slot),
            BindingSignal::Manager(work) => self.manager_work(work),
        }
    }

    pub(super) fn manager_work(&mut self, work: ManagerWork) {
        let mut manager = lock(&self.manager);
        match work {
            ManagerWork::Consent(record) => {
                manager.governance_mut().record_consent(record.clone());
                drop(manager);
                // A consent no installed extension build carries is a web bundle's (P-RESOLVE): the portal learns it.
                self.catalog.refresh_if_due();
                let extension = self.catalog.builds.values().any(|build| {
                    build.facts.package_id == record.package_id
                        && build.facts.content_hash.eq_ignore_ascii_case(&record.content_hash)
                });
                if !extension {
                    portal_learn_consent(&record);
                }
                self.activator.mark_all_dirty(Trigger::Lifecycle);
            }
            ManagerWork::Override {
                verse_id,
                content_hash,
            } => {
                manager
                    .governance_mut()
                    .add_kill_list_override(&verse_id, &content_hash);
                drop(manager);
                self.activator.mark_dirty(&verse_id, Trigger::KillList);
            }
            ManagerWork::Reevaluate(verse_id) => {
                let stopped = manager.reevaluate_verse_policy(&verse_id);
                drop(manager);
                if !stopped.is_empty() {
                    tracing::info!(%verse_id, stopped = ?stopped, "verse policy change stopped extension instances");
                }
                self.activator.mark_dirty(&verse_id, Trigger::Policy);
            }
        }
    }

    /// "Allow in <verse>" from governance's lossless, actor-keyed replay (R-GOV-31); only the local DID's word counts.
    pub(super) fn hydrate_acceptances(&mut self) {
        // Until this lands no binding instance starts (row 12 refuses), so retry on the snapshot spacing.
        self.acceptance_retry = Some(Instant::now() + SNAPSHOT_RETRY);
        let Some(accepted) = self.governance.local_acceptances() else {
            return;
        };
        {
            let mut manager = lock(&self.manager);
            for (installation_id, verse_id) in &accepted {
                manager
                    .governance_mut()
                    .accept_in_verse(installation_id, verse_id);
            }
        }
        self.acceptance_retry = None;
        self.activator.mark_all_dirty(Trigger::Lifecycle);
    }

    pub(super) fn local_facts(&self, verse_id: &str, petals: &[PetalAddress]) -> LocalFacts {
        let is_owner = self.owners.is_owner(verse_id);
        let default_access = self.owners.default_access(verse_id);
        // Portal pairs only: an installed extension build is never a web bundle `portal.open` may name.
        let mut admitted_bundles = portal_admitted_bundles();
        admitted_bundles.retain(|(package_id, content_hash)| {
            !self.catalog.builds.values().any(|build| {
                build.facts.package_id == *package_id
                    && build.facts.content_hash.eq_ignore_ascii_case(content_hash)
            })
        });
        let manager = lock(&self.manager);
        let local = manager.governance();
        LocalFacts {
            local_did: self.local_did.clone(),
            host_enabled: self.admission && !local.is_disabled_all(),
            is_verse_owner_host: is_owner,
            builds: self
                .catalog
                .builds
                .values()
                .map(|build| build_facts(build, verse_id, petals, local, &self.governance))
                .collect(),
            accepted: local.acceptances(),
            registry: self.registry_offers.snapshot(),
            package_policy: PackagePolicy::default(),
            default_access,
            admitted_bundles,
        }
    }

    pub(super) fn resolve_dirty(&mut self) {
        for (verse_id, triggers) in self.activator.take_dirty() {
            let petals = self.petals.of_verse(&verse_id);
            let bindings = match load_binding_snapshot(&self.db, &verse_id) {
                Ok(bindings) => bindings,
                Err(reason) => {
                    tracing::warn!(%verse_id, %reason, "binding snapshot did not load; retrying");
                    // A provision never reads a stale or absent resolution: it is answered unavailable.
                    self.settle_provisions(&verse_id, &Err(reason));
                    self.retry.insert(verse_id, Instant::now() + SNAPSHOT_RETRY);
                    continue;
                }
            };
            let rows = stored_rows(&bindings);
            // Unknown roles judge every author below Manager: the built-in serves (fail closed).
            let roles = bindings.roles.clone().unwrap_or_default();
            let facts = self.local_facts(&verse_id, &petals);
            let running = lock(&self.manager).running_instances();
            let snapshot = BindingSnapshot {
                rows: &rows,
                roles: &roles,
                facts: &facts,
                stored_terrain_petals: &bindings.stored_terrain_petals,
            };
            let plan = self
                .activator
                .reconcile(&verse_id, &snapshot, &petals, &running);
            tracing::debug!(%verse_id, ?triggers, starts = plan.starts.len(), stops = plan.stops.len(), "bindings re-resolved");
            self.settle_provisions(&verse_id, &Ok(()));
            self.snapshots.insert(verse_id, bindings);
            self.apply(plan);
            self.changed = true;
        }
    }

    pub(super) fn apply(&mut self, plan: ActivationPlan) {
        if plan.is_empty() {
            return;
        }
        let Self {
            activator,
            manager,
            catalog,
            stager,
            live,
            ..
        } = self;
        let mut launcher = BridgeLauncher {
            catalog: &*catalog,
            stager: &*stager,
            live: &*live,
        };
        let report = activator.apply(plan, &**manager, &mut launcher);
        for (plugin_id, reason, detail) in &report.refused {
            tracing::info!(%plugin_id, ?reason, %detail, "binding instance refused");
        }
        for (plugin_id, detail) in &report.faults {
            tracing::warn!(%plugin_id, %detail, "binding configure faulted");
        }
        self.changed = true;
    }

    pub(super) fn decode(&mut self, job: &DecodeJob) {
        let Self {
            activator,
            manager,
            catalog,
            stager,
            live,
            reader,
            ..
        } = self;
        let mut launcher = BridgeLauncher {
            catalog: &*catalog,
            stager: &*stager,
            live: &*live,
        };
        let mut host = DecodePort {
            reader: &*reader,
            live: &*live,
        };
        match decode_payload(activator, &**manager, &mut launcher, &mut host, job) {
            DecodeOutcome::Decoded { staged } => {
                tracing::debug!(payload_id = %job.payload_id, staged, "payload decoded")
            }
            DecodeOutcome::Opaque(reason) => {
                tracing::debug!(payload_id = %job.payload_id, ?reason, "payload stays opaque")
            }
            DecodeOutcome::Failed(error) => {
                tracing::warn!(payload_id = %job.payload_id, ?error, "payload decode failed")
            }
        }
        self.changed = true;
    }

    /// Call the instance serving `point` at `petal_id`, lazily started; any failure leaves the built-in serving.
    pub(super) fn call_point(&mut self, petal_id: &str, point: &BindingPoint, input: &Value) -> HookAnswer {
        let target = {
            let Self {
                activator,
                manager,
                catalog,
                stager,
                live,
                ..
            } = self;
            let mut launcher = BridgeLauncher {
                catalog: &*catalog,
                stager: &*stager,
                live: &*live,
            };
            activator.ensure_call_target(petal_id, point, &**manager, &mut launcher)
        };
        self.changed = true;
        let Ok(target) = target else {
            return HookAnswer::Builtin;
        };
        let request = HookRequest {
            hook: point.hook.clone(),
            key: point.key.clone(),
            binding_scope: target.binding_scope.clone(),
            config: target.config.to_string(),
            input: input.to_string(),
            body: Vec::new(),
        };
        match self.live.call(&target.plugin_id, &request, None) {
            Ok(reply) => HookAnswer::Output {
                package_id: target.key.package_id,
                output: reply.output,
            },
            Err(failure) => HookAnswer::Failed(format!("{failure:?}")),
        }
    }

    /// One `scene.*` call on the instance the binding names (lazily started); a moved binding, no instance or a
    /// degraded one is a fault, which the gate turns into a refusal naming the package (fail closed).
    pub(super) fn scene_call(&mut self, call: SceneHookCall, reply: SceneHookReplySender) {
        if reply.is_closed() {
            return;
        }
        let point = BindingPoint::new(call.hook.id(), "");
        let target = {
            let Self {
                activator,
                manager,
                catalog,
                stager,
                live,
                ..
            } = self;
            let mut launcher = BridgeLauncher {
                catalog: &*catalog,
                stager: &*stager,
                live: &*live,
            };
            activator.ensure_call_target(&call.petal_id, &point, &**manager, &mut launcher)
        };
        self.changed = true;
        let started = std::time::Instant::now();
        let answer = match target {
            Ok(target) if target.key.package_id == call.package_id => {
                let request = HookRequest {
                    hook: call.hook.id().to_string(),
                    key: String::new(),
                    binding_scope: call.binding_scope,
                    config: call.config.to_string(),
                    input: call.input.to_string(),
                    body: Vec::new(),
                };
                match self.live.call(&target.plugin_id, &request, None) {
                    Ok(reply) => fe_plugin::host_bridge::scene_hook_reply_from_output(&reply.output),
                    Err(HookFailure::Refused(reason)) => SceneHookReply::Refused(reason),
                    Err(HookFailure::Declined(reason)) => SceneHookReply::Declined(reason),
                    Err(HookFailure::Fault(reason)) => SceneHookReply::Fault(reason),
                }
            }
            Ok(_) => SceneHookReply::Fault("the binding now names another package".into()),
            Err(_) => SceneHookReply::Fault(format!("no instance serves {} here", call.hook.id())),
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        fe_plugin::host_bridge::record_scene_hook_trace(
            &self.trace,
            &call.package_id,
            &call.petal_id,
            call.hook.id(),
            &answer,
            Some(duration_ms),
        );
        let _ = reply.send(answer);
    }

    /// `egress.snippet`: `{sql, query_url, export_urls}` -> `{label, snippet}`; a failure stores an empty answer.
    pub(super) fn answer_snippets(&mut self, petal_id: &str, requests: Vec<SnippetRequest>) {
        for request in requests {
            let point = BindingPoint::new(SNIPPET_HOOK, request.target.key.clone());
            let input = json!({
                "sql": request.args.sql,
                "query_url": request.args.query_url,
                "export_urls": { "parquet": request.args.export_parquet, "csv": request.args.export_csv },
            });
            let (label, text) = match self.call_point(petal_id, &point, &input) {
                HookAnswer::Output { package_id, output }
                    if package_id == request.target.package_id =>
                {
                    let parsed: Value = serde_json::from_str(&output).unwrap_or(Value::Null);
                    let field = |name: &str| {
                        parsed
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (field("label"), field("snippet"))
                }
                HookAnswer::Failed(reason) => {
                    tracing::debug!(key = %request.target.key, %reason, "egress.snippet provider failed");
                    (String::new(), String::new())
                }
                // The point moved to another package or back to the built-in: the stale answer is dropped.
                HookAnswer::Output { .. } | HookAnswer::Builtin => (String::new(), String::new()),
            };
            if self
                .answers
                .send(SnippetAnswer {
                    target: request.target,
                    args: request.args,
                    label,
                    text,
                })
                .is_err()
            {
                return;
            }
        }
    }

    /// The winning row per point at each petal with this host's resolution (BindingHost::effective, bound slots,
    /// `petal.map`). A row binding the compiled-in terrain package is never staged, so it publishes as resolved.
    pub(super) fn published_petals(&self) -> HashMap<String, PublishedPetal> {
        let mut petals_out = HashMap::new();
        for (verse_id, bindings) in &self.snapshots {
            let rows = stored_rows(bindings);
            let roles = bindings.roles.clone().unwrap_or_default();
            for petal in self.petals.of_verse(verse_id) {
                let points = self
                    .activator
                    .resolutions_at(&petal.petal_id)
                    .into_iter()
                    .map(|(point, resolved)| {
                        // The resolver's own deciding row: none when the petal's terrain column decides (R-BND-13).
                        let winner: Option<BindingRecord> = deciding_binding(
                            &rows,
                            &roles,
                            &bindings.stored_terrain_petals,
                            &petal,
                            &point,
                        )
                        .cloned();
                        let manifest = match &resolved.resolution {
                            BindingResolution::Resolved { package_id, .. } => self
                                .catalog
                                .builds
                                .get(package_id)
                                .and_then(|build| build.facts.manifest.clone()),
                            _ => None,
                        };
                        let mut published = PublishedPoint {
                            point,
                            resolved,
                            winner,
                            manifest,
                        };
                        if let Some(scope) = published
                            .native_terrain()
                            .map(|record| record.scope.clone())
                        {
                            published.resolved.resolution = BindingResolution::Resolved {
                                package_id: TERRAIN_EXTENSION_ID.to_string(),
                                scope,
                            };
                        }
                        published
                    })
                    .collect();
                let scope = fe_database::build_scope(
                    &petal.verse_id,
                    Some(petal.fractal_id.as_str()),
                    Some(petal.petal_id.as_str()),
                );
                petals_out.insert(petal.petal_id.clone(), PublishedPetal { scope, points });
            }
        }
        petals_out
    }

    pub(super) fn publish(&mut self) {
        let running = lock(&self.manager).running_instances();
        // The ops drain starts, disables and uninstalls without a signal: a changed running set is a lifecycle
        // trigger, so local Disable/Enable/Uninstall reach the resolution cache (and the bound slots) in one turn.
        let fingerprint: Vec<(String, bool)> = running
            .iter()
            .map(|instance| (instance.plugin_id.clone(), instance.degraded))
            .collect();
        if fingerprint != self.running {
            self.running = fingerprint;
            self.activator.mark_all_dirty(Trigger::Lifecycle);
        }
        let mut consents = BTreeSet::new();
        let mut disabled = BTreeSet::new();
        {
            let manager = lock(&self.manager);
            let local = manager.governance();
            for build in self.catalog.builds.values() {
                let id = &build.facts.package_id;
                let consent = local.consent(id);
                if consent
                    .is_some_and(|record| record.capability_digest == build.facts.capability_digest)
                {
                    consents.insert(id.clone());
                }
                if local.is_disabled_all() || local.is_locally_disabled(id, None) {
                    disabled.insert(id.clone());
                }
            }
        }
        let rows = self
            .catalog
            .builds
            .values()
            .map(|build| {
                let id = build.facts.package_id.clone();
                let hashes: Vec<&str> = build.facts.hashes().collect();
                let row = IndexRow {
                    package_id: id.clone(),
                    version: build.facts.version.clone(),
                    name: build.name.clone(),
                    revoked: self.governance.is_revoked_any(&hashes),
                    disabled: disabled.contains(&id),
                    consented: consents.contains(&id),
                    instances: running
                        .iter()
                        .filter(|instance| instance.installation_id == id)
                        .cloned()
                        .collect(),
                    content_hash: build.facts.content_hash.clone(),
                    manifest: build.facts.manifest.clone(),
                };
                (id, row)
            })
            .collect();
        self.index.replace(rows);
        if std::mem::take(&mut self.changed) {
            let mut status = self.activator.status_rows();
            if let Some(source) = &self.blob_source {
                status.extend(source.status_rows());
            }
            if let Some(mirror) = &self.blob_mirror {
                status.extend(mirror.status_rows());
            }
            self.view.replace(self.published_petals(), status);
        }
    }
}

