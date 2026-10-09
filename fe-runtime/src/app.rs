use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;

use crate::bevy_blob_reader::OnMissCallback;
use crate::blob_store::BlobStoreHandle;
use crate::messages::{
    ApiCommand, DbCommand, DbResult, LifecycleEvent, NetworkCommand, NetworkEvent, TransformUpdate,
    VerseHierarchyData,
};

#[derive(Resource)]
pub struct NetworkCommandSender(pub crossbeam::channel::Sender<NetworkCommand>);

#[derive(Resource)]
pub struct DbCommandSender(pub crossbeam::channel::Sender<DbCommand>);

#[derive(Resource)]
pub struct NetworkEventReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<NetworkEvent>>>);

#[derive(Resource)]
pub struct DbResultReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<DbResult>>>);

/// Receiver half of the DB thread's lifecycle-event seam, pumped into Bevy
/// `Messages<LifecycleEvent>` (T2 integration; fe-ui consumes `PathReflow`).
#[derive(Resource)]
pub struct LifecycleEventReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<LifecycleEvent>>>);

/// Channel handles that the Bevy app needs to communicate with background
/// threads (network, database). Constructed during engine wiring and passed
/// to [`setup_core_systems`] to register the corresponding ECS resources.
pub struct BevyHandles {
    pub net_cmd_tx: crossbeam::channel::Sender<NetworkCommand>,
    pub net_evt_rx: crossbeam::channel::Receiver<NetworkEvent>,
    pub db_cmd_tx: crossbeam::channel::Sender<DbCommand>,
    pub db_res_rx: crossbeam::channel::Receiver<DbResult>,
    /// Shared blob store handle for the `blob://` Bevy asset source.
    pub blob_store: Option<BlobStoreHandle>,
    /// Optional callback fired when the blob asset reader encounters a cache
    /// miss.  Set by the sync layer (Phase D) to trigger peer fetch.
    pub on_blob_miss: Option<OnMissCallback>,
    /// Optional receiver for DB-thread lifecycle events (create/promote/
    /// tombstone/reflow), pumped into `Messages<LifecycleEvent>`. The GUI binary
    /// passes `Some`; headless relay/tests pass `None`.
    pub lifecycle_rx: Option<crossbeam::channel::Receiver<LifecycleEvent>>,
}

/// Register ECS resources and drain systems shared by both GUI and headless
/// binaries. Call this on any `App` (with `DefaultPlugins` *or*
/// `MinimalPlugins`) to wire up the core channel infrastructure.
pub fn setup_core_systems(app: &mut App, handles: BevyHandles) {
    app.add_message::<NetworkEvent>();
    app.add_message::<DbResult>();
    app.add_message::<LifecycleEvent>();
    app.insert_resource(NetworkCommandSender(handles.net_cmd_tx));
    app.insert_resource(DbCommandSender(handles.db_cmd_tx));
    app.insert_resource(NetworkEventReceiver(Arc::new(Mutex::new(
        handles.net_evt_rx,
    ))));
    app.insert_resource(DbResultReceiver(Arc::new(Mutex::new(handles.db_res_rx))));
    if let Some(lifecycle_rx) = handles.lifecycle_rx {
        app.insert_resource(LifecycleEventReceiver(Arc::new(Mutex::new(lifecycle_rx))));
    }
    app.add_systems(
        Update,
        (
            drain_network_events,
            drain_db_results,
            drain_lifecycle_events,
        ),
    );
}

/// Build a full GUI application with `DefaultPlugins`, blob asset source, and
/// the core ECS resources. The caller is responsible for adding GUI-specific
/// plugins such as `EguiPlugin` and `FrameTimeDiagnosticsPlugin` after this
/// returns.
pub fn build_app(handles: BevyHandles) -> App {
    let mut app = App::new();

    // Phase B: register the "blob" asset source BEFORE DefaultPlugins so Bevy
    // knows how to load `blob://{hash}.glb` paths via BlobAssetReader.
    if let Some(ref blob_store) = handles.blob_store {
        let blob_store = blob_store.clone();
        let on_miss = handles.on_blob_miss.clone();
        app.register_asset_source(
            "blob",
            AssetSourceBuilder::new(move || match on_miss.clone() {
                Some(cb) => Box::new(crate::bevy_blob_reader::BlobAssetReader::with_on_miss(
                    blob_store.clone(),
                    cb,
                )),
                None => Box::new(crate::bevy_blob_reader::BlobAssetReader::new(
                    blob_store.clone(),
                )),
            }),
        );
        tracing::info!("Registered 'blob' asset source (BlobAssetReader)");
    }

    // Override AssetPlugin's file_path so Bevy loads assets from the same
    // directory the GLB import writer uses (`fractalengine/assets` relative
    // to CWD). We resolve it to an ABSOLUTE path to avoid Bevy's normal
    // CARGO_MANIFEST_DIR prefixing (which would yield
    // `fractalengine/fractalengine/assets`) and to avoid the exe-relative
    // fallback path when launched standalone from `target/{debug,release}/`.
    let asset_root = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("fractalengine")
        .join("assets");
    tracing::info!("Bevy AssetPlugin file_path = {}", asset_root.display());
    app.add_plugins(DefaultPlugins.set(bevy::asset::AssetPlugin {
        file_path: asset_root.to_string_lossy().into_owned(),
        ..Default::default()
    }));

    app.add_plugins(crate::diag15m::Diag15MPlugin); // DIAG-15M

    setup_core_systems(&mut app, handles);
    app
}

fn drain_network_events(
    receiver: Res<NetworkEventReceiver>,
    mut writer: MessageWriter<NetworkEvent>,
) {
    let _span = tracing::debug_span!("drain_network_events").entered();
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(evt) = rx.try_recv() {
            writer.write(evt);
        }
    }
}

