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
    /// Creates an `IrohDocsReplicator` and opens its document: with the
    /// `namespace_secret` present the namespace is imported as a write
    /// capability; without it the namespace is opened read-only (a previously
    /// imported / ticket-joined replica).
    ///
    /// `bootstrap_peers` carries serialized iroh `NodeAddr` entries (JSON) to
    /// dial when the replica starts syncing — empty for the default
    /// `FE_SYNC_BOOTSTRAP` set.
    OpenVerseReplica {
        verse_id: String,
        namespace_id: String,
        namespace_secret: Option<String>,
        /// JSON-serialized `iroh::NodeAddr` values parsed with iroh's own
        /// serde types; invalid entries warn loudly and are skipped.
        bootstrap_peers: Vec<String>,
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
    /// Declare the local peer's shard-hosting capacity and seeder role
    /// (M2/F6 — A14). Published to `__peers/{local_did}` in every open
    /// verse's namespace so peers' placement plans see it. `None` capacity
    /// means unlimited.
    SetShardDeclaration {
        capacity_bytes: Option<u64>,
        seeder: bool,
    },
    /// Request the current per-verse shard ledger + settings (M2/F6
    /// diagnostics — the harness asserts placement/mode through this).
    GetShardLedger { verse_id: String },
    /// Gracefully shut down the sync thread.
    Shutdown,
    /// Legacy unsigned transform command.
    ///
    /// The sync thread deliberately drops this command. Networked transform
    /// replication must use a signed canonical operation rather than this
    /// preview-shaped payload.
    UpdateNodeTransform {
        verse_id: String,
        node_id: String,
        position: [f32; 3],
        /// Euler angles in radians (XYZ order).
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Submit a distributed timeseries query (M2/F7 — A15/A16/A17): the
    /// structured spec is planned against the verse's fabric, fanned out over
    /// the verse's gossip topic with bounded concurrency, answered by every
    /// peer hosting requested shards (relay seeders answer for shards they
    /// host), and merged commutatively with covered/missing shard honesty
    /// metadata. The merged outcome is delivered on the call's embedded
    /// crossbeam reply channel; [`SyncEvent::ComputeResultReady`] carries the
    /// diagnostics. The spec is never raw SQL — responders render their own
    /// SQL from it via the fe-query builders.
    SubmitComputeTask {
        call: fe_runtime::distributed_query::DistributedQueryCall,
    },
}

/// Events emitted **from** the sync thread.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// The sync thread has started.
    ///
    /// `online == true` means the iroh endpoint bound successfully and the
    /// node is reachable (at least via relay).  `false` means the thread is
    /// running in offline/local-only mode. `node_addr` is the JSON-serialized
    /// `iroh::NodeAddr` peers can dial us on, when online.
    Started {
        online: bool,
        node_addr: Option<String>,
    },
    /// A blob is now available in the local blob store.
    BlobReady { hash: BlobHash },
    /// An inbound replicated row was received from a peer and forwarded to
    /// the DB thread via `DbCommand::ApplyReplicatedRow` (A4). The DB thread
    /// applies it as the single writer; the durable outcome comes back as
    /// `DbResult::ReplicatedRowApplied`.
    RowApplied {
        verse_id: String,
        table: String,
        record_id: String,
    },
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
    /// A verse replica's document failed to open **while the P2P stack was
    /// online** (F20/M1 finding 4).
    ///
    /// The replica is registered but loudly NON-replicating: no inbound
    /// pump, `WriteRowEntry` warns and fails, reconciliation is skipped.
    /// Emitted alongside an `error!` log so hosts can surface honest
    /// replica status — the offline mock fallback is a different (sanctioned)
    /// mode and does not emit this.
    ReplicaOpenFailed { verse_id: String, reason: String },
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
    /// Reply to [`SyncCommand::GetShardLedger`] (M2/F6): the verse's current
    /// timeseries fabric — settings, peer declarations, shard ledger — as a
    /// JSON dump. Diagnostics/harness assertions only; the fabric itself is
    /// sync-thread-internal.
    ShardLedger {
        verse_id: String,
        ledger_json: String,
    },
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
                bootstrap_peers: vec![],
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
        let _ = format!(
            "{:?}",
            SyncEvent::Started {
                online: true,
                node_addr: None,
            }
            .clone()
        );
        let _ = format!("{:?}", SyncEvent::BlobReady { hash: [1u8; 32] }.clone());
        let _ = format!("{:?}", SyncEvent::Stopped.clone());
    }

    #[test]
    fn row_applied_debug_clone() {
        let ev = SyncEvent::RowApplied {
            verse_id: "verse-1".into(),
            table: "node".into(),
            record_id: "node-9".into(),
        };
        let dbg = format!("{:?}", ev.clone());
        assert!(dbg.contains("RowApplied"));
        assert!(dbg.contains("node-9"));
        match ev {
            SyncEvent::RowApplied {
                verse_id,
                table,
                record_id,
            } => {
                assert_eq!(verse_id, "verse-1");
                assert_eq!(table, "node");
                assert_eq!(record_id, "node-9");
            }
            _ => panic!("expected RowApplied"),
        }
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
        let (reply_tx, _reply_rx) = crossbeam::channel::bounded(1);
        let cmd = SyncCommand::SubmitComputeTask {
            call: fe_runtime::distributed_query::DistributedQueryCall {
                request: fe_runtime::distributed_query::DistributedQueryRequest {
                    request_id: "task-001".into(),
                    verse_id: "verse-1".into(),
                    spec: fe_runtime::distributed_query::TsQueryKind::WindowAggregate {
                        metric: "temperature_c".into(),
                        start_ms: 0,
                        end_ms: 60_000,
                        petal_id: "petal-abc".into(),
                    },
                    timeout_ms: 3_000,
                    row_cap: 0,
                },
                reply: reply_tx,
            },
        };
        let cloned = cmd.clone();
        let dbg = format!("{cloned:?}");
        assert!(dbg.contains("SubmitComputeTask"));
        assert!(dbg.contains("task-001"));

        // Also test with a raw-window spec
        let (reply_tx, _reply_rx) = crossbeam::channel::bounded(1);
        let cmd_no_scope = SyncCommand::SubmitComputeTask {
            call: fe_runtime::distributed_query::DistributedQueryCall {
                request: fe_runtime::distributed_query::DistributedQueryRequest {
                    request_id: "task-002".into(),
                    verse_id: "verse-1".into(),
                    spec: fe_runtime::distributed_query::TsQueryKind::ReadingsInWindow {
                        metric: "temperature_c".into(),
                        start_ms: 0,
                        end_ms: 60_000,
                        petal_id: "petal-abc".into(),
                    },
                    timeout_ms: 3_000,
                    row_cap: 0,
                },
                reply: reply_tx,
            },
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
    fn replica_open_failed_debug_clone() {
        let ev = SyncEvent::ReplicaOpenFailed {
            verse_id: "01VERSEOPENFAILED0000000000".into(),
            reason: "namespace import failed (test)".into(),
        };
        let cloned = ev.clone();
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("ReplicaOpenFailed"));
        assert!(dbg.contains("01VERSEOPENFAILED0000000000"));
        match ev {
            SyncEvent::ReplicaOpenFailed { verse_id, reason } => {
                assert_eq!(verse_id, "01VERSEOPENFAILED0000000000");
                assert_eq!(reason, "namespace import failed (test)");
            }
            _ => panic!("expected ReplicaOpenFailed"),
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
        let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
        tx.send(SyncCommand::SubmitComputeTask {
            call: fe_runtime::distributed_query::DistributedQueryCall {
                request: fe_runtime::distributed_query::DistributedQueryRequest {
                    request_id: "task-rt".into(),
                    verse_id: "verse-1".into(),
                    spec: fe_runtime::distributed_query::TsQueryKind::LatestPerAnchor {
                        petal_id: "petal-abc".into(),
                        metric: None,
                    },
                    timeout_ms: 3_000,
                    row_cap: 0,
                },
                reply: reply_tx,
            },
        })
        .unwrap();
        match rx.recv().unwrap() {
            SyncCommand::SubmitComputeTask { call } => {
                assert_eq!(call.request.request_id, "task-rt");
                assert_eq!(call.request.verse_id, "verse-1");
                assert!(matches!(
                    call.request.spec,
                    fe_runtime::distributed_query::TsQueryKind::LatestPerAnchor { .. }
                ));
                // The embedded reply seam survives the channel round trip.
                call.reply
                    .send(fe_runtime::distributed_query::DistributedQueryOutcome {
                        rows: Vec::new(),
                        meta: fe_runtime::distributed_query::DistributedQueryMeta {
                            covered_shards: Vec::new(),
                            missing_shards: Vec::new(),
                            answered_hosts: Vec::new(),
                            missing_hosts: Vec::new(),
                            mode: "mirror".into(),
                            replication_factor: 1,
                            truncated: false,
                        },
                        error: None,
                    })
                    .unwrap();
                assert!(reply_rx.recv().is_ok());
            }
            _ => panic!("expected SubmitComputeTask"),
        }
    }

    #[test]
    fn set_shard_declaration_debug_clone() {
        let cmd = SyncCommand::SetShardDeclaration {
            capacity_bytes: Some(1_000_000),
            seeder: true,
        };
        let dbg = format!("{:?}", cmd.clone());
        assert!(dbg.contains("SetShardDeclaration"));
        match cmd {
            SyncCommand::SetShardDeclaration {
                capacity_bytes,
                seeder,
            } => {
                assert_eq!(capacity_bytes, Some(1_000_000));
                assert!(seeder);
            }
            _ => panic!("expected SetShardDeclaration"),
        }
    }

    #[test]
    fn shard_ledger_event_debug_clone() {
        let ev = SyncEvent::ShardLedger {
            verse_id: "v-1".into(),
            ledger_json: "{}".into(),
        };
        let dbg = format!("{:?}", ev.clone());
        assert!(dbg.contains("ShardLedger"));
        match ev {
            SyncEvent::ShardLedger {
                verse_id,
                ledger_json,
            } => {
                assert_eq!(verse_id, "v-1");
                assert_eq!(ledger_json, "{}");
            }
            _ => panic!("expected ShardLedger"),
        }
    }
}
