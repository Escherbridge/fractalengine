//! Production extension host: scene DB backend, integration bus, plugin manager and ticks, network broker,
//! governance, ops drain and scoped bindings; rationale in src/AGENTS.md §plugin-bridge.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use bevy::prelude::{App, Local, MessageReader, Res, ResMut, Resource, Update};
use crossbeam::channel::{Receiver, RecvTimeoutError, Sender};
use fe_api::bindings::{BindingHost, BindingListReply, BindingWriteReply, EffectivePoint};
use fe_api::extensions::{
    AdminOp, AdminRefusal, AdminReply, ApiCaller, ApiCallerKind, CommandFailure,
    CommandReply, ExtensionHost, HostRefusal, IngressAnswer, IngressCall, IngressRefusal,
    WsAcceptCall, WsData, WsHostSession, WsOut, MAX_AUDIT_ROWS,
};
use fe_database::handlers::device_payload::{set_accepted_payload_sink, AcceptedPayloadEvent};
use fe_database::handlers::extension_binding::BINDING_TABLE;
use fe_database::handlers::extension_policy::{
    AuditAction as PolicyAction, AuditEntry as PolicyEntry, AuditOutcome as PolicyOutcome,
    POLICY_TABLE,
};
use fe_database::RoleLevel;
use fe_hexon::package::{verify_plugin_package, PluginPackage, PluginPackageStore, PluginTrust};
use fe_hexon::registry_source::SourceTrust;
use fe_plugin::bindings::activation::{
    ActivationPlan, BindingActivator, BindingLauncher, CallTarget, ConfigurePlan, InstancePlan,
    Need, Trigger,
};
use fe_plugin::bindings::blob_source::{blob_source_channel, BlobSourceService, HookCaller};
use fe_plugin::bindings::egress::{blob_mirror_channel, BlobMirror, MirrorQueue};
use fe_plugin::bindings::decode::{
    decode_payload, DecodeCall, DecodeCallError, DecodeFile, DecodeHost, DecodeJob, DecodeOutcome,
    DecodeReply, DecodeStamp, DECODE_HOOK, MAX_DECODE_BODY_BYTES,
};
use fe_plugin::bindings::resolver::{
    deciding_binding, BindingSnapshot, BuildFacts, BuildIntegrity, LocalFacts, PackagePolicy,
    PetalAddress, PolicyVerdict, ResolvedPoint, StoredRow,
};
use fe_plugin::bus::{
    role_level, AuditAction, AuditEntry, AuditSink, Bus, BusPolicy, BusRefusal, BusSources, Caller,
    CallerKind, ClientGrants, CommandError, CommandOutcome, HostError, InstanceKey, InstanceSpec,
    PreparedQuery, QueryRequest, QueryRows, QuerySource, ReadingEvent, RoleSource,
    SceneSnapshotSource,
};
use fe_plugin::capability::{CapabilityToken, V2Grants};
use fe_plugin::lifecycle::{
    binding_instance_id, ConsentRecord, Governance, GovernanceGates, InstanceOrigin, PackageCheck,
    PackageFacts, PluginManager, RunningInstance, StagedInstance, TrustTier,
};
use fe_plugin::net::{
    HttpIngressRequest, IngressRefusal as NetRefusal, IngressReply, Net, WsAcceptRequest, WsFrame,
    WsOutbound,
};
use fe_plugin::package::UnsignedPolicy;
use fe_plugin::registry::PluginRegistry;
use fe_plugin::scene_host::{PluginContexts, SceneBackendFactory};
use fe_plugin::tick::{
    ContextTickSink, InstanceSeat, Launch, NetSeat, SharedWindow, TickHost, TickScheduler,
};
use fe_plugin::transaction::{StagedCommitSink, StagedFlush};
use fe_plugin::wasm::component::hooks::{HookCallError, HookRefusal, HookReply, HookRequest};
use fe_plugin::wasm::component::net_bridge::{HostBroker, NetBroker};
use fe_plugin::wasm::component::{
    window_for_token, Binding, BusBroker, ComponentHost, InstanceHandle, InstanceSetup, KvSlot,
    MemoryKv,
};
use fe_plugin::wasm::WasmEngine;
use fe_plugin::{
    NodeQuery, NodeReader, NodeSnapshot, PluginCommand, TransactionReceipt, TransactionSink,
};
use fe_runtime::extension_binding::{
    resolution_scopes, BindingMode, BindingPoint, BindingRecord, BindingResolution, BindingScopeLevel,
    UnresolvedReason,
};
use fe_runtime::messages::{
    BindingRefusal, DbCommand, DbResult, ReplicatedRowOutcome, StoredBinding, VerseBindings,
};
use fe_runtime::bevy_blob_reader::BlobFallbackSlot;
use fe_runtime::plugin_host::{
    PackageIdentity, PluginAdmission, PluginRead, PluginRefusal, PluginReply, PluginResult,
    ReadingsAdmission,
};
use fe_runtime::scene_hook_gate::{
    AugmentOp, ProvisionFailure, ProvisionReplySender, ProvisionalPetal, SceneBinding, SceneHook,
    SceneHookBackend, SceneHookCall, SceneHookReply, SceneHookReplySender,
};
use fe_runtime::service_lifecycle::{OwnedService, StopSignal};
use fe_sdk::manifest_v2::{parse_and_validate, ExtensionManifest, Ingress, RoleFloor, SceneWrite};
use fe_sdk::plugin_host::{PendingOp, PluginLease, StagedReading};
use fe_sdk::source_class::SourceClass;
use fe_terrain::extension::TERRAIN_EXTENSION_ID;
use fe_ui::extensions::bindings::{BindingPointRow, BindingsSnapshot, ScopeOption, ScopeTarget};
use fe_ui::extensions::panel::{AuditLine, ExtensionEvent, ExtensionsPanelState};
use fe_ui::extensions::{
    BoundPanel, BoundSlotsState, ConsentState, ExtensionSlotState, SnippetCallArgs, SnippetRequest,
    SnippetTarget,
};
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::verse_manager::VerseManager;
use serde_json::{json, Value};

