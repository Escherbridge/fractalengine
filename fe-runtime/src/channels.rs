use crate::messages::{ApiCommand, DbCommand, DbResult, NetworkCommand, NetworkEvent, TransformUpdate};

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
        let (net_cmd_tx, net_cmd_rx) = crossbeam::channel::bounded(256);
        let (net_evt_tx, net_evt_rx) = crossbeam::channel::bounded(256);
        let (db_cmd_tx, db_cmd_rx) = crossbeam::channel::bounded(256);
        let (db_res_tx, db_res_rx) = crossbeam::channel::bounded(256);
        Self {
            net_cmd_tx,
            net_cmd_rx,
            net_evt_tx,
            net_evt_rx,
            db_cmd_tx,
            db_cmd_rx,
            db_res_tx,
            db_res_rx,
        }
    }
}

impl Default for ChannelHandles {
    fn default() -> Self {
        Self::new()
    }
}

/// Channels for the API gateway thread.
pub struct ApiChannelHandles {
    /// API thread receives commands from Bevy-side drain.
    pub api_cmd_tx: crossbeam::channel::Sender<ApiCommand>,
    pub api_cmd_rx: crossbeam::channel::Receiver<ApiCommand>,
    /// Broadcast channel for real-time transform fan-out (data plane).
    pub transform_broadcast_tx: tokio::sync::broadcast::Sender<TransformUpdate>,
}

impl ApiChannelHandles {
    pub fn new() -> Self {
        let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel::<TransformUpdate>(1024);
        Self {
            api_cmd_tx,
            api_cmd_rx,
            transform_broadcast_tx,
        }
    }
}

impl Default for ApiChannelHandles {
    fn default() -> Self {
        Self::new()
    }
}
