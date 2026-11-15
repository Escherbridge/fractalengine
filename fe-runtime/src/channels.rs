use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::messages::{
    ApiCommand, DbCommand, DbResult, NetworkCommand, NetworkEvent, SceneChange, TransformUpdate,
};

/// Buffer size for all bounded crossbeam channels.
pub const CHANNEL_BUFFER: usize = 256;

/// Row bytes the DB command channel holds in flight before byte-carrying senders wait (src/AGENTS.md §db-command-bytes).
pub const DB_COMMAND_BYTE_CAP: u64 = 64 << 20;

/// In-flight [`DbCommand::payload_bytes`] on the DB command channel, shared by its senders and the DB thread.
#[derive(Debug, Clone, Default)]
pub struct DbCommandBytes(Arc<AtomicU64>);

impl DbCommandBytes {
    /// Add `bytes` unconditionally.
    pub fn charge(&self, bytes: u64) {
        self.0.fetch_add(bytes, Ordering::AcqRel);
    }

    /// Add `bytes` when they fit under `cap` or nothing is in flight (an oversized row passes alone).
    pub fn try_charge(&self, bytes: u64, cap: u64) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current == 0 || current.saturating_add(bytes) <= cap)
                    .then(|| current.saturating_add(bytes))
            })
            .is_ok()
    }

    /// Subtract `bytes`, saturating at zero (a release nobody charged is harmless).
    pub fn release(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(bytes))
            });
    }

    /// Bytes in flight now.
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// Buffer size for the transform broadcast channel (higher because of streaming).
pub const TRANSFORM_BROADCAST_BUFFER: usize = 1024;

/// Core channels for the network and database threads.
pub struct ChannelHandles {
    pub net_cmd_tx: crossbeam::channel::Sender<NetworkCommand>,
    pub net_cmd_rx: crossbeam::channel::Receiver<NetworkCommand>,
    pub net_evt_tx: crossbeam::channel::Sender<NetworkEvent>,
    pub net_evt_rx: crossbeam::channel::Receiver<NetworkEvent>,
    pub db_cmd_tx: crossbeam::channel::Sender<DbCommand>,
    pub db_cmd_rx: crossbeam::channel::Receiver<DbCommand>,
    pub db_res_tx: crossbeam::channel::Sender<DbResult>,
    pub db_res_rx: crossbeam::channel::Receiver<DbResult>,
    /// Byte gate shared by `db_cmd_tx`'s byte-carrying senders and the DB thread that drains `db_cmd_rx`.
    pub db_cmd_bytes: DbCommandBytes,
}

impl ChannelHandles {
    pub fn new() -> Self {
        let (net_cmd_tx, net_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (net_evt_tx, net_evt_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (db_cmd_tx, db_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (db_res_tx, db_res_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        Self {
            net_cmd_tx,
            net_cmd_rx,
            net_evt_tx,
            net_evt_rx,
            db_cmd_tx,
            db_cmd_rx,
            db_res_tx,
            db_res_rx,
            db_cmd_bytes: DbCommandBytes::default(),
        }
    }
}

impl Default for ChannelHandles {
    fn default() -> Self {
        Self::new()
    }
}

/// Channels for the API gateway thread.
pub struct ApiChannels {
    /// Bevy->API command channel (crossbeam for cross-thread).
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub api_cmd_rx: crossbeam::channel::Receiver<ApiCommand>,
    /// Real-time transform fan-out for WebSocket subscribers.
    pub transform_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
    /// Scene change broadcast for entity CUD operations (Phase 3).
    pub entity_change_tx: tokio::sync::broadcast::Sender<SceneChange>,
}

impl ApiChannels {
    pub fn new() -> Self {
        let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (transform_tx, _) = tokio::sync::broadcast::channel(TRANSFORM_BROADCAST_BUFFER);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(CHANNEL_BUFFER);
        Self {
            api_cmd_tx,
            api_cmd_rx,
            transform_tx,
            entity_change_tx,
        }
    }
}

impl Default for ApiChannels {
    fn default() -> Self {
        Self::new()
    }
}
