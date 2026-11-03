//! Bevy resources for sync thread integration (P2P Mycelium Phase D).

use bevy::prelude::*;
use std::sync::{Arc, Mutex};

use crate::messages::{SyncCommandSender, SyncEvent, SyncEventReceiver};
use crate::relay_config::RelayHealth;

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
    /// Relay reachability (relay-health hardening) — see AGENTS.md §relay-health.
    pub health: RelayHealth,
}

/// Bevy resource that buffers tileset distribution events for the UI layer.
///
/// `drain_sync_events` pushes tileset-related events here; `fe-ui` drains
/// them each frame to update the Hexon Manager dialog.
#[derive(Resource, Default)]
pub struct TilesetEventBuffer {
    pub events: Vec<SyncEvent>,
}

/// Bevy resource duplicating the subset of tileset events the STREAMED
/// (not-installed) path needs (campaign plan S7): advertisements, meta, and
/// chunk arrivals/failures. `TilesetEventBuffer`'s own consumer
/// (`fe-ui::terrain_map::drain_tileset_events`) destructively drains the
/// WHOLE buffer every frame, so a second consumer reading the same buffer
/// would race it; this sibling buffer lets `fractalengine::terrain_bridge`
/// react to the same network events without that race. `Option<ResMut<_>>`
/// in `drain_sync_events` below: only a binary that registers this resource
/// (the streamed path needs it) pays for it — see `src/AGENTS.md`
/// §tileset-transfer.
#[derive(Resource, Default)]
pub struct StreamedTilesetEventBuffer {
    pub events: Vec<SyncEvent>,
}

/// Single-consumer native scene transfer events; overflow requires fresh current reconciliation.
#[derive(Resource,Default)]
pub struct SceneAssetEventBuffer {
    pub events:Vec<SyncEvent>,
    pub overflowed:bool,
}

const MAX_SCENE_ASSET_EVENTS:usize=1024;

/// Most events either tileset buffer holds between drains; past it new events
/// are dropped (a binary with no consumer — the headless relay — never grows
/// without bound; see `src/AGENTS.md` §tileset-transfer).
pub const MAX_BUFFERED_TILESET_EVENTS: usize = 4096;

/// Push `ev` unless `events` is full; returns whether it was kept.
fn push_bounded(events: &mut Vec<SyncEvent>, ev: SyncEvent) -> bool {
    if events.len() >= MAX_BUFFERED_TILESET_EVENTS {
        return false;
    }
    events.push(ev);
    true
}