use crate::extension_host::admin_channel::{self, AdminChannelPorts};
use crate::extension_host::governance::{
    self, ExtensionGovernance, GovernanceConfig, ManagerControl,
};
use crate::extension_host::import_hooks::{ImportHostAdapter, ImportWorkerPort, WorkerAnswer};
use crate::extension_host::ops_drain::{
    ActiveScope, BuildToRun, ExtensionOpsPlugin, OpsPorts, OpsWorker, CHANNEL_GUI,
};
use crate::extension_host::registry_offers::RegistryOffers;
use crate::extension_host::slot_drain::{BusSlotCaller, SlotCommand, SlotDrain, SlotReport};
use crate::extension_host::sources::{load_source_configs, SourceSet};
use crate::extension_host::{net_sources, revocation_pull, ui_registry};
use crate::portal_host::resolver::PortalGovernance;

/// Bound on enqueueing one DB command from a host thread.
pub(super) const DB_SEND_TIMEOUT: Duration = Duration::from_secs(1);
/// Bound on one governance or binding DB round trip.
pub(super) const DB_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on a plugin read or commit; a write past it is an unknown outcome.
pub(super) const PLUGIN_DB_REPLY: Duration = Duration::from_secs(15);
/// Longest the binding worker sleeps between turns.
pub(super) const WORKER_TURN: Duration = Duration::from_millis(500);
/// Spacing of the lazy-instance idle sweep (bindings.md §5.6).
pub(super) const IDLE_SWEEP: Duration = Duration::from_secs(30);
/// Spacing of the installed-package store scan.
pub(super) const CATALOG_SCAN: Duration = Duration::from_secs(2);
/// Wait before re-verifying a stored build that failed (it may have been mid-write).
pub(super) const REFUSAL_RETRY: Duration = Duration::from_secs(30);
/// Spacing of the petal directory mirror.
pub(super) const PETAL_MIRROR: Duration = Duration::from_secs(1);
/// How long one verse-owner answer is reused.
pub(super) const OWNER_CACHE: Duration = Duration::from_secs(60);
/// Bound on one verse-owner lookup (the caller may hold the manager lock).
pub(super) const OWNER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// Wait before re-reading a verse whose binding snapshot failed to load.
pub(super) const SNAPSHOT_RETRY: Duration = Duration::from_secs(5);
/// Accepted payloads queued for decode before new ones are dropped.
pub(super) const MAX_PAYLOAD_BACKLOG: usize = 256;
/// Longest the worker serves queued `scene.*` calls before it runs a turn (src/AGENTS.md §scene-hook-queue).
pub(super) const SCENE_SERVE_SLICE: Duration = Duration::from_millis(50);
/// Audit lines the details view shows.
pub(super) const DETAILS_AUDIT_LINES: usize = 10;
/// Sealed builds in the extension data dir.
pub(super) const PACKAGES_DIR: &str = "packages";
/// The 1.0 core-module AOT cache in the extension data dir.
pub(super) const AOT_DIR: &str = "aot";
/// Configured registry sources in the extension data dir (F-OPS `SourceConfig` array).
pub(super) const SOURCES_FILE: &str = "extension_sources.json";
/// Replicated role statements (fe-database `role` table).
pub(super) const ROLE_TABLE: &str = "role";
/// Catalogue ids the bound-slot cache reads.
pub(super) const PANEL_HOOK: &str = "ui.panel";
pub(super) const SNIPPET_HOOK: &str = "egress.snippet";
/// Hook id of the upload converter (fe-api `import_hooks::IMPORT_HOOK`).
pub(super) const IMPORT_HOOK: &str = fe_api::import_hooks::IMPORT_HOOK;
/// Web-bundle consents kept while the portal has not linked yet.
pub(super) const MAX_PENDING_BUNDLE_CONSENTS: usize = 1024;

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// RFC 3339 UTC with milliseconds; civil date by Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html#civil_from_days>).
pub(super) fn rfc3339_utc(epoch_ms: u64) -> String {
    let secs = epoch_ms / 1_000;
    let (hour, minute, second) = ((secs % 86_400) / 3_600, (secs % 3_600) / 60, secs % 60);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        epoch_ms % 1_000
    )
}

