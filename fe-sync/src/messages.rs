//! Command and event types for the sync thread (P2P Mycelium Phase D).
//!
//! The main thread sends [`SyncCommand`]s to the sync thread over a crossbeam
//! channel; the sync thread sends [`SyncEvent`]s back.

use fe_runtime::blob_store::BlobHash;

use crate::relay_config::RelayHealth;

/// Commands sent **to** the sync thread.
#[derive(Debug, Clone)]
pub enum SyncCommand {
    /// Request a blob from the network for a given verse.
    ///
    /// The sync thread will first check the local blob store; if present it
    /// immediately emits [`SyncEvent::BlobReady`].  Otherwise it attempts a
    /// peer fetch (stub in Phase D — actual discovery comes in Phase F).
    FetchBlob { hash: BlobHash, verse_id: String },
    /// Open (or join) an iroh-docs replica for a verse.
    ///
    /// Phase E: creates an `IrohDocsReplicator` (stub) and inserts it into the
    /// sync thread's replica map. The `namespace_secret` is `Some` if the
    /// local node is the owner; `None` for read-only join.
    OpenVerseReplica {
        verse_id: String,
        namespace_id: String,
        namespace_secret: Option<String>,
    },
    /// Close a previously-opened verse replica.
    CloseVerseReplica { verse_id: String },
    /// Write a row entry to the verse's replica.
    ///
    /// The `content_hash` references a blob in the shared blob store containing
    /// the serialised row JSON.
    WriteRowEntry {
        verse_id: String,
        table: String,
        record_id: String,
        content_hash: BlobHash,
    },
    /// Subscribe to petal-level replication for a specific petal.
    SubscribePetal { petal_id: String },
    /// Unsubscribe from petal-level replication for a specific petal.
    UnsubscribePetal { petal_id: String },
    /// Advertise locally-seeding tilesets to connected peers.
    AdvertiseTilesets {
        /// The verse to broadcast to.
        verse_id: String,
        /// Serialized JSON of `Vec<TilesetAdvertisement>` from fe-terrain.
        advertisements_json: String,
    },
    /// Request tileset metadata from a specific peer.
    RequestTilesetMeta { peer_id: String, tileset_id: String },
    /// Request a single chunk from a specific peer.
    RequestChunk {
        peer_id: String,
        tileset_id: String,
        chunk_seq: u32,
    },
    /// Cancel an in-progress tileset download.
    CancelTilesetDownload { tileset_id: String },
    /// Gracefully shut down the sync thread.
    Shutdown,
    /// Broadcast a node transform change to peers in real time.
    UpdateNodeTransform {
        verse_id: String,
        node_id: String,
        position: [f32; 3],
        /// Euler angles in radians (XYZ order).
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Submit a compute task to a peer or execute locally.
    ///
    /// Phase 6.2: tasks are always executed locally. Peer offloading is
    /// deferred to Phase 8 (fe-hexon P2P distribution).
    SubmitComputeTask {
        task_id: String,
        query: String,
        petal_scope: Option<String>,
        requester_did: String,
    },
}

/// Events emitted **from** the sync thread.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// The sync thread has started.
    ///
    /// `online == true` means the iroh endpoint bound successfully and the
    /// node is reachable (at least via relay).  `false` means the thread is
    /// running in offline/local-only mode.
    Started { online: bool },
    /// A blob is now available in the local blob store.
    BlobReady { hash: BlobHash },
    /// The sync thread has shut down.
    Stopped,
    /// A peer has connected to the current verse.
    ///
    /// `peer_id` holds the peer's `did:key` format identifier.
    PeerConnected { peer_id: String },
    /// A peer has disconnected from the current verse.
    ///
    /// `peer_id` holds the peer's `did:key` format identifier.
    PeerDisconnected { peer_id: String },
    /// A compute task has completed (locally or from a peer).
    ///
    /// `result_hash` is the blake3 digest of the serialised result rows,
    /// used for cross-peer verification.
    ComputeResultReady {
        task_id: String,
        row_count: usize,
        result_hash: String,
    },
    /// A peer has advertised its available tilesets.
    PeerTilesetAdvertisement {
        peer_id: String,
        /// Serialized JSON of `Vec<TilesetAdvertisement>` from fe-terrain.
        advertisements_json: String,
    },
    /// Tileset metadata received from a peer.
    TilesetMetaReceived {
        peer_id: String,
        tileset_id: String,
        /// Serialized JSON of `TilesetMeta` from fe-format.
        meta_json: String,
        total_chunks: u32,
        approx_size_bytes: u64,
    },
    /// A chunk has been received from a peer.
    ChunkReceived {
        tileset_id: String,
        chunk_seq: u32,
        /// Raw `.hexon` chunk archive bytes.
        chunk_bytes: Vec<u8>,
    },
    /// A chunk download has failed.
    ChunkFailed {
        tileset_id: String,
        chunk_seq: u32,
        reason: String,
    },
    /// A node transform was updated by a peer.
    NodeTransformed {
        verse_id: String,
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
        author_id: String,
    },
    /// Relay reachability changed (bind result or a runtime signal).
    ///
    /// Drained into [`crate::status::SyncStatus::health`] — see AGENTS.md
    /// §relay-health. Not silent: a `Degraded`/`Unreachable` transition is
    /// loud-logged by the sync thread at the point of failure.
    RelayHealthChanged { health: RelayHealth },
}

