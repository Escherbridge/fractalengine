use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::app::PluginGroupBuilder;
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

/// Local API address confirmed by the server's bound-listener notification.
#[derive(Resource, Default, Clone, Copy)]
pub struct RuntimeApiEndpoint(pub Option<std::net::SocketAddr>);

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
    // Periodic fire-and-forget asset GC (src/AGENTS.md §asset-gc); env read once here.
    app.insert_resource(crate::asset_gc::AssetGcSchedule::from_env());
    app.add_systems(
        Update,
        (
            drain_network_events,
            drain_db_results,
            drain_lifecycle_events,
            crate::asset_gc::schedule_asset_gc,
        ),
    );
}

/// The executable's folder when it holds a packaged build (an `assets/` dir
/// beside the binary), else `None` (see src/AGENTS.md §packaged-layout).
pub fn packaged_root() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    exe_dir.join("assets").is_dir().then_some(exe_dir)
}

/// Absolute root of Bevy's default file asset source: `<exe dir>/assets` for
/// a packaged build, `<cwd>/fractalengine/assets` for a workspace checkout.
pub fn file_asset_root() -> std::path::PathBuf {
    match packaged_root() {
        Some(root) => root.join("assets"),
        None => std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("fractalengine")
            .join("assets"),
    }
}

/// Process-wide tracing ownership; see AGENTS.md logging-ownership.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LoggingOwnership {
    #[default]
    Bevy,
    External,
}

impl LoggingOwnership {
    fn configure(self, plugins: PluginGroupBuilder) -> PluginGroupBuilder {
        match self {
            Self::Bevy => plugins,
            Self::External => plugins.disable::<bevy::log::LogPlugin>(),
        }
    }
}

/// Runtime GUI policies; existing constructors retain these defaults.
#[derive(Debug, Clone, Copy)]
pub struct AppOptions {
    pub close_when_requested: bool,
    pub logging: LoggingOwnership,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            close_when_requested: true,
            logging: LoggingOwnership::Bevy,
        }
    }
}

/// Build a full GUI application with `DefaultPlugins`, blob asset source, and
/// the core ECS resources. The caller is responsible for adding GUI-specific
/// plugins such as `EguiPlugin` and `FrameTimeDiagnosticsPlugin` after this
/// returns.
pub fn build_app(handles: BevyHandles) -> App {
    build_app_with_window(handles, Window::default())
}

/// [`build_app`] with an explicit primary-window description (the GUI binary's
/// frameless chrome — fractalengine/src/AGENTS.md §window-chrome).
pub fn build_app_with_window(handles: BevyHandles, primary_window: Window) -> App {
    build_app_with_options(handles, primary_window, AppOptions::default())
}

/// Build the editor GUI with close requests delegated to its confirmation UI.
pub fn build_app_with_confirmed_close(handles: BevyHandles, primary_window: Window) -> App {
    build_app_with_options(
        handles,
        primary_window,
        AppOptions {
            close_when_requested: false,
            ..Default::default()
        },
    )
}

/// Build a GUI app with explicit window-close and process-logging policies.
pub fn build_app_with_options(
    handles: BevyHandles,
    primary_window: Window,
    options: AppOptions,
) -> App {
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

    // Absolute file_path: bypasses Bevy's CARGO_MANIFEST_DIR / exe-dir base
    // (src/AGENTS.md §packaged-layout).
    let asset_root = file_asset_root();
    tracing::info!("Bevy AssetPlugin file_path = {}", asset_root.display());
    let plugins = DefaultPlugins
        .set(bevy::asset::AssetPlugin {
            file_path: asset_root.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .set(bevy::window::WindowPlugin {
            primary_window: Some(primary_window),
            close_when_requested: options.close_when_requested,
            ..Default::default()
        });
    app.add_plugins(options.logging.configure(plugins));

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
        DbCommand::ReadEditorBatch { correlation_id, .. }
        | DbCommand::CommitCheckedEditorBatch { correlation_id, .. }
        | DbCommand::ResolveCheckedEditorBatch { correlation_id, .. } => Some(correlation_id.as_str()),
        DbCommand::InsertIotReadings { correlation_id, .. }
        | DbCommand::MintApiToken { correlation_id, .. }
        | DbCommand::CreateNode { correlation_id, .. }
        | DbCommand::SetNodeProperty { correlation_id, .. }
        | DbCommand::GetNodeProperties { correlation_id, .. }
        | DbCommand::DeleteNodeProperty { correlation_id, .. }
        | DbCommand::CreateAsset { correlation_id, .. }
        | DbCommand::CreateNodeWithAsset { correlation_id, .. }
        | DbCommand::SetPetalTerrain { correlation_id, .. }
        | DbCommand::GetPetalTerrain { correlation_id, .. }
        | DbCommand::SetNodeView { correlation_id, .. }
        | DbCommand::SetPetalVisibilityGroups { correlation_id, .. }
        | DbCommand::ComparePetalVisibilityGroups { correlation_id, .. }
        | DbCommand::GetPetalVisibilityGroups { correlation_id, .. }
        | DbCommand::SetScopeHidden { correlation_id, .. }
        | DbCommand::SetPetalRenderDistance { correlation_id, .. }
        | DbCommand::GetPetalRenderDistance { correlation_id, .. }
        | DbCommand::EnrolLocalDevice { correlation_id, .. }
        | DbCommand::RevokeLocalDevice { correlation_id, .. }
        | DbCommand::RegisterDeviceFormat { correlation_id, .. }
        | DbCommand::ListDeviceFormats { correlation_id, .. }
        | DbCommand::InsertDevicePayloads { correlation_id, .. }
        | DbCommand::ListDevicePayloads { correlation_id, .. }
        | DbCommand::RawQuery { correlation_id, .. }
        | DbCommand::SetFieldSchema { correlation_id, .. }
        | DbCommand::GetFieldSchema { correlation_id, .. }
        | DbCommand::CommitNodeMutation { correlation_id, .. }
        | DbCommand::CreateObject { correlation_id, .. }
        | DbCommand::ReadNodeHistory { correlation_id, .. } => correlation_id.as_deref(),
        DbCommand::ElevatedQuery { correlation_id, .. }
        | DbCommand::ScopedQuery { correlation_id, .. } => Some(correlation_id.as_str()),
        _ => None,
    }
}

/// The correlation id a reply echoes (mirror of [`correlation_of_command`]).
fn correlation_of_result(result: &DbResult) -> Option<&str> {
    match result {
        DbResult::EditorBatchLoaded { correlation_id, .. }
        | DbResult::EditorBatchCommitted { correlation_id, .. }
        | DbResult::EditorBatchResolved { correlation_id, .. } => Some(correlation_id.as_str()),
        DbResult::NodeMaterialized { correlation_id, .. } => Some(correlation_id.as_str()),
        DbResult::IotReadingsInserted { correlation_id, .. }
        | DbResult::ApiTokenMinted { correlation_id, .. }
        | DbResult::NodeCreated { correlation_id, .. }
        | DbResult::NodePropertySet { correlation_id, .. }
        | DbResult::NodeMutationCommitted { correlation_id, .. }
        | DbResult::NodeHistoryLoaded { correlation_id, .. }
        | DbResult::NodeStateAtVersionLoaded { correlation_id, .. }
        | DbResult::NodePropertiesLoaded { correlation_id, .. }
        | DbResult::NodePropertyDeleted { correlation_id, .. }
        | DbResult::IotReadingsRejected { correlation_id, .. }
        | DbResult::AssetCreated { correlation_id, .. }
        | DbResult::GltfImported { correlation_id, .. }
        | DbResult::PetalTerrainLoaded { correlation_id, .. }
        | DbResult::NodeViewSet { correlation_id, .. }
        | DbResult::PetalVisibilityGroupsLoaded { correlation_id, .. }
        | DbResult::ScopeHiddenSet { correlation_id, .. }
        | DbResult::PetalRenderDistanceLoaded { correlation_id, .. }
        | DbResult::LocalDeviceEnrolled { correlation_id, .. }
        | DbResult::LocalDeviceRevoked { correlation_id, .. }
        | DbResult::DeviceFormatRegistered { correlation_id, .. }
        | DbResult::DeviceFormatRejected { correlation_id, .. }
        | DbResult::DeviceFormatsListed { correlation_id, .. }
        | DbResult::DevicePayloadsInserted { correlation_id, .. }
        | DbResult::DevicePayloadsRejected { correlation_id, .. }
        | DbResult::DevicePayloadsListed { correlation_id, .. }
        | DbResult::QueryResult { correlation_id, .. } => correlation_id.as_deref(),
        DbResult::QueryFailed { correlation_id, .. }
        | DbResult::PetalTerrainSetFailed { correlation_id, .. }
        | DbResult::PetalTerrainFetched { correlation_id, .. }
        | DbResult::ElevatedQueryResult { correlation_id, .. }
        | DbResult::RequestFailed { correlation_id, .. } => Some(correlation_id.as_str()),
        _ => None,
    }
}

/// The reply family a command's result belongs to — the correlation key that
/// replaced FIFO pairing. Kept private: callers enqueue with the command and
/// never name a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReplyKind {
    EditorBatchLoaded,
    EditorBatchCommitted,
    EditorBatchResolved,
    /// GUI-only materialization events have no command/pending queue.
    NodeMaterialized,
    NodeMutationCommitted,
    NodeHistoryLoaded,
    NodeStateAtVersionLoaded,
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
    ApiTokenMinted,
    QueryResult,
    IotReadingsInserted,
    PetalTerrain,
    VerseTimeseriesSettings,
    AssetCreated,
    PlacedAsset,
    /// API `SetPetalTerrain` replies (correlated only — see `reply_kind_of_result`).
    PetalTerrainSet,
    /// API `GetPetalTerrain` replies (`PetalTerrainFetched`, correlated only).
    PetalTerrainFetch,
    /// `/query/elevated` replies (`ElevatedQueryResult`, correlated only).
    ElevatedQuery,
    /// `SetNodeView` replies (`NodeViewSet`).
    NodeViewSet,
    /// `LoadPetalViewState` replies (`PetalViewStateLoaded`).
    PetalViewState,
    /// `Set`/`GetPetalVisibilityGroups` replies (`PetalVisibilityGroupsLoaded`).
    VisibilityGroups,
    /// `SetScopeHidden` replies (`ScopeHiddenSet`).
    ScopeHidden,
    /// `Set`/`GetPetalRenderDistance` replies (`PetalRenderDistanceLoaded`).
    RenderDistance,
    /// `EnrolLocalDevice`/`RevokeLocalDevice` replies (`LocalDeviceEnrolled`/`LocalDeviceRevoked`).
    DeviceEnrolment,
    /// `RegisterDeviceFormat`/`ListDeviceFormats` replies (CDL-SPEC10 A3).
    DeviceFormat,
    /// `InsertDevicePayloads`/`ListDevicePayloads` replies (CDL-SPEC10 A3).
    DevicePayload,
}

