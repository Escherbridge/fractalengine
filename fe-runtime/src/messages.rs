use bevy::prelude::Message;

/// Operation-owned input-file and ancestry handles retained through a read.
pub type FileReadLease = std::sync::Arc<Vec<std::fs::File>>;

/// Local dialog ownership; this capture grants no database authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeImportRequest {
    pub request_id: String,
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
}

/// Initial ordinary object state, validated before the atomic creation commit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateObjectInput {
    pub name: String,
    pub asset_id: Option<String>,
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}

fn unit_scale() -> [f32; 3] {
    [1.0; 3]
}

/// Supported ordinary object mutations with committed journal receipts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeMutation {
    /// Replace one literal outer property key.
    PropertySet {
        key: String,
        value: serde_json::Value,
    },
    /// Remove one optional outer property.
    PropertyDeleted { key: String },
    /// Replace all transform components; rotation is Euler XYZ radians.
    FullTransform {
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
}

/// Local GUI source capture for a checked first stamp address.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedStampPromotion {
    pub path_id: String,
    pub petal_id: String,
    pub scope: String,
    pub instance_index: u32,
    pub path_version: Option<i64>,
    pub before_properties: serde_json::Value,
    pub world_scale: f32,
}

/// Local editor snapshot for one atomic, path-locked stamp override save.
#[derive(Debug, Clone)]
pub struct StampOverrideEdit {
    pub node_id: String,
    pub path_id: String,
    pub instance_index: usize,
    pub scope: String,
    pub history_version: Option<i64>,
    pub path_version: Option<i64>,
    pub before_properties: serde_json::Value,
    pub path_anchors: Vec<fe_sdk::path_geometry::BezierAnchor>,
    pub path_descriptor: serde_json::Value,
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    pub arc_m: f32,
}

/// Re-exported so callers that depend on `fe-runtime` but not directly on
/// `fe-sdk` (fe-sim, fe-api, fe-test-harness) can construct
/// `InsertIotReadings`/`InsertDevicePayloads.source_class` without a new
/// Cargo dependency — the type is identical either way (CDL-SPEC10 A3).
pub use fe_sdk::source_class::SourceClass;

/// JTI string broadcast for token revocation notifications.
/// Carried by the API config so the validation middleware can invalidate
/// in-process caches without a DB round-trip.
pub type RevocationNotifier = tokio::sync::broadcast::Sender<String>;

/// One incoming IoT sensor reading, pre-validation (M2/F7). Defined here —
/// like every cross-thread wire shape — because `DbCommand::InsertIotReadings`
/// carries it and fe-database (the validator/ingester) depends on fe-runtime,
/// never the reverse; fe-database re-exports it so existing import paths are
/// unchanged (see fe-database `src/AGENTS.md` §iot-readings).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IotReadingInput {
    /// Anchor node the reading is spatially attached to.
    pub node_id: String,
    /// Metric name, e.g. `temperature_c`.
    pub metric: String,
    pub value: f64,
    #[serde(default)]
    pub units: String,
    /// RFC-3339 sensor timestamp; server ingest time when absent.
    #[serde(default)]
    pub recorded_at: Option<String>,
    /// R5: "decoded from / sent with this payload" — the `device_payload`
    /// this reading was decoded from, if any. `#[serde(default)]` so every
    /// pre-existing caller (REST body, GUI, harness fixtures) still parses
    /// unchanged. Validated against `device_payload` by the write handler
    /// (fe-database `src/AGENTS.md` §iot-readings).
    #[serde(default)]
    pub payload_id: Option<String>,
}

// ---------------------------------------------------------------------------
// R5: device payload / format descriptor wire shapes (CDL-SPEC10 A3)
// ---------------------------------------------------------------------------

/// Max asset ids one device payload may carry — the ONE bound every door
/// (REST, MCP, the DB-thread ingest handler) and the inbound replication gate
/// enforce (fe-database `src/AGENTS.md` §device-payload; "shared cores +
/// separate guards = drift").
pub const DEVICE_PAYLOAD_MAX_ASSETS: usize = 32;

/// One device payload to ingest: a format descriptor id plus the ordered list
/// of already-stored asset ids that make up this payload's opaque bytes
/// (R5: "allow for multiple records (a list of records)"). Defined here, like
/// [`IotReadingInput`], because `DbCommand::InsertDevicePayloads` carries it
/// cross-thread and fe-database (the validator/ingester) depends on
/// fe-runtime, never the reverse.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DevicePayloadInput {
    /// Anchor node the payload is spatially attached to.
    pub node_id: String,
    /// The declared `device_format.format_id` this payload's assets conform to.
    pub format_id: String,
    /// Ordered, already-stored asset ids (1..=`DEVICE_PAYLOAD_MAX_ASSETS`,
    /// fe-api `src/AGENTS.md` §device-ingest) whose `content_type` must equal
    /// the descriptor's `mime`.
    pub asset_ids: Vec<String>,
    /// RFC-3339 sensor timestamp; server ingest time when absent.
    #[serde(default)]
    pub recorded_at: Option<String>,
    /// Client-supplied id for idempotent retries (R5 assumption 4); the
    /// server mints a ULID when absent.
    #[serde(default)]
    pub payload_id: Option<String>,
}

/// A declared `device_format` row, as returned by `ListDeviceFormats`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceFormatDto {
    pub format_id: String,
    pub verse_id: String,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    pub declared_by: String,
    pub created_at: String,
}

/// A `device_payload` row, as returned by `ListDevicePayloads`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DevicePayloadDto {
    pub payload_id: String,
    pub node_id: String,
    pub petal_id: String,
    pub verse_id: Option<String>,
    pub format_id: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    /// Ordered asset ids this payload's bytes live in (preserves ingest order).
    pub asset_ids: Vec<String>,
    pub source_did: String,
}

/// Validation-shaped device-payload ingest rejection, mirroring
/// [`IotIngestRejection`]'s F24 pattern: the typed detail a
/// `DbResult::DevicePayloadsRejected` reply carries back across the DB-thread
/// seam so the API layer maps real HTTP statuses (422) without string-sniffing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePayloadIngestRejection {
    /// The anchor node is not an existing node of the target petal.
    UnknownAnchor { node_id: String },
    /// No `device_format` with this id is declared for the verse.
    UnknownFormat { format_id: String },
    /// A referenced asset id does not exist.
    UnknownAsset { asset_id: String },
    /// A referenced asset is anchored to a different petal than this payload.
    ForeignAssetAnchor { asset_id: String },
    /// A referenced asset's `content_type` does not equal the descriptor's `mime`.
    MimeMismatch { asset_id: String },
    /// `asset_ids` was empty.
    EmptyAssetList,
    /// `recorded_at` is not RFC-3339.
    InvalidTimestamp { raw: String },
    /// More than [`DEVICE_PAYLOAD_MAX_ASSETS`] asset ids.
    TooManyAssets { max: usize },
    /// A client-supplied `payload_id` is not a ULID.
    InvalidPayloadId { raw: String },
    /// A client-supplied `payload_id` is already taken by a row this caller
    /// may not treat as its own retry (another petal or another principal).
    /// Deliberately carries nothing about the existing row.
    PayloadIdConflict,
}

impl std::fmt::Display for DevicePayloadIngestRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAnchor { node_id } => {
                write!(f, "unknown anchor node '{node_id}' in this petal")
            }
            Self::UnknownFormat { format_id } => {
                write!(f, "no device_format '{format_id}' declared for this verse")
            }
            Self::UnknownAsset { asset_id } => write!(f, "unknown asset id '{asset_id}'"),
            Self::ForeignAssetAnchor { asset_id } => {
                write!(f, "asset '{asset_id}' is anchored to a different petal")
            }
            Self::MimeMismatch { asset_id } => write!(
                f,
                "asset '{asset_id}' content_type does not match the declared format's mime"
            ),
            Self::EmptyAssetList => write!(f, "asset_ids must be non-empty"),
            Self::InvalidTimestamp { raw } => {
                write!(f, "invalid recorded_at '{raw}' (expected RFC-3339)")
            }
            Self::TooManyAssets { max } => {
                write!(f, "at most {max} asset ids per device payload")
            }
            Self::InvalidPayloadId { raw } => {
                write!(f, "invalid payload_id '{raw}' (expected a ULID)")
            }
            Self::PayloadIdConflict => write!(f, "payload_id is already in use"),
        }
    }
}

impl std::error::Error for DevicePayloadIngestRejection {}

/// Validation-shaped `RegisterDeviceFormat` rejection (F24 pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFormatRejection {
    /// `format_id` fails the SPEC-10 §2.2 rule 4 grammar.
    InvalidFormatId { format_id: String },
    /// `mime` is blank.
    EmptyMime,
    /// `mime` is not a `type/subtype` token (fe-sdk `is_valid_mime`).
    InvalidMime { mime: String },
    /// A `device_format` with this id already exists for the verse with a
    /// DIFFERENT mime (the row is immutable per key — R5 assumption 7).
    ConflictingMime {
        format_id: String,
        existing_mime: String,
    },
    /// `schema` is present but not a JSON object (critic tick 8, AC-49): the
    /// descriptor's doc comment calls it "informative JSON Schema", which is
    /// always an object, never e.g. an array, string or bare literal.
    InvalidSchema,
}

impl std::fmt::Display for DeviceFormatRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFormatId { format_id } => {
                write!(
                    f,
                    "invalid format_id '{format_id}' (expected <namespace>/<name>@<major>)"
                )
            }
            Self::EmptyMime => write!(f, "mime must be non-empty"),
            Self::InvalidMime { mime } => {
                write!(f, "invalid mime '{mime}' (expected type/subtype)")
            }
            Self::ConflictingMime {
                format_id,
                existing_mime,
            } => write!(
                f,
                "device_format '{format_id}' already declared with mime '{existing_mime}'"
            ),
            Self::InvalidSchema => {
                write!(f, "schema must be a JSON object when present")
            }
        }
    }
}

impl std::error::Error for DeviceFormatRejection {}

/// Validation-shaped IoT ingest rejection (F24) — the typed detail a
/// `DbResult::IotReadingsRejected` reply carries back across the DB-thread
/// seam, so the API layer maps real HTTP statuses (422) instead of
/// string-sniffing `DbResult::Error`. Mirrors fe-database's
/// `IotIngestError` validation variants; `Display` wording is identical so
/// the fallback path's HTTP body matches the direct path byte-for-byte
/// (parity pinned by a fe-database test).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IotIngestRejection {
    /// An anchor node is not an existing node of the target petal.
    UnknownAnchor { node_id: String },
    /// `recorded_at` is not RFC-3339.
    InvalidTimestamp { raw: String },
    /// The metric name is empty.
    EmptyMetric,
    /// `payload_id` ("decoded from / sent with this payload", R5) names no
    /// existing `device_payload` row.
    UnknownPayload { payload_id: String },
}

