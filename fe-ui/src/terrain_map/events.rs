//! Drains tileset distribution events from the sync thread into the Hexon
//! Manager dialog's available/download state. See `fe-ui/src/AGENTS.md`
//! §terrain-map.

use bevy::prelude::*;

use super::dto::{AvailableTilesetDto, DownloadStatus};
use crate::dialogs::ActiveDialog;

/// Drains tileset distribution events from the sync thread and updates
/// the Hexon Manager dialog's available/download state.
pub(crate) fn drain_tileset_events(
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut tileset_buf: ResMut<fe_sync::TilesetEventBuffer>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    if tileset_buf.events.is_empty() {
        return;
    }

    let events: Vec<fe_sync::SyncEvent> = tileset_buf.events.drain(..).collect();

    for evt in events {
        match evt {
            fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id,
                advertisements_json,
            } => {
                // Parse advertisements and merge into available tilesets
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
                        available_tilesets.push(AvailableTilesetDto {
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
                ..
            } => {
                // Update the download tracker with chunk count and start requesting chunks
                if let ActiveDialog::HexonManager {
                    ref mut download_progress,
                    ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.total_chunks = total_chunks;
                        dl.total_bytes_estimate = approx_size_bytes;
                        dl.status = DownloadStatus::Downloading;
                    }
                }
                // Request the first chunk
                if let Some(ref sender) = sync_sender {
                    sender
                        .0
                        .send(fe_sync::SyncCommand::RequestChunk {
                            peer_id: String::new(),
                            tileset_id,
                            chunk_seq: 0,
                        })
                        .ok();
                }
            }
            fe_sync::SyncEvent::ChunkReceived {
                tileset_id,
                chunk_seq,
                chunk_bytes,
            } => {
                let chunk_size = chunk_bytes.len() as u64;
                let mut request_next = None;

                if let ActiveDialog::HexonManager {
                    ref mut download_progress,
                    ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.chunks_received += 1;
                        dl.bytes_received += chunk_size;

                        if dl.chunks_received >= dl.total_chunks {
                            dl.status = DownloadStatus::Verifying;
                        } else {
                            // Request next missing chunk
                            request_next = Some((tileset_id.clone(), dl.chunks_received));
                        }
                    }
                }

                // Request next chunk if needed
                if let (Some((ts_id, next_seq)), Some(ref sender)) = (request_next, &sync_sender) {
                    sender
                        .0
                        .send(fe_sync::SyncCommand::RequestChunk {
                            peer_id: String::new(),
                            tileset_id: ts_id,
                            chunk_seq: next_seq,
                        })
                        .ok();
                }

                bevy::log::debug!(
                    "Chunk {chunk_seq} received for tileset {tileset_id} ({chunk_size} bytes)"
                );
            }
            fe_sync::SyncEvent::ChunkFailed {
                tileset_id,
                chunk_seq,
                reason,
            } => {
                bevy::log::warn!("Chunk {chunk_seq} failed for tileset {tileset_id}: {reason}");
                if let ActiveDialog::HexonManager {
                    ref mut download_progress,
                    ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.status = DownloadStatus::Failed(format!(
                            "Chunk {} failed: {}",
                            chunk_seq, reason
                        ));
                    }
                }
            }
            _ => {} // other SyncEvent variants handled elsewhere
        }
    }
}