/// `role` capped by a token's `max_role` (a delegated token of this node stays at its ceiling, decision #8).
pub(super) fn capped(role: RoleLevel, ceiling: RoleLevel) -> RoleLevel {
    if role.is_at_least(ceiling) {
        ceiling
    } else {
        role
    }
}


mod bindings;
mod catalog;
mod hooks;
mod ops;
mod scene_backend;
mod worker;

#[allow(unused_imports)]
pub(crate) use self::bindings::{
    api_binding_host, api_extension_host, extension_trace, BusExtensionHost, ExtensionBindings,
    ExtensionHostParts,
};
#[allow(unused_imports)]
pub(crate) use self::catalog::ExtensionBlobMirror;
#[allow(unused_imports)]
pub(crate) use self::hooks::{import_hooks_handle, scene_hook_backend};
#[allow(unused_imports)]
pub(crate) use self::scene_backend::{
    link_portal_resolver, ExtensionApiSlot, ExtensionBindingService, ExtensionBlobSlot,
    ExtensionBridgeConfig, ExtensionBusParts, ExtensionExternalPolicy, ExtensionRoleView,
    ReadingFeed,
};
#[allow(unused_imports)]
pub(crate) use self::worker::HookAnswer;

#[allow(unused_imports)]
use self::{bindings::*, catalog::*, hooks::*, ops::*, scene_backend::*, worker::*};

// ---------------------------------------------------------------------------
// Bevy systems: hierarchy mirror, navigation, replicated rows, bound slots
// ---------------------------------------------------------------------------

/// Mirror the loaded hierarchy's petal addresses once a second; a verse seen or changed is re-resolved.
fn mirror_petals(
    verses: Option<Res<VerseManager>>,
    bindings: Res<ExtensionBindings>,
    mut last: Local<Option<Instant>>,
) {
    if last.is_some_and(|at| at.elapsed() < PETAL_MIRROR) {
        return;
    }
    *last = Some(Instant::now());
    let Some(verses) = verses else { return };
    let mut next = HashMap::new();
    for verse in &verses.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                next.insert(
                    petal.id.clone(),
                    PetalAddress {
                        verse_id: verse.id.clone(),
                        fractal_id: fractal.id.clone(),
                        petal_id: petal.id.clone(),
                    },
                );
            }
        }
    }
    for verse_id in bindings.petals.replace(next) {
        bindings.signals.dirty(&verse_id, Trigger::Navigation);
    }
}

/// The active petal changed: lazy points there start, the petal left starts its idle clock.
fn follow_navigation(
    nav: Option<Res<NavigationManager>>,
    bindings: Res<ExtensionBindings>,
    mut last: Local<Option<String>>,
) {
    let Some(nav) = nav else { return };
    if last.as_deref() == nav.active_petal_id.as_deref() {
        return;
    }
    *last = nav.active_petal_id.clone();
    let active = match (
        &nav.active_verse_id,
        &nav.active_fractal_id,
        &nav.active_petal_id,
    ) {
        (Some(verse_id), Some(fractal_id), Some(petal_id)) => Some(PetalAddress {
            verse_id: verse_id.clone(),
            fractal_id: fractal_id.clone(),
            petal_id: petal_id.clone(),
        }),
        _ => None,
    };
    bindings.signals.send(BindingSignal::Navigate(active));
}

/// Replicated binding, role and policy rows re-resolve their verse (bindings.md §5.6 triggers).
fn follow_binding_rows(mut results: MessageReader<DbResult>, bindings: Res<ExtensionBindings>) {
    for result in results.read() {
        match result {
            DbResult::ReplicatedRowApplied {
                verse_id,
                table,
                outcome: ReplicatedRowOutcome::Applied,
                ..
            } => {
                let trigger = if table == BINDING_TABLE {
                    Trigger::BindingRow
                } else if table == ROLE_TABLE {
                    Trigger::Role
                } else if table == POLICY_TABLE {
                    Trigger::Policy
                } else {
                    continue;
                };
                bindings.signals.dirty(verse_id, trigger);
            }
            DbResult::RoleAssigned { .. } | DbResult::RoleRevoked { .. } => {
                bindings
                    .signals
                    .send(BindingSignal::AllDirty(Trigger::Role));
            }
            _ => {}
        }
    }
}

/// Answers from the worker for the bound snippet cache, and what the cache last showed.
#[derive(Resource)]
struct BoundSlotFeed {
    answers: Receiver<SnippetAnswer>,
    generation: u64,
    petal_id: Option<String>,
}

