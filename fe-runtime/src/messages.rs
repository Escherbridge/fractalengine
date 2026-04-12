use bevy::prelude::Message;

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
}