fn drain_db_results(receiver: Res<DbResultReceiver>, mut writer: MessageWriter<DbResult>) {
    let _span = tracing::debug_span!("drain_db_results").entered();
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(result) = rx.try_recv() {
            writer.write(result);
        }
    }
}

/// Drain-without-blocking pump mirroring `drain_db_results`; `Option` so apps
/// wired without a lifecycle channel (relay, tests) run it as a no-op.
fn drain_lifecycle_events(
    receiver: Option<Res<LifecycleEventReceiver>>,
    mut writer: MessageWriter<LifecycleEvent>,
) {
    let _span = tracing::debug_span!("drain_lifecycle_events").entered();
    let Some(receiver) = receiver else { return };
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(event) = rx.try_recv() {
            writer.write(event);
        }
    };
}

// ---------------------------------------------------------------------------
// API Gateway resources and drain system
// ---------------------------------------------------------------------------

/// Bevy resource: the receiver end of the API command channel.
#[derive(Resource)]
pub struct ApiCommandReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<ApiCommand>>>);

/// Bevy resource: sender end that the API thread clones for its use.
#[derive(Resource)]
pub struct ApiCommandSender(pub crossbeam::channel::Sender<ApiCommand>);

/// Bevy resource: broadcast sender for real-time transform fan-out.
#[derive(Resource, Clone)]
pub struct TransformBroadcastSender(pub tokio::sync::broadcast::Sender<TransformUpdate>);

/// Bevy resource: crossbeam receiver for inbound API transform updates.
/// Bridged from the tokio broadcast in the API thread so Bevy can poll
/// without a tokio runtime.
#[derive(Resource)]
pub struct InboundTransformReceiver(pub crossbeam::channel::Receiver<TransformUpdate>);

/// Bevy resource: broadcast sender for revoked token JTI notifications.
/// When a token is revoked, the JTI is broadcast so the API thread can
/// update its revocation cache immediately.
#[derive(Resource, Clone)]
pub struct RevocationBroadcastSender(pub tokio::sync::broadcast::Sender<String>);

/// Bevy resource: pending API requests awaiting DB results.
///
/// **Reply correlation (F5).** Requests used to be matched to replies by bare
/// FIFO order, so a `DbResult` that arrives with *no* pending request — the
/// relay's startup-scan `HierarchyLoaded`, a GUI-initiated hierarchy reload, an
/// unsolicited `ReplicatedRowApplied` from the inbound replication pump — could
/// consume an unrelated waiter's oneshot (`/ready`'s `Ping` receiving a
/// `HierarchyLoaded`). Each request is now filed under the reply family its
/// `DbCommand` produces, and a result is handed only to a request filed under
/// that same family; a result with no match is dropped for the API path (it
/// still reaches the Bevy `Messages<DbResult>` stream for UI consumers).
///
/// `DbResult::Error` is the DB thread's universal failure reply and carries no
/// family, so it goes to the **oldest** pending request of any family — the
/// failing command is the one that has been waiting longest, and a caller must
/// never lose its error to a family mismatch.
#[derive(Resource, Default)]
pub struct PendingApiRequests {
    by_kind: HashMap<ReplyKind, std::collections::VecDeque<PendingEntry>>,
    /// Requests whose command has no awaited reply family (legacy behaviour:
    /// matched only by other uncorrelated results, plus wildcard errors).
    uncorrelated: std::collections::VecDeque<PendingEntry>,
    pending_hierarchy: Vec<tokio::sync::oneshot::Sender<Vec<VerseHierarchyData>>>,
    next_id: u64,
}

/// One API request waiting for its reply, with its enqueue order so an
/// untyped (`Error`) reply can be routed to the oldest waiter, and the
/// command's correlation id (if its family carries one).
struct PendingEntry {
    id: u64,
    correlation_id: Option<String>,
    reply_tx: tokio::sync::oneshot::Sender<DbResult>,
}

/// The caller-supplied correlation id a command carries, for the families
/// that echo one on their reply (see [`PendingApiRequests::try_deliver`]).
fn correlation_of_command(cmd: &DbCommand) -> Option<&str> {
    match cmd {
        DbCommand::InsertIotReadings { correlation_id, .. }
        | DbCommand::CreateAsset { correlation_id, .. }
        | DbCommand::CreateNodeWithAsset { correlation_id, .. }
        | DbCommand::RawQuery { correlation_id, .. } => correlation_id.as_deref(),
        _ => None,
    }
}

/// The correlation id a reply echoes (mirror of [`correlation_of_command`]).
fn correlation_of_result(result: &DbResult) -> Option<&str> {
    match result {
        DbResult::IotReadingsInserted { correlation_id, .. }
        | DbResult::IotReadingsRejected { correlation_id, .. }
        | DbResult::AssetCreated { correlation_id, .. }
        | DbResult::GltfImported { correlation_id, .. }
        | DbResult::QueryResult { correlation_id, .. } => correlation_id.as_deref(),
        DbResult::QueryFailed { correlation_id, .. } => Some(correlation_id.as_str()),
        _ => None,
    }
}

/// The reply family a command's result belongs to — the correlation key that
/// replaced FIFO pairing. Kept private: callers enqueue with the command and
/// never name a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReplyKind {
    Pong,
    VerseCreated,
    FractalCreated,
    PetalCreated,
    NodeCreated,
    Hierarchy,
    ScopeResolved,
    NodesLoaded,
    NodeTransformLoaded,
    NodeProperties,
    NodePropertySet,
    NodePropertyDeleted,
    NodeDeleted,
    NodePromoted,
    FieldDefCreated,
    FieldDefsListed,
    FieldDefUpdated,
    FieldDefDeleted,
    QueryResult,
    IotReadingsInserted,
    PetalTerrain,
    VerseTimeseriesSettings,
    AssetCreated,
    PlacedAsset,
}

