#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PetalId(pub ulid::Ulid);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoleId(pub String);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OpType {
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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpLogEntry {
    pub lamport_clock: u64,
    pub node_id: NodeId,
    pub op_type: OpType,
    pub payload: serde_json::Value,
    pub sig: String, // hex-encoded ed25519 signature bytes
}