/// Feed `BoundSlotsState` (B10c): resolution for the active petal, snippet calls out, answers in.
fn feed_bound_slots(
    slots: Option<ResMut<BoundSlotsState>>,
    nav: Option<Res<NavigationManager>>,
    bindings: Res<ExtensionBindings>,
    mut feed: ResMut<BoundSlotFeed>,
) {
    let (Some(mut slots), Some(nav)) = (slots, nav) else {
        return;
    };
    let generation = bindings.view.generation();
    if generation != feed.generation || feed.petal_id.as_deref() != nav.active_petal_id.as_deref() {
        feed.generation = generation;
        feed.petal_id = nav.active_petal_id.clone();
        match nav.active_petal_id.as_deref() {
            Some(petal_id) => {
                let (panels, targets) = bindings.view.slots_at(petal_id);
                slots.set_resolution(petal_id, panels, targets);
            }
            None => slots.clear(),
        }
    }
    let requests = slots.take_requests();
    if let (false, Some(petal_id)) = (requests.is_empty(), nav.active_petal_id.as_ref()) {
        bindings.signals.send(BindingSignal::Snippets {
            petal_id: petal_id.clone(),
            requests,
        });
    }
    // Each answer is stored only against the args it was computed for: a stale one never shows for a newer query.
    while let Ok(answer) = feed.answers.try_recv() {
        slots.store_snippet_answer(&answer.target, &answer.args, &answer.label, &answer.text);
    }
}