/// The reply family `cmd` produces, or `None` when the command has no reply on
/// this channel (fire-and-forget writes) — see [`PendingApiRequests`].
fn reply_kind_of_command(cmd: &DbCommand) -> Option<ReplyKind> {
    use DbCommand::*;
    Some(match cmd {
        Ping => ReplyKind::Pong,
        CreateVerse { .. } => ReplyKind::VerseCreated,
        CreateFractal { .. } => ReplyKind::FractalCreated,
        CreatePetal { .. } => ReplyKind::PetalCreated,
        CreateNode { .. } => ReplyKind::NodeCreated,
        LoadHierarchy => ReplyKind::Hierarchy,
        ResolvePetalScope { .. }
        | ResolveNodeScope { .. }
        | ResolveFractalScope { .. }
        | ResolveVerseScope { .. } => ReplyKind::ScopeResolved,
        LoadNodesByPetal { .. } => ReplyKind::NodesLoaded,
        GetNodeTransform { .. } => ReplyKind::NodeTransformLoaded,
        GetNodeProperties { .. } => ReplyKind::NodeProperties,
        SetNodeProperty { .. } => ReplyKind::NodePropertySet,
        DeleteNodeProperty { .. } => ReplyKind::NodePropertyDeleted,
        DeleteNode { .. } | TombstoneNode { .. } | CascadeTombstoneNode { .. } => {
            ReplyKind::NodeDeleted
        }
        PromoteInstance { .. } => ReplyKind::NodePromoted,
        CreateFieldDef { .. } => ReplyKind::FieldDefCreated,
        ListFieldDefs { .. } => ReplyKind::FieldDefsListed,
        UpdateFieldDef { .. } => ReplyKind::FieldDefUpdated,
        DeleteFieldDef { .. } => ReplyKind::FieldDefDeleted,
        RawQuery { .. } => ReplyKind::QueryResult,
        InsertIotReadings { .. } => ReplyKind::IotReadingsInserted,
        GetPetalTerrain { .. } => ReplyKind::PetalTerrain,
        SetVerseTimeseriesSettings { .. } => ReplyKind::VerseTimeseriesSettings,
        CreateAsset { .. } => ReplyKind::AssetCreated,
        CreateNodeWithAsset { .. } => ReplyKind::PlacedAsset,
        _ => return None,
    })
}

/// The reply family a result belongs to, or `None` when it is not a correlated
/// reply (`Error` is handled separately as the universal reply — see
/// [`PendingApiRequests`]).
fn reply_kind_of_result(result: &DbResult) -> Option<ReplyKind> {
    use DbResult::*;
    Some(match result {
        Pong => ReplyKind::Pong,
        VerseCreated { .. } => ReplyKind::VerseCreated,
        FractalCreated { .. } => ReplyKind::FractalCreated,
        PetalCreated { .. } => ReplyKind::PetalCreated,
        NodeCreated { .. } => ReplyKind::NodeCreated,
        HierarchyLoaded { .. } => ReplyKind::Hierarchy,
        ScopeResolved { .. } => ReplyKind::ScopeResolved,
        NodesLoaded { .. } => ReplyKind::NodesLoaded,
        NodeTransformLoaded { .. } => ReplyKind::NodeTransformLoaded,
        NodePropertiesLoaded { .. } => ReplyKind::NodeProperties,
        NodePropertySet { .. } => ReplyKind::NodePropertySet,
        NodePropertyDeleted { .. } => ReplyKind::NodePropertyDeleted,
        NodeDeleted { .. } => ReplyKind::NodeDeleted,
        NodePromoted { .. } => ReplyKind::NodePromoted,
        FieldDefCreated { .. } => ReplyKind::FieldDefCreated,
        FieldDefsListed { .. } => ReplyKind::FieldDefsListed,
        FieldDefUpdated { .. } => ReplyKind::FieldDefUpdated,
        FieldDefDeleted { .. } => ReplyKind::FieldDefDeleted,
        QueryResult { .. } | QueryFailed { .. } => ReplyKind::QueryResult,
        IotReadingsInserted { .. } => ReplyKind::IotReadingsInserted,
        IotReadingsRejected { .. } => ReplyKind::IotReadingsInserted,
        PetalTerrainLoaded { .. } => ReplyKind::PetalTerrain,
        VerseTimeseriesSettingsSet { .. } => ReplyKind::VerseTimeseriesSettings,
        AssetCreated { .. } => ReplyKind::AssetCreated,
        // Only an API placement (correlated) is a reply; a GUI `ImportGltf`
        // result (`None`) keeps its legacy unmapped routing (§api-reply-correlation).
        GltfImported {
            correlation_id: Some(_),
            ..
        } => ReplyKind::PlacedAsset,
        _ => return None,
    })
}

impl PendingApiRequests {
    /// Enqueue a DB request under the reply family `cmd` produces and return
    /// its correlation id (insertion order, used only to route untyped
    /// `Error` replies to the oldest waiter).
    pub fn enqueue_for(
        &mut self,
        cmd: &DbCommand,
        reply_tx: tokio::sync::oneshot::Sender<DbResult>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let entry = PendingEntry {
            id,
            correlation_id: correlation_of_command(cmd).map(str::to_string),
            reply_tx,
        };
        match reply_kind_of_command(cmd) {
            Some(kind) => self.by_kind.entry(kind).or_default().push_back(entry),
            None => self.uncorrelated.push_back(entry),
        }
        id
    }

