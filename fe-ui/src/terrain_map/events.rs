//! Drains tileset distribution events from the sync thread into the
//! Hexon Manager's view state. See `fe-ui/src/AGENTS.md` §terrain-map.
//!
//! S4 (`p2p_asset_streaming_20260718`) rewrote this into a pure **view
//! updater**: since S3 wired `SyncCommand::DownloadTileset` to drive the
//! whole fetch (meta resolution + every bundle) on the sync thread, this
//! system no longer ping-pongs `RequestChunk` after each `ChunkReceived` —
//! that was the regression where closing the Hexon Manager (which used to
//! own `download_progress` inside `ActiveDialog::HexonManager`) stalled a
//! download outright, because the ping-pong only ran `if let
//! ActiveDialog::HexonManager`. Download progress now lives in
//! [`crate::terrain_map::TilesetDownloads`], a resource independent of the
//! dialog's open/closed state, so every event below updates it
//! unconditionally.

use bevy::prelude::*;

use super::dto::{DownloadProgress, DownloadStatus};
use super::{HexonOp, PendingHexonOps, TilesetDownloads};
use crate::dialogs::ActiveDialog;

/// Drains tileset distribution events from the sync thread, updates
/// [`TilesetDownloads`] (survives the dialog closing), and queues
/// [`HexonOp::AssembleDownload`] once a download completes.
pub(crate) fn drain_tileset_events(
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut tileset_buf: ResMut<fe_sync::TilesetEventBuffer>,
    mut downloads: ResMut<TilesetDownloads>,
    mut hexon_ops: ResMut<PendingHexonOps>,
) {
    if tileset_buf.events.is_empty() {
        return;
    }

    let events: Vec<fe_sync::SyncEvent> = std::mem::take(&mut tileset_buf.events);

    for evt in events {
        match evt {
            fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id,
                advertisements_json,
            } => {
                // Parse advertisements and merge into the dialog's available
                // list — this one stays dialog-scoped (only meaningful while
                // the Available tab is in view; a closed dialog re-seeds it
                // via `HexonRefreshList` on reopen).
                let Ok(ads): Result<Vec<serde_json::Value>, _> =
                    serde_json::from_str(&advertisements_json)
                else {
                    bevy::log::warn!("Failed to parse peer tileset advertisements");
                    continue;
                };

                if let ActiveDialog::HexonManager {
                    ref mut available_tilesets,
                    ref installed_tilesets,
                    ..
                } = ui_mgr.active_dialog
                {
                    for ad in ads {
                        let Some(tileset_id) = ad.get("tileset_id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        // Skip if we already have it in available list
                        if available_tilesets.iter().any(|t| t.hexon_id == tileset_id) {
                            // Increment peer count
                            if let Some(existing) = available_tilesets
                                .iter_mut()
                                .find(|t| t.hexon_id == tileset_id)
                            {
                                existing.peer_count += 1;
                            }
                            continue;
                        }
                        let already_installed =
                            installed_tilesets.iter().any(|t| t.hexon_id == tileset_id);
                        available_tilesets.push(super::dto::AvailableTilesetDto {
                            hexon_id: tileset_id.to_string(),
                            region_name: ad
                                .get("region_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown")
                                .to_string(),
                            bounds: ad
                                .get("bounds")
                                .and_then(|v| serde_json::from_value::<[f64; 4]>(v.clone()).ok())
                                .unwrap_or([0.0; 4]),
                            zoom_range: (
                                ad.get("min_zoom").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                                ad.get("max_zoom").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                            ),
                            tile_count: ad.get("tile_count").and_then(|v| v.as_u64()).unwrap_or(0)
                                as u32,
                            approx_size_bytes: ad
                                .get("approx_size_bytes")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            peer_count: 1,
                            already_installed,
                        });
                    }
                }
                bevy::log::info!("Received tileset advertisements from peer {}", peer_id,);
            }
            fe_sync::SyncEvent::TilesetMetaReceived {
                tileset_id,
                total_chunks,
                approx_size_bytes,
                root_hash,
                meta_json,
                ..
            } => {
                // PAS-P5 S7a: the root resolving does NOT mean bundles are
                // flowing yet — the host still has to verify the signed
                // header and send `ConfirmTilesetDownload` before the sync
                // thread (`TilesetTransfer::download`) ever requests a chunk.
                // Reuse `Verifying` (already used for post-download assembly)
                // rather than add a UI-only status variant: either way it
                // means "not actively transferring bytes right now". The
                // first `ChunkReceived` below flips it to `Downloading`. An
                // empty `root_hash` is fe-sync's honest failure reply (no
                // provider, offline, download stopped); a header refused by
                // the host is written straight onto this tracker as `Failed`
                // by `fractalengine::terrain_bridge` (§tileset-p2p).
                if let Some(dl) = downloads.0.get_mut(&tileset_id) {
                    if root_hash.is_empty() {
                        let reason = serde_json::from_str::<serde_json::Value>(&meta_json)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_string))
                            .unwrap_or_else(|| "download failed".to_string());
                        dl.status = DownloadStatus::Failed(reason);
                    } else {
                        dl.total_chunks = total_chunks;
                        dl.total_bytes_estimate = approx_size_bytes;
                        // Never mask the host's own integrity refusal.
                        if !matches!(dl.status, DownloadStatus::Failed(_)) {
                            dl.status = DownloadStatus::Verifying;
                        }
                    }
                }
            }
            fe_sync::SyncEvent::ChunkReceived {
                tileset_id,
                chunk_seq,
                byte_len,
                ..
            } => {
                if let Some(dl) = downloads.0.get_mut(&tileset_id) {
                    // The host's header verification + confirmation
                    // succeeded the moment the first real bundle arrives.
                    if matches!(dl.status, DownloadStatus::Verifying) {
                        dl.status = DownloadStatus::Downloading;
                    }
                    dl.chunks_received += 1;
                    dl.bytes_received += byte_len;
                }
                bevy::log::debug!(
                    "Chunk {chunk_seq} received for tileset {tileset_id} ({byte_len} bytes)"
                );
            }
            fe_sync::SyncEvent::ChunkFailed {
                tileset_id,
                chunk_seq,
                reason,
            } => {
                bevy::log::warn!("Chunk {chunk_seq} failed for tileset {tileset_id}: {reason}");
                if let Some(dl) = downloads.0.get_mut(&tileset_id) {
                    dl.status =
                        DownloadStatus::Failed(format!("Chunk {chunk_seq} failed: {reason}"));
                }
            }
            fe_sync::SyncEvent::TilesetDownloadComplete {
                tileset_id,
                index_path,
                bundle_paths,
            } => {
                if let Some(dl) = downloads.0.get_mut(&tileset_id) {
                    dl.status = DownloadStatus::Verifying;
                    dl.chunks_received = dl.total_chunks.max(dl.chunks_received);
                } else {
                    // A resumed/background completion with no dialog-opened
                    // tracker yet — seed one so the Downloads tab shows it.
                    downloads.0.insert(
                        tileset_id.clone(),
                        DownloadProgress {
                            tileset_id: tileset_id.clone(),
                            chunks_received: bundle_paths.len() as u32,
                            total_chunks: bundle_paths.len() as u32,
                            bytes_received: 0,
                            total_bytes_estimate: 0,
                            status: DownloadStatus::Verifying,
                        },
                    );
                }
                // Every bundle is already verified on disk (S2/S3 invariant —
                // `TilesetTransfer` only ever writes a chunk after its blake3
                // matches). Assembly + install run off the Bevy main thread
                // (`terrain_bridge::drain_hexon_ops` in the host binary).
                hexon_ops.0.push(HexonOp::AssembleDownload {
                    tileset_id,
                    index_path,
                    bundle_paths,
                });
            }
            fe_sync::SyncEvent::TilesetDownloadResumed {
                tileset_id,
                received,
                total,
            } => {
                let status = if received >= total && total > 0 {
                    DownloadStatus::Verifying
                } else {
                    DownloadStatus::Downloading
                };
                downloads
                    .0
                    .entry(tileset_id.clone())
                    .and_modify(|dl| {
                        dl.chunks_received = received;
                        dl.total_chunks = total;
                        dl.status = status.clone();
                    })
                    .or_insert(DownloadProgress {
                        tileset_id,
                        chunks_received: received,
                        total_chunks: total,
                        bytes_received: 0,
                        total_bytes_estimate: 0,
                        status,
                    });
            }
            fe_sync::SyncEvent::TilesetPublished { tileset_id, .. } => {
                bevy::log::info!("Now seeding tileset {tileset_id} over P2P");
            }
            _ => {} // other SyncEvent variants handled elsewhere
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::UiManager;

    /// Final review: fe-sync answers a download it cannot run (no provider
    /// advertised the tileset, P2P offline, over its byte cap) with an
    /// empty-root meta reply; the tracker must show Failed with the reason,
    /// never sit on "Downloading" forever.
    #[test]
    fn a_failed_download_shows_failed_not_downloading() {
        let mut app = App::new();
        app.init_resource::<UiManager>()
            .init_resource::<fe_sync::TilesetEventBuffer>()
            .init_resource::<TilesetDownloads>()
            .init_resource::<PendingHexonOps>()
            .add_systems(Update, drain_tileset_events);
        let tileset_id = "ts-unadvertised".to_string();
        app.world_mut().resource_mut::<TilesetDownloads>().0.insert(
            tileset_id.clone(),
            DownloadProgress {
                tileset_id: tileset_id.clone(),
                chunks_received: 0,
                total_chunks: 0,
                bytes_received: 0,
                total_bytes_estimate: 0,
                status: DownloadStatus::Queued,
            },
        );
        app.world_mut()
            .resource_mut::<fe_sync::TilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::TilesetMetaReceived {
                peer_id: String::new(),
                tileset_id: tileset_id.clone(),
                meta_json: r#"{"error":"no advertised root for this tileset yet"}"#.into(),
                total_chunks: 0,
                approx_size_bytes: 0,
                root_hash: String::new(),
                chunk_hashes: Vec::new(),
                index_path: std::path::PathBuf::new(),
                ad_publisher_did: String::new(),
            });
        app.update();
        let status = app.world().resource::<TilesetDownloads>().0[&tileset_id]
            .status
            .clone();
        assert!(
            matches!(&status, DownloadStatus::Failed(reason)
                if reason == "no advertised root for this tileset yet"),
            "{status:?}"
        );
    }

    /// Regression test for the S4 bug: before this slice, download progress
    /// lived inside `ActiveDialog::HexonManager`, so `ChunkReceived`'s
    /// ping-pong only ran `if let ActiveDialog::HexonManager { .. }` — closing
    /// the Hexon Manager silently stalled any in-flight download. With the
    /// dialog CLOSED (`ActiveDialog::None`) throughout, meta/chunk/complete
    /// events must still drive `TilesetDownloads` to completion and queue the
    /// assemble op.
    #[test]
    fn closing_the_hexon_manager_does_not_stall_a_download() {
        let mut app = App::new();
        app.init_resource::<UiManager>()
            .init_resource::<fe_sync::TilesetEventBuffer>()
            .init_resource::<TilesetDownloads>()
            .init_resource::<PendingHexonOps>()
            .add_systems(Update, drain_tileset_events);

        assert!(matches!(
            app.world().resource::<UiManager>().active_dialog,
            ActiveDialog::None
        ));

        let tileset_id = "ts-closed-dialog".to_string();
        // `start_download` (actions/hexon.rs) seeds this before sending
        // `DownloadTileset`; the test drives the event path directly, so it
        // seeds the tracker the same way.
        app.world_mut().resource_mut::<TilesetDownloads>().0.insert(
            tileset_id.clone(),
            DownloadProgress {
                tileset_id: tileset_id.clone(),
                chunks_received: 0,
                total_chunks: 0,
                bytes_received: 0,
                total_bytes_estimate: 0,
                status: DownloadStatus::Queued,
            },
        );
        app.world_mut()
            .resource_mut::<fe_sync::TilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::TilesetMetaReceived {
                peer_id: String::new(),
                tileset_id: tileset_id.clone(),
                meta_json: "{}".into(),
                total_chunks: 2,
                approx_size_bytes: 2048,
                root_hash: "deadbeef".into(),
                chunk_hashes: vec!["a".into(), "b".into()],
                index_path: "irrelevant/index.json".into(),
                ad_publisher_did: String::new(),
            });
        app.update();
        // PAS-P5 S7a: the root resolved, but no bundle has arrived yet — the
        // host is still verifying the signed header and has not sent
        // `ConfirmTilesetDownload`. The tracker must not claim "Downloading"
        // for a download that cannot fetch a single chunk yet.
        assert!(
            matches!(
                app.world().resource::<TilesetDownloads>().0[&tileset_id].status,
                DownloadStatus::Verifying
            ),
            "{:?}",
            app.world().resource::<TilesetDownloads>().0[&tileset_id].status
        );

        app.world_mut()
            .resource_mut::<fe_sync::TilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::ChunkReceived {
                tileset_id: tileset_id.clone(),
                chunk_seq: 0,
                path: "downloads/ts-closed-dialog/0.bundle".into(),
                byte_len: 1024,
            });
        app.world_mut()
            .resource_mut::<fe_sync::TilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::ChunkReceived {
                tileset_id: tileset_id.clone(),
                chunk_seq: 1,
                path: "downloads/ts-closed-dialog/1.bundle".into(),
                byte_len: 1024,
            });
        app.update();

        {
            let downloads = app.world().resource::<TilesetDownloads>();
            let dl = downloads
                .0
                .get(&tileset_id)
                .expect("tracker survives a closed dialog");
            assert_eq!(dl.total_chunks, 2);
            assert_eq!(dl.chunks_received, 2);
            assert_eq!(dl.bytes_received, 2048);
            assert!(
                matches!(dl.status, DownloadStatus::Downloading),
                "the first ChunkReceived flips Verifying -> Downloading: {:?}",
                dl.status
            );
        }
        assert!(
            matches!(
                app.world().resource::<UiManager>().active_dialog,
                ActiveDialog::None
            ),
            "dialog stayed closed the whole time — this is the regression's precondition"
        );

        let index_path = std::path::PathBuf::from("downloads/ts-closed-dialog/index.json");
        let bundle_paths = vec![
            std::path::PathBuf::from("downloads/ts-closed-dialog/0.bundle"),
            std::path::PathBuf::from("downloads/ts-closed-dialog/1.bundle"),
        ];
        app.world_mut()
            .resource_mut::<fe_sync::TilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::TilesetDownloadComplete {
                tileset_id: tileset_id.clone(),
                index_path: index_path.clone(),
                bundle_paths: bundle_paths.clone(),
            });
        app.update();

        let ops = &app.world().resource::<PendingHexonOps>().0;
        assert_eq!(ops.len(), 1, "exactly one assemble op queued: {ops:?}");
        match &ops[0] {
            HexonOp::AssembleDownload {
                tileset_id: id,
                index_path: idx,
                bundle_paths: paths,
            } => {
                assert_eq!(id, &tileset_id);
                assert_eq!(idx, &index_path);
                assert_eq!(paths, &bundle_paths);
            }
            other => panic!("expected AssembleDownload, got {other:?}"),
        }
    }
}
