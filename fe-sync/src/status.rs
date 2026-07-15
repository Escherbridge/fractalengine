//! Bevy resources for sync thread integration (P2P Mycelium Phase D).

use bevy::prelude::*;
use std::sync::{Arc, Mutex};

use crate::messages::{SyncCommandSender, SyncEvent, SyncEventReceiver};

/// Bevy resource wrapping the sync command sender.
///
/// Insert this into the Bevy world so that any system can send commands to
/// the sync thread (e.g. `FetchBlob` on asset miss).
#[derive(Resource, Clone)]
pub struct SyncCommandSenderRes(pub SyncCommandSender);

/// Bevy resource wrapping the sync event receiver.
#[derive(Resource)]
pub struct SyncEventReceiverRes(pub Arc<Mutex<SyncEventReceiver>>);

/// Bevy resource tracking the current state of the sync thread.
#[derive(Resource, Default, Debug, Clone)]
pub struct SyncStatus {
    /// Whether the iroh endpoint is online.
    pub online: bool,
    /// Serialised node address for UI display (e.g. node ID + relay URL).
    pub node_addr: Option<String>,
    /// Number of connected peers (Phase F).
    pub peer_count: usize,
}

/// Bevy resource that buffers tileset distribution events for the UI layer.
///
/// `drain_sync_events` pushes tileset-related events here; `fe-ui` drains
/// them each frame to update the Hexon Manager dialog.
#[derive(Resource, Default)]
pub struct TilesetEventBuffer {
    pub events: Vec<SyncEvent>,
}

/// Bevy system that drains [`SyncEvent`]s from the sync thread and updates
/// [`SyncStatus`].
pub fn drain_sync_events(
    receiver: Option<Res<SyncEventReceiverRes>>,
    mut status: ResMut<SyncStatus>,
    mut tileset_buf: ResMut<TilesetEventBuffer>,
) {
    let Some(receiver) = receiver else { return };
    let Ok(rx) = receiver.0.lock() else { return };
    while let Ok(evt) = rx.try_recv() {
        match evt {
            SyncEvent::Started { online } => {
                status.online = online;
                tracing::info!(online, "SyncStatus updated: started");
            }
            SyncEvent::BlobReady { hash } => {
                tracing::debug!(
                    hash = %fe_runtime::blob_store::hash_to_hex(&hash),
                    "Blob ready from sync"
                );
            }
            SyncEvent::Stopped => {
                status.online = false;
                tracing::info!("SyncStatus updated: stopped");
            }
            SyncEvent::PeerConnected { ref peer_id } => {
                status.peer_count = status.peer_count.saturating_add(1);
                tracing::info!(peer_id, peer_count = status.peer_count, "Peer connected");
            }
            SyncEvent::PeerDisconnected { ref peer_id } => {
                status.peer_count = status.peer_count.saturating_sub(1);
                tracing::info!(peer_id, peer_count = status.peer_count, "Peer disconnected");
            }
            SyncEvent::ComputeResultReady {
                ref task_id,
                row_count,
                ref result_hash,
            } => {
                tracing::info!(task_id, row_count, result_hash, "Compute result ready");
            }
            // Tileset distribution events — buffer for the UI/terrain layer.
            ev @ SyncEvent::PeerTilesetAdvertisement { .. }
            | ev @ SyncEvent::TilesetMetaReceived { .. }
            | ev @ SyncEvent::ChunkReceived { .. }
            | ev @ SyncEvent::ChunkFailed { .. } => {
                tileset_buf.events.push(ev);
            }
            // Peer transform updates — applying to local nodes is deferred.
            // TODO(iroh-0.35): apply peer NodeTransformed to the local ECS/world.
            SyncEvent::NodeTransformed {
                ref node_id,
                ref verse_id,
                ..
            } => {
                tracing::debug!(
                    node_id,
                    verse_id,
                    "Received peer NodeTransformed (not yet applied)"
                );
            }
        }
    }
}