/// The reply family `cmd` produces, or `None` when the command has no reply on
/// this channel (fire-and-forget writes) — see [`PendingApiRequests`].
fn reply_kind_of_command(cmd: &DbCommand) -> Option<ReplyKind> {
    use DbCommand::*;
    Some(match cmd {
        ReadEditorBatch { .. } => ReplyKind::EditorBatchLoaded,
        CommitCheckedEditorBatch { .. } => ReplyKind::EditorBatchCommitted,
        ResolveCheckedEditorBatch { .. } => ReplyKind::EditorBatchResolved,
        MintApiToken { .. } => ReplyKind::ApiTokenMinted,
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
        RawQuery { .. } | ScopedQuery { .. } | SetFieldSchema { .. } | GetFieldSchema { .. } => {
            ReplyKind::QueryResult
        }
        CommitNodeMutation { .. } | CreateObject { .. } => ReplyKind::NodeMutationCommitted,
        ReadNodeHistory {
            at_version: Some(_),
            ..
        } => ReplyKind::NodeStateAtVersionLoaded,
        ReadNodeHistory { .. } => ReplyKind::NodeHistoryLoaded,
        ElevatedQuery { .. } => ReplyKind::ElevatedQuery,
        InsertIotReadings { .. } => ReplyKind::IotReadingsInserted,
        GetPetalTerrain {
            correlation_id: Some(_),
            ..
        } => ReplyKind::PetalTerrainFetch,
        GetPetalTerrain { .. } => ReplyKind::PetalTerrain,
        SetVerseTimeseriesSettings { .. } => ReplyKind::VerseTimeseriesSettings,
        CreateAsset { .. } => ReplyKind::AssetCreated,
        CreateNodeWithAsset { .. } => ReplyKind::PlacedAsset,
        SetPetalTerrain { .. } => ReplyKind::PetalTerrainSet,
        SetNodeView { .. } => ReplyKind::NodeViewSet,
        LoadPetalViewState { .. } => ReplyKind::PetalViewState,
        SetPetalVisibilityGroups { .. } | ComparePetalVisibilityGroups { .. } | GetPetalVisibilityGroups { .. } => {
            ReplyKind::VisibilityGroups
        }
        SetScopeHidden { .. } => ReplyKind::ScopeHidden,
        SetPetalRenderDistance { .. } | GetPetalRenderDistance { .. } => ReplyKind::RenderDistance,
        EnrolLocalDevice { .. } | RevokeLocalDevice { .. } => ReplyKind::DeviceEnrolment,
        RegisterDeviceFormat { .. } | ListDeviceFormats { .. } => ReplyKind::DeviceFormat,
        InsertDevicePayloads { .. } | ListDevicePayloads { .. } => ReplyKind::DevicePayload,
        _ => return None,
    })
}

