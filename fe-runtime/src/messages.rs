use bevy::prelude::Message;

/// JTI string broadcast for token revocation notifications.
/// Carried by the API config so the validation middleware can invalidate
/// in-process caches without a DB round-trip.
pub type RevocationNotifier = tokio::sync::broadcast::Sender<String>;

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
// Database command/result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum DbCommand {
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
    },
    ImportGltf {
        petal_id: String,
        name: String,
        file_path: String,
        position: [f32; 3],
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
    },
    // --- Entity Settings: General config ---
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
        scope: String,
        max_role: String,
        ttl_hours: u32,
        label: Option<String>,
    },
    /// Revoke an API token by JTI. Ownership is enforced server-side.
    RevokeApiToken {
        jti: String,
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
    /// Load all nodes belonging to a petal (for scene snapshot).
    LoadNodesByPetal {
        petal_id: String,
    },
    /// Read a single node's persisted transform (position/rotation/scale).
    GetNodeTransform {
        node_id: String,
    },
}

#[derive(Debug, Clone, Message)]
pub enum DbResult {
    Pong,
    Seeded {
        petal_name: String,
        rooms: Vec<String>,
    },
    Started,
    Stopped,
    Error(String),
    VerseCreated {
        id: String,
        name: String,
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
    },
    GltfImported {
        node_id: String,
        asset_id: String,
        petal_id: String,
        name: String,
        asset_path: String,
        position: [f32; 3],
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
    /// Database was fully reset (tables wiped, schema re-applied, re-seeded).
    DatabaseReset {
        petal_name: String,
        rooms: Vec<String>,
    },
    // --- Entity Settings results ---
    EntityRenamed {
        entity_type: EntityType,
        entity_id: String,
        new_name: String,
    },
    VerseDefaultAccessSet {
        verse_id: String,
        default_access: String,
    },
    FractalDescriptionUpdated {
        fractal_id: String,
        description: String,
    },
    EntityDeleted {
        entity_type: EntityType,
        entity_id: String,
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
        token: String,
        jti: String,
        scope: String,
        max_role: String,
        expires_at: String,
        label: Option<String>,
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
    /// Result of `ResolvePetalScope` or `ResolveNodeScope`.
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
}

#[derive(Debug, Clone)]
pub struct VerseHierarchyData {
    pub id: String,
    pub name: String,
    /// Phase E: hex-encoded namespace ID for the verse's iroh-docs replica.
    pub namespace_id: Option<String>,
    pub fractals: Vec<FractalHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct FractalHierarchyData {
    pub id: String,
    pub name: String,
    pub petals: Vec<PetalHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct PetalHierarchyData {
    pub id: String,
    pub name: String,
    pub nodes: Vec<NodeHierarchyData>,
}

#[derive(Debug, Clone)]
pub struct NodeHierarchyData {
    pub id: String,
    pub name: String,
    pub has_asset: bool,
    pub position: [f32; 3],
    pub asset_path: Option<String>,
    /// The petal that owns this node — used by the UI to scope scene entity
    /// spawn/despawn to the currently active petal.
    pub petal_id: String,
    /// Portal URL from the node's associated model record, if any.
    pub webpage_url: Option<String>,
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

/// Describes a single change to the scene graph.
///
/// Each variant maps to a CUD operation on the node table. Serialized with
/// `serde(tag = "op")` so the JSON looks like `{"op": "node_added", ...}`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneChange {
    /// A new node was created in a petal.
    NodeAdded { node: NodeDto },
    /// A node was removed from the scene.
    NodeRemoved { node_id: String },
    /// A node was renamed.
    NodeRenamed { node_id: String, new_name: String },
    /// A node's transform was updated.
    NodeTransform {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// A transform persist failed — the optimistic update should be rolled back.
    /// Contains the last-known-good values read from the DB before the failed write.
    TransformFailed {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
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
        let change = SceneChange::NodeRemoved { node_id: "n1".into() };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_removed\""));
        let deserialized: SceneChange = serde_json::from_str(&json).unwrap();
        match deserialized {
            SceneChange::NodeRemoved { node_id } => assert_eq!(node_id, "n1"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scene_change_node_renamed_serde_roundtrip() {
        let change = SceneChange::NodeRenamed { node_id: "n1".into(), new_name: "New Name".into() };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_renamed\""));
        let _: SceneChange = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn scene_change_node_transform_serde_roundtrip() {
        let change = SceneChange::NodeTransform {
            node_id: "n1".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_transform\""));
        let _: SceneChange = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn scene_change_broadcast_send_recv() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<SceneChange>(16);
        let change = SceneChange::NodeRemoved { node_id: "test".into() };
        tx.send(change.clone()).unwrap();
        let received = rx.try_recv().unwrap();
        match received {
            SceneChange::NodeRemoved { node_id } => assert_eq!(node_id, "test"),
            _ => panic!("wrong variant"),
        }
    }
}