/// Without a plugin manager every panel op answers `Failed`, so the Extensions panel never waits forever.
fn refuse_extension_ops(
    panel: Option<ResMut<ExtensionsPanelState>>,
    consent: Option<ResMut<ConsentState>>,
) {
    let (Some(mut panel), Some(mut consent)) = (panel, consent) else {
        return;
    };
    if panel.take_ops().is_empty() {
        return;
    }
    let message = "the extension host did not start on this machine; see the log".to_string();
    panel.apply_event(ExtensionEvent::Failed { message }, &mut consent);
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// `AllowDev` only in a debug build with the governance dev flag (read back from the trust index).
#[cfg(debug_assertions)]
fn unsigned_policy(allow_dev: bool) -> UnsignedPolicy {
    if allow_dev {
        UnsignedPolicy::AllowDev
    } else {
        UnsignedPolicy::Refuse
    }
}

/// Release builds never admit unsigned packages, whatever the flag says.
#[cfg(not(debug_assertions))]
fn unsigned_policy(_allow_dev: bool) -> UnsignedPolicy {
    UnsignedPolicy::Refuse
}

/// The shared source trust: every loaded root (revocation-capable included, R-REG), no shared index key, and the
/// accepted kill list matched by hash under any package id (the listing's id never narrows it).
fn source_trust(governance: &ExtensionGovernance, allow_dev: bool) -> SourceTrust {
    let kill_list = governance.clone();
    SourceTrust {
        is_trust_root: governance.trust_root_predicate(),
        index_signer: Arc::new(|_: &str| None),
        is_killed: Arc::new(move |_package_id: &str, hash: &str| kill_list.is_revoked_any(&[hash])),
        allow_unsigned_dev: allow_dev,
    }
}

/// One process-wide plugin manager on the worker's `TickHost`: the node DID, governance's gates and its very trust
/// roots (so a kill list verifies identically in both), fail closed on every gate.
fn build_manager(
    dir: &Path,
    commands: Sender<PluginCommand>,
    governance: &ExtensionGovernance,
    ticks: TickHost,
    contexts: PluginContexts,
    admission: bool,
) -> Result<PluginManager, String> {
    let engine = WasmEngine::new().map_err(|error| error.to_string())?;
    // A separate registry from the Bevy `PluginRegistry`: governance state lives here (F-TICK note).
    let registry = Arc::new(Mutex::new(PluginRegistry::new()));
    let allow_dev = governance.trust().tier_for(None).is_some();
    let manager = PluginManager::new(Arc::new(engine), dir.join(AOT_DIR), registry, commands)
        .map_err(|error| error.to_string())?
        .with_local_did(governance.local_did())
        .with_gates(governance.gates())
        .with_ticks(ticks)
        .with_contexts(contexts)
        .with_trust(
            governance.trust().roots().clone(),
            unsigned_policy(allow_dev),
        )
        .with_admission(admission);
    Ok(manager)
}

/// Build the bus (governance audit, real sources, local DID), governance, the network broker, the plugin host and
/// manager, the revocation pull, the ops drain and the binding worker (src/AGENTS.md §plugin-bridge).
pub(crate) fn install_with(app: &mut App, config: ExtensionBridgeConfig) {
    let db = app
        .world()
        .resource::<fe_runtime::app::DbCommandSender>()
        .0
        .clone();
    let local_did = config.local_did.clone();
    let verified = VerifiedBuilds::default();
    app.insert_resource(
        PluginContexts::default().with_factory(Arc::new(DbSceneFactory {
            sender: db.clone(),
            verified: verified.clone(),
        })),
    );

    let slot = ExtensionApiSlot::default();
    if let Some(api) = config.api_cmd_tx {
        slot.bind(api);
    }
    let roles = Arc::new(ExtensionRoles {
        local_did: local_did.clone(),
        grants: RwLock::new(HashMap::new()),
    });
    let policy = ExtensionExternalPolicy::default();
    app.insert_resource(policy.clone());
    app.insert_resource(ExtensionRoleView(roles.clone()));

    // Governance reads the role view and external policy just inserted; its sink is the bus's durable audit.
    let qa_root = fe_identity::file_fallback::qa_storage_root().ok().flatten();
    // Row 6 is the real verifier only with feature `entitlement`; without it (or a device key) licensed builds are refused.
    let licence_keys = config
        .identity_key
        .zip(config.device_key.as_ref().map(|device| device.secret_bytes()))
        .map(|(identity, device_secret)| LicenceKeys { identity, device_secret });
    let governance_config =
        GovernanceConfig::from_env(local_did.clone(), qa_root.as_deref()).with_device_entitlement(config.device_key);
    let data_dir: PathBuf = governance_config.config_dir.clone();
    // The token client only exists beside licence keys; an unreadable issuers file leaves it off (fail closed).
    #[cfg(feature = "entitlement")]
    let licence_tokens = licence_keys.as_ref().and_then(|_| {
        crate::extension_host::licence_client::LicenceTokens::new(&data_dir)
            .map_err(|reason| tracing::error!(%reason, "licence token client unavailable"))
            .ok()
    });
    let governance = governance::install(app, governance_config);
    let audit = governance.audit_sink();
    let role_source: Arc<dyn RoleSource> = roles;
    let allow_dev = governance.trust().tier_for(None).is_some();

    let in_flight = Arc::new(AtomicUsize::new(0));
    let sources = BusSources {
        query: Some(Arc::new(GuardedQuerySource {
            api: slot.clone(),
            in_flight: in_flight.clone(),
            rate: Mutex::new(HashMap::new()),
        })),
        scene: Some(Arc::new(DbSceneSnapshots {
            api: slot.clone(),
            in_flight,
        })),
        roles: role_source.clone(),
        policy: Arc::new(policy),
        audit: audit.clone(),
    };
    // The bus shares the host clock; insert it so PluginHostPlugin reuses the same TickHost.
    let ticks = app
        .world()
        .get_resource::<TickHost>()
        .cloned()
        .unwrap_or_default();
    let bus = Bus::new(ticks.clock(), sources, local_did.clone());
    app.insert_resource(ticks.clone());
    app.insert_resource(fe_plugin::BusHandle(bus.clone()));
    app.insert_resource(ExtensionBusParts {
        bus: bus.clone(),
        roles: role_source.clone(),
        audit: audit.clone(),
    });
    app.insert_resource(slot.clone());
    let blobs = ExtensionBlobSlot::default();
    app.insert_resource(blobs.clone());

    // The binding activator exists before the broker: bound connectors draw from its shared egress buckets (B10d).
    let activator = BindingActivator::new(ticks.clock());
    let shared_egress = activator.egress_buckets();
    // The network broker owns a small runtime for dial-outs and egress (net_sources builds inside it).
    let net_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("fe-ext-net")
        .enable_all()
        .build();
    let (net, runtime_handle) = match net_runtime {
        Ok(runtime) => {
            let net = {
                let _entered = runtime.enter();
                net_sources::build_net(
                    ticks.clock(),
                    role_source.clone(),
                    audit.clone(),
                    &data_dir,
                    shared_egress.clone(),
                )
            };
            let handle = runtime.handle().clone();
            app.insert_resource(ExtensionNetRuntime(runtime));
            (net, Some(handle))
        }
        Err(error) => {
            tracing::warn!(%error, "extension network runtime did not start: no dial-outs or egress");
            (
                net_sources::build_net(
                    ticks.clock(),
                    role_source.clone(),
                    audit.clone(),
                    &data_dir,
                    shared_egress,
                ),
                None,
            )
        }
    };

    let (reading_tx, reading_rx) = crossbeam::channel::bounded::<ReadingEvent>(1024);
    app.insert_resource(ReadingFeed(reading_tx));
    spawn_reading_feed(bus.clone(), reading_rx);
    if let Some(changes) = config.scene_changes {
        spawn_scene_feed(bus.clone(), changes);
    }

    app.add_plugins(fe_plugin::PluginHostPlugin);
    // PluginHostPlugin re-inserted the TickHost bound to the shared contexts; every stage and stop uses that one.
    let ticks = app.world().resource::<TickHost>().clone();
    let contexts = app.world().resource::<PluginContexts>().clone();
    let commands = app
        .world()
        .resource::<fe_plugin::PluginCommandSender>()
        .0
        .clone();
    let admission = app
        .world()
        .get_resource::<fe_plugin::PackageAdmission>()
        .is_some_and(|admission| admission.0);

    let index = InstallationIndex::default();
    let live = LiveInstances::default();
    let manager = match build_manager(
        &data_dir,
        commands.clone(),
        &governance,
        ticks.clone(),
        contexts,
        admission,
    ) {
        Ok(manager) => Some(Arc::new(Mutex::new(manager))),
        Err(error) => {
            tracing::error!(%error, "plugin manager did not start: extensions stay off on this host");
            None
        }
    };

    let (signal_tx, signal_rx) = crossbeam::channel::unbounded::<BindingSignal>();
    let signals = BindingSignals {
        sender: signal_tx,
        backlog: Arc::new(AtomicUsize::new(0)),
    };
    let view = BindingView::default();
    let petals = PetalDirectory::default();
    let bindings = ExtensionBindings {
        local_did: local_did.clone(),
        local_client_did: config.local_client_did.clone(),
        db: db.clone(),
        bus: bus.clone(),
        roles: role_source.clone(),
        governance: governance.clone(),
        view: view.clone(),
        petals: petals.clone(),
        signals: signals.clone(),
    };
    let trace = fe_runtime::extension_trace::ExtensionTraceBuffer::default();
    app.insert_resource(trace.clone());
    app.insert_resource(ExtensionHostParts {
        local_did: local_did.clone(),
        local_client_did: config.local_client_did,
        bus: bus.clone(),
        net: net.clone(),
        runtime: runtime_handle,
        ticks: ticks.clone(),
        governance: governance.clone(),
        index: index.clone(),
        view: view.clone(),
        live: live.clone(),
        roles: role_source,
        audit,
        signals: manager.as_ref().map(|_| signals.clone()),
        trace: trace.clone(),
    });
    app.insert_resource(bindings);
    app.add_systems(
        Update,
        (mirror_petals, follow_navigation, follow_binding_rows),
    );

    let Some(manager) = manager else {
        app.add_systems(Update, refuse_extension_ops);
        return;
    };
    // Disable all from `FE_EXT_DISABLE_ALL=1` (the manager starts from it) reaches the portal once it links.
    portal_host_switch(lock(&manager).governance().is_disabled_all());
    governance.attach_manager(Arc::new(ManagerControlHandle {
        manager: manager.clone(),
        signals: signals.clone(),
    }));
    let (pull, pull_handle) = revocation_pull::start(
        governance.clone(),
        Some(data_dir.join(revocation_pull::CACHE_DIR)),
        revocation_pull::interval_from_env(),
    );
    app.insert_resource(revocation_pull::RevocationPullService(Mutex::new(pull)));
    app.insert_resource(pull_handle);

    let components = match ComponentHost::new() {
        Ok(host) => Some(Arc::new(host)),
        Err(error) => {
            tracing::error!(%error, "extension component host did not start: no 2.0 instance can run");
            None
        }
    };
    let stager = InstanceStager {
        ticks: ticks.clone(),
        bus,
        net,
        components,
        commands,
        local_did: local_did.clone(),
        verified,
        petals: petals.clone(),
        live: live.clone(),
    };
    let owners = VerseOwners {
        api: slot.clone(),
        local_did: local_did.clone(),
        cache: Arc::default(),
    };

    // Accepted device payloads reach the decoder through the worker; the DB thread never waits.
    let payload_signals = signals.clone();
    let installed = set_accepted_payload_sink(Box::new(move |event: AcceptedPayloadEvent| {
        payload_signals.payload(DecodeJob {
            payload_id: event.payload_id,
            format_id: event.format_id,
            mime: event.mime,
            node_id: event.node_id,
            petal_id: event.petal_id,
            recorded_at_ms: event.recorded_at_ms,
            asset_ids: event.asset_ids,
        });
    }));
    if !installed {
        tracing::warn!("an accepted-payload sink was already installed; payload decode is off");
    }

    app.insert_resource(ExtensionSceneHooks(Arc::new(SceneHooks {
        local_did: local_did.clone(),
        view: view.clone(),
        live: live.clone(),
        signals: signals.clone(),
    })));

    let (answer_tx, answer_rx) = crossbeam::channel::unbounded::<SnippetAnswer>();
    app.insert_resource(BoundSlotFeed {
        answers: answer_rx,
        generation: 0,
        petal_id: None,
    });
    app.add_systems(Update, feed_bound_slots);

    // Third-party ui entries follow activation (R-UI-03); other packages' slot commands go to the bus (R-HOOKS-33).
    app.insert_resource(ExtensionUiSync {
        index: index.clone(),
        seen: u64::MAX,
        applied: BTreeMap::new(),
    });
    app.insert_resource(ExtensionSlotDrain(Mutex::new(SlotDrain::new(BusSlotCaller::new(
        Arc::new(stager.bus.clone()),
    )))));
    app.add_systems(Update, (sync_extension_ui, drain_slot_commands));

    app.insert_resource(ExtensionImportHooks(Arc::new(ImportPort {
        view: view.clone(),
        signals: signals.clone(),
        in_flight: Arc::new(AtomicUsize::new(0)),
    })));

    // `blob.source`: the sync thread's scene transfers and the `blob://` reader ask the worker after peers miss.
    let blob_slot = config.blob_fallback;
    let blob_source = blob_slot.as_ref().map(|slot| {
        let (queue, service) = blob_source_channel();
        slot.install(Arc::new(queue));
        service
    });
    let mirror_queue = ExtensionBlobMirror::default();
    app.insert_resource(mirror_queue.clone());
    // Committed local imports feed the mirror queue once the worker built it (src/AGENTS.md §plugin-host).
    app.add_systems(Update, crate::extension_host::blob_mirror_tap::feed_blob_mirror);
    // One offer set: the ops worker refreshes it, the binding worker resolves against it.
    let registry_offers = RegistryOffers::new();

    let worker = BindingWorker {
        manager: manager.clone(),
        governance: governance.clone(),
        activator,
        catalog: InstalledCatalog::new(&data_dir, governance.clone(), allow_dev),
        stager: stager.clone(),
        hook_caller: Arc::new(LiveHookCaller(live.clone())),
        live,
        petals,
        db: db.clone(),
        view: view.clone(),
        index: index.clone(),
        reader: PayloadReader { api: slot, blobs },
        owners: owners.clone(),
        answers: answer_tx,
        backlog: signals.backlog.clone(),
        admission,
        local_did,
        snapshots: HashMap::new(),
        retry: HashMap::new(),
        acceptance_retry: Some(Instant::now()),
        last_sweep: Instant::now(),
        changed: true,
        running: Vec::new(),
        active_petal: None,
        blob_source,
        blob_slot,
        blob_mirror: None,
        mirror_queue,
        registry_offers: registry_offers.clone(),
        provisions: Vec::new(),
        scene_calls: SceneCallQueue::default(),
        trace,
    };
    let service = OwnedService::spawn("ext-bindings", move |stop| worker.run(signal_rx, stop));
    app.insert_resource(ExtensionBindingService(Mutex::new(Some(service))));

    // The ops drain builds its sources on its own thread (protocol-v1 sources own a private runtime).
    let ports = BridgeOpsPorts {
        governance: governance.clone(),
        stager,
        db,
        owners,
        index,
        signals,
        view,
        licence_keys,
        #[cfg(feature = "entitlement")]
        licence_tokens,
    };
    let ops_trust = source_trust(&governance, allow_dev);
    let worker_trust = source_trust(&governance, allow_dev);
    let ops = ExtensionOpsPlugin::new(move || {
        let configs = load_source_configs(&data_dir.join(SOURCES_FILE)).unwrap_or_else(|reason| {
            tracing::warn!(%reason, "extension sources unreadable: Browse lists none");
            Vec::new()
        });
        let sources = SourceSet::build(&configs, &ops_trust);
        OpsWorker::new(
            manager,
            PluginPackageStore::new(data_dir.join(PACKAGES_DIR)),
            worker_trust,
            sources,
            Box::new(ports),
        )
        .with_registry_offers(registry_offers)
    });
    let admin_jobs = ops.admin_jobs();
    app.add_plugins(ops);
    // `fe-ext admin` over this OS user's local channel only (never REST/MCP); this binary always has a window, so
    // consent can be shown. `FE_EXT_ADMIN_CHANNEL=0` keeps it off (fe-plugin/src/admin/AGENTS.md).
    app.insert_resource(admin_channel::spawn(AdminChannelPorts::new(admin_jobs, true)));
}

// ---------------------------------------------------------------------------
// UI wiring: manifest ui entries and other packages' slot commands
// ---------------------------------------------------------------------------

/// What the ui registry last applied per package: the build's content hash.
#[derive(Resource)]
struct ExtensionUiSync {
    index: InstallationIndex,
    seen: u64,
    applied: BTreeMap<String, String>,
}

/// Register a running build's manifest `ui` entries and panel content set; forget them when it stops (disable,
/// revoke, uninstall). Idle frames compare one counter.
fn sync_extension_ui(
    mut sync: ResMut<ExtensionUiSync>,
    registry: Option<ResMut<fe_ui::plugin::UiExtensions>>,
    bound: Option<ResMut<BoundSlotsState>>,
    slots: Option<ResMut<ExtensionSlotState>>,
) {
    let (Some(mut registry), Some(mut bound), Some(mut slots)) = (registry, bound, slots) else {
        return;
    };
    let generation = sync.index.generation();
    if generation == sync.seen {
        return;
    }
    sync.seen = generation;
    let active: BTreeMap<String, (String, Arc<ExtensionManifest>)> = sync
        .index
        .rows()
        .into_values()
        // The terrain package's entries are first-party (main.rs registers them); this sync never owns them.
        .filter(|row| !crate::extension_host::slot_drain::is_host_command(&row.package_id))
        .filter(|row| row.instances.iter().any(|instance| !instance.degraded))
        .filter_map(|row| Some((row.package_id, (row.content_hash, row.manifest?))))
        .collect();
    let gone: Vec<String> = sync
        .applied
        .keys()
        .filter(|package_id| !active.contains_key(*package_id))
        .cloned()
        .collect();
    for package_id in gone {
        ui_registry::deactivate_package(&mut registry.0, &mut bound, &mut slots, &package_id);
        sync.applied.remove(&package_id);
    }
    for (package_id, (content_hash, manifest)) in active {
        if sync.applied.get(&package_id) != Some(&content_hash) {
            ui_registry::activate_build(&mut registry.0, &mut bound, &manifest);
            sync.applied.insert(package_id, content_hash);
        }
    }
}

/// The slot drain over the host bus (local user, kind `ui`).
#[derive(Resource)]
struct ExtensionSlotDrain(Mutex<SlotDrain<BusSlotCaller>>);

/// Send other packages' pressed slot commands to the bus with their args; the terrain package's stay queued for
/// `terrain_commands`. A refusal or failure shows as an error notice.
fn drain_slot_commands(
    drain: Res<ExtensionSlotDrain>,
    slots: Option<ResMut<ExtensionSlotState>>,
    nav: Option<Res<NavigationManager>>,
    ui: Option<ResMut<fe_ui::actions::UiManager>>,
    clock: Option<Res<fe_ui::notice::NoticeClock>>,
) {
    let mut drain = lock(&drain.0);
    let commands = match slots {
        Some(mut slots) if slots.pending_commands.iter().any(|request| !slot_drain_is_host(request)) => {
            let (host, foreign): (Vec<_>, Vec<_>) =
                slots.take_commands().into_iter().partition(slot_drain_is_host);
            slots.pending_commands = host;
            foreign
                .into_iter()
                .map(|request| SlotCommand {
                    package_id: request.plugin_id,
                    command: request.command,
                    args: request.args,
                })
                .collect()
        }
        _ if drain.in_flight() == 0 => return,
        _ => Vec::new(),
    };
    let petal_id = nav.as_ref().and_then(|nav| nav.active_petal_id.clone());
    let reports = drain.run(petal_id.as_deref(), commands);
    let Some(mut ui) = ui else { return };
    let now = clock.map_or(0.0, |clock| clock.elapsed_secs_f64());
    for report in reports {
        match report {
            SlotReport::Done { package_id, command } => {
                tracing::debug!(%package_id, %command, "extension slot command answered");
            }
            SlotReport::Refused { package_id, command, message, .. }
            | SlotReport::Failed { package_id, command, message } => {
                ui.show_error(format!("{package_id}: {command} did not run ({message})"), now);
            }
        }
    }
}

/// True for a queued command another dispatcher owns (read-only scan, no take).
fn slot_drain_is_host(request: &fe_ui::extensions::ExtensionCommandRequest) -> bool {
    crate::extension_host::slot_drain::is_host_command(&request.plugin_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::manifest_v2::ExtensionSourceClass;

    fn admission() -> PluginAdmission {
        PluginAdmission::new(
            "org.example.fleet@petal-1".into(),
            "petal-1".into(),
            vec![
                fe_sdk::CAP_STORAGE_READ.into(),
                fe_sdk::CAP_STORAGE_WRITE.into(),
            ],
            Vec::new(),
            vec!["VERSE#v1".into()],
            vec!["VERSE#v1-FRACTAL#f1-PETAL#petal-1".into()],
        )
        .expect("admission")
    }

    fn grants(package_id: &str) -> V2Grants {
        V2Grants {
            package_id: package_id.into(),
            scene_read: true,
            scene_write: vec![SceneWrite::SetProperty],
            metrics: vec!["temperature".into()],
            readings_class: Some(ExtensionSourceClass::Device),
            routes: false,
            tick: None,
        }
    }

    /// G6-C6 at the bridge: a v2 admission is stamped only with the build this host verified under the context id.
    #[test]
    fn sealed_admission_stamps_only_the_verified_build() {
        let verified = VerifiedBuilds::default();
        let plugin_id = "org.example.fleet@petal-1";
        let hash = "a".repeat(64);

        let unstaged = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        );
        assert!(
            unstaged.is_err(),
            "no verified build: refused, never an unstamped admission"
        );

        verified.record(
            plugin_id,
            PackageIdentity::new("org.example.other".into(), hash.clone()).expect("identity"),
        );
        let swapped = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        );
        assert!(
            swapped.is_err(),
            "a verified build of another package never stamps this token"
        );

        verified.record(
            plugin_id,
            PackageIdentity::new("org.example.fleet".into(), hash.clone()).expect("identity"),
        );
        let stamped = sealed_admission(
            admission(),
            plugin_id,
            &grants("org.example.fleet"),
            &verified,
        )
        .expect("stamped");
        assert_eq!(
            stamped.package().map(PackageIdentity::content_hash),
            Some(hash.as_str())
        );
        assert!(stamped.allows_write(SceneWrite::SetProperty));
        assert!(
            !stamped.allows_write(SceneWrite::DeleteNode),
            "writes narrow to the declared kinds"
        );
        let readings = stamped.readings().expect("readings bound");
        assert_eq!(readings.source_class(), SourceClass::Device);
        assert!(readings.allows_metric("temperature") && !readings.allows_metric("pressure"));
    }
}