/// The reply family a result belongs to, or `None` when it is not a correlated
/// reply (`Error` is handled separately as the universal reply — see
/// [`PendingApiRequests`]).
fn reply_kind_of_result(result: &DbResult) -> Option<ReplyKind> {
    use DbResult::*;
    Some(match result {
        EditorBatchLoaded { .. } => ReplyKind::EditorBatchLoaded,
        EditorBatchCommitted { .. } => ReplyKind::EditorBatchCommitted,
        EditorBatchResolved { .. } => ReplyKind::EditorBatchResolved,
        ApiTokenMinted { .. } => ReplyKind::ApiTokenMinted,
        NodeMaterialized { .. } => ReplyKind::NodeMaterialized,
        NodeMutationCommitted { .. } => ReplyKind::NodeMutationCommitted,
        NodeHistoryLoaded { .. } => ReplyKind::NodeHistoryLoaded,
        NodeStateAtVersionLoaded { .. } => ReplyKind::NodeStateAtVersionLoaded,
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
        ElevatedQueryResult { .. } => ReplyKind::ElevatedQuery,
        IotReadingsInserted { .. } => ReplyKind::IotReadingsInserted,
        IotReadingsRejected { .. } => ReplyKind::IotReadingsInserted,
        // Only an API set (correlated) is a `PetalTerrainSet` reply; a GUI set
        // echo (`None`) stays in the Get family (§api-reply-correlation).
        PetalTerrainLoaded {
            correlation_id: Some(_),
            ..
        }
        | PetalTerrainSetFailed { .. } => ReplyKind::PetalTerrainSet,
        PetalTerrainLoaded { .. } => ReplyKind::PetalTerrain,
        PetalTerrainFetched { .. } => ReplyKind::PetalTerrainFetch,
        VerseTimeseriesSettingsSet { .. } => ReplyKind::VerseTimeseriesSettings,
        AssetCreated { .. } => ReplyKind::AssetCreated,
        // Only an API placement (correlated) is a reply; a GUI `ImportGltf`
        // result (`None`) keeps its legacy unmapped routing (§api-reply-correlation).
        GltfImported {
            correlation_id: Some(_),
            ..
        } => ReplyKind::PlacedAsset,
        NodeViewSet { .. } => ReplyKind::NodeViewSet,
        PetalViewStateLoaded { .. } => ReplyKind::PetalViewState,
        PetalVisibilityGroupsLoaded { .. } => ReplyKind::VisibilityGroups,
        ScopeHiddenSet { .. } => ReplyKind::ScopeHidden,
        PetalRenderDistanceLoaded { .. } => ReplyKind::RenderDistance,
        LocalDeviceEnrolled { .. } | LocalDeviceRevoked { .. } => ReplyKind::DeviceEnrolment,
        DeviceFormatRegistered { .. }
        | DeviceFormatRejected { .. }
        | DeviceFormatsListed { .. } => ReplyKind::DeviceFormat,
        DevicePayloadsInserted { .. }
        | DevicePayloadsRejected { .. }
        | DevicePayloadsListed { .. } => ReplyKind::DevicePayload,
        _ => return None,
    })
}

impl PendingApiRequests {
    /// Enqueue a DB request under the reply family `cmd` produces and return
    /// its entry id (insertion order: routes untyped `Error` replies to the
    /// oldest waiter, and names the entry for [`Self::cancel`]).
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

    /// Pop-and-drop FIFO restricted to entries WITHOUT a correlation id
    /// (correlated entries wait for their own echoed reply): the oldest such
    /// entry consumes `result` — a closed one (timed-out caller) drops it, so
    /// a late reply never shifts onto the next waiter (src/AGENTS.md
    /// §api-reply-correlation).
    fn deliver_uncorrelated(
        queue: &mut std::collections::VecDeque<PendingEntry>,
        result: DbResult,
    ) -> bool {
        let Some(position) = queue.iter().position(|e| e.correlation_id.is_none()) else {
            return false;
        };
        let entry = queue
            .remove(position)
            .expect("position is within the queue");
        Self::send_unless_closed(entry, result)
    }

    /// Pop-and-drop FIFO: the head entry consumes `result`, a closed head
    /// dropping it (see [`Self::deliver_uncorrelated`]).
    fn deliver_to(queue: &mut std::collections::VecDeque<PendingEntry>, result: DbResult) -> bool {
        match queue.pop_front() {
            Some(entry) => Self::send_unless_closed(entry, result),
            None => false,
        }
    }

    /// Send to a removed entry; a closed one (its caller timed out) consumes
    /// and drops the reply.
    fn send_unless_closed(entry: PendingEntry, result: DbResult) -> bool {
        if entry.reply_tx.is_closed() {
            tracing::debug!("late DB reply for a timed-out API request — consumed by its entry");
            return false;
        }
        entry.reply_tx.send(result).is_ok()
    }

    /// Hand a `RequestFailed` to the one entry (any family) filed under its
    /// correlation id; a closed entry consumes it, an unknown id is dropped.
    fn deliver_by_correlation_id(&mut self, correlation_id: &str, result: DbResult) -> bool {
        let matches = |e: &PendingEntry| e.correlation_id.as_deref() == Some(correlation_id);
        let queue = self
            .by_kind
            .values_mut()
            .chain(std::iter::once(&mut self.uncorrelated))
            .find(|queue| queue.iter().any(matches));
        let Some(queue) = queue else {
            tracing::debug!(
                correlation_id,
                "correlated failure has no pending waiter — dropped"
            );
            return false;
        };
        let position = queue
            .iter()
            .position(matches)
            .expect("queue was selected for containing the id");
        let entry = queue
            .remove(position)
            .expect("position is within the queue");
        Self::send_unless_closed(entry, result)
    }

    /// Remove the entry `enqueue_for` returned `id` for (its command never
    /// reached the DB thread), so it cannot consume a later reply.
    pub fn cancel(&mut self, id: u64) {
        for queue in self
            .by_kind
            .values_mut()
            .chain(std::iter::once(&mut self.uncorrelated))
        {
            if let Some(position) = queue.iter().position(|e| e.id == id) {
                queue.remove(position);
                return;
            }
        }
    }

    /// The id of the oldest entry in `queue` that has no correlation id, if
    /// any — entries are enqueued in increasing-id order, so this is the
    /// first `correlation_id: None` entry, not necessarily the front.
    fn oldest_uncorrelated_id(queue: &std::collections::VecDeque<PendingEntry>) -> Option<u64> {
        queue
            .iter()
            .find(|e| e.correlation_id.is_none())
            .map(|e| e.id)
    }