    /// Hand a correlated reply ONLY to the entry filed under its correlation
    /// id — even a closed one (its caller timed out): that entry consumes and
    /// drops the late reply, so it can never shift onto the next waiter
    /// (src/AGENTS.md §api-reply-correlation). An id with no entry is not
    /// delivered. An uncorrelated reply reaches only an uncorrelated entry
    /// (never a correlated waiter — a GUI `RawQuery` result must not land on
    /// an API request, M4 fix B2).
    fn deliver_correlated(
        queue: &mut std::collections::VecDeque<PendingEntry>,
        correlation_id: Option<&str>,
        result: DbResult,
    ) -> bool {
        let Some(correlation_id) = correlation_id else {
            return Self::deliver_uncorrelated(queue, result);
        };
        let Some(position) = queue
            .iter()
            .position(|e| e.correlation_id.as_deref() == Some(correlation_id))
        else {
            tracing::debug!(
                correlation_id,
                "correlated DB reply has no pending waiter — dropped"
            );
            return false;
        };
        let entry = queue
            .remove(position)
            .expect("position is within the queue");
        if entry.reply_tx.is_closed() {
            tracing::debug!(
                correlation_id,
                "late DB reply for a timed-out API request — consumed by its own entry"
            );
            return false;
        }
        entry.reply_tx.send(result).is_ok()
    }

    /// Skip-closed FIFO restricted to entries WITHOUT a correlation id;
    /// correlated entries are left in place for their own echoed reply.
    fn deliver_uncorrelated(
        queue: &mut std::collections::VecDeque<PendingEntry>,
        result: DbResult,
    ) -> bool {
        let mut idx = 0;
        while idx < queue.len() {
            if queue[idx].correlation_id.is_some() {
                idx += 1;
                continue;
            }
            let entry = queue.remove(idx).expect("idx is within the queue");
            if entry.reply_tx.is_closed() {
                continue;
            }
            let _ = entry.reply_tx.send(result);
            return true;
        }
        false
    }

    /// Pop the next live entry (skipping ones whose receiver was dropped by a
    /// timed-out caller) and send `result` to it.
    fn deliver_to(queue: &mut std::collections::VecDeque<PendingEntry>, result: DbResult) -> bool {
        while let Some(entry) = queue.pop_front() {
            if entry.reply_tx.is_closed() {
                // Receiver dropped — discard stale entry and try the next one.
                continue;
            }
            let _ = entry.reply_tx.send(result);
            return true;
        }
        false
    }

    /// Route an untyped reply (the DB thread's universal `Error`) to the oldest
    /// pending request of any family.
    fn deliver_to_oldest(&mut self, result: DbResult) -> bool {
        let mut oldest: Option<ReplyKind> = None;
        let mut oldest_id = u64::MAX;
        for (kind, queue) in &self.by_kind {
            if let Some(entry) = queue.front() {
                if entry.id < oldest_id {
                    oldest_id = entry.id;
                    oldest = Some(*kind);
                }
            }
        }
        let oldest_uncorrelated = self.uncorrelated.front().map(|e| e.id).unwrap_or(u64::MAX);
        if oldest_uncorrelated < oldest_id {
            return Self::deliver_to(&mut self.uncorrelated, result);
        }
        match oldest {
            Some(kind) => {
                let queue = self.by_kind.get_mut(&kind).expect("kind came from by_kind");
                Self::deliver_to(queue, result)
            }
            None => Self::deliver_to(&mut self.uncorrelated, result),
        }
    }

    /// Try to deliver a `DbResult` to the pending request that asked for it.
    /// Returns true if a result was delivered.
    ///
    /// The result's [`ReplyKind`] selects the queue; a result whose family has
    /// no waiter is dropped (never offered to an unrelated request). A reply
    /// carrying a correlation id goes only to its own entry
    /// ([`Self::deliver_correlated`]). `Error` is the wildcard that reaches
    /// the oldest waiter (family-blind — a known sharp edge, AGENTS.md).
    pub fn try_deliver(&mut self, result: DbResult) -> bool {
        if matches!(result, DbResult::Error(_)) {
            return self.deliver_to_oldest(result);
        }
        match reply_kind_of_result(&result) {
            Some(kind) => {
                let Some(queue) = self.by_kind.get_mut(&kind) else {
                    return false;
                };
                let correlation_id = correlation_of_result(&result).map(str::to_string);
                Self::deliver_correlated(queue, correlation_id.as_deref(), result)
            }
            None => Self::deliver_to(&mut self.uncorrelated, result),
        }
    }

    pub fn enqueue_hierarchy(
        &mut self,
        reply_tx: tokio::sync::oneshot::Sender<Vec<VerseHierarchyData>>,
    ) {
        self.pending_hierarchy.push(reply_tx);
    }

    pub fn deliver_hierarchy(&mut self, data: Vec<VerseHierarchyData>) {
        for tx in self.pending_hierarchy.drain(..) {
            let _ = tx.send(data.clone());
        }
    }
}

/// Bevy system: deliver every `DbResult` to pending API requests.
///
/// The GUI binary does this delivery from fe-ui's `apply_db_results`
/// dispatcher (which owns UI-side skip semantics). A headless host has no
/// fe-ui — without this system, every channel-fallback API request
/// (`DbCommand::Ping` → `/ready`, `GET /api/v1/hierarchy`,
/// `ResolvePetalScope` fallbacks, …) times out even though the DB thread
/// answered: the reply lands in `Messages<DbResult>` and nobody forwards it
/// to the oneshot. The relay registers this system; the GUI does not (its
/// dispatcher already delivers).
pub fn deliver_pending_api_results(
    mut results: bevy::prelude::MessageReader<DbResult>,
    mut pending: bevy::prelude::ResMut<PendingApiRequests>,
) {
    let _span = tracing::debug_span!("deliver_pending_api_results").entered();
    for result in results.read() {
        if let DbResult::HierarchyLoaded { verses } = result {
            // Full verse tree to pending hierarchy requests (the
            // `GET /api/v1/hierarchy` channel fallback) — mirrors the fe-ui
            // dispatcher's `HierarchyLoaded` arm.
            pending.deliver_hierarchy(verses.to_vec());
        }
        pending.try_deliver(result.clone());
    }
}

