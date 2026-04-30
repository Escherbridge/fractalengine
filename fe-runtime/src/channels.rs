use crate::messages::{ApiCommand, DbCommand, DbResult, NetworkCommand, NetworkEvent, TransformUpdate};

/// Buffer size for all bounded crossbeam channels.
pub const CHANNEL_BUFFER: usize = 256;

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
}

impl ChannelHandles {
    pub fn new() -> Self {
        let (net_cmd_tx, net_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (net_evt_tx, net_evt_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (db_cmd_tx, db_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (db_res_tx, db_res_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        Self {
            net_cmd_tx, net_cmd_rx,
            net_evt_tx, net_evt_rx,
            db_cmd_tx, db_cmd_rx,
            db_res_tx, db_res_rx,
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
}

impl ApiChannels {
    pub fn new() -> Self {
        let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(CHANNEL_BUFFER);
        let (transform_tx, _) = tokio::sync::broadcast::channel(TRANSFORM_BROADCAST_BUFFER);
        Self {
            api_cmd_tx,
            api_cmd_rx,
            transform_tx,
        }
    }
}

impl Default for ApiChannels {
    fn default() -> Self {
        Self::new()
    }
}
