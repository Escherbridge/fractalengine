use bevy::prelude::Message;

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
        entity_type: String,
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
        entity_type: String,
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
        entity_type: String,
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
        entity_type: String,
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
            entity_type: "verse".to_string(),
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
            entity_type: "verse".to_string(),
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
