use super::*;

/// `asset.import` over the binding worker: the published cache answers `bound`, the worker resolves and converts.
pub(super) struct ImportPort {
    pub(super) view: BindingView,
    pub(super) signals: BindingSignals,
    /// Conversions in flight ([`MAX_IMPORTS_IN_FLIGHT`]).
    pub(super) in_flight: Arc<AtomicUsize>,
}

impl ImportWorkerPort for ImportPort {
    fn bound(&self, petal_id: &str, mime: &str) -> bool {
        self.view.is_resolved(petal_id, &BindingPoint::new(IMPORT_HOOK, mime))
    }

    fn convert(&self, petal_id: &str, mime: &str, input: Value, body: Vec<u8>) -> Receiver<WorkerAnswer> {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        // Bounded before the 16 MiB body reaches the unbounded worker queue: an upload burst cannot pile up bytes or threads.
        let Some(slot) = ImportSlot::take(&self.in_flight) else {
            tracing::warn!(petal_id, mime, "asset.import busy: {MAX_IMPORTS_IN_FLIGHT} conversions already running");
            let _ = reply.send(WorkerAnswer::Failed);
            return receiver;
        };
        self.signals.send(BindingSignal::Import {
            petal_id: petal_id.to_string(),
            mime: mime.to_string(),
            input,
            body,
            reply,
            slot,
        });
        receiver
    }
}

/// Present only when the plugin manager started: the upload route's `asset.import` seam.
#[derive(Resource, Clone)]
pub(super) struct ExtensionImportHooks(pub(super) Arc<ImportPort>);

/// The `asset.import` seam fe-api layers onto the upload route (`ApiConfig::import_hooks`); `None` keeps non-GLB refused.
pub(crate) fn import_hooks_handle(app: &App) -> Option<fe_api::import_hooks::ImportHooksHandle> {
    let hooks = app.world().get_resource::<ExtensionImportHooks>()?;
    let port: Arc<dyn ImportWorkerPort> = hooks.0.clone();
    Some(ImportHostAdapter::new(port).handle())
}

/// The `scene.*` gate's backend (BND-10a): the published cache answers `bound`, the binding worker runs calls,
/// augments stage straight into the serving instance's window.
pub(super) struct SceneHooks {
    pub(super) local_did: String,
    pub(super) view: BindingView,
    pub(super) live: LiveInstances,
    pub(super) signals: BindingSignals,
}

impl SceneHookBackend for SceneHooks {
    fn local_did(&self) -> &str {
        &self.local_did
    }

    fn bound(&self, hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
        self.view.scene_binding(petal_id, hook.id())
    }

    fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
        self.signals.send(BindingSignal::Scene { call, reply });
    }

    fn stage_augment(
        &self,
        petal_id: &str,
        package_id: &str,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String> {
        let key = InstanceKey::new(petal_id.to_string(), package_id.to_string());
        self.live.stage_augment(&key, node_id, ops)
    }

    fn provision(&self, petal: ProvisionalPetal, reply: ProvisionReplySender) {
        let ProvisionalPetal {
            verse_id,
            fractal_id,
            petal_id,
            hold,
        } = petal;
        // A gone worker drops `reply`, which the gate reads as unavailable.
        self.signals.send(BindingSignal::Provision {
            petal: PetalAddress {
                verse_id,
                fractal_id,
                petal_id,
            },
            hold,
            reply,
        });
    }

    fn release(&self, petal: &ProvisionalPetal) {
        self.signals.send(BindingSignal::Release {
            petal_id: petal.petal_id.clone(),
            hold: petal.hold,
        });
    }
}

/// Present only when the plugin manager started (otherwise no gate: every write proceeds as built in).
#[derive(Resource, Clone)]
pub(super) struct ExtensionSceneHooks(pub(super) Arc<SceneHooks>);

/// The backend `main.rs` installs `fe_runtime::scene_hook_gate::SceneHookGate` over before the API serves.
pub(crate) fn scene_hook_backend(app: &App) -> Option<Arc<dyn SceneHookBackend>> {
    let hooks = app.world().get_resource::<ExtensionSceneHooks>()?;
    Some(hooks.0.clone())
}