impl std::fmt::Display for IotIngestRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAnchor { node_id } => {
                write!(f, "unknown anchor node '{node_id}' in this petal")
            }
            Self::InvalidTimestamp { raw } => {
                write!(f, "invalid recorded_at '{raw}' (expected RFC-3339)")
            }
            Self::EmptyMetric => write!(f, "metric name must be non-empty"),
            Self::UnknownPayload { payload_id } => {
                write!(f, "unknown payload_id '{payload_id}'")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// API Gateway types (Phase: Realtime API Gateway)
// ---------------------------------------------------------------------------

/// A real-time transform snapshot for fan-out to WebSocket subscribers.
/// Sent on the `tokio::broadcast` data-plane channel, bypassing the DB.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformUpdate {
    pub node_id: String,
    pub petal_id: String,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub timestamp_ms: u64,
    pub source_did: String,
}

/// Commands from the API thread to Bevy ECS.
///
/// Each request-style variant embeds a `tokio::sync::oneshot::Sender` so the
/// async API handler can `.await` the typed response without blocking the
/// crossbeam channel for other traffic.
#[derive(Debug)]
pub enum ApiCommand {
    /// Forward a DbCommand and route the DbResult back to the caller.
    DbRequest {
        cmd: DbCommand,
        reply_tx: tokio::sync::oneshot::Sender<DbResult>,
    },
    /// Fire-and-forget sync command.
    SyncForward {
        verse_id: String,
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Request a snapshot of the full hierarchy.
    GetHierarchy {
        reply_tx: tokio::sync::oneshot::Sender<Vec<VerseHierarchyData>>,
    },
    /// Fire-and-forget transform persist — bypasses PendingApiRequests.
    /// The DB thread will emit `SceneChange::TransformFailed` on error so the
    /// API layer can broadcast a rollback to WebSocket subscribers.
    TransformPersist {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
}

// ---------------------------------------------------------------------------
// Network types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum NetworkCommand {
    Ping,
    Shutdown,
}

#[derive(Debug, Clone, Message)]
pub enum NetworkEvent {
    Pong,
    Started,
    Stopped,
}

// ---------------------------------------------------------------------------
// Entity type enum
// ---------------------------------------------------------------------------

/// Type-safe entity classification replacing stringly-typed "verse"/"fractal"/"petal" fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityType {
    Verse,
    Fractal,
    Petal,
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verse => write!(f, "verse"),
            Self::Fractal => write!(f, "fractal"),
            Self::Petal => write!(f, "petal"),
        }
    }
}

/// Viewer-authorized hierarchy settings read from one fenced transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDetails {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub verse_id: String,
    /// The parent fractal for a petal; absent for verse/fractal settings.
    pub fractal_id: Option<String>,
    pub scope: String,
    pub name: String,
    pub default_access: Option<String>,
    pub description: Option<String>,
    pub role: String,
}

