pub mod bevy_asset_reader;
pub mod blob_store;
pub mod blob_transport;
pub mod cache;
pub mod compute;
pub mod distributed_query;
pub mod docs_engine;
pub mod endpoint;
pub mod event_outbox;
pub mod gossip_auth;
pub mod join_ticket;
pub mod lifecycle;
pub mod messages;
pub mod net_diagnostics;
pub mod offline;
pub mod p2p_store_migration;
pub mod placement;
pub mod reachability;
pub mod reconciliation;
pub mod relay_advertise;
pub mod relay_config;
pub mod relay_reconcile;
#[cfg(feature = "relay-server")]
pub mod relay_server;
pub mod relay_trust;
pub mod replication;
pub mod replicator;
pub mod sharding;
mod peer_book;
mod scene_asset_gate;
mod scene_assets;
pub mod status;
pub mod sync_thread;
pub mod tileset_transfer;
pub mod verse_peers;
pub mod virtual_transport;
pub mod write_policy;

pub use bevy_asset_reader::BlobAssetReader;
pub use blob_store::FsBlobStore;
pub use blob_transport::IrohBlobTransport;
pub use docs_engine::{p2p_data_dir, DocsStack, P2P_DIR_ENV_VAR};
pub use endpoint::bound_endpoint_count;
pub use fe_database::invite::VerseInvite;
pub use lifecycle::{
    lifecycle_channel, LifecycleEventReceiver, LifecycleEventSender, LifecycleForwarder,
};
pub use messages::{SyncCommand, SyncCommandSender, SyncEvent, SyncEventReceiver};
pub use placement::{plan_shard_hosts, PlacementPlan, Retention, TransferRoute};
pub use relay_config::{
    RelayAdvertisePolicy, RelayConfig, RelayConfigError, RelayHealth, RelayStatusSummary, RELAY_CONFIG_ENV_VAR,
};
pub use replicator::{
    IncomingEntryApplicator, IrohDocsReplicator, IrohPetalReplicator, MockVerseReplicator,
    PetalReplicator, RowChange, VerseReplicator,
};
pub use sharding::{
    PeerDeclaration, ShardId, ShardLedgerEntry, VerseFabric, PEER_DECL_TABLE, SHARD_TABLE,
};
pub use status::{
    drain_sync_events, SceneAssetEventBuffer, StreamedTilesetEventBuffer, SyncCommandSenderRes, SyncEventReceiverRes,
    SyncStatus, TilesetEventBuffer,
};
pub use sync_thread::{spawn_sync_thread, spawn_sync_thread_with_transport};
pub use tileset_transfer::{
    CatalogEntry, ChunkError, CompletedDownload, DownloadReport, MetaResolved, ResumedDownload,
    StoredChunk, TilesetTransfer, TransferError, TransferEvent,
};
pub use verse_peers::VersePeers;
pub use virtual_transport::{
    VirtualGossipMessage, VirtualGossipTopic, VirtualReplica, VirtualTransportFactory,
};
pub use write_policy::PolicyHandle;