/// Bevy system: drain API commands and forward to the DB command channel.
pub fn drain_api_commands(
    api_rx: Option<Res<ApiCommandReceiver>>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingApiRequests>,
) {
    let _span = tracing::debug_span!("drain_api_commands").entered();
    let Some(api_rx) = api_rx else { return };
    let Ok(rx) = api_rx.0.lock() else { return };
    // Drain up to 64 commands per frame to avoid stalling the main loop.
    for _ in 0..64 {
        match rx.try_recv() {
            Ok(ApiCommand::DbRequest { cmd, reply_tx }) => {
                pending.enqueue_for(&cmd, reply_tx);
                if let Err(e) = db_tx.0.send(cmd) {
                    tracing::warn!("API DB request dropped — DB command channel closed: {e}");
                }
            }
            Ok(ApiCommand::GetHierarchy { reply_tx }) => {
                pending.enqueue_hierarchy(reply_tx);
                if let Err(e) = db_tx.0.send(DbCommand::LoadHierarchy) {
                    tracing::warn!(
                        "API hierarchy request dropped — DB command channel closed: {e}"
                    );
                }
            }
            Ok(ApiCommand::SyncForward { .. }) => {
                // Transform sync forwarding handled via broadcast channel
            }
            Ok(ApiCommand::TransformPersist {
                node_id,
                position,
                rotation,
                scale,
            }) => {
                // Fire-and-forget: send directly to DB without enqueuing a reply.
                let _ = db_tx.0.send(DbCommand::UpdateNodeTransform {
                    node_id,
                    position,
                    rotation,
                    scale,
                });
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_core_systems_inserts_resources() {
        let ch = crate::channels::ChannelHandles::new();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        setup_core_systems(
            &mut app,
            BevyHandles {
                net_cmd_tx: ch.net_cmd_tx,
                net_evt_rx: ch.net_evt_rx,
                db_cmd_tx: ch.db_cmd_tx,
                db_res_rx: ch.db_res_rx,
                blob_store: None,
                on_blob_miss: None,
                lifecycle_rx: None,
            },
        );
        app.update(); // should not panic (lifecycle pump is a no-op without rx)
        assert!(app.world().get_resource::<DbCommandSender>().is_some());
        assert!(app.world().get_resource::<NetworkCommandSender>().is_some());
        assert!(app.world().get_resource::<DbResultReceiver>().is_some());
        assert!(app.world().get_resource::<NetworkEventReceiver>().is_some());
        assert!(app
            .world()
            .get_resource::<LifecycleEventReceiver>()
            .is_none());
    }

    #[test]
    fn lifecycle_pump_forwards_events_into_bevy_messages() {
        let ch = crate::channels::ChannelHandles::new();
        let (lifecycle_tx, lifecycle_rx) = crossbeam::channel::bounded(8);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        setup_core_systems(
            &mut app,
            BevyHandles {
                net_cmd_tx: ch.net_cmd_tx,
                net_evt_rx: ch.net_evt_rx,
                db_cmd_tx: ch.db_cmd_tx,
                db_res_rx: ch.db_res_rx,
                blob_store: None,
                on_blob_miss: None,
                lifecycle_rx: Some(lifecycle_rx),
            },
        );
        lifecycle_tx
            .send(LifecycleEvent::PathReflow {
                path_id: "path-1".into(),
                deleted_index: Some(2),
            })
            .unwrap();
        app.update();
        let messages = app
            .world()
            .resource::<bevy::ecs::message::Messages<LifecycleEvent>>();
        assert!(
            !messages.is_empty(),
            "pump must forward lifecycle events into Bevy messages"
        );
    }

    // -- Reply correlation (F5) ---------------------------------------------

    use crate::messages::{CallerAuth, ReplicatedRowOutcome};

    fn node_created(id: &str) -> DbResult {
        DbResult::NodeCreated {
            id: id.to_string(),
            petal_id: "p1".to_string(),
            name: id.to_string(),
            has_asset: false,
            correlation_id: None,
            position: [0.0, 0.0, 0.0],
        }
    }

    fn create_node_cmd() -> DbCommand {
        DbCommand::CreateNode {
            petal_id: "p1".to_string(),
            name: "n".to_string(),
            position: [0.0, 0.0, 0.0],
            correlation_id: None,
        }
    }

    fn insert_iot_cmd(correlation_id: Option<&str>) -> DbCommand {
        DbCommand::InsertIotReadings {
            petal_id: "p".to_string(),
            verse_id: None,
            source_did: "did:key:z6MkTest".to_string(),
            readings: Vec::new(),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn iot_inserted(written: usize, correlation_id: &str) -> DbResult {
        DbResult::IotReadingsInserted {
            petal_id: "p".to_string(),
            written,
            correlation_id: Some(correlation_id.to_string()),
        }
    }

    fn create_asset_cmd(correlation_id: Option<&str>) -> DbCommand {
        DbCommand::CreateAsset {
            name: "a.glb".to_string(),
            content_type: "model/gltf-binary".to_string(),
            size_bytes: 12,
            content_hash: "00".repeat(32),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn asset_created(correlation_id: Option<&str>) -> DbResult {
        DbResult::AssetCreated {
            asset_id: "a".to_string(),
            content_hash: "00".repeat(32),
            size_bytes: 12,
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn place_asset_cmd(correlation_id: Option<&str>) -> DbCommand {
        DbCommand::CreateNodeWithAsset {
            petal_id: "p".to_string(),
            name: "n".to_string(),
            asset_id: "a".to_string(),
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn gltf_imported(correlation_id: Option<&str>) -> DbResult {
        DbResult::GltfImported {
            node_id: "n".to_string(),
            asset_id: "a".to_string(),
            petal_id: "p".to_string(),
            name: "n".to_string(),
            asset_path: "blob://x.glb".to_string(),
            position: [0.0; 3],
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    /// F13: a GUI-originated `GltfImported` (no correlation id) must never
    /// satisfy an API `place_asset` waiter, and a placement reply reaches only
    /// the waiter whose correlation id it echoes.
    #[test]
    fn placement_replies_route_only_by_correlation_id() {
        let mut pending = PendingApiRequests::default();
        let (tx_a, mut rx_a) = tokio::sync::oneshot::channel();
        let (tx_b, mut rx_b) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&place_asset_cmd(Some("a")), tx_a);
        pending.enqueue_for(&place_asset_cmd(Some("b")), tx_b);

        pending.try_deliver(gltf_imported(None));
        assert!(
            rx_a.try_recv().is_err(),
            "GUI import leaked to API waiter A"
        );
        assert!(
            rx_b.try_recv().is_err(),
            "GUI import leaked to API waiter B"
        );

        assert!(pending.try_deliver(gltf_imported(Some("b"))));
        assert!(matches!(
            rx_b.try_recv(),
            Ok(DbResult::GltfImported { correlation_id: Some(ref c), .. }) if c == "b"
        ));
        assert!(rx_a.try_recv().is_err(), "B's reply must not reach A");
    }

    /// F24 cross-delivery (DEC-C13): caller A times out (receiver dropped),
    /// B enqueues, THEN A's late reply lands. Skip-closed FIFO handed A's
    /// reply to B and B's to nobody; correlated routing lets A's own
    /// (closed) entry consume it, so B gets exactly its own reply and the
    /// queue stays aligned for C.
    #[test]
    fn late_correlated_reply_is_consumed_by_its_own_timed_out_entry() {
        let mut pending = PendingApiRequests::default();
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        let (c_tx, mut c_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&insert_iot_cmd(Some("a")), a_tx);
        drop(a_rx); // A's timeout
        pending.enqueue_for(&insert_iot_cmd(Some("b")), b_tx);

        assert!(
            !pending.try_deliver(iot_inserted(2, "a")),
            "A's late reply is delivered to nobody"
        );
        assert!(b_rx.try_recv().is_err(), "B must not receive A's reply");

        pending.enqueue_for(&insert_iot_cmd(Some("c")), c_tx);
        // Replies may even arrive out of enqueue order: each finds its own.
        assert!(pending.try_deliver(iot_inserted(3, "c")));
        assert!(pending.try_deliver(iot_inserted(1, "b")));
        assert!(matches!(
            b_rx.try_recv(),
            Ok(DbResult::IotReadingsInserted { written: 1, .. })
        ));
        assert!(matches!(
            c_rx.try_recv(),
            Ok(DbResult::IotReadingsInserted { written: 3, .. })
        ));
        assert!(
            !pending.try_deliver(iot_inserted(9, "b")),
            "a duplicate/unknown correlation id finds no waiter"
        );
    }

    fn raw_query_cmd(correlation_id: Option<&str>) -> DbCommand {
        DbCommand::RawQuery {
            sql: "SELECT * FROM node".to_string(),
            vars: std::collections::HashMap::new(),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn query_result(tag: &str, correlation_id: Option<&str>) -> DbResult {
        DbResult::QueryResult {
            data: vec![serde_json::json!({ "owner": tag })],
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    /// M4 fix B2: a GUI Query-tab result (uncorrelated) must never satisfy an
    /// API `RawQuery` waiter, and a timed-out API caller's late rows are
    /// consumed by its own entry instead of landing on the next caller.
    #[test]
    fn raw_query_replies_route_only_by_correlation_id() {
        let mut pending = PendingApiRequests::default();
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&raw_query_cmd(Some("a")), a_tx);
        drop(a_rx); // A timed out
        pending.enqueue_for(&raw_query_cmd(Some("b")), b_tx);

        assert!(
            !pending.try_deliver(query_result("gui", None)),
            "GUI rows have no API waiter"
        );
        assert!(
            !pending.try_deliver(query_result("a", Some("a"))),
            "A's late rows are consumed by A's own closed entry"
        );
        assert!(b_rx.try_recv().is_err(), "B received a foreign result");

        assert!(pending.try_deliver(query_result("b", Some("b"))));
        match b_rx.try_recv() {
            Ok(DbResult::QueryResult { data, .. }) => assert_eq!(data[0]["owner"], "b"),
            other => panic!("B expected its own rows, got {other:?}"),
        }

        let (c_tx, mut c_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&raw_query_cmd(Some("c")), c_tx);
        assert!(pending.try_deliver(DbResult::QueryFailed {
            error: "parse error".to_string(),
            correlation_id: "c".to_string(),
        }));
        assert!(matches!(c_rx.try_recv(), Ok(DbResult::QueryFailed { .. })));
    }

    /// The hazard this replaced FIFO pairing for: an unsolicited
    /// `HierarchyLoaded` (the relay's startup scan, a GUI reload) must never be
    /// handed to a racing `/ready` `Ping`.
    #[test]
    fn hierarchy_result_never_satisfies_a_ping_request() {
        let mut pending = PendingApiRequests::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&DbCommand::Ping, tx);

        assert!(
            !pending.try_deliver(DbResult::HierarchyLoaded { verses: Vec::new() }),
            "a hierarchy result must not be delivered to a Ping waiter"
        );

        assert!(
            pending.try_deliver(DbResult::Pong),
            "the Ping is still waiting for its own reply"
        );
        assert!(matches!(rx.try_recv(), Ok(DbResult::Pong)));
    }

    /// Same-kind requests keep FIFO order (the DB thread answers sequentially),
    /// so two racing requests of one kind still pair with their own replies.
    #[test]
    fn same_kind_requests_pair_in_enqueue_order() {
        let mut pending = PendingApiRequests::default();
        let (tx1, mut rx1) = tokio::sync::oneshot::channel();
        let (tx2, mut rx2) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&create_node_cmd(), tx1);
        pending.enqueue_for(&create_node_cmd(), tx2);

        assert!(pending.try_deliver(node_created("n1")));
        assert!(pending.try_deliver(node_created("n2")));

        match rx1.try_recv().expect("first request answered") {
            DbResult::NodeCreated { id, .. } => assert_eq!(id, "n1"),
            other => panic!("unexpected result: {other:?}"),
        }
        match rx2.try_recv().expect("second request answered") {
            DbResult::NodeCreated { id, .. } => assert_eq!(id, "n2"),
            other => panic!("unexpected result: {other:?}"),
        }
    }

    /// `Error` is the DB thread's universal reply and carries no family — it
    /// must reach the oldest waiter, whatever kind that waiter asked for.
    #[test]
    fn error_reply_reaches_the_oldest_pending_request() {
        let mut pending = PendingApiRequests::default();
        let (ping_tx, mut ping_rx) = tokio::sync::oneshot::channel();
        let (props_tx, mut props_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&DbCommand::Ping, ping_tx);
        pending.enqueue_for(
            &DbCommand::GetNodeProperties {
                node_id: "n1".to_string(),
            },
            props_tx,
        );

        assert!(pending.try_deliver(DbResult::Error("db exploded".to_string())));
        assert!(matches!(ping_rx.try_recv(), Ok(DbResult::Error(_))));
        assert!(
            props_rx.try_recv().is_err(),
            "only the oldest request receives the untyped error"
        );
    }

    /// A result with no waiter of its family is dropped for the API path (it
    /// still flows to the Bevy `Messages<DbResult>` stream for UI consumers)
    /// and never consumes an unrelated pending request.
    #[test]
    fn result_without_a_waiter_is_not_delivered_and_leaves_waiters_untouched() {
        let mut pending = PendingApiRequests::default();
        assert!(!pending.try_deliver(DbResult::Started));

        let (tx, mut rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&DbCommand::Ping, tx);
        assert!(!pending.try_deliver(DbResult::ScopeResolved { scope: None }));
        assert!(
            rx.try_recv().is_err(),
            "a resolved-scope reply must not satisfy a Ping"
        );
    }

    /// Every command the API awaits has a reply family, and the family of its
    /// reply matches — the guard against a silent one-sided mapping (which
    /// would turn an endpoint into a timeout).
    #[test]
    fn reply_kind_mapping_is_consistent_for_every_awaited_command() {
        let cases: Vec<(DbCommand, DbResult)> = vec![
            (DbCommand::Ping, DbResult::Pong),
            (
                DbCommand::CreateVerse {
                    name: "v".to_string(),
                },
                DbResult::VerseCreated {
                    id: "v".to_string(),
                    name: "v".to_string(),
                    namespace_id: None,
                },
            ),
            (
                DbCommand::CreateFractal {
                    verse_id: "v".to_string(),
                    name: "f".to_string(),
                },
                DbResult::FractalCreated {
                    id: "f".to_string(),
                    verse_id: "v".to_string(),
                    name: "f".to_string(),
                },
            ),
            (
                DbCommand::CreatePetal {
                    fractal_id: "f".to_string(),
                    name: "p".to_string(),
                },
                DbResult::PetalCreated {
                    id: "p".to_string(),
                    fractal_id: "f".to_string(),
                    name: "p".to_string(),
                },
            ),
            (create_node_cmd(), node_created("n1")),
            (
                DbCommand::LoadHierarchy,
                DbResult::HierarchyLoaded { verses: Vec::new() },
            ),
            (
                DbCommand::ResolvePetalScope {
                    petal_id: "p".to_string(),
                },
                DbResult::ScopeResolved { scope: None },
            ),
            (
                DbCommand::ResolveNodeScope {
                    node_id: "n".to_string(),
                },
                DbResult::ScopeResolved { scope: None },
            ),
            (
                DbCommand::LoadNodesByPetal {
                    petal_id: "p".to_string(),
                },
                DbResult::NodesLoaded {
                    petal_id: "p".to_string(),
                    nodes: Vec::new(),
                },
            ),
            (
                DbCommand::GetNodeTransform {
                    node_id: "n".to_string(),
                },
                DbResult::NodeTransformLoaded {
                    node_id: "n".to_string(),
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0],
                    scale: [1.0, 1.0, 1.0],
                },
            ),
            (
                DbCommand::GetNodeProperties {
                    node_id: "n".to_string(),
                },
                DbResult::NodePropertiesLoaded {
                    node_id: "n".to_string(),
                    properties: serde_json::Value::Null,
                },
            ),
            (
                DbCommand::SetNodeProperty {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                    value: serde_json::Value::Null,
                },
                DbResult::NodePropertySet {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                },
            ),
            (
                DbCommand::DeleteNodeProperty {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                },
                DbResult::NodePropertyDeleted {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                },
            ),
            (
                DbCommand::DeleteNode {
                    node_id: "n".to_string(),
                },
                DbResult::NodeDeleted {
                    node_id: "n".to_string(),
                    petal_id: "p".to_string(),
                },
            ),
            (
                DbCommand::TombstoneNode {
                    node_id: "n".to_string(),
                    auth: CallerAuth::Anonymous,
                },
                DbResult::NodeDeleted {
                    node_id: "n".to_string(),
                    petal_id: "p".to_string(),
                },
            ),
            (
                DbCommand::CascadeTombstoneNode {
                    node_id: "n".to_string(),
                    auth: CallerAuth::Anonymous,
                },
                DbResult::NodeDeleted {
                    node_id: "n".to_string(),
                    petal_id: "p".to_string(),
                },
            ),
            (
                DbCommand::PromoteInstance {
                    petal_id: "p".to_string(),
                    path_id: "path".to_string(),
                    instance_index: 0,
                    auth: CallerAuth::Anonymous,
                },
                DbResult::NodePromoted {
                    node_id: "n".to_string(),
                    petal_id: "p".to_string(),
                    path_id: "path".to_string(),
                    instance_index: 0,
                    newly_promoted: true,
                },
            ),
            (
                DbCommand::CreateFieldDef {
                    scope: "VERSE#v".to_string(),
                    entity_type: "node".to_string(),
                    key: "k".to_string(),
                    value_type: "string".to_string(),
                    default_val: None,
                },
                DbResult::FieldDefCreated {
                    field_def_id: "fd".to_string(),
                    scope: "VERSE#v".to_string(),
                    key: "k".to_string(),
                },
            ),
            (
                DbCommand::ListFieldDefs {
                    scope: "VERSE#v".to_string(),
                },
                DbResult::FieldDefsListed {
                    scope: "VERSE#v".to_string(),
                    field_defs: Vec::new(),
                },
            ),
            (
                DbCommand::UpdateFieldDef {
                    field_def_id: "fd".to_string(),
                    value_type: "string".to_string(),
                    default_val: None,
                },
                DbResult::FieldDefUpdated {
                    field_def_id: "fd".to_string(),
                },
            ),
            (
                DbCommand::DeleteFieldDef {
                    field_def_id: "fd".to_string(),
                },
                DbResult::FieldDefDeleted {
                    field_def_id: "fd".to_string(),
                },
            ),
            (
                DbCommand::RawQuery {
                    sql: "SELECT 1".to_string(),
                    vars: std::collections::HashMap::new(),
                    correlation_id: None,
                },
                DbResult::QueryResult {
                    data: Vec::new(),
                    correlation_id: None,
                },
            ),
            (
                insert_iot_cmd(None),
                DbResult::IotReadingsInserted {
                    petal_id: "p".to_string(),
                    written: 0,
                    correlation_id: None,
                },
            ),
            (
                insert_iot_cmd(None),
                DbResult::IotReadingsRejected {
                    petal_id: "p".to_string(),
                    reason: crate::messages::IotIngestRejection::EmptyMetric,
                    correlation_id: None,
                },
            ),
            (
                DbCommand::GetPetalTerrain {
                    petal_id: "p".to_string(),
                },
                DbResult::PetalTerrainLoaded {
                    petal_id: "p".to_string(),
                    terrain: None,
                },
            ),
            (
                DbCommand::SetVerseTimeseriesSettings {
                    verse_id: "v".to_string(),
                    mode: "balanced".to_string(),
                    replication_factor: 2,
                    bucket_width_ms: crate::timeseries::DEFAULT_BUCKET_WIDTH_MS,
                },
                DbResult::VerseTimeseriesSettingsSet {
                    verse_id: "v".to_string(),
                    mode: "balanced".to_string(),
                    replication_factor: 2,
                    bucket_width_ms: crate::timeseries::DEFAULT_BUCKET_WIDTH_MS,
                },
            ),
            (
                DbCommand::ResolveFractalScope {
                    fractal_id: "f".to_string(),
                },
                DbResult::ScopeResolved { scope: None },
            ),
            (
                DbCommand::ResolveVerseScope {
                    verse_id: "v".to_string(),
                },
                DbResult::ScopeResolved { scope: None },
            ),
            (create_asset_cmd(Some("c1")), asset_created(Some("c1"))),
            (place_asset_cmd(Some("c2")), gltf_imported(Some("c2"))),
        ];

        for (cmd, result) in cases {
            let cmd_kind = reply_kind_of_command(&cmd);
            assert!(cmd_kind.is_some(), "no reply family mapped for {cmd:?}");
            assert_eq!(
                cmd_kind,
                reply_kind_of_result(&result),
                "reply family mismatch for {cmd:?}"
            );
        }

        // Commands with no awaited reply on this channel, and results that are
        // never a correlated reply, must stay unmapped.
        assert_eq!(
            reply_kind_of_command(&DbCommand::UpdateNodeTransform {
                node_id: "n".to_string(),
                position: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            }),
            None
        );
        assert_eq!(
            reply_kind_of_result(&DbResult::Error("x".to_string())),
            None
        );
        assert_eq!(reply_kind_of_result(&DbResult::Started), None);
        assert_eq!(
            reply_kind_of_result(&gltf_imported(None)),
            None,
            "a GUI ImportGltf result must never satisfy an API placement waiter"
        );
        assert_eq!(
            reply_kind_of_result(&DbResult::ReplicatedRowApplied {
                verse_id: "v".to_string(),
                table: "iot_reading".to_string(),
                record_id: "r".to_string(),
                outcome: ReplicatedRowOutcome::Applied,
            }),
            None,
            "an unsolicited inbound-apply echo must never satisfy a pending request"
        );
    }
}
