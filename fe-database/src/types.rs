#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PetalId(pub ulid::Ulid);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoleId(pub String);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OpType {
    /// One admitted atomic batch from a trusted plugin instance.
    PluginTransaction,
    CreatePetal,
    CreateRoom,
    PlaceModel,
    AssignRole,
    RevokeSession,
    DeletePetal,
    // Atlas op-log variants (Fractal Atlas track)
    UpdatePetalMeta,
    UpdateRoomMeta,
    UpdateModelMeta,
    ExportPetal,
    ImportPetal,
    TransformUpdate,
    PropertySet,
    PropertyDeleted,
    /// Legacy `CreateNode` materialized a durable node row.
    NodeCreated,
    /// Legacy `DeleteNode` physically removed a node row and direct waypoints.
    NodeDeleted,
    // Node lifecycle (node_lifecycle_addressing_20260725)
    /// FR-1: a node was deleted via a sync-safe tombstone (survives P2P merge).
    NodeTombstoned,
    /// FR-5: a stamp instance was promoted to a full addressable node.
    NodePromoted,
    /// An Owner's `/query/elevated` write script (fe-api AGENTS.md §elevated-query).
    ElevatedQuery,
    // p2p_asset_streaming_20260718 S6: petal-scoped settings writers, both
    // op-logged before the row mutates (src/AGENTS.md §log-first-commit).
    /// `SetPetalRenderDistance` — the petal's durable residency horizon.
    SetPetalRenderDistance,
    /// `SetPetalTerrain` — closes the parity gap (campaign plan §10 C7):
    /// previously this write neither replicated nor op-logged.
    SetPetalTerrain,
    /// Petal declaration metadata, independent of residency settings.
    SetPetalRequirements,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpLogEntry {
    pub lamport_clock: u64,
    /// HLC timestamp string in the format `"<wall_ms>:<counter_hex>"`.
    /// Populated by [`crate::op_log::next_hlc_timestamp`] before persistence.
    #[serde(default)]
    pub hlc_timestamp: String,
    pub node_id: NodeId,
    pub op_type: OpType,
    pub payload: serde_json::Value,
    pub sig: String, // hex-encoded ed25519 signature bytes
}