/// Bevy system that drains [`SyncEvent`]s from the sync thread and updates
/// [`SyncStatus`].
pub fn drain_sync_events(
    receiver: Option<Res<SyncEventReceiverRes>>,
    mut status: ResMut<SyncStatus>,
    mut tileset_buf: ResMut<TilesetEventBuffer>,
    mut streamed_buf: Option<ResMut<StreamedTilesetEventBuffer>>,
    mut scene_buf: Option<ResMut<SceneAssetEventBuffer>>,
) {
    let Some(receiver) = receiver else { return };
    let Ok(rx) = receiver.0.lock() else { return };
    let mut dropped = 0usize;
    let mut scene_dropped=0usize;
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt,SyncEvent::BlobReady {..}|SyncEvent::SceneAssetFetchFailed {..}|SyncEvent::SceneAssetFetchSettled {..}) {
            if let Some(buffer)=scene_buf.as_mut() {
                if buffer.events.len()<MAX_SCENE_ASSET_EVENTS {buffer.events.push(evt.clone());}
                else {buffer.overflowed=true;scene_dropped+=1;}
            }
        }
        match evt {
            SyncEvent::Started { online, node_addr } => {
                status.online = online;
                status.node_addr = node_addr;
                // The dialable EndpointAddr JSON is what peers/relays paste into
                // FE_SYNC_BOOTSTRAP — log it raw (not Debug-escaped) so it is
                // copy-pasteable off the start banner, in exactly the form
                // `parse_bootstrap_peer` accepts.
                match status.node_addr.as_deref() {
                    Some(addr) => {
                        tracing::info!(
                            online,
                            node_addr = %addr,
                            "SyncStatus updated: started — dialable address above \
                             (paste into FE_SYNC_BOOTSTRAP)"
                        );
                    }
                    None => {
                        tracing::info!(online, "SyncStatus updated: started — no dialable address");
                    }
                }
            }
            SyncEvent::SceneAssetFetchFailed {ref verse_id,hash,ref reason} => {
                tracing::warn!(verse_id,hash=%fe_runtime::blob_store::hash_to_hex(&hash),reason,"Scene asset remains unavailable");
            }
            SyncEvent::SceneAssetFetchSettled {ref verse_id,hash,ref request_ids,ref result} => {
                tracing::debug!(verse_id,hash=%fe_runtime::blob_store::hash_to_hex(&hash),owners=request_ids.len(),success=result.is_ok(),"Owned scene asset transfer settled");
            }
            SyncEvent::BlobReady { hash } => {
                tracing::debug!(
                    hash = %fe_runtime::blob_store::hash_to_hex(&hash),
                    "Blob ready from sync"
                );
            }
            SyncEvent::RowApplied {
                ref verse_id,
                ref table,
                ref record_id,
            } => {
                tracing::info!(
                    verse_id,
                    table,
                    record_id,
                    "Replicated row applied from peer"
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
            SyncEvent::RelayHealthChanged { health } => {
                // Unconditional info-level line (PAS-P1V): the headless
                // relay-health script greps for this regardless of mode, so
                // a `Disabled`/`Healthy` run logs exactly as reliably as a
                // `Degraded`/`Unreachable` one (which additionally warns
                // below).
                tracing::info!(?health, "SyncStatus updated: relay health changed");
                if health.is_problem() {
                    tracing::warn!(?health, "Relay health degraded");
                } else {
                    tracing::info!(?health, "Relay health updated");
                }
                status.health = health;
            }
            SyncEvent::ReplicaOpenFailed {
                ref verse_id,
                ref reason,
            } => {
                // F20 finding 4: honest replica status — this verse is NOT
                // replicating while its document remains unopenable on an
                // online stack. Loud by design (see AGENTS.md §iroh-1.x:
                // the mock fallback is offline-only).
                tracing::error!(
                    verse_id,
                    reason,
                    "Verse replica NOT replicating — document open failed on the online stack"
                );
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
            | ev @ SyncEvent::ChunkFailed { .. }
            | ev @ SyncEvent::TilesetDownloadComplete { .. }
            | ev @ SyncEvent::TilesetPublished { .. }
            | ev @ SyncEvent::TilesetDownloadResumed { .. } => {
                // S7: the streamed path only consumes advertisements, meta
                // and chunk events — duplicate just those into its own
                // buffer (never the install-flow-only variants) before this
                // buffer takes ownership.
                if let Some(streamed) = streamed_buf.as_mut() {
                    if matches!(
                        ev,
                        SyncEvent::PeerTilesetAdvertisement { .. }
                            | SyncEvent::TilesetMetaReceived { .. }
                            | SyncEvent::ChunkReceived { .. }
                            | SyncEvent::ChunkFailed { .. }
                    ) && !push_bounded(&mut streamed.events, ev.clone())
                    {
                        dropped += 1;
                    }
                }
                if !push_bounded(&mut tileset_buf.events, ev) {
                    dropped += 1;
                }
            }
            // Unsupported legacy unsigned transform event; no current emitter (see AGENTS.md).
            SyncEvent::NodeTransformed {
                ref node_id,
                ref verse_id,
                ..
            } => {
                tracing::debug!(
                    node_id,
                    verse_id,
                    "Ignored unsupported legacy unsigned NodeTransformed"
                );
            }
            // M2/F6: the shard-ledger dump is a diagnostics reply (the
            // harness asserts placement/mode through it); log it at debug.
            SyncEvent::ShardLedger {
                ref verse_id,
                ref ledger_json,
            } => {
                tracing::debug!(verse_id, ledger = %ledger_json, "Shard ledger snapshot");
            }
        }
    }
    if scene_dropped>0 {tracing::warn!(dropped=scene_dropped,"Native scene asset event buffer overflowed; reconcile current state without inventing transfer completion");}
    if dropped > 0 {
        tracing::warn!(
            dropped,
            cap = MAX_BUFFERED_TILESET_EVENTS,
            "Tileset event buffer full (nothing drains it) — events dropped"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_status_default_health_is_unknown() {
        let status = SyncStatus::default();
        assert_eq!(status.health, RelayHealth::Unknown);
        assert!(!status.online);
        assert_eq!(status.peer_count, 0);
    }

    /// A binary with no tileset consumer (the headless relay) drains the
    /// sync channel every frame; its buffers must stay capped, not grow
    /// with every advertisement for the life of the process.
    #[test]
    fn undrained_tileset_buffers_stay_bounded() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app = App::new();
        app.insert_resource(SyncEventReceiverRes(Arc::new(Mutex::new(rx))))
            .init_resource::<SyncStatus>()
            .init_resource::<TilesetEventBuffer>()
            .init_resource::<StreamedTilesetEventBuffer>()
            .init_resource::<SceneAssetEventBuffer>()
            .add_systems(Update, drain_sync_events);
        for frame in 0..3 {
            for i in 0..MAX_BUFFERED_TILESET_EVENTS {
                tx.send(SyncEvent::PeerTilesetAdvertisement {
                    peer_id: format!("peer-{frame}-{i}"),
                    advertisements_json: "[]".to_string(),
                })
                .unwrap();
            }
            for index in 0..MAX_SCENE_ASSET_EVENTS+1 {
                tx.send(SyncEvent::SceneAssetFetchSettled {verse_id:"scene-v".into(),hash:[7;32],request_ids:vec![format!("owner-{frame}-{index}")],result:Ok(())}).unwrap();
            }
            app.update();
        }
        let scene=app.world().resource::<SceneAssetEventBuffer>();
        assert_eq!(scene.events.len(),MAX_SCENE_ASSET_EVENTS);assert!(scene.overflowed);
        assert!(matches!(&scene.events[0],SyncEvent::SceneAssetFetchSettled {request_ids,..} if request_ids==&vec!["owner-0-0".to_owned()]));
        assert_eq!(
            app.world().resource::<TilesetEventBuffer>().events.len(),
            MAX_BUFFERED_TILESET_EVENTS
        );
        assert_eq!(
            app.world()
                .resource::<StreamedTilesetEventBuffer>()
                .events
                .len(),
            MAX_BUFFERED_TILESET_EVENTS
        );
    }

    #[test]
    fn sync_status_health_is_settable_and_reads_back() {
        let status = SyncStatus {
            health: RelayHealth::Unreachable,
            ..Default::default()
        };
        assert_eq!(status.health, RelayHealth::Unreachable);
        assert!(status.health.is_problem());
    }
}