/// Real-time transform update message for P2P gossip.
///
/// This is the payload broadcast via iroh-gossip when a node's
/// transform changes (drag commit, etc).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformUpdate {
    pub verse_id: String,
    pub node_id: String,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub author_id: String,
    pub timestamp: u64,
}

/// Sender half for sync commands (type alias for ergonomics).
pub type SyncCommandSender = crossbeam::channel::Sender<SyncCommand>;
/// Receiver half for sync commands.
pub type SyncCommandReceiver = crossbeam::channel::Receiver<SyncCommand>;
/// Sender half for sync events.
pub type SyncEventSender = crossbeam::channel::Sender<SyncEvent>;
/// Receiver half for sync events.
pub type SyncEventReceiver = crossbeam::channel::Receiver<SyncEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_command_debug_clone() {
        let cmd = SyncCommand::FetchBlob {
            hash: [0u8; 32],
            verse_id: "test-verse".into(),
        };
        let _ = format!("{:?}", cmd.clone());
        let _ = format!("{:?}", SyncCommand::Shutdown.clone());

        // Phase E variants
        let _ = format!(
            "{:?}",
            SyncCommand::OpenVerseReplica {
                verse_id: "v1".into(),
                namespace_id: "ns1".into(),
                namespace_secret: Some("secret".into()),
            }
            .clone()
        );
        let _ = format!(
            "{:?}",
            SyncCommand::CloseVerseReplica {
                verse_id: "v1".into(),
            }
            .clone()
        );
        let _ = format!(
            "{:?}",
            SyncCommand::WriteRowEntry {
                verse_id: "v1".into(),
                table: "verse".into(),
                record_id: "r1".into(),
                content_hash: [1u8; 32],
            }
            .clone()
        );

        // Petal replication variants
        let _ = format!(
            "{:?}",
            SyncCommand::SubscribePetal {
                petal_id: "p1".into(),
            }
            .clone()
        );
        let _ = format!(
            "{:?}",
            SyncCommand::UnsubscribePetal {
                petal_id: "p1".into(),
            }
            .clone()
        );
    }

    #[test]
    fn sync_event_debug_clone() {
        let _ = format!("{:?}", SyncEvent::Started { online: true }.clone());
        let _ = format!("{:?}", SyncEvent::BlobReady { hash: [1u8; 32] }.clone());
        let _ = format!("{:?}", SyncEvent::Stopped.clone());
    }

    #[test]
    fn peer_connected_construction() {
        let ev = SyncEvent::PeerConnected {
            peer_id: "did:key:z6Mk123".into(),
        };
        match &ev {
            SyncEvent::PeerConnected { peer_id } => {
                assert_eq!(peer_id, "did:key:z6Mk123");
            }
            _ => panic!("expected PeerConnected"),
        }
        // Debug + Clone should work
        let _ = format!("{:?}", ev.clone());
    }

    #[test]
    fn peer_disconnected_construction() {
        let ev = SyncEvent::PeerDisconnected {
            peer_id: "did:key:z6Mk456".into(),
        };
        match &ev {
            SyncEvent::PeerDisconnected { peer_id } => {
                assert_eq!(peer_id, "did:key:z6Mk456");
            }
            _ => panic!("expected PeerDisconnected"),
        }
        let _ = format!("{:?}", ev.clone());
    }

    #[test]
    fn peer_variants_distinguished() {
        let connected = SyncEvent::PeerConnected {
            peer_id: "did:key:aaa".into(),
        };
        let disconnected = SyncEvent::PeerDisconnected {
            peer_id: "did:key:aaa".into(),
        };
        let stopped = SyncEvent::Stopped;

        // Each arm matches only its own variant
        assert!(matches!(connected, SyncEvent::PeerConnected { .. }));
        assert!(!matches!(connected, SyncEvent::PeerDisconnected { .. }));
        assert!(matches!(disconnected, SyncEvent::PeerDisconnected { .. }));
        assert!(!matches!(disconnected, SyncEvent::PeerConnected { .. }));
        assert!(!matches!(stopped, SyncEvent::PeerConnected { .. }));
        assert!(!matches!(stopped, SyncEvent::PeerDisconnected { .. }));
    }

    #[test]
    fn submit_compute_task_debug_clone() {
        let cmd = SyncCommand::SubmitComputeTask {
            task_id: "task-001".into(),
            query: "SELECT * FROM node".into(),
            petal_scope: Some("petal-abc".into()),
            requester_did: "did:key:z6Mk789".into(),
        };
        let cloned = cmd.clone();
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("SubmitComputeTask"));
        assert!(dbg.contains("task-001"));

        // Also test with no petal scope
        let cmd_no_scope = SyncCommand::SubmitComputeTask {
            task_id: "task-002".into(),
            query: "SELECT count() FROM node GROUP ALL".into(),
            petal_scope: None,
            requester_did: "did:key:z6Mk000".into(),
        };
        let _ = format!("{:?}", cmd_no_scope.clone());
    }

    #[test]
    fn compute_result_ready_debug_clone() {
        let ev = SyncEvent::ComputeResultReady {
            task_id: "task-001".into(),
            row_count: 42,
            result_hash: "abc123".into(),
        };
        let cloned = ev.clone();
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("ComputeResultReady"));
        assert!(dbg.contains("42"));

        match &ev {
            SyncEvent::ComputeResultReady {
                task_id,
                row_count,
                result_hash,
            } => {
                assert_eq!(task_id, "task-001");
                assert_eq!(*row_count, 42);
                assert_eq!(result_hash, "abc123");
            }
            _ => panic!("expected ComputeResultReady"),
        }
    }

    #[test]
    fn channel_roundtrip() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(SyncCommand::Shutdown).unwrap();
        assert!(matches!(rx.recv().unwrap(), SyncCommand::Shutdown));
    }

    #[test]
    fn advertise_tilesets_debug_clone() {
        let cmd = SyncCommand::AdvertiseTilesets {
            verse_id: "verse-1".into(),
            advertisements_json: "[]".into(),
        };
        let _ = format!("{:?}", cmd.clone());
    }

    #[test]
    fn request_chunk_debug_clone() {
        let cmd = SyncCommand::RequestChunk {
            peer_id: "did:key:z6Mk123".into(),
            tileset_id: "ts-001".into(),
            chunk_seq: 0,
        };
        let _ = format!("{:?}", cmd.clone());
    }

    #[test]
    fn relay_health_changed_debug_clone() {
        let ev = SyncEvent::RelayHealthChanged {
            health: RelayHealth::Unreachable,
        };
        let cloned = ev.clone();
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("RelayHealthChanged"));
        assert!(dbg.contains("Unreachable"));
        match ev {
            SyncEvent::RelayHealthChanged { health } => {
                assert_eq!(health, RelayHealth::Unreachable);
            }
            _ => panic!("expected RelayHealthChanged"),
        }
    }

    #[test]
    fn peer_tileset_advertisement_debug_clone() {
        let ev = SyncEvent::PeerTilesetAdvertisement {
            peer_id: "did:key:z6Mk123".into(),
            advertisements_json: "[]".into(),
        };
        let _ = format!("{:?}", ev.clone());
    }

    #[test]
    fn chunk_received_construction() {
        let ev = SyncEvent::ChunkReceived {
            tileset_id: "ts-001".into(),
            chunk_seq: 2,
            chunk_bytes: vec![0u8; 64],
        };
        match &ev {
            SyncEvent::ChunkReceived {
                tileset_id,
                chunk_seq,
                chunk_bytes,
            } => {
                assert_eq!(tileset_id, "ts-001");
                assert_eq!(*chunk_seq, 2);
                assert_eq!(chunk_bytes.len(), 64);
            }
            _ => panic!("expected ChunkReceived"),
        }
        let _ = format!("{:?}", ev.clone());
    }

    #[test]
    fn channel_roundtrip_compute_task() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(SyncCommand::SubmitComputeTask {
            task_id: "task-rt".into(),
            query: "SELECT * FROM node".into(),
            petal_scope: None,
            requester_did: "did:key:z6MkRT".into(),
        })
        .unwrap();
        match rx.recv().unwrap() {
            SyncCommand::SubmitComputeTask {
                task_id,
                query,
                petal_scope,
                requester_did,
            } => {
                assert_eq!(task_id, "task-rt");
                assert_eq!(query, "SELECT * FROM node");
                assert!(petal_scope.is_none());
                assert_eq!(requester_did, "did:key:z6MkRT");
            }
            _ => panic!("expected SubmitComputeTask"),
        }
    }
}