    /// Route an untyped reply (the DB thread's universal `Error`) to the
    /// oldest pending request of any family that has NO correlation id
    /// (DEC-C19 N2). A correlated entry (RawQuery/InsertIotReadings/
    /// CreateAsset/CreateNodeWithAsset/SetPetalTerrain) always gets its own
    /// typed `QueryFailed`/rejection reply from the DB thread, so the untyped
    /// wildcard must skip past it rather than consume it — a correlated
    /// waiter stealing the wildcard would leave its own typed reply (which
    /// still arrives later) with no entry to land on, AND could leak an
    /// unrelated caller's error text (e.g. a raw SQL string) to whichever
    /// request happened to be oldest. If every pending request is
    /// correlated, the error is dropped, never delivered to a correlated
    /// waiter.
    fn deliver_to_oldest(&mut self, result: DbResult) -> bool {
        let mut oldest: Option<ReplyKind> = None;
        let mut oldest_id = u64::MAX;
        for (kind, queue) in &self.by_kind {
            if let Some(id) = Self::oldest_uncorrelated_id(queue) {
                if id < oldest_id {
                    oldest_id = id;
                    oldest = Some(*kind);
                }
            }
        }
        let oldest_uncorrelated = self.uncorrelated.front().map(|e| e.id).unwrap_or(u64::MAX);
        if oldest_uncorrelated < oldest_id {
            return Self::deliver_uncorrelated(&mut self.uncorrelated, result);
        }
        match oldest {
            Some(kind) => {
                let queue = self.by_kind.get_mut(&kind).expect("kind came from by_kind");
                Self::deliver_uncorrelated(queue, result)
            }
            None => Self::deliver_uncorrelated(&mut self.uncorrelated, result),
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
        if let DbResult::RequestFailed { correlation_id, .. } = &result {
            let correlation_id = correlation_id.clone();
            return self.deliver_by_correlation_id(&correlation_id, result);
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
                let entry_id = pending.enqueue_for(&cmd, reply_tx);
                if let Err(e) = db_tx.0.send(cmd) {
                    // Never answered: under pop-and-drop it would swallow the
                    // family's next reply.
                    pending.cancel(entry_id);
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
        assert_eq!(AppOptions::default().logging, LoggingOwnership::Bevy);
        assert!(AppOptions::default().close_when_requested);
        for (ownership, expects_log_plugin) in [
            (LoggingOwnership::Bevy, true),
            (LoggingOwnership::External, false),
        ] {
            let plugins = ownership.configure(DefaultPlugins.build());
            assert_eq!(plugins.enabled::<bevy::log::LogPlugin>(), expects_log_plugin);
            assert!(plugins.enabled::<bevy::asset::AssetPlugin>());
            assert!(plugins.enabled::<bevy::window::WindowPlugin>());
            assert!(plugins.enabled::<bevy::winit::WinitPlugin>());
            assert!(plugins.enabled::<bevy::render::RenderPlugin>());
        }
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
            source_class: fe_sdk::source_class::SourceClass::Device,
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
            owner_did: None,
            petal_id: None,
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
            request: None,
            node_id: "n".to_string(),
            asset_id: "a".to_string(),
            petal_id: "p".to_string(),
            name: "n".to_string(),
            asset_path: "blob://x.glb".to_string(),
            position: [0.0; 3],
            materialization: None,
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

    /// VIS-PH3: a correlated `SetNodeView` is filed under its id, so its own
    /// `NodeViewSet` echo reaches it and the desktop's `fe-ui:view:` echo does not.
    #[test]
    fn a_correlated_set_node_view_receives_its_own_echo() {
        let echo = |correlation_id: &str| DbResult::NodeViewSet {
            node_id: "n1".to_string(),
            petal_id: "p".to_string(),
            state: Some("hide".to_string()),
            groups: None,
            correlation_id: Some(correlation_id.to_string()),
        };
        let mut pending = PendingApiRequests::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(
            &DbCommand::SetNodeView {
                node_id: "n1".to_string(),
                state: Some("hide".to_string()),
                groups: None,
                auth: crate::messages::CallerAuth::Local,
                correlation_id: Some("api-view-1".to_string()),
            },
            tx,
        );
        assert!(!pending.try_deliver(echo("fe-ui:view:n1")));
        assert!(pending.try_deliver(echo("api-view-1")));
        assert!(matches!(
            rx.try_recv(),
            Ok(DbResult::NodeViewSet { correlation_id: Some(ref c), .. }) if c == "api-view-1"
        ));
    }

    /// W4-8: an enrol waiter receives only its own echo, a revoke waiter its own
    /// tombstone reply, and a correlated failure reaches the one entry it names.
    #[test]
    fn device_enrolment_replies_reach_only_their_own_waiter() {
        let enrolled = |correlation_id: Option<&str>| DbResult::LocalDeviceEnrolled {
            verse_id: "v".to_string(),
            record_id_hex: "r".to_string(),
            principal_did: "did:key:z".to_string(),
            device_key_hex: "d".to_string(),
            correlation_id: correlation_id.map(str::to_string),
        };
        let mut pending = PendingApiRequests::default();
        let (enrol_tx, mut enrol_rx) = tokio::sync::oneshot::channel();
        let (revoke_tx, mut revoke_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(
            &DbCommand::EnrolLocalDevice {
                verse_id: "v".to_string(),
                correlation_id: Some("api-enrol".to_string()),
            },
            enrol_tx,
        );
        pending.enqueue_for(
            &DbCommand::RevokeLocalDevice {
                verse_id: "v".to_string(),
                device_key_hex: "d".to_string(),
                correlation_id: Some("api-revoke".to_string()),
            },
            revoke_tx,
        );
        assert!(!pending.try_deliver(enrolled(None)));
        assert!(enrol_rx.try_recv().is_err(), "an uncorrelated echo leaked");
        assert!(pending.try_deliver(enrolled(Some("api-enrol"))));
        assert!(matches!(
            enrol_rx.try_recv(),
            Ok(DbResult::LocalDeviceEnrolled { correlation_id: Some(ref c), .. }) if c == "api-enrol"
        ));
        assert!(pending.try_deliver(DbResult::RequestFailed {
            error: "no node keypair".to_string(),
            correlation_id: "api-revoke".to_string(),
        }));
        assert!(matches!(
            revoke_rx.try_recv(),
            Ok(DbResult::RequestFailed { ref correlation_id, .. }) if correlation_id == "api-revoke"
        ));
    }

    /// CDL-SPEC10 A3 (critic C2/AC-35f): a `device_format` waiter never
    /// receives a `device_payload` reply (and vice versa) even though both
    /// families reached `app.rs` in the same change — distinct `ReplyKind`s
    /// keep the two command surfaces from cross-delivering.
    #[test]
    fn device_format_and_device_payload_replies_reach_only_their_own_waiter() {
        let mut pending = PendingApiRequests::default();
        let (format_tx, mut format_rx) = tokio::sync::oneshot::channel();
        let (payload_tx, mut payload_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(
            &DbCommand::RegisterDeviceFormat {
                verse_id: "v".to_string(),
                descriptor: fe_sdk::device_format::DeviceFormatDescriptor {
                    format_id: "acme.thermo/frame@1".to_string(),
                    mime: "application/octet-stream".to_string(),
                    schema: None,
                    description: String::new(),
                },
                declared_by: "did:key:z6MkManager".to_string(),
                correlation_id: Some("api-format".to_string()),
            },
            format_tx,
        );
        pending.enqueue_for(
            &DbCommand::InsertDevicePayloads {
                petal_id: "p".to_string(),
                verse_id: None,
                source_did: "did:key:z6MkEditor".to_string(),
                source_class: fe_sdk::source_class::SourceClass::Device,
                payload: crate::messages::DevicePayloadInput {
                    node_id: "n1".to_string(),
                    format_id: "acme.thermo/frame@1".to_string(),
                    asset_ids: vec!["a1".to_string()],
                    recorded_at: None,
                    payload_id: None,
                },
                correlation_id: Some("api-payload".to_string()),
            },
            payload_tx,
        );
        // A device_payload reply never satisfies the device_format waiter.
        assert!(!pending.try_deliver(DbResult::DevicePayloadsInserted {
            petal_id: "p".to_string(),
            payload_id: "pay-1".to_string(),
            asset_ids: Vec::new(),
            correlation_id: Some("api-format".to_string()),
        }));
        assert!(format_rx.try_recv().is_err(), "a cross-family reply leaked");
        assert!(pending.try_deliver(DbResult::DeviceFormatRegistered {
            verse_id: "v".to_string(),
            format_id: "acme.thermo/frame@1".to_string(),
            correlation_id: Some("api-format".to_string()),
        }));
        assert!(matches!(
            format_rx.try_recv(),
            Ok(DbResult::DeviceFormatRegistered { correlation_id: Some(ref c), .. }) if c == "api-format"
        ));
        assert!(pending.try_deliver(DbResult::DevicePayloadsInserted {
            petal_id: "p".to_string(),
            payload_id: "pay-1".to_string(),
            asset_ids: vec!["a1".to_string()],
            correlation_id: Some("api-payload".to_string()),
        }));
        assert!(matches!(
            payload_rx.try_recv(),
            Ok(DbResult::DevicePayloadsInserted { correlation_id: Some(ref c), .. }) if c == "api-payload"
        ));
    }

    /// VIS-PH6: the desktop's own registry echo (its `fe-ui:` correlation id, or
    /// none) never satisfies an API visibility-groups waiter; the API's echo does.
    #[test]
    fn visibility_group_replies_reach_only_their_own_api_waiter() {
        let loaded = |correlation_id: Option<&str>| DbResult::PetalVisibilityGroupsLoaded {
            petal_id: "p".to_string(),
            groups: Vec::new(),
            correlation_id: correlation_id.map(str::to_string),
        };
        let mut pending = PendingApiRequests::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(
            &DbCommand::GetPetalVisibilityGroups {
                petal_id: "p".to_string(),
                correlation_id: Some("api-1".to_string()),
            },
            tx,
        );
        assert!(!pending.try_deliver(loaded(None)));
        assert!(!pending.try_deliver(loaded(Some("fe-ui:groups:p"))));
        assert!(
            rx.try_recv().is_err(),
            "a GUI echo leaked to the API waiter"
        );
        assert!(pending.try_deliver(loaded(Some("api-1"))));
        assert!(matches!(
            rx.try_recv(),
            Ok(DbResult::PetalVisibilityGroupsLoaded { correlation_id: Some(ref c), .. }) if c == "api-1"
        ));
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
        let mut scoped_pending = PendingApiRequests::default();
        let (scoped_tx, mut scoped_rx) = tokio::sync::oneshot::channel();
        scoped_pending.enqueue_for(
            &DbCommand::ScopedQuery {
                sql: "SELECT * FROM node".into(),
                vars: Default::default(),
                petal_scope: vec!["p".into()],
                correlation_id: "scoped".into(),
            },
            scoped_tx,
        );
        assert!(scoped_pending.try_deliver(DbResult::QueryFailed {
            error: "not found or not permitted".into(),
            correlation_id: "scoped".into()
        }));
        assert!(
            matches!(scoped_rx.try_recv(), Ok(DbResult::QueryFailed { correlation_id, .. }) if correlation_id == "scoped")
        );

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

    fn set_terrain_cmd(correlation_id: Option<&str>) -> DbCommand {
        DbCommand::SetPetalTerrain {
            petal_id: "p".to_string(),
            terrain: Some(serde_json::json!({ "enabled": true })),
            auth: crate::messages::CallerAuth::Local,
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn terrain_set(correlation_id: Option<&str>, petal_id: &str) -> DbResult {
        DbResult::PetalTerrainLoaded {
            petal_id: petal_id.to_string(),
            terrain: Some(serde_json::json!({ "owner": correlation_id })),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    /// The terrain echo `rx` received, by its `owner` tag (None = nothing).
    fn received_owner(
        rx: &mut tokio::sync::oneshot::Receiver<DbResult>,
    ) -> Option<serde_json::Value> {
        match rx.try_recv().ok()? {
            DbResult::PetalTerrainLoaded { terrain, .. } => Some(terrain.unwrap()["owner"].clone()),
            DbResult::PetalTerrainSetFailed { correlation_id, .. } => {
                Some(serde_json::json!(format!("failed:{correlation_id}")))
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    /// API terrain sets (SetPetalTerrain correlation): a GUI set echo
    /// (uncorrelated) never satisfies a pending API set; each correlated
    /// reply — success or typed failure — reaches only its own waiter; a
    /// timed-out caller's late reply is consumed by its own closed entry so
    /// the family never shifts onto the next caller.
    #[test]
    fn terrain_set_replies_route_only_by_correlation_id() {
        // (step, reply, delivered?, owner tag seen by [B, C]); A is closed.
        type Step = (&'static str, DbResult, bool, [Option<serde_json::Value>; 2]);
        let mut pending = PendingApiRequests::default();
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        let (c_tx, mut c_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&set_terrain_cmd(Some("a")), a_tx);
        drop(a_rx); // A timed out
        pending.enqueue_for(&set_terrain_cmd(Some("b")), b_tx);
        pending.enqueue_for(&set_terrain_cmd(Some("c")), c_tx);

        let steps: Vec<Step> = vec![
            ("GUI set echo", terrain_set(None, "p"), false, [None, None]),
            (
                "A's late reply",
                terrain_set(Some("a"), "p"),
                false,
                [None, None],
            ),
            (
                "C's typed failure",
                DbResult::PetalTerrainSetFailed {
                    petal_id: "p".to_string(),
                    error: "write failed".to_string(),
                    correlation_id: "c".to_string(),
                },
                true,
                [None, Some(serde_json::json!("failed:c"))],
            ),
            (
                "B's reply",
                terrain_set(Some("b"), "p"),
                true,
                [Some(serde_json::json!("b")), None],
            ),
            (
                "B duplicate",
                terrain_set(Some("b"), "p"),
                false,
                [None, None],
            ),
        ];
        for (step, reply, delivered, [b_expect, c_expect]) in steps {
            assert_eq!(pending.try_deliver(reply), delivered, "{step}: delivered");
            assert_eq!(received_owner(&mut b_rx), b_expect, "{step}: B");
            assert_eq!(received_owner(&mut c_rx), c_expect, "{step}: C");
        }

        // A GET waiter (uncorrelated family) still receives a GUI set echo —
        // unchanged legacy routing — and never a correlated API set reply.
        let (get_tx, mut get_rx) = tokio::sync::oneshot::channel();
        let (set_tx, mut set_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&get_terrain_cmd("p", None), get_tx);
        pending.enqueue_for(&set_terrain_cmd(Some("d")), set_tx);
        assert!(pending.try_deliver(terrain_set(Some("d"), "p")));
        assert!(
            get_rx.try_recv().is_err(),
            "API set reply leaked to a GET waiter"
        );
        assert_eq!(received_owner(&mut set_rx), Some(serde_json::json!("d")));

        for family in 0..3 {
            let command = |id: &str| match family {
                0 => DbCommand::SetNodeProperty {
                    node_id: "n".into(),
                    key: "k".into(),
                    value: serde_json::json!(10),
                    correlation_id: Some(id.into()),
                },
                1 => DbCommand::GetNodeProperties {
                    node_id: "n".into(),
                    correlation_id: Some(id.into()),
                },
                _ => DbCommand::DeleteNodeProperty {
                    node_id: "n".into(),
                    key: "k".into(),
                    correlation_id: Some(id.into()),
                },
            };
            let reply = |id: Option<&str>| {
                let correlation_id = id.map(str::to_string);
                match family {
                    0 => DbResult::NodePropertySet {
                        node_id: "n".into(),
                        key: "k".into(),
                        correlation_id,
                    },
                    1 => DbResult::NodePropertiesLoaded {
                        node_id: "n".into(),
                        properties: serde_json::json!({"k":10}),
                        correlation_id,
                    },
                    _ => DbResult::NodePropertyDeleted {
                        node_id: "n".into(),
                        key: "k".into(),
                        correlation_id,
                    },
                }
            };
            let mut properties = PendingApiRequests::default();
            let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
            let (live_tx, mut live_rx) = tokio::sync::oneshot::channel();
            properties.enqueue_for(&command("closed"), closed_tx);
            drop(closed_rx);
            properties.enqueue_for(&command("live"), live_tx);
            for other in [
                reply(None),
                reply(Some("unknown")),
                reply(Some("closed")),
                DbResult::Error("unrelated GUI error".into()),
            ] {
                assert!(!properties.try_deliver(other));
                assert!(
                    live_rx.try_recv().is_err(),
                    "property family {family} consumed another reply"
                );
            }
            assert!(properties.try_deliver(reply(Some("live"))));
            assert_eq!(
                correlation_of_result(&live_rx.try_recv().unwrap()),
                Some("live")
            );
            assert!(!properties.try_deliver(reply(Some("live"))));
            let (failed_tx, mut failed_rx) = tokio::sync::oneshot::channel();
            properties.enqueue_for(&command("failed"), failed_tx);
            assert!(properties.try_deliver(DbResult::RequestFailed {
                correlation_id: "failed".into(),
                error: "property failed".into()
            }));
            assert!(
                matches!(failed_rx.try_recv(), Ok(DbResult::RequestFailed { correlation_id, .. }) if correlation_id == "failed")
            );
        }
    }

    fn get_terrain_cmd(petal_id: &str, correlation_id: Option<&str>) -> DbCommand {
        DbCommand::GetPetalTerrain {
            petal_id: petal_id.to_string(),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    fn terrain_fetched(petal_id: &str, correlation_id: &str) -> DbResult {
        DbResult::PetalTerrainFetched {
            petal_id: petal_id.to_string(),
            terrain: Some(serde_json::json!({ "owner": petal_id })),
            correlation_id: correlation_id.to_string(),
        }
    }

    /// FEAPI-GETTERRAIN-CORR: two API terrain reads in flight answered out
    /// of order, with a GUI load echo for one of the petals racing between —
    /// each fetched reply lands on its own waiter and the GUI echo on neither.
    #[test]
    fn concurrent_terrain_fetches_never_misroute() {
        let mut pending = PendingApiRequests::default();
        let (a_tx, mut a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&get_terrain_cmd("A", Some("ga")), a_tx);
        pending.enqueue_for(&get_terrain_cmd("B", Some("gb")), b_tx);

        assert!(pending.try_deliver(terrain_fetched("B", "gb")));
        assert!(
            !pending.try_deliver(DbResult::PetalTerrainLoaded {
                petal_id: "A".to_string(),
                terrain: None,
                correlation_id: None,
            }),
            "a GUI load echo must not satisfy an API terrain read"
        );
        assert!(a_rx.try_recv().is_err(), "GUI echo leaked to API waiter A");
        assert!(pending.try_deliver(terrain_fetched("A", "ga")));

        for (rx, petal) in [(&mut a_rx, "A"), (&mut b_rx, "B")] {
            match rx.try_recv() {
                Ok(DbResult::PetalTerrainFetched {
                    petal_id, terrain, ..
                }) => {
                    assert_eq!(petal_id, petal);
                    assert_eq!(terrain.unwrap()["owner"], petal);
                }
                other => panic!("{petal} expected its own fetched terrain, got {other:?}"),
            }
        }
    }

    /// RT-CORR-EDGE pop-and-drop: caller A times out, B enqueues, A's late
    /// (uncorrelated) reply arrives — A's closed entry consumes it instead of
    /// skipping to B, so B receives exactly its own reply.
    #[test]
    fn late_uncorrelated_reply_is_consumed_by_its_timed_out_entry_not_the_next_waiter() {
        let transform = |node_id: &str| DbResult::NodeTransformLoaded {
            node_id: node_id.to_string(),
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        };
        let get = |node_id: &str| DbCommand::GetNodeTransform {
            node_id: node_id.to_string(),
        };
        let mut pending = PendingApiRequests::default();
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&get("a"), a_tx);
        drop(a_rx); // A's timeout
        pending.enqueue_for(&get("b"), b_tx);

        assert!(
            !pending.try_deliver(transform("a")),
            "A's late reply is consumed by A's closed entry"
        );
        assert!(b_rx.try_recv().is_err(), "B must not receive A's reply");

        assert!(pending.try_deliver(transform("b")));
        assert!(matches!(
            b_rx.try_recv(),
            Ok(DbResult::NodeTransformLoaded { ref node_id, .. }) if node_id == "b"
        ));
    }

    /// The same rule on the wildcard path: a timed-out uncorrelated waiter's
    /// late `Error` is consumed by its own entry, never shifted to the next.
    #[test]
    fn late_error_is_consumed_by_the_timed_out_oldest_waiter() {
        let mut pending = PendingApiRequests::default();
        let (a_tx, a_rx) = tokio::sync::oneshot::channel();
        let (b_tx, mut b_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&DbCommand::Ping, a_tx);
        drop(a_rx);
        pending.enqueue_for(&DbCommand::Ping, b_tx);

        assert!(!pending.try_deliver(DbResult::Error("a failed".to_string())));
        assert!(b_rx.try_recv().is_err(), "B must not receive A's error");
        assert!(pending.try_deliver(DbResult::Pong));
        assert!(matches!(b_rx.try_recv(), Ok(DbResult::Pong)));
    }

    /// RT-CORR-EDGE: a correlated asset upload / placement that fails at the
    /// DB layer gets an immediate typed `RequestFailed` on ITS waiter (it
    /// used to hang to the timeout: the wildcard `Error` never reaches a
    /// correlated waiter); other families and ids are untouched.
    #[test]
    fn request_failed_reaches_only_its_correlated_entry_across_families() {
        let mut pending = PendingApiRequests::default();
        let (upload_tx, mut upload_rx) = tokio::sync::oneshot::channel();
        let (place_tx, mut place_rx) = tokio::sync::oneshot::channel();
        let (ping_tx, mut ping_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&create_asset_cmd(Some("up")), upload_tx);
        pending.enqueue_for(&DbCommand::Ping, ping_tx);
        pending.enqueue_for(&place_asset_cmd(Some("pl")), place_tx);
        let failed = |id: &str| DbResult::RequestFailed {
            error: format!("{id} failed"),
            correlation_id: id.to_string(),
        };

        assert!(pending.try_deliver(failed("pl")));
        assert!(matches!(
            place_rx.try_recv(),
            Ok(DbResult::RequestFailed { ref correlation_id, .. }) if correlation_id == "pl"
        ));
        assert!(
            upload_rx.try_recv().is_err(),
            "placement failure leaked to upload"
        );
        assert!(
            ping_rx.try_recv().is_err(),
            "placement failure leaked to Ping"
        );

        assert!(
            !pending.try_deliver(failed("unknown")),
            "an id with no waiter is dropped"
        );
        assert!(pending.try_deliver(failed("up")));
        assert!(matches!(
            upload_rx.try_recv(),
            Ok(DbResult::RequestFailed { ref correlation_id, .. }) if correlation_id == "up"
        ));
        assert!(ping_rx.try_recv().is_err());
        assert!(
            !pending.try_deliver(failed("up")),
            "the consumed entry cannot be answered twice"
        );

        let mint = |id: Option<&str>| DbCommand::MintApiToken {
            scope: "VERSE#v".into(), max_role: "viewer".into(), ttl_hours: 1,
            label: None, client_did: None, correlation_id: id.map(str::to_owned),
        };
        let minted = |id: Option<&str>| DbResult::ApiTokenMinted {
            correlation_id: id.map(str::to_owned), token: "private-fixture".into(),
            jti: "jti".into(), scope: "VERSE#v".into(), max_role: "viewer".into(),
            expires_at: "2026-11-02T12:00:00Z".into(), label: None, client: "client".into(),
        };
        let (old_tx, old_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&mint(Some("mint-old")), old_tx);
        drop(old_rx);
        let (new_tx, mut new_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&mint(Some("mint-new")), new_tx);
        let (legacy_tx, mut legacy_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&mint(None), legacy_tx);
        assert!(!pending.try_deliver(minted(Some("mint-old"))));
        assert!(!pending.try_deliver(minted(Some("unknown"))));
        assert!(new_rx.try_recv().is_err() && legacy_rx.try_recv().is_err());
        assert!(pending.try_deliver(minted(Some("mint-new"))));
        assert!(matches!(new_rx.try_recv(), Ok(DbResult::ApiTokenMinted { correlation_id: Some(id), .. }) if id == "mint-new"));
        assert!(legacy_rx.try_recv().is_err());
        assert!(pending.try_deliver(minted(None)));
        assert!(matches!(legacy_rx.try_recv(), Ok(DbResult::ApiTokenMinted { correlation_id: None, .. })));
        let (failed_tx, mut failed_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&mint(Some("mint-failed")), failed_tx);
        assert!(pending.try_deliver(failed("mint-failed")));
        assert!(matches!(failed_rx.try_recv(), Ok(DbResult::RequestFailed { correlation_id, .. }) if correlation_id == "mint-failed"));
        assert!(ping_rx.try_recv().is_err(), "token replies cannot fall back to another family");
    }

    /// RT-CORR-EDGE: a command that never reached the DB thread must not
    /// leave an entry that, under pop-and-drop, would swallow the next
    /// caller's reply.
    #[test]
    fn failed_db_send_cancels_the_entry() {
        let (api_tx, api_rx) = crossbeam::channel::bounded(4);
        let (db_tx, db_rx) = crossbeam::channel::bounded(4);
        drop(db_rx); // DB thread gone
        let mut app = App::new();
        app.insert_resource(ApiCommandReceiver(Arc::new(Mutex::new(api_rx))));
        app.insert_resource(DbCommandSender(db_tx));
        app.init_resource::<PendingApiRequests>();
        app.add_systems(Update, drain_api_commands);

        let (dead_tx, _dead_rx) = tokio::sync::oneshot::channel();
        api_tx
            .send(ApiCommand::DbRequest {
                cmd: DbCommand::Ping,
                reply_tx: dead_tx,
            })
            .unwrap();
        app.update();

        let (live_tx, mut live_rx) = tokio::sync::oneshot::channel();
        let mut pending = app.world_mut().resource_mut::<PendingApiRequests>();
        pending.enqueue_for(&DbCommand::Ping, live_tx);
        assert!(pending.try_deliver(DbResult::Pong));
        assert!(
            matches!(live_rx.try_recv(), Ok(DbResult::Pong)),
            "the unsent request's entry must not consume the next reply"
        );
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
        let (legacy_tx, mut legacy_rx) = tokio::sync::oneshot::channel();
        let (receipt_tx, mut receipt_rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&create_node_cmd(), legacy_tx);
        pending.enqueue_for(
            &DbCommand::RawQuery {
                sql: "SELECT 1".into(),
                vars: std::collections::HashMap::new(),
                correlation_id: Some("object-receipt".into()),
            },
            receipt_tx,
        );
        let mut birth = node_created("object-from-atomic-route");
        if let DbResult::NodeCreated { correlation_id, .. } = &mut birth {
            *correlation_id = Some("unsolicited-object-birth".into());
        }
        assert!(!pending.try_deliver(birth));
        assert!(!pending.try_deliver(DbResult::NodeMaterialized {
            node: crate::messages::CommittedNodeMaterialization {
                node: crate::messages::NodeDto {
                    node_id: "atomic-object".into(),
                    petal_id: "p".into(),
                    name: "full birth".into(),
                    position: [1.0, 2.0, 3.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [2.0, 1.0, 0.5],
                    has_asset: false,
                    asset_path: None
                },
                rotation_euler: [0.0; 3],
                asset_id: None,
                properties: serde_json::json!({"nested":{"array":[1,true]}}),
                history_version: 1,
            },
            correlation_id: "unknown-materialization".into(),
        }));
        let replica = crate::scene_replication::ReplicatedNodeCurrent {
            scope: "VERSE#v-FRACTAL#f-PETAL#p".into(),
            state: serde_json::from_value(serde_json::json!({
                "node_id":"remote","petal_id":"p","display_name":"remote","asset_id":"missing-asset",
                "position":[1.0,2.0,3.0],"elevation":2.0,"rotation":[0.0,0.0,0.0],"scale":[1.0,1.0,1.0],
                "interactive":true,"created_at":"2026-11-02","edit_seq":4,
                "properties":{"nested":{"measurement":null}},"tombstone":null
            })).unwrap(),
            provenance: crate::scene_replication::ReplicaNodeProvenance::OriginalSceneCommit {
                hlc: 42, publisher:"did:peer".into(),
            },
            asset: crate::scene_replication::ReplicaNodeAsset::Unavailable {
                asset_id:"missing-asset".into(), reason:crate::scene_replication::ReplicaAssetUnavailable::MissingBlob,
            },
        };
        assert_eq!(replica.asset.asset_id(),replica.state.asset_id.as_deref());
        assert!(replica.asset.asset_path().is_none());
        let value=serde_json::to_value(&replica).unwrap();
        assert!(value.get("history_version").is_none());
        assert_eq!(value["state"]["properties"]["nested"]["measurement"],serde_json::Value::Null);
        let reply=DbResult::ReplicatedNodeCurrent {current:replica};
        assert!(correlation_of_result(&reply).is_none());
        assert!(reply_kind_of_result(&reply).is_none());
        assert!(!pending.try_deliver(reply),"a GUI replica current read cannot consume a local/API waiter");
        assert!(legacy_rx.try_recv().is_err());
        assert!(receipt_rx.try_recv().is_err());

        assert!(
            legacy_rx.try_recv().is_err(),
            "GUI birth cannot consume legacy API create waiter"
        );
        assert!(
            receipt_rx.try_recv().is_err(),
            "birth is not a committed query receipt"
        );
        assert!(pending.try_deliver(DbResult::QueryResult {
            data: vec![],
            correlation_id: Some("object-receipt".into())
        }));
        assert!(matches!(
            receipt_rx.try_recv(),
            Ok(DbResult::QueryResult { .. })
        ));
        assert!(pending.try_deliver(node_created("n3")));
        assert!(matches!(legacy_rx.try_recv(),Ok(DbResult::NodeCreated {id,..}) if id == "n3"));
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
                correlation_id: None,
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

    /// DEC-C19 N2: a correlated family (RawQuery/InsertIotReadings/
    /// CreateAsset/CreateNodeWithAsset) always gets its own typed reply from
    /// the DB thread — the untyped wildcard `Error` must skip it even when it
    /// is the globally oldest pending request, and land on an uncorrelated
    /// waiter instead.
    #[test]
    fn error_reply_skips_correlated_waiters_for_the_oldest_uncorrelated_one() {
        let mut pending = PendingApiRequests::default();
        let (raw_tx, mut raw_rx) = tokio::sync::oneshot::channel();
        let (ping_tx, mut ping_rx) = tokio::sync::oneshot::channel();
        // The correlated RawQuery is enqueued FIRST (oldest by id) — under
        // the old FIFO-by-id rule it would have stolen the wildcard.
        pending.enqueue_for(&raw_query_cmd(Some("a")), raw_tx);
        pending.enqueue_for(&DbCommand::Ping, ping_tx);

        assert!(pending.try_deliver(DbResult::Error("db exploded".to_string())));
        assert!(
            raw_rx.try_recv().is_err(),
            "a correlated waiter must never receive the untyped wildcard error"
        );
        assert!(matches!(ping_rx.try_recv(), Ok(DbResult::Error(_))));

        // The correlated RawQuery is still alive and gets its own typed reply.
        assert!(pending.try_deliver(DbResult::QueryFailed {
            error: "parse error".to_string(),
            correlation_id: "a".to_string(),
        }));
        assert!(matches!(
            raw_rx.try_recv(),
            Ok(DbResult::QueryFailed { .. })
        ));
    }

    /// When every pending request is correlated, the untyped wildcard error
    /// is dropped — never handed to a correlated waiter just because nothing
    /// uncorrelated is pending.
    #[test]
    fn error_reply_is_dropped_when_only_correlated_waiters_are_pending() {
        let mut pending = PendingApiRequests::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        pending.enqueue_for(&raw_query_cmd(Some("a")), tx);

        assert!(
            !pending.try_deliver(DbResult::Error("db exploded".to_string())),
            "a correlated-only pending set must drop the untyped error, not deliver it"
        );
        assert!(rx.try_recv().is_err());
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
        let mut cases: Vec<(DbCommand, DbResult)> = vec![
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
                    correlation_id: None,
                },
                DbResult::NodePropertiesLoaded {
                    node_id: "n".to_string(),
                    properties: serde_json::Value::Null,
                    correlation_id: None,
                },
            ),
            (
                DbCommand::SetNodeProperty {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                    value: serde_json::Value::Null,
                    correlation_id: None,
                },
                DbResult::NodePropertySet {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                    correlation_id: None,
                },
            ),
            (
                DbCommand::DeleteNodeProperty {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                    correlation_id: None,
                },
                DbResult::NodePropertyDeleted {
                    node_id: "n".to_string(),
                    key: "k".to_string(),
                    correlation_id: None,
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
                get_terrain_cmd("p", None),
                DbResult::PetalTerrainLoaded {
                    petal_id: "p".to_string(),
                    terrain: None,
                    correlation_id: None,
                },
            ),
            (get_terrain_cmd("p", Some("g1")), terrain_fetched("p", "g1")),
            (set_terrain_cmd(Some("t1")), terrain_set(Some("t1"), "p")),
            (
                set_terrain_cmd(Some("t2")),
                DbResult::PetalTerrainSetFailed {
                    petal_id: "p".to_string(),
                    error: "x".to_string(),
                    correlation_id: "t2".to_string(),
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
            (
                DbCommand::ElevatedQuery {
                    statements: vec!["SELECT * FROM node TIMEOUT 5s".to_string()],
                    vars: std::collections::HashMap::new(),
                    write_tables: Vec::new(),
                    actor_did: "did:key:z".to_string(),
                    correlation_id: "e1".to_string(),
                },
                DbResult::ElevatedQueryResult {
                    data: Vec::new(),
                    changed_rows: 0,
                    correlation_id: "e1".to_string(),
                },
            ),
        ];

        for availability in ["inside_atomic_batch", "incomplete_atomic_batch"] {
            let mut wire = serde_json::json!({"node_id":"n","journal_version":1,"current_version":4,
                    "coverage_start_version":1,"coverage_started_at":"now","supported_operations":["baseline","property_set"],
                    "order":"per_object_commit_version","event_time_ordering":false,"current_state_verified":true,
                    "at_version":2,"availability":availability});
            if availability == "inside_atomic_batch" {
                wire["atomic_save"] =
                    serde_json::json!({"batch_id":"save-n","first_version":2,"committed_version":4});
            }
            let state: crate::node_history::NodeStateAtVersion =
                serde_json::from_value(wire.clone()).unwrap();
            assert!(state.state.is_none());
            assert_eq!(
                serde_json::to_value(&state).unwrap(),
                wire,
                "unavailable replay never synthesizes a partial snapshot"
            );
            cases.push((
                DbCommand::ReadNodeHistory {
                    node_id: "n".into(),
                    after_version: 0,
                    limit: 1,
                    at_version: Some(2),
                    grant: crate::node_history::NodeAccessGrant::Local,
                    correlation_id: Some("save-read".into()),
                },
                DbResult::NodeStateAtVersionLoaded {
                    state,
                    correlation_id: Some("save-read".into()),
                },
            ));
        }
        {
            use crate::editor_batch::*;
            let capture:EditorBatchCapture=serde_json::from_value(serde_json::json!({"scope":"VERSE#v-FRACTAL#f-PETAL#p","petal_id":"p","groups":[],"petal_snapshot":"opaque","world_scale":1.0,
                    "members":[{"node_id":"n","history_version":null,"stamp_path":null,"state":{"node_id":"n","petal_id":"p","display_name":"Object","asset_id":null,"position":[0,0,0],"elevation":0,"rotation":[0,0,0],"scale":[1,1,1],"interactive":false,"created_at":"now","edit_seq":0,"properties":{"nested":{"null":null}},"tombstone":null}}]})).unwrap();
            let receipt = EditorBatchReceipt {
                request_id: "batch-1".into(),
                scope: capture.scope.clone(),
                petal_id: "p".into(),
                members: capture.members.clone(),
                groups: vec![],
                created_group_id: None,
            };
            let serialized = serde_json::to_value(&receipt).unwrap();
            let decoded: EditorBatchReceipt = serde_json::from_value(serialized.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(decoded).unwrap(),
                serialized,
                "batch receipt retains nulls and unknown legacy versions"
            );
            cases.extend([
                (
                    DbCommand::ReadEditorBatch {
                        scope: capture.scope.clone(),
                        petal_id: "p".into(),
                        node_ids: vec!["n".into()],
                        correlation_id: "batch-read".into(),
                    },
                    DbResult::EditorBatchLoaded {
                        capture: capture.clone(),
                        correlation_id: "batch-read".into(),
                    },
                ),
                (
                    DbCommand::CommitCheckedEditorBatch {
                        request: EditorBatchRequest {
                            capture: capture.clone(),
                            edit: EditorBatchEdit::Membership {
                                group_id: "g".into(),
                                member: true,
                            },
                        },
                        correlation_id: "batch-1".into(),
                    },
                    DbResult::EditorBatchCommitted {
                        receipt: receipt.clone(),
                        correlation_id: "batch-1".into(),
                    },
                ),
                (
                    DbCommand::ResolveCheckedEditorBatch {
                        scope: capture.scope.clone(),
                        petal_id: "p".into(),
                        node_ids: vec!["n".into()],
                        request_id: "batch-1".into(),
                        correlation_id: "batch-resolve".into(),
                    },
                    DbResult::EditorBatchResolved {
                        resolution: EditorBatchResolution {
                            request_id: "batch-1".into(),
                            receipt: Some(receipt),
                            current: capture,
                        },
                        correlation_id: "batch-resolve".into(),
                    },
                ),
                (
                    DbCommand::ComparePetalVisibilityGroups {
                        petal_id: "p".into(),
                        expected: vec![],
                        groups: vec![],
                        auth: crate::messages::CallerAuth::Local,
                        correlation_id: Some("group-cas".into()),
                    },
                    DbResult::PetalVisibilityGroupsLoaded {
                        petal_id: "p".into(),
                        groups: vec![],
                        correlation_id: Some("group-cas".into()),
                    },
                ),
            ]);
        }
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
            reply_kind_of_result(&terrain_set(None, "p")),
            Some(ReplyKind::PetalTerrain),
            "a GUI SetPetalTerrain echo must never satisfy an API terrain-set waiter"
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