impl std::str::FromStr for EntityType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "verse" => Ok(Self::Verse),
            "fractal" => Ok(Self::Fractal),
            "petal" => Ok(Self::Petal),
            other => Err(format!("Unknown entity type: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Caller identity for authorized mutations (node_lifecycle_addressing FR-1/N-5)
// ---------------------------------------------------------------------------

/// Caller identity threaded into the authorized lifecycle mutations
/// (`TombstoneNode` / `CascadeTombstoneNode` / `PromoteInstance`).
///
/// `Identified` carries an *already-resolved* role — the API layer (T5) derives
/// it from the session/relay token upstream. `Local` is the local desktop
/// session issuing its own lifecycle commands: it asserts NO role; the DB thread
/// resolves the caller's identity and real role at the target scope (N-5: the UI
/// must never self-authorize). `Anonymous` is sub-Editor by construction. The DB
/// thread maps each to a `fe_policy::AuthContext` and enforces Editor+ before
/// mutating. See fe-policy/AGENTS.md §node-lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CallerAuth {
    /// An external/API caller whose role at the target scope was already
    /// validated upstream (T5). `role` is the lowercased role string
    /// (`"owner"`/`"manager"`/`"editor"`/`"viewer"`/`"none"`).
    Identified { did: String, role: String },
    /// The local desktop session; the DB thread resolves its identity + role for
    /// the target scope. The UI sends this for its own lifecycle commands (N-5:
    /// no UI-side role assertion).
    Local,
    /// An unauthenticated caller — sub-Editor by construction, always denied.
    Anonymous,
}

impl CallerAuth {
    /// Convenience constructor for an identified caller with a resolved role.
    pub fn identified(did: impl Into<String>, role: impl Into<String>) -> Self {
        Self::Identified {
            did: did.into(),
            role: role.into(),
        }
    }

    /// The local desktop session; the DB resolves its identity + role (N-5).
    pub fn local() -> Self {
        Self::Local
    }

    /// The caller's self-asserted DID, or `None` when it asserts none — `Local`
    /// (DB-resolved) or `Anonymous`.
    pub fn did(&self) -> Option<&str> {
        match self {
            CallerAuth::Identified { did, .. } => Some(did),
            CallerAuth::Local | CallerAuth::Anonymous => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Database command/result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum DbCommand {
    /// Private native current read; actual local Viewer authority is resolved by the DB.
    ReadReplicatedNodeCurrent { node_id:String, request_id:String },
    ReadEditorBatch {
        scope: String,
        petal_id: String,
        node_ids: Vec<String>,
        correlation_id: String,
    },
    CommitCheckedEditorBatch {
        request: crate::editor_batch::EditorBatchRequest,
        correlation_id: String,
    },
    ResolveCheckedEditorBatch {
        scope: String,
        petal_id: String,
        node_ids: Vec<String>,
        request_id: String,
        correlation_id: String,
    },
    /// Commit scale, rotation and slide together under current stamp/route authority.
    PromoteCheckedStamp {
        capture: CheckedStampPromotion,
        correlation_id: String,
    },
    CommitStampOverrides { edit: StampOverrideEdit, correlation_id: String },
    /// Local editor mutation with atomic expected-state and version checks.
    CommitCheckedNodeMutation {
        node_id: String,
        mutation: NodeMutation,
        expected: crate::node_history::NodeEditPrecondition,
        correlation_id: String,
    },
    /// Resolve a previously enqueued editor request by its exact immutable fact.
    ResolveCheckedNodeMutation {
        node_id: String,
        petal_id: String,
        request_id: String,
        correlation_id: String,
    },
    /// Trusted plugin read with a private typed reply channel.
    PluginRead {
        admission: crate::plugin_host::PluginAdmission,
        request: crate::plugin_host::PluginRead,
        reply: crossbeam::channel::Sender<Result<crate::plugin_host::PluginReply,String>>,
    },
    /// An admitted atomic plugin batch; queued work is never a commit receipt.
    PluginCommit {
        admission: crate::plugin_host::PluginAdmission,
        ops: Vec<fe_sdk::plugin_host::PendingOp>,
        reply: crossbeam::channel::Sender<Result<crate::plugin_host::PluginReply,String>>,
    },
    Ping,
    Seed,
    Shutdown,
    CreateVerse {
        name: String,
    },
    CreateFractal {
        verse_id: String,
        name: String,
    },
    CreatePetal {
        fractal_id: String,
        name: String,
    },
    CreateNode {
        petal_id: String,
        name: String,
        position: [f32; 3],
        /// Optional caller-supplied id echoed back on `NodeCreated`, letting a
        /// sender disambiguate its own result without the ambiguous
        /// `(petal_id, name)` tuple (FR-4). `None` = legacy content-correlated.
        correlation_id: Option<String>,
    },
    ImportGltf {
        request: Option<NativeImportRequest>,
        file_lease: Option<FileReadLease>,
        petal_id: String,
        name: String,
        file_path: String,
        position: [f32; 3],
    },
    /// Register an asset row for a blob the API thread ALREADY wrote to the
    /// shared blob store. Metadata only — asset bytes never ride this channel
    /// (fe-api/AGENTS.md §asset-ingest). Replies `AssetCreated`.
    CreateAsset {
        name: String,
        content_type: String,
        size_bytes: u64,
        /// 64-hex BLAKE3 hash; the DB thread verifies the blob is present.
        content_hash: String,
        /// Uploader DID (API token `sub`) — the `asset.owner_did` column.
        owner_did: Option<String>,
        /// Anchor petal the upload was authorized against — `asset.petal_id`.
        petal_id: Option<String>,
        /// Echoed on `AssetCreated` (reply correlation, DEC-C13).
        correlation_id: Option<String>,
    },
    /// Maintenance: delete asset rows no node references and older than
    /// `grace_secs`, then their blobs once nothing else names the hash.
    /// FIRE-AND-FORGET from the Bevy GC timer — never awaited by an API caller,
    /// so it is NOT in `reply_kind_of_command` (src/AGENTS.md §asset-gc).
    /// Replies `AssetsGarbageCollected` on success only.
    GcUnreferencedAssets {
        grace_secs: u64,
    },
    /// Create a node bound to an existing asset row (MCP `place_asset`).
    /// `rotation` is Euler XYZ radians, stored as `UpdateNodeTransform` does.
    /// Replies `GltfImported` with the echoed `correlation_id`.
    CreateNodeWithAsset {
        petal_id: String,
        name: String,
        asset_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
        correlation_id: Option<String>,
    },
    LoadHierarchy,
    /// Phase F: generate an invite string for a verse.
    GenerateVerseInvite {
        verse_id: String,
        include_write_cap: bool,
        expiry_hours: u32,
    },
    /// Phase F: join a verse using a base64 invite string.
    JoinVerseByInvite {
        invite_string: String,
    },
    /// Wipe all tables, re-apply schema, and re-seed default data.
    ResetDatabase,
    /// Correlated local development reset; host admission is captured outside the command.
    ResetDevelopmentDatabase { request_id: String },
    /// One host-proven terminal reset; always exits the database worker afterward.
    TerminalDevelopmentReset {
        request_id: String,
        confirmation: String,
        proof: crate::maintenance::QuiescenceProof,
    },
    /// Persist a transform change for a node (position/rotation/scale).
    UpdateNodeTransform {
        node_id: String,
        position: [f32; 3],
        /// Euler angles in radians (XYZ order).
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Persist a portal URL change for a node's associated model.
    UpdateNodeUrl {
        node_id: String,
        /// `None` clears the URL.
        url: Option<String>,
        request_id: String,
        expected_scope: String,
    },
    /// Local current URL read with an optional exact committed model anchor.
    ReadNodeUrl {
        node_id: String,
        expected_scope: String,
        expected_asset_id: Option<String>,
        request_id: String,
    },
    // --- Entity Settings: General config ---
    /// Read the local caller's current target settings; never an API waiter reply.
    GetEntityDetails {
        entity_type: EntityType,
        entity_id: String,
        request_id: u64,
    },
    /// Rename a verse, fractal, or petal.
    RenameEntity {
        entity_type: EntityType,
        entity_id: String,
        new_name: String,
    },
    /// Set the default access level for a verse ("viewer" or "none").
    SetVerseDefaultAccess {
        verse_id: String,
        default_access: String,
    },
    /// Set a verse's timeseries replication settings (M2/F6 — A13): mode
    /// (`mirror`|`sharded`|`balanced`), the durability slider `R`, and the
    /// shard bucket width. Persisted on the verse row's `ts_*` columns and
    /// re-emitted as the verse manifest so peers and the sync thread's
    /// fabric converge on the same settings through the existing seams.
    SetVerseTimeseriesSettings {
        verse_id: String,
        mode: String,
        replication_factor: u32,
        bucket_width_ms: u64,
    },
    /// Update a fractal's description.
    UpdateFractalDescription {
        fractal_id: String,
        description: String,
    },
    /// Delete a verse, fractal, or petal (with cascade).
    DeleteEntity {
        entity_type: EntityType,
        entity_id: String,
    },
    /// Local GUI hierarchy access operation; actor comes only from the DB thread.
    EntityAccess {
        target: crate::entity_access::EntityAccessTarget,
        action: crate::entity_access::EntityAccessAction,
        request_id: String,
    },
    PrepareJoinInvite {
        target: crate::entity_access::EntityAccessTarget,
        recipient_did: String,
        role: String,
        expiry_hours: u32,
        request_id: String,
    },
    IssueJoinInvite {
        source: crate::join_invite::InviteSource,
        ticket: crate::join_invite::JoinCredential,
        request_id: String,
    },
    AdmitJoinInvite {
        credential: crate::join_invite::JoinCredential,
        request_id: String,
    },
    CheckJoinReadiness {
        credential: crate::join_invite::JoinCredential,
        request_id: String,
    },
    FinalizeJoinProof {
        credential: crate::join_invite::JoinCredential,
        request_id: String,
        deadline: std::time::Instant,
    },
    // --- Entity Settings: Access management ---
    /// Resolve effective roles for a list of peers at a scope.
    ResolveRolesForPeers {
        scope: String,
        peer_dids: Vec<String>,
    },
    /// Assign a role to a peer at a scope (privilege-checked).
    AssignRole {
        peer_did: String,
        scope: String,
        role: String,
    },
    /// Revoke a peer's explicit role at a scope (privilege-checked).
    RevokeRole {
        peer_did: String,
        scope: String,
    },
    /// Generate a scoped invite link with a specific role and expiry.
    GenerateScopedInvite {
        scope: String,
        role: String,
        expiry_hours: u32,
    },
    /// Resolve the local user's effective role at a scope.
    ResolveLocalRole {
        scope: String,
    },
    // --- API Token management ---
    /// Mint a new API token and persist its metadata.
    /// Authorization is enforced server-side: caller must have Manager+ at scope.
    MintApiToken {
        correlation_id: Option<String>,
        scope: String,
        max_role: String,
        ttl_hours: u32,
        label: Option<String>,
        /// Ed25519 `did:key` of the API client to mint for; `None` mints a
        /// fresh client. Validated by the DB door (fe-identity
        /// `validate_client_did`); never an authorization subject.
        client_did: Option<String>,
    },
    /// Revoke an API token by JTI. Ownership is enforced server-side
    /// using the `sub` provided by the caller (from the token info).
    RevokeApiToken {
        jti: String,
        sub: String,
    },
    /// List active (non-revoked, non-expired) API tokens for this node, paginated.
    ListApiTokens {
        offset: u32,
        limit: u32,
    },
    /// List active (non-revoked, non-expired) API tokens whose scope falls within
    /// a prefix, paginated. Used by admin dashboards.
    ListApiTokensByScope {
        scope_prefix: String,
        offset: u32,
        limit: u32,
    },
    /// Resolve a petal's full scope string (VERSE#v-FRACTAL#f-PETAL#p) by
    /// walking petal → fractal → verse in the DB.
    ResolvePetalScope {
        petal_id: String,
    },
    /// Resolve a node's full scope string by walking node → petal → fractal → verse.
    ResolveNodeScope {
        node_id: String,
    },
    /// Resolve a fractal's scope string (VERSE#v-FRACTAL#f) by walking fractal → verse.
    ResolveFractalScope {
        fractal_id: String,
    },
    /// Resolve a verse's scope string (VERSE#v); `None` when the verse row is absent.
    ResolveVerseScope {
        verse_id: String,
    },
    /// Load all nodes belonging to a petal (for scene snapshot).
    LoadNodesByPetal {
        petal_id: String,
    },
    /// Read a single node's persisted transform (position/rotation/scale).
    GetNodeTransform {
        node_id: String,
    },
    /// Set a custom property on a node.
    SetNodeProperty {
        node_id: String,
        key: String,
        value: serde_json::Value,
        correlation_id: Option<String>,
    },
    /// Get all custom properties of a node.
    GetNodeProperties {
        node_id: String,
        correlation_id: Option<String>,
    },
    /// Delete a custom property from a node.
    DeleteNodeProperty {
        node_id: String,
        key: String,
        correlation_id: Option<String>,
    },
    /// Delete a node row and cascade to its child waypoint nodes.
    DeleteNode {
        node_id: String,
    },
    // --- Node lifecycle (node_lifecycle_addressing_20260725 spine) ---
    /// FR-1 sync-safe delete: soft-deletes the node by stamping a durable
    /// `tombstone` (HLC + source) on the row instead of dropping it, plus a
    /// tombstone entry in the op-log. The row persists (surviving reload) but is
    /// filtered from every read, so the delete survives P2P/HLC merge and a
    /// stale replica cannot resurrect it (N-4). Authorized in fe-policy (Editor+,
    /// via `auth`) before mutating. Distinct from the legacy hard-drop
    /// `DeleteNode`; Wave-1 T4 sends this, T5 reports the resulting tombstone.
    TombstoneNode {
        node_id: String,
        /// Caller identity — mapped to a `fe_policy::AuthContext` and checked
        /// (Editor+ on the node's scope) before any mutation (N-5).
        auth: CallerAuth,
    },
    /// FR-2 cascade delete: tombstones a node and every descendant as one
    /// atomic (transactional) op. Confirm-gating is a UI concern (T4); this is
    /// the data op. Authorized in fe-policy (Editor+, via `auth`).
    CascadeTombstoneNode {
        node_id: String,
        /// Caller identity — authorized before mutating (see `TombstoneNode`).
        auth: CallerAuth,
    },
    /// FR-5 lazy promotion: materialize a full addressable node for a single
    /// stamp instance on first individual select/edit. Idempotent — promoting
    /// an already-materialized instance is a no-op. Wave-1 T2 constructs this.
    /// Authorized in fe-policy (Editor+, via `auth`) before materializing.
    PromoteInstance {
        petal_id: String,
        /// Owning path node id whose curve the stamp follows.
        path_id: String,
        /// Zero-based index of the instance within the stamp group.
        instance_index: u32,
        /// Caller identity — authorized before materializing the node (N-5).
        auth: CallerAuth,
    },
    /// Rename a node's `display_name`. Authorized in fe-policy (Editor+, via
    /// `auth`); a missing/tombstoned node is an error, never a silent success.
    RenameNode {
        node_id: String,
        new_name: String,
        /// Caller identity — authorized before mutating (see `TombstoneNode`).
        auth: CallerAuth,
    },
    /// Duplicate a node: full row copy with a fresh id, " (copy)" name, +1m x/z
    /// offset, and path-binding identity keys stripped — see
    /// fe-database/src/AGENTS.md §lifecycle. Authorized in fe-policy (Editor+).
    DuplicateNode {
        node_id: String,
        /// Caller identity — authorized before mutating (see `TombstoneNode`).
        auth: CallerAuth,
    },
    /// Count a node's live (non-tombstoned) descendants, excluding the node
    /// itself — read-only (no auth, mirrors `GetNodeProperties`); feeds the
    /// cascade-delete confirm copy ("its N child nodes").
    CountNodeDescendants {
        node_id: String,
    },
    /// Persist a node's visibility view properties (`view.state` /
    /// `view.groups`, hierarchy_visibility_groups D-2/D-6). Auth-carrying
    /// (D-15): authorized Editor+ like `RenameNode`, never a raw
    /// `SetNodeProperty`. See fe-database/src/AGENTS.md §lifecycle.
    SetNodeView {
        node_id: String,
        /// `"auto" | "show" | "hide"`; `"auto"` removes the key. `None` = unchanged.
        state: Option<String>,
        /// Group ids the node belongs to; empty removes the key. `None` = unchanged.
        groups: Option<Vec<String>>,
        /// Caller identity — authorized before mutating (see `TombstoneNode`).
        auth: CallerAuth,
        /// `Some` routes a failure to `RequestFailed`; `None` to `Error`.
        correlation_id: Option<String>,
    },
    /// Read every live node in `petal_id` carrying a persisted view property —
    /// read-only (no auth, mirrors `GetNodeProperties`). Replies
    /// `PetalViewStateLoaded`.
    LoadPetalViewState {
        petal_id: String,
    },
    /// Replace a petal's whole visibility-Group registry (`petal.visibility_groups`,
    /// hierarchy_visibility_groups D-3) — O(groups), never touches membership
    /// (`view.groups` lives on nodes). Auth-carrying: Editor+ at the petal scope
    /// (fe-database/src/AGENTS.md §lifecycle). Replies `PetalVisibilityGroupsLoaded`.
    SetPetalVisibilityGroups {
        petal_id: String,
        groups: Vec<VisibilityGroupDto>,
        auth: CallerAuth,
        /// `Some` routes a failure to `RequestFailed`; `None` to `Error`.
        correlation_id: Option<String>,
    },
    ComparePetalVisibilityGroups {
        petal_id: String,
        expected: Vec<VisibilityGroupDto>,
        groups: Vec<VisibilityGroupDto>,
        auth: CallerAuth,
        correlation_id: Option<String>,
    },
    /// Read a petal's visibility-Group registry (read-only; REST gates Viewer+).
    /// Replies `PetalVisibilityGroupsLoaded`; a missing petal fails.
    GetPetalVisibilityGroups {
        petal_id: String,
        correlation_id: Option<String>,
    },
    /// Persist a verse/fractal/petal eye (`view_hidden`, D-1). Auth-carrying:
    /// Editor+ at that scope. Replies `ScopeHiddenSet`.
    SetScopeHidden {
        level: ScopeLevel,
        id: String,
        hidden: bool,
        auth: CallerAuth,
        correlation_id: Option<String>,
    },
    // --- Field definition (property schema) management ---
    /// Create a new field definition for a scope.
    CreateFieldDef {
        scope: String,
        entity_type: String,
        key: String,
        value_type: String,
        default_val: Option<serde_json::Value>,
    },
    /// List all field definitions for a scope.
    ListFieldDefs {
        scope: String,
    },
    /// Update a field definition's type and default value.
    UpdateFieldDef {
        field_def_id: String,
        value_type: String,
        default_val: Option<serde_json::Value>,
    },
    /// Install the next bounded property-schema revision; replies with correlated QueryResult.
    SetFieldSchema {
        field_def_id: String,
        descriptor: serde_json::Value,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: Option<String>,
    },
    /// Read one enforced descriptor with DB-time scope/local-role checks.
    GetFieldSchema {
        field_def_id: String,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: Option<String>,
    },
    /// Committed actor-attributed ordinary mutation; API actor comes from the validated principal.
    CommitNodeMutation {
        node_id: String,
        mutation: NodeMutation,
        actor_did: String,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: Option<String>,
    },
    /// Create initial properties and complete baseline in one committed transaction.
    CreateObject {
        petal_id: String,
        input: CreateObjectInput,
        actor_did: String,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: Option<String>,
    },
    /// Durable bounded journal page or exact-version replay; not generic raw-table access.
    ReadNodeHistory {
        node_id: String,
        after_version: i64,
        limit: usize,
        at_version: Option<i64>,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: Option<String>,
    },
    /// Desktop-only scoped measurement projection; not an API pending-reply family.
    ReadNodeMeasurements {
        node_id: String,
        petal_id: String,
        series: Option<crate::node_measurements::MeasurementSeries>,
        limit: usize,
        grant: crate::node_history::NodeAccessGrant,
        correlation_id: String,
    },
    /// Desktop-only material projection, checked as the local Viewer at DB use.
    ReadSceneMaterials {
        petal_id: String,
        expected_scope: String,
        node_ids: Vec<String>,
        correlation_id: String,
    },
    /// Delete a field definition by ID.
    DeleteFieldDef {
        field_def_id: String,
    },
    /// User-authored desktop SELECT, stored Viewer authorization and petal scoping at use.
    ScopedQuery {
        sql: String,
        vars: std::collections::HashMap<String, serde_json::Value>,
        petal_scope: Vec<String>,
        correlation_id: String,
    },
    /// Trusted server-built/prevalidated SELECT; user-authored desktop text uses ScopedQuery.
    RawQuery {
        sql: String,
        vars: std::collections::HashMap<String, serde_json::Value>,
        /// API-originated queries carry a fresh id the DB thread echoes on
        /// `QueryResult`/`QueryFailed`; uncorrelated trusted callers send `None`
        /// (fe-runtime src/AGENTS.md §api-reply-correlation).
        correlation_id: Option<String>,
    },
    /// `/api/v1/query/elevated` (Owner, operator opt-in): a pre-validated
    /// multi-statement script run on the DB thread — writes atomic with one
    /// op-log entry, then replicated + broadcast (fe-api AGENTS.md
    /// §elevated-query). Replies `ElevatedQueryResult` or `RequestFailed`.
    ElevatedQuery {
        /// Top-level statements, each DML already carrying `TIMEOUT 5s`.
        statements: Vec<String>,
        vars: std::collections::HashMap<String, serde_json::Value>,
        /// Lowercase tables the script may write (empty = read-only).
        write_tables: Vec<String>,
        /// The caller's DID (token `sub`), recorded on the op-log entry.
        actor_did: String,
        correlation_id: String,
    },
    /// Execute a distributed-query local partial (M2/F7 — A15).
    ///
    /// Sent by the sync thread (requester AND responder sides) to run this
    /// host's slice of a distributed timeseries query. The spec is
    /// structured (see `distributed_query.rs`): the DB thread renders its own
    /// SQL via the fe-query builders with bound parameters — no SQL ever
    /// crosses a channel boundary. The reply rides the embedded crossbeam
    /// sender, deliberately NOT the `DbResult` channel: this command has no
    /// reply family and must never be registered with
    /// `PendingApiRequests::enqueue_for` (its answer would mis-deliver into
    /// an API waiter's oneshot; see fe-runtime `AGENTS.md`
    /// §api-reply-correlation).
    ExecuteTsPartial {
        /// The query's verse: the request's verse on the requester, the
        /// AUTHENTICATED arrival topic's verse on a responder — scopes the
        /// `deferred_shards` honesty report (OF-7b).
        verse_id: String,
        spec: crate::distributed_query::TsQueryKind,
        shards: Vec<crate::distributed_query::PartialShard>,
        /// Per-partial row cap (clamped by the executor into
        /// `distributed_query::{MIN,MAX}_PARTIAL_ROW_CAP`).
        row_cap: usize,
        reply: crossbeam::channel::Sender<crate::distributed_query::TsPartialRows>,
    },
    /// Ingest a batch of IoT readings through the DB thread (M2/F7 — the
    /// DB-thread-originated ingestion leg; same handler the fe-api
    /// `db_reader` path calls). Durable-first, then one `ReplicationEvent`
    /// per accepted row over the thread's replication seam — see
    /// fe-database `src/AGENTS.md` §iot-readings.
    InsertIotReadings {
        petal_id: String,
        /// The verse whose replica the rows belong in; `None` disables the
        /// replication emit (durable-only).
        verse_id: Option<String>,
        source_did: String,
        readings: Vec<IotReadingInput>,
        /// Caller-declared source class (SPEC-10 §7.1 rule 6) — threaded
        /// explicitly from caller to door; NEVER inferred from `source_did`
        /// (the simulation lab's host-peer DID would otherwise read as
        /// `device`/`connector`). See fe-sdk `source_class` AGENTS.md
        /// §source-class.
        source_class: fe_sdk::source_class::SourceClass,
        /// Caller-supplied id echoed on the reply (`CreateNode` precedent);
        /// `PendingApiRequests` routes a correlated reply ONLY to its own
        /// waiter (src/AGENTS.md §api-reply-correlation). `None` = legacy.
        correlation_id: Option<String>,
    },
    // --- R5 device formats / payloads (CDL-SPEC10 A3) ---
    /// Declare a device payload format for a verse (Manager+, verse scope).
    /// Immutable per `(verse_id, format_id)` — re-declaring with a different
    /// `mime` is a conflict (`DbResult::DeviceFormatRejected`).
    RegisterDeviceFormat {
        verse_id: String,
        descriptor: fe_sdk::device_format::DeviceFormatDescriptor,
        /// The declaring caller's DID (`device_format.declared_by`).
        declared_by: String,
        correlation_id: Option<String>,
    },
    /// List every `device_format` declared for a verse (Viewer+).
    ListDeviceFormats {
        verse_id: String,
        correlation_id: Option<String>,
    },
    /// Ingest one device payload (Editor+, petal scope) — one REST/MCP call
    /// ingests one payload whose `asset_ids` carry its (possibly multi-file)
    /// opaque bytes (R5: "a list of records" IS the payload). Mirrors
    /// `InsertIotReadings`: durable-first, then one `ReplicationEvent` per
    /// accepted asset/payload row (fe-database `src/AGENTS.md` §iot-readings).
    InsertDevicePayloads {
        petal_id: String,
        /// The verse whose replica the rows belong in; `None` disables the
        /// replication emit (durable-only).
        verse_id: Option<String>,
        source_did: String,
        /// Caller-declared source class (SPEC-10 §7.1 rule 6); see
        /// `InsertIotReadings::source_class`.
        source_class: fe_sdk::source_class::SourceClass,
        payload: DevicePayloadInput,
        correlation_id: Option<String>,
    },
    /// List `device_payload` rows for a petal, optionally filtered by node,
    /// format and a recorded-time window (Viewer+).
    ListDevicePayloads {
        petal_id: String,
        node_id: Option<String>,
        format_id: Option<String>,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        correlation_id: Option<String>,
    },
    // --- Hexon crate registry (Phase 8) ---
    /// Install a hexon crate into a petal.
    InstallCrate {
        hexon_uri: String,
        manifest_hash: String,
        publisher_did: String,
        hexon_type: String, // HexonKind serialized
        version: String,
        name: String,
        tags: Vec<String>,
        petal_id: String,
        size_bytes: u64,
    },
    /// Install a single asset entry for a hexon crate.
    InstallCrateEntry {
        entry_id: String,
        hexon_uri: String,
        kind: String, // EntryKind serialized
        asset_hash: String,
        format: String,
        label: String,
        metadata: serde_json::Value,
    },
    /// Uninstall a hexon crate from a petal.
    UninstallCrate {
        hexon_uri: String,
    },
    // --- Petal terrain binding ---
    /// Read declarations using the DB thread's actual local identity.
    GetPetalRequirements {
        petal_id: String,
        correlation_id: String,
    },
    /// Replace declarations only while the authorized document and scope still match.
    SetPetalRequirements {
        expected: crate::petal_requirements::PetalRequirementsSnapshot,
        requirements: crate::petal_requirements::PetalRequirements,
        correlation_id: String,
    },
    /// Set or clear a petal's terrain config (None clears).
    SetPetalTerrain {
        petal_id: String,
        terrain: Option<serde_json::Value>,
        /// Editor+ at the petal scope, enforced by the DB thread (role
        /// propagation RP-4 step 7 — the `SetPetalRenderDistance` shape): the
        /// GUI sends `Local`, the API the role it already validated.
        auth: CallerAuth,
        /// API-originated sets carry a fresh id the DB thread echoes on
        /// `PetalTerrainLoaded`/`PetalTerrainSetFailed`; GUI sets send `None`
        /// (fe-runtime src/AGENTS.md §api-reply-correlation).
        correlation_id: Option<String>,
    },
    /// Load a petal's terrain config.
    GetPetalTerrain {
        petal_id: String,
        /// API reads carry a fresh id answered by `PetalTerrainFetched`/
        /// `RequestFailed`; GUI loads send `None` and get `PetalTerrainLoaded`
        /// (fe-runtime src/AGENTS.md §api-reply-correlation).
        correlation_id: Option<String>,
    },
    // --- Petal render distance (p2p_asset_streaming_20260718 S6) ---
    /// Set or clear a petal's durable residency horizon (`petal.render_distance`,
    /// `NONE` = inherit the global `AppSettings.render_distance`). Auth-carrying:
    /// Editor+ at the petal scope (fe-database `src/AGENTS.md` §handlers "Petal
    /// render distance"). Replies `PetalRenderDistanceLoaded`.
    SetPetalRenderDistance {
        petal_id: String,
        render_distance: Option<f32>,
        auth: CallerAuth,
        /// `Some` routes a failure to `RequestFailed`; `None` to `Error`.
        correlation_id: Option<String>,
    },
    /// Read a petal's `render_distance` (read-only; REST gates Viewer+).
    /// Replies `PetalRenderDistanceLoaded`.
    GetPetalRenderDistance {
        petal_id: String,
        correlation_id: Option<String>,
    },
    // --- Explicit orphaned-verse recovery (F6 in-app lockout root cause) ---
    /// User-confirmed, EXPLICIT claim of a verse whose `created_by` is a
    /// `did:key:` identity that no longer matches this node's local DID (an
    /// identity rotation/loss orphaned it). Unlike the startup `reconcile`
    /// rule (`orphaned_creator`), this command is allowed to adopt a
    /// `did:key:`-shaped creator — but ONLY for a verse this store has never
    /// shared (no foreign `verse_member`/`role_grant`/`role_statement_pending`
    /// row). GUI-only today, so `correlation_id` is always `None`; replies
    /// `OrphanedVerseClaimed` or `OrphanedVerseClaimRefused`. See
    /// fe-database/src/AGENTS.md §reconcile "ClaimOrphanedVerse".
    ClaimOrphanedVerse {
        verse_id: String,
        correlation_id: Option<String>,
    },
    // --- Device enrolment (CDL-R2, W4-8) ---
    /// Sign this node's X25519 device key into a self-signed SPEC-3 §10.2.1 enrolment (owner
    /// R2), store it, and publish the row into `verse_id`'s replica. Explicit only: nothing
    /// auto-enrols (fe-database `canon_log/AGENTS.md` §device-enrolment). Replies
    /// `LocalDeviceEnrolled`; a failure is `RequestFailed` when correlated, else `Error`.
    EnrolLocalDevice {
        verse_id: String,
        correlation_id: Option<String>,
    },
    /// Sign a revocation (signed tombstone) of one of this principal's device keys, store it and
    /// publish it into `verse_id`'s replica; revoking this node's own key rotates it. Replies
    /// `LocalDeviceRevoked`; failures as `EnrolLocalDevice`.
    RevokeLocalDevice {
        verse_id: String,
        /// Lowercase hex of the 32-byte X25519 device key.
        device_key_hex: String,
        correlation_id: Option<String>,
    },
    // --- Inbound P2P replication (A4 seam) ---
    /// Apply one row received from a peer's verse replica.
    ///
    /// Emitted by the sync thread's inbound replica event pump. The DB thread
    /// is the single SurrealDB writer and the enforcement point: it resolves
    /// the peer's role at the verse scope (fe-policy, deny-by-default) before
    /// applying and never re-emits replication for the applied row (loop
    /// prevention at the source).
    ApplyReplicatedRow {
        verse_id: String,
        table: String,
        record_id: String,
        /// Serialised row JSON as published by the authoring peer.
        row_bytes: Vec<u8>,
        /// `did:key` of the peer that authored the entry.
        author_did: String,
    },
    /// Re-publish stored locally Manager-authorized Verse/Petal catalog after replica open.
    ReemitLocalVerseCatalog {
        verse_id: String,
    },
    /// A verse replica just opened (sent by the sync thread): re-emit this node's own live
    /// role statements for the verse so a grant or revocation issued while the replica was
    /// closed, or evicted from pending writes, still reaches peers (role_propagation AC-42).
    /// Fire-and-forget, no reply; a re-publish of durable state, not a DB mutation.
    ReemitLocalRoleStatements {
        verse_id: String,
    },
}

/// How one inbound replicated row was reconciled by the DB thread (A4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicatedRowOutcome {
    /// Row applied (created or field-merged).
    Applied,
    /// Payload was for a table the inbound path does not handle.
    NotApplicable,
    /// A local tombstone dominated the incoming live row (N-4 non-resurrection).
    SkippedTombstoned,
    /// An incoming tombstone converged the local row to deleted.
    AppliedTombstone,
    /// Apply failed — row left untouched, error logged by the DB thread.
    Failed,
    /// The row is permanently refused (a structural violation, a forged or
    /// cross-verse statement, a failed signature) — never applied, never
    /// re-emitted, never retried.
    Denied,
    /// The row failed only because something has not converged yet (role
    /// propagation §2.4): not applied now, retried when the awaited fact lands.
    /// A content row waits in the DB thread's in-memory `InboundAdmissions`
    /// queue; a role statement is held durably in `role_statement_pending`.
    Deferred(DeferralReason),
}

/// What a [`ReplicatedRowOutcome::Deferred`] row is waiting for (plan §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeferralReason {
    /// No verse manifest (verse row) has converged locally yet.
    Manifest,
    /// A referenced parent row (`table`, `id`) is not known locally yet.
    Parent { table: String, id: String },
    /// The author (or a role statement's grantor) resolves below the
    /// threshold the row needs; retried when that DID's effective role rises.
    AuthorRole { did: String },
}

#[derive(Debug, Clone, Message)]
pub enum DbResult {
    EditorBatchLoaded {
        capture: crate::editor_batch::EditorBatchCapture,
        correlation_id: String,
    },
    EditorBatchCommitted {
        receipt: crate::editor_batch::EditorBatchReceipt,
        correlation_id: String,
    },
    EditorBatchResolved {
        resolution: crate::editor_batch::EditorBatchResolution,
        correlation_id: String,
    },
    /// Exact persisted outcome for a prior local editor request, never a coordinate guess.
    CheckedNodeMutationResolved {
        node_id: String,
        request_id: String,
        receipt: Option<crate::node_history::NodeMutationReceipt>,
        current: crate::node_history::NodeRowSnapshot,
        correlation_id: String,
    },
    /// GUI-only committed plugin removal; it has no pending API reply family.
    PluginNodeRemoved { node_id: String, petal_id: String },
    Pong,
    Seeded {
        petal_name: String,
        rooms: Vec<String>,
    },
    Started,
    Stopped,
    Error(String),
    /// GUI-only committed birth carrying full state; has no API request family.
    NodeMaterialized {
        node: CommittedNodeMaterialization,
        correlation_id: String,
    },
    /// GUI current state after a successful replica apply; has no API reply or local history version.
    ReplicatedNodeCurrent { current: crate::scene_replication::ReplicatedNodeCurrent },
    /// Private native current-read echo; its request nonce is not a history or ordering version.
    ReplicatedNodeCurrentRead {
        node_id:String, request_id:String,
        result:Result<crate::scene_replication::ReplicatedNodeCurrent,String>,
    },
    /// Confirmed current state after an ordinary object create/mutation.
    NodeMutationCommitted {
        receipt: crate::node_history::NodeMutationReceipt,
        correlation_id: Option<String>,
    },
    /// Read-only immutable facts; never a live-state materialization.
    NodeHistoryLoaded {
        page: crate::node_history::NodeHistoryPage,
        correlation_id: Option<String>,
    },
    /// Read-only version preview; never a live-state materialization.
    NodeStateAtVersionLoaded {
        state: crate::node_history::NodeStateAtVersion,
        correlation_id: Option<String>,
    },
    NodeMeasurementsLoaded {
        measurements: crate::node_measurements::NodeMeasurements,
        correlation_id: String,
    },
    SceneMaterialsLoaded {
        materials: crate::scene_materials::SceneMaterials,
        correlation_id: String,
    },
    VerseCreated {
        id: String,
        name: String,
        /// The verse's hex-encoded iroh-docs namespace id (F22/M2). The DB
        /// handler computes it at creation, so the GUI tree entry becomes
        /// authoritative the moment the result lands — navigation can open
        /// the replica without waiting for a hierarchy reload. `None` only
        /// for callers that do not know the id (pre-existing tests).
        namespace_id: Option<String>,
    },
    FractalCreated {
        id: String,
        verse_id: String,
        name: String,
    },
    PetalCreated {
        id: String,
        fractal_id: String,
        name: String,
    },
    NodeCreated {
        id: String,
        petal_id: String,
        name: String,
        has_asset: bool,
        /// Echoes the originating `CreateNode.correlation_id` (FR-4); `None`
        /// when the command carried none.
        correlation_id: Option<String>,
        /// Echoes the originating `CreateNode.position` (camera_focus_clip_20260716
        /// FR-1) so callers don't have to wait for a hierarchy reload to know
        /// where the node actually landed.
        position: [f32; 3],
    },
    /// Native import failure retains its originating dialog ownership.
    ImportGltfFailed {
        request: Option<NativeImportRequest>,
        message: String,
    },
    GltfImported {
        request: Option<NativeImportRequest>,
        node_id: String,
        asset_id: String,
        petal_id: String,
        name: String,
        asset_path: String,
        position: [f32; 3],
        /// Full committed birth; absent legacy adapters require authoritative hydration.
        materialization: Option<CommittedNodeMaterialization>,
        /// Echoes `CreateNodeWithAsset.correlation_id`; `None` for GUI
        /// `ImportGltf` results (never routed to an API waiter).
        correlation_id: Option<String>,
    },
    /// Result of `CreateAsset` — the asset row now references the blob.
    AssetCreated {
        asset_id: String,
        content_hash: String,
        size_bytes: u64,
        correlation_id: Option<String>,
    },
    /// Result of `GcUnreferencedAssets` (uncorrelated; no API waiter).
    AssetsGarbageCollected {
        removed_assets: u64,
        removed_blobs: u64,
    },
    HierarchyLoaded {
        verses: Vec<VerseHierarchyData>,
    },
    /// Phase F: a verse invite string was generated.
    VerseInviteGenerated {
        verse_id: String,
        invite_string: String,
    },
    /// Phase F: successfully joined a verse via invite.
    VerseJoined {
        verse_id: String,
        verse_name: String,
    },
    /// Local database rows were cleared; reload even if restoring defaults failed.
    DatabaseReset {
        petal_name: String,
        rooms: Vec<String>,
    },
    /// Exact local reset outcome; a failed seed can follow a committed clear.
    DevelopmentDatabaseReset { request_id: String, cleared: bool, error: Option<String> },
    TerminalDevelopmentReset { request_id: String, cleared: bool, error: Option<String> },
    // --- Entity Settings results ---
    EntityDetailsLoaded {
        entity_type: EntityType,
        entity_id: String,
        request_id: u64,
        details: Result<EntityDetails, String>,
    },
    EntityRenamed {
        entity_type: EntityType,
        entity_id: String,
        new_name: String,
    },
    VerseDefaultAccessSet {
        verse_id: String,
        default_access: String,
    },
    /// Echo of `SetVerseTimeseriesSettings` — the authoritative persisted
    /// values (the handler sanitizes), for the settings surface to display.
    VerseTimeseriesSettingsSet {
        verse_id: String,
        mode: String,
        replication_factor: u32,
        bucket_width_ms: u64,
    },
    FractalDescriptionUpdated {
        fractal_id: String,
        description: String,
    },
    EntityDeleted {
        entity_type: EntityType,
        entity_id: String,
    },
    EntityAccessFinished {
        target: crate::entity_access::EntityAccessTarget,
        request_id: String,
        result: Result<crate::entity_access::EntityAccessSnapshot, String>,
    },
    JoinInvitePrepared {
        request_id: String,
        result: Result<crate::join_invite::InviteSource, String>,
    },
    JoinInviteIssued {
        request_id: String,
        result: Result<crate::join_invite::IssuedInvitation, String>,
    },
    JoinInviteAdmitted {
        request_id: String,
        result: Result<crate::join_invite::JoinAdmission, String>,
    },
    JoinReadinessChecked {
        request_id: String,
        result: Result<bool, String>,
    },
    JoinProofFinalized {
        request_id: String,
        result: Result<(), String>,
    },
    PeerRolesResolved {
        scope: String,
        roles: Vec<(String, String)>,
    },
    RoleAssigned {
        peer_did: String,
        scope: String,
        role: String,
    },
    RoleRevoked {
        peer_did: String,
        scope: String,
    },
    ScopedInviteGenerated {
        invite_link: String,
    },
    LocalRoleResolved {
        scope: String,
        role: String,
    },
    // --- API Token results ---
    ApiTokenMinted {
        correlation_id: Option<String>,
        token: String,
        jti: String,
        scope: String,
        max_role: String,
        expires_at: String,
        label: Option<String>,
        /// The minted token's `client` claim (requested or freshly generated).
        client: String,
    },
    ApiTokenRevoked {
        jti: String,
    },
    ApiTokensListed {
        tokens: Vec<ApiTokenInfo>,
        total: u64,
    },
    /// Tokens listed for an admin scope view (separate from user's own tokens).
    ScopedApiTokensListed {
        tokens: Vec<ApiTokenInfo>,
        total: u64,
    },
    /// Result of `ResolvePetalScope` / `ResolveNodeScope` / `ResolveFractalScope` /
    /// `ResolveVerseScope`.
    /// `scope` is `None` when the requested entity was not found.
    ScopeResolved {
        scope: Option<String>,
    },
    /// Result of `LoadNodesByPetal` — all nodes in a petal as DTOs.
    NodesLoaded {
        petal_id: String,
        nodes: Vec<NodeDto>,
    },
    /// Result of `GetNodeTransform` — a single node's persisted transform.
    NodeTransformLoaded {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Result of `GetNodeProperties`.
    NodePropertiesLoaded {
        node_id: String,
        properties: serde_json::Value,
        correlation_id: Option<String>,
    },
    /// Result of `SetNodeProperty`.
    NodePropertySet {
        node_id: String,
        key: String,
        correlation_id: Option<String>,
    },
    /// Result of `DeleteNodeProperty`.
    NodePropertyDeleted {
        node_id: String,
        key: String,
        correlation_id: Option<String>,
    },
    /// Result of `DeleteNode` — the node and its cascaded waypoints are gone.
    NodeDeleted {
        node_id: String,
        petal_id: String,
    },
    /// Result of `PromoteInstance` (FR-5) — a stamp instance became a full node.
    CheckedStampPromoted {
        capture: CheckedStampPromotion,
        node_id: String,
        properties: serde_json::Value,
        newly_promoted: bool,
        correlation_id: String,
    },
    NodePromoted {
        node_id: String,
        petal_id: String,
        path_id: String,
        instance_index: u32,
        /// `true` when this call materialized a new row; `false` when the
        /// instance was already promoted (idempotent no-op).
        newly_promoted: bool,
    },
    /// Result of `RenameNode` — the node's `display_name` was updated.
    NodeRenamed {
        node_id: String,
        new_name: String,
    },
    /// Local URL write receipt after the authorized transaction commits.
    NodeUrlUpdated {
        node_id: String,
        request_id: String,
        expected_scope: String,
        outcome: Result<Option<String>, String>,
        asset_id: Option<String>,
    },
    NodeUrlRead {
        node_id: String,
        expected_scope: String,
        request_id: String,
        outcome: Result<NodeUrlSnapshot, String>,
    },
    /// Result of `CountNodeDescendants` — live descendants, root excluded.
    NodeDescendantCount {
        node_id: String,
        count: usize,
    },
    /// Result of `SetNodeView` — the persisted view properties after the write
    /// (`state: None` = the key is absent, i.e. Auto).
    NodeViewSet {
        node_id: String,
        petal_id: String,
        state: Option<String>,
        groups: Option<Vec<String>>,
        /// Echoes `SetNodeView.correlation_id`.
        correlation_id: Option<String>,
    },
    /// Result of `LoadPetalViewState` — every live node with a persisted view.
    PetalViewStateLoaded {
        petal_id: String,
        entries: Vec<NodeViewDto>,
    },
    /// The stored visibility-Group registry of `petal_id` — the reply to both
    /// `GetPetalVisibilityGroups` and `SetPetalVisibilityGroups` (the echo).
    PetalVisibilityGroupsLoaded {
        petal_id: String,
        groups: Vec<VisibilityGroupDto>,
        correlation_id: Option<String>,
    },
    /// Result of `SetScopeHidden` — the persisted eye.
    ScopeHiddenSet {
        level: ScopeLevel,
        id: String,
        hidden: bool,
        correlation_id: Option<String>,
    },
    // --- Field definition results ---
    /// Result of `CreateFieldDef`.
    FieldDefCreated {
        field_def_id: String,
        scope: String,
        key: String,
    },
    /// Result of `ListFieldDefs`.
    FieldDefsListed {
        scope: String,
        field_defs: Vec<FieldDefInfo>,
    },
    /// Result of `UpdateFieldDef`.
    FieldDefUpdated {
        field_def_id: String,
    },
    /// Result of `DeleteFieldDef`.
    FieldDefDeleted {
        field_def_id: String,
    },
    /// Result of `RawQuery`.
    QueryResult {
        data: Vec<serde_json::Value>,
        /// Echoes `RawQuery.correlation_id` (`None` = GUI-originated).
        correlation_id: Option<String>,
    },
    /// Result of `ElevatedQuery`: the caller's statements' rows and how many
    /// rows the script changed (a GUI host reloads its tree when > 0).
    ElevatedQueryResult {
        data: Vec<serde_json::Value>,
        changed_rows: usize,
        correlation_id: String,
    },
    /// Failure of a CORRELATED (API-originated) `RawQuery`. GUI queries
    /// (`correlation_id: None`) keep failing with `DbResult::Error`.
    QueryFailed {
        error: String,
        correlation_id: String,
    },
    /// Result of `InsertIotReadings` — how many rows the batch wrote.
    IotReadingsInserted {
        petal_id: String,
        written: usize,
        /// Echoes `InsertIotReadings.correlation_id`.
        correlation_id: Option<String>,
    },
    /// Validation rejection of `InsertIotReadings` (F24) — same reply family
    /// as `IotReadingsInserted` (the API fallback distinguishes 422-class
    /// validation failures from 502-class `DbResult::Error` DB failures
    /// without string-sniffing).
    IotReadingsRejected {
        petal_id: String,
        reason: IotIngestRejection,
        /// Echoes `InsertIotReadings.correlation_id`.
        correlation_id: Option<String>,
    },
    // --- R5 device formats / payloads results (CDL-SPEC10 A3) ---
    /// Result of `RegisterDeviceFormat`.
    DeviceFormatRegistered {
        verse_id: String,
        format_id: String,
        correlation_id: Option<String>,
    },
    /// Validation rejection of `RegisterDeviceFormat` (F24 pattern).
    DeviceFormatRejected {
        verse_id: String,
        reason: DeviceFormatRejection,
        correlation_id: Option<String>,
    },
    /// Result of `ListDeviceFormats`.
    DeviceFormatsListed {
        verse_id: String,
        formats: Vec<DeviceFormatDto>,
        correlation_id: Option<String>,
    },
    /// Result of `InsertDevicePayloads` — one row written (its id + ordered
    /// asset ids), or the already-existing row on an idempotent retry.
    DevicePayloadsInserted {
        petal_id: String,
        payload_id: String,
        asset_ids: Vec<String>,
        correlation_id: Option<String>,
    },
    /// Validation rejection of `InsertDevicePayloads` (F24 pattern).
    DevicePayloadsRejected {
        petal_id: String,
        reason: DevicePayloadIngestRejection,
        correlation_id: Option<String>,
    },
    /// Result of `ListDevicePayloads`.
    DevicePayloadsListed {
        petal_id: String,
        payloads: Vec<DevicePayloadDto>,
        correlation_id: Option<String>,
    },
    // --- Hexon crate registry results (Phase 8) ---
    /// Result of `InstallCrate`.
    CrateInstalled {
        hexon_uri: String,
        petal_id: String,
    },
    /// Result of `InstallCrateEntry`.
    CrateEntryInstalled {
        entry_id: String,
        hexon_uri: String,
    },
    /// Result of `UninstallCrate`.
    CrateUninstalled {
        hexon_uri: String,
    },
    /// Authoritative declaration readback for one exact UI request.
    PetalRequirementsLoaded {
        snapshot: crate::petal_requirements::PetalRequirementsSnapshot,
        correlation_id: String,
    },
    /// Result of Get/SetPetalTerrain — authoritative petal terrain JSON (None = no terrain).
    PetalTerrainLoaded {
        petal_id: String,
        terrain: Option<serde_json::Value>,
        /// Echoes `SetPetalTerrain.correlation_id` (`None` = GUI set or any Get).
        correlation_id: Option<String>,
    },
    /// Failure of a CORRELATED (API-originated) `SetPetalTerrain`. GUI sets
    /// (`correlation_id: None`) keep failing with `DbResult::Error`.
    PetalTerrainSetFailed {
        petal_id: String,
        error: String,
        correlation_id: String,
    },
    /// Reply to a CORRELATED `GetPetalTerrain` — private to the API caller, so
    /// GUI terrain consumers (which match `PetalTerrainLoaded`) never re-apply it.
    PetalTerrainFetched {
        petal_id: String,
        terrain: Option<serde_json::Value>,
        correlation_id: String,
    },
    /// Reply to both `SetPetalRenderDistance` and `GetPetalRenderDistance`
    /// (the echo); `None` = inherit the global.
    PetalRenderDistanceLoaded {
        petal_id: String,
        render_distance: Option<f32>,
        correlation_id: Option<String>,
    },
    /// `ClaimOrphanedVerse` succeeded: the verse's `created_by` now reads
    /// `new_did` (always this node's local DID today).
    OrphanedVerseClaimed {
        verse_id: String,
        old_did: String,
        new_did: String,
        correlation_id: Option<String>,
    },
    /// `ClaimOrphanedVerse` was refused before any write — `reason` is a
    /// human-readable rendering of `ClaimRefusalReason` (fe-database
    /// `handlers::claim_orphaned_verse`); OF-4 stays dead regardless of reason.
    OrphanedVerseClaimRefused {
        verse_id: String,
        reason: String,
        correlation_id: Option<String>,
    },
    /// Typed failure of a correlated command whose family has no
    /// family-specific failure variant; the GUI never sees it, the API caller
    /// maps it to its error status.
    RequestFailed {
        error: String,
        correlation_id: String,
    },
    /// Reply to `EnrolLocalDevice`: the row the slot holds, already published.
    LocalDeviceEnrolled {
        verse_id: String,
        record_id_hex: String,
        principal_did: String,
        device_key_hex: String,
        correlation_id: Option<String>,
    },
    /// Reply to `RevokeLocalDevice`: the tombstone the slot holds, already published.
    LocalDeviceRevoked {
        verse_id: String,
        record_id_hex: String,
        device_key_hex: String,
        correlation_id: Option<String>,
    },
    /// Result of `ApplyReplicatedRow` — how the DB thread reconciled one
    /// inbound replicated row (A4).
    ReplicatedRowApplied {
        verse_id: String,
        table: String,
        record_id: String,
        outcome: ReplicatedRowOutcome,
    },
    /// Typed failure of `JoinVerseByInvite` (polish_lane_20261031 OD-2/M-03):
    /// GUI-only today (no correlation id — `JoinVerseByInvite` carries none),
    /// so this always reaches fe-ui's dispatcher, never an API waiter. Lets
    /// the Join dialog show the failure inline instead of a generic status-bar
    /// `Error` (see fe-ui `verse_manager/AGENTS.md` §db-results).
    JoinVerseFailed {
        error: String,
    },
}

/// Field definition info for UI display.
#[derive(Debug, Clone)]
pub struct FieldDefInfo {
    pub field_def_id: String,
    pub scope: String,
    pub entity_type: String,
    pub key: String,
    pub value_type: String,
    pub default_val: Option<serde_json::Value>,
    pub created_by: String,
    pub created_at: String,
}

/// Lightweight API token info for UI display (avoids exposing the actual JWT).
#[derive(Debug, Clone)]
pub struct ApiTokenInfo {
    pub jti: String,
    pub scope: String,
    pub max_role: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub revoked: bool,
    /// DID of the node that minted this token.
    pub sub: String,
    /// The token's `client` claim; `None` for a legacy (node-level) token.
    pub client: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VerseHierarchyData {
    pub id: String,
    pub name: String,
    /// Phase E: hex-encoded namespace ID for the verse's iroh-docs replica.
    pub namespace_id: Option<String>,
    /// M2/F6: the verse's timeseries replication settings (ts_* columns),
    /// so the settings surface and the UI tree speak the persisted truth.
    pub timeseries: crate::timeseries::VerseTimeseriesSettings,
    /// Persisted verse eye (`verse.view_hidden`, VIS-PH4); `false` = visible.
    pub view_hidden: bool,
    pub fractals: Vec<FractalHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct FractalHierarchyData {
    pub id: String,
    pub name: String,
    /// Persisted fractal eye (`fractal.view_hidden`); `false` = visible.
    pub view_hidden: bool,
    pub petals: Vec<PetalHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct PetalHierarchyData {
    pub id: String,
    pub name: String,
    /// Persisted petal eye (`petal.view_hidden`); `false` = visible.
    pub view_hidden: bool,
    /// Durable per-petal residency horizon (p2p_asset_streaming S6);
    /// `None` = inherit the global `AppSettings.render_distance`.
    pub render_distance: Option<f32>,
    pub nodes: Vec<NodeHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct NodeHierarchyData {
    pub id: String,
    pub name: String,
    pub has_asset: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub asset_path: Option<String>,
    /// The petal that owns this node — used by the UI to scope scene entity
    /// spawn/despawn to the currently active petal.
    pub petal_id: String,
    /// Portal URL from the node's associated model record, if any.
    pub webpage_url: Option<String>,
}

/// Local Portal URL persistence availability for one exact node/model read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeUrlStorage {
    Ready,
    MissingModel,
    NoAsset,
    SharedModel,
}

impl NodeUrlStorage {
    pub fn supports_save(self) -> bool {
        matches!(self, Self::Ready | Self::MissingModel)
    }

    pub fn explanation(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::MissingModel => Some("This object has no local Portal metadata. Saving a nonempty URL creates it for this object; clearing an absent URL makes no change."),
            Self::NoAsset => Some("Portal URL storage is unavailable for this object because it has no model asset."),
            Self::SharedModel => Some("Portal URL storage is unavailable because this asset is shared by other objects. Choose an object with an independent asset."),
        }
    }
}

/// Exact node/model identity and bounded current URL from one authorized read.
#[derive(Debug, Clone)]
pub struct NodeUrlSnapshot {
    pub storage: NodeUrlStorage,
    pub node_id: String,
    pub scope: String,
    pub asset_id: Option<String>,
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Bevy components
// ---------------------------------------------------------------------------

/// Bevy component: last transform value confirmed by the DB.
/// Used alongside the live `Transform` for optimistic update + rollback.
/// Attach to any entity whose transform is persisted via `TransformPersist`.
#[derive(Debug, Clone, bevy::prelude::Component)]
pub struct DbConfirmedTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

// ---------------------------------------------------------------------------
// Scene graph change events (Phase 3: scene streaming)
// ---------------------------------------------------------------------------

/// One visibility Group of a petal's registry (hierarchy_visibility_groups
/// D-3). Membership is NOT here — it is each node's `view.groups`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VisibilityGroupDto {
    pub id: String,
    pub name: String,
    pub visible: bool,
    /// Display order (ascending); any finite number.
    pub order: f64,
}

/// Which hierarchy level a persisted eye (`SetScopeHidden`) addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ScopeLevel {
    Verse,
    Fractal,
    Petal,
}

/// One node's persisted visibility view (`view.state` / `view.groups`);
/// `None` = the property is absent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeViewDto {
    pub node_id: String,
    pub state: Option<String>,
    pub groups: Option<Vec<String>>,
}

/// A DTO for node data sent over the scene streaming protocol.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeDto {
    pub node_id: String,
    pub petal_id: String,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub has_asset: bool,
    pub asset_path: Option<String>,
}

/// Authoritative ordinary object birth for scene/tree/property hydration.
#[derive(Debug, Clone)]
pub struct CommittedNodeMaterialization {
    pub node: NodeDto,
    /// Euler XYZ radians, matching the committed database transform.
    pub rotation_euler: [f32; 3],
    pub asset_id: Option<String>,
    pub properties: serde_json::Value,
    pub history_version: i64,
}

/// Describes a single change to the scene graph.
///
/// Each variant maps to a CUD operation on the node table. Serialized with
/// `serde(tag = "op")` so the JSON looks like `{"op": "node_added", ...}`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneChange {
    /// A new node was created in a petal.
    NodeAdded { node: NodeDto },
    /// A node was removed from its owning petal.
    NodeRemoved { node_id: String, petal_id: String },
    /// A node was renamed.
    NodeRenamed {
        node_id: String,
        petal_id: String,
        new_name: String,
    },
    /// A node's transform was updated.
    NodeTransform {
        node_id: String,
        petal_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// A transform persist failed — the optimistic update should be rolled back.
    /// Contains the last-known-good values read from the DB before the failed write.
    TransformFailed {
        node_id: String,
        petal_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// A custom property was changed on a node.
    PropertyChanged {
        node_id: String,
        petal_id: String,
        key: String,
        value: serde_json::Value,
    },
}

impl SceneChange {
    /// Returns the petal that owns this node-scoped change.
    pub fn petal_id(&self) -> &str {
        match self {
            Self::NodeAdded { node } => &node.petal_id,
            Self::NodeRemoved { petal_id, .. }
            | Self::NodeRenamed { petal_id, .. }
            | Self::NodeTransform { petal_id, .. }
            | Self::TransformFailed { petal_id, .. }
            | Self::PropertyChanged { petal_id, .. } => petal_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Node lifecycle events (node_lifecycle_addressing_20260725 FR-6)
// ---------------------------------------------------------------------------

/// Program-wide node lifecycle events emitted on the op-log/replication seam so
/// sync and reporting (T5) observe the same truth (FR-6). The node-scoped
/// variants carry the node's stable `fe://verse/fractal/petal/node` address
/// (FR-4); `PathReflow` carries the owning path's node id, which is itself an
/// addressable node. Wave-1: T2 consumes `NodePromoted`/`PathReflow`, T5 reports
/// all variants. Serialized with `serde(tag = "lifecycle")` →
/// `{"lifecycle": "node_created", ...}`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Message)]
#[serde(tag = "lifecycle", rename_all = "snake_case")]
pub enum LifecycleEvent {
    /// A node was created (empty or with an asset).
    NodeCreated { address: String, node_id: String },
    /// A stamp instance was promoted to a full addressable node (FR-5).
    NodePromoted {
        address: String,
        node_id: String,
        path_id: String,
        instance_index: u32,
    },
    /// A node was deleted via tombstone — sync-safe, survives merge (FR-1).
    NodeDeleted { address: String, node_id: String },
    /// A node's `display_name` changed — metadata only, address unchanged.
    NodeRenamed { address: String, node_id: String },
    /// A stamp delete asks the owning path to re-flow its remaining stamps
    /// (FR-2). Geometry re-flow is T2's; this event is the lifecycle hook only.
    PathReflow {
        path_id: String,
        /// The deleted stamp's `instance_index`, when the tombstoned node was a
        /// promoted stamp. Optional + defaulted so pre-field payloads still parse.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deleted_index: Option<u32>,
    },
}

impl LifecycleEvent {
    /// The stable `fe://` address the event refers to, when the variant carries
    /// one. `PathReflow` returns `None` (it references a path by node id).
    pub fn address(&self) -> Option<&str> {
        match self {
            LifecycleEvent::NodeCreated { address, .. }
            | LifecycleEvent::NodePromoted { address, .. }
            | LifecycleEvent::NodeDeleted { address, .. }
            | LifecycleEvent::NodeRenamed { address, .. } => Some(address),
            LifecycleEvent::PathReflow { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_command_debug_clone() {
        let _ = format!("{:?}", NetworkCommand::Ping.clone());
        let _ = format!("{:?}", NetworkCommand::Shutdown.clone());
    }

    #[test]
    fn test_network_event_debug_clone() {
        let _ = format!("{:?}", NetworkEvent::Pong.clone());
        let _ = format!("{:?}", NetworkEvent::Started.clone());
        let _ = format!("{:?}", NetworkEvent::Stopped.clone());
    }

    #[test]
    fn test_db_command_debug_clone() {
        let _ = format!("{:?}", DbCommand::Ping.clone());
        let _ = format!("{:?}", DbCommand::Shutdown.clone());
    }

    #[test]
    fn test_db_result_debug_clone() {
        let _ = format!("{:?}", DbResult::Pong.clone());
        let _ = format!("{:?}", DbResult::Started.clone());
        let _ = format!("{:?}", DbResult::Stopped.clone());
        let _ = format!("{:?}", DbResult::Error("test".to_string()).clone());
    }

    #[test]
    fn test_network_channel_send_recv_roundtrip() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(NetworkCommand::Ping).unwrap();
        assert!(matches!(rx.recv().unwrap(), NetworkCommand::Ping));
    }

    #[test]
    fn test_db_channel_send_recv_roundtrip() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(DbResult::Pong).unwrap();
        assert!(matches!(rx.recv().unwrap(), DbResult::Pong));
    }

    #[test]
    fn test_new_db_command_variants_debug_clone() {
        let cmd = DbCommand::RenameEntity {
            entity_type: EntityType::Verse,
            entity_id: "v1".to_string(),
            new_name: "New Name".to_string(),
        };
        let _ = format!("{:?}", cmd.clone());

        let cmd2 = DbCommand::ResolveRolesForPeers {
            scope: "VERSE#v1".to_string(),
            peer_dids: vec!["did:key:z6Mk...".to_string()],
        };
        let _ = format!("{:?}", cmd2.clone());
    }

    #[test]
    fn test_new_db_result_variants_debug_clone() {
        let r = DbResult::EntityRenamed {
            entity_type: EntityType::Verse,
            entity_id: "v1".to_string(),
            new_name: "New".to_string(),
        };
        let _ = format!("{:?}", r.clone());

        let r2 = DbResult::PeerRolesResolved {
            scope: "VERSE#v1".to_string(),
            roles: vec![("did:key:z6Mk".to_string(), "editor".to_string())],
        };
        let _ = format!("{:?}", r2.clone());
    }
}

#[cfg(test)]
mod scene_change_tests {
    use super::*;

    #[test]
    fn scene_change_node_added_serde_roundtrip() {
        let change = SceneChange::NodeAdded {
            node: NodeDto {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test Node".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_added\""));
        let deserialized: SceneChange = serde_json::from_str(&json).unwrap();
        match deserialized {
            SceneChange::NodeAdded { node } => assert_eq!(node.node_id, "n1"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scene_change_node_removed_serde_roundtrip() {
        let change = SceneChange::NodeRemoved {
            node_id: "n1".into(),
            petal_id: "p1".into(),
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_removed\""));
        let deserialized: SceneChange = serde_json::from_str(&json).unwrap();
        match deserialized {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "n1");
                assert_eq!(petal_id, "p1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scene_change_node_renamed_serde_roundtrip() {
        let change = SceneChange::NodeRenamed {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            new_name: "New Name".into(),
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_renamed\""));
        let _: SceneChange = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn scene_change_node_transform_serde_roundtrip() {
        let change = SceneChange::NodeTransform {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_transform\""));
        let _: SceneChange = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn scene_change_attribution_is_available_for_every_variant() {
        let changes = [
            SceneChange::NodeRemoved {
                node_id: "n1".into(),
                petal_id: "p1".into(),
            },
            SceneChange::NodeRenamed {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                new_name: "N2".into(),
            },
            SceneChange::NodeTransform {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                position: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
            SceneChange::TransformFailed {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                position: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
            SceneChange::PropertyChanged {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                key: "opacity".into(),
                value: serde_json::Value::Null,
            },
        ];

        for change in changes {
            assert_eq!(change.petal_id(), "p1");
        }
    }

    #[test]
    fn scene_change_broadcast_send_recv() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<SceneChange>(16);
        let change = SceneChange::NodeRemoved {
            node_id: "test".into(),
            petal_id: "p1".into(),
        };
        tx.send(change.clone()).unwrap();
        let received = rx.try_recv().unwrap();
        match received {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "test");
                assert_eq!(petal_id, "p1");
            }
            _ => panic!("wrong variant"),
        }
    }
}

#[cfg(test)]
mod lifecycle_vocabulary_tests {
    use super::*;

    #[test]
    fn new_db_command_variants_debug_clone() {
        for cmd in [
            DbCommand::TombstoneNode {
                node_id: "n1".into(),
                auth: CallerAuth::identified("did:key:z6MkA", "editor"),
            },
            DbCommand::CascadeTombstoneNode {
                node_id: "n1".into(),
                auth: CallerAuth::Anonymous,
            },
            DbCommand::PromoteInstance {
                petal_id: "p1".into(),
                path_id: "path1".into(),
                instance_index: 7,
                auth: CallerAuth::identified("did:key:z6MkA", "editor"),
            },
            DbCommand::RenameNode {
                node_id: "n1".into(),
                new_name: "N2".into(),
                auth: CallerAuth::Local,
            },
            DbCommand::DuplicateNode {
                node_id: "n1".into(),
                auth: CallerAuth::identified("did:key:z6MkA", "editor"),
            },
            DbCommand::CountNodeDescendants {
                node_id: "n1".into(),
            },
            DbCommand::SetNodeView {
                node_id: "n1".into(),
                state: Some("hide".into()),
                groups: None,
                auth: CallerAuth::Local,
                correlation_id: None,
            },
            DbCommand::LoadPetalViewState {
                petal_id: "p1".into(),
            },
            DbCommand::SetPetalVisibilityGroups {
                petal_id: "p1".into(),
                groups: vec![VisibilityGroupDto {
                    id: "g1".into(),
                    name: "G".into(),
                    visible: false,
                    order: 0.0,
                }],
                auth: CallerAuth::Local,
                correlation_id: None,
            },
            DbCommand::GetPetalVisibilityGroups {
                petal_id: "p1".into(),
                correlation_id: Some("c".into()),
            },
            DbCommand::SetScopeHidden {
                level: ScopeLevel::Fractal,
                id: "f1".into(),
                hidden: true,
                auth: CallerAuth::Local,
                correlation_id: None,
            },
        ] {
            let _ = format!("{:?}", cmd.clone());
        }
    }

    #[test]
    fn caller_auth_round_trips_and_reports_did() {
        let id = CallerAuth::identified("did:key:z6MkA", "editor");
        assert_eq!(id.did(), Some("did:key:z6MkA"));
        // Local + Anonymous assert no DID — the DB resolves Local's identity.
        assert_eq!(CallerAuth::local().did(), None);
        assert_eq!(CallerAuth::Anonymous.did(), None);
        let json = serde_json::to_string(&id).unwrap();
        let back: CallerAuth = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        // The `Local` unit variant round-trips over the wire too.
        let local = CallerAuth::Local;
        let back: CallerAuth =
            serde_json::from_str(&serde_json::to_string(&local).unwrap()).unwrap();
        assert_eq!(local, back);
    }

    #[test]
    fn node_promoted_result_debug_clone() {
        let r = DbResult::NodePromoted {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            path_id: "path1".into(),
            instance_index: 3,
            newly_promoted: true,
        };
        let _ = format!("{:?}", r.clone());
    }

    #[test]
    fn rename_and_count_results_debug_clone() {
        let r = DbResult::NodeRenamed {
            node_id: "n1".into(),
            new_name: "N2".into(),
        };
        let _ = format!("{:?}", r.clone());
        let c = DbResult::NodeDescendantCount {
            node_id: "n1".into(),
            count: 4,
        };
        let _ = format!("{:?}", c.clone());
    }

    #[test]
    fn lifecycle_event_carries_address_and_round_trips() {
        let events = [
            LifecycleEvent::NodeCreated {
                address: "fe://v1/f1/p1/n1".into(),
                node_id: "n1".into(),
            },
            LifecycleEvent::NodePromoted {
                address: "fe://v1/f1/p1/path1%23inst-3".into(),
                node_id: "path1#inst-3".into(),
                path_id: "path1".into(),
                instance_index: 3,
            },
            LifecycleEvent::NodeDeleted {
                address: "fe://v1/f1/p1/n1".into(),
                node_id: "n1".into(),
            },
            LifecycleEvent::NodeRenamed {
                address: "fe://v1/f1/p1/n1".into(),
                node_id: "n1".into(),
            },
        ];
        for ev in events {
            // Every node-scoped variant must carry the stable address (FR-6).
            assert!(ev.address().is_some(), "node event must carry address");
            let json = serde_json::to_string(&ev).unwrap();
            assert!(json.contains("\"lifecycle\":"), "tagged repr: {json}");
            let back: LifecycleEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back.address(), ev.address());
        }
    }

    #[test]
    fn path_reflow_has_no_address_but_carries_path_id() {
        let ev = LifecycleEvent::PathReflow {
            path_id: "path1".into(),
            deleted_index: None,
        };
        assert!(ev.address().is_none());
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"lifecycle\":\"path_reflow\""), "{json}");
        // `None` is skipped on the wire — the legacy payload shape is preserved.
        assert!(!json.contains("deleted_index"), "{json}");
        let back: LifecycleEvent = serde_json::from_str(&json).unwrap();
        match back {
            LifecycleEvent::PathReflow {
                path_id,
                deleted_index,
            } => {
                assert_eq!(path_id, "path1");
                assert_eq!(deleted_index, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn path_reflow_deleted_index_round_trips_and_old_payloads_parse() {
        let ev = LifecycleEvent::PathReflow {
            path_id: "path1".into(),
            deleted_index: Some(4),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"deleted_index\":4"), "{json}");
        let back: LifecycleEvent = serde_json::from_str(&json).unwrap();
        match back {
            LifecycleEvent::PathReflow { deleted_index, .. } => {
                assert_eq!(deleted_index, Some(4));
            }
            _ => panic!("wrong variant"),
        }
        // Back-compat: a pre-field payload (no `deleted_index`) still parses.
        let old = r#"{"lifecycle":"path_reflow","path_id":"path1"}"#;
        let back: LifecycleEvent = serde_json::from_str(old).unwrap();
        match back {
            LifecycleEvent::PathReflow {
                path_id,
                deleted_index,
            } => {
                assert_eq!(path_id, "path1");
                assert_eq!(deleted_index, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn node_renamed_event_round_trips() {
        let ev = LifecycleEvent::NodeRenamed {
            address: "fe://v1/f1/p1/n1".into(),
            node_id: "n1".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"lifecycle\":\"node_renamed\""), "{json}");
        let back: LifecycleEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.address(), Some("fe://v1/f1/p1/n1"));
    }
}
