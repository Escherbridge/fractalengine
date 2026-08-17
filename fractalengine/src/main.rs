use std::sync::{Arc, Mutex};

use fe_identity::NodeIdentity;
use fe_runtime::app::{
    ApiCommandReceiver, ApiCommandSender, BevyHandles, PendingApiRequests, TransformBroadcastSender,
};
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
use fe_runtime::PeerRegistry;
use fe_ui::plugin::LocalUserRole;
use tracing_subscriber::EnvFilter;

mod asset_bridge;
mod gpx_bridge;
mod panic_log;
mod terrain_bridge;

/// Default SurrealKV database path. Must match the path used by
/// `fe_database::spawn_db_thread_with_sync` so the API reader opens the same store.
const DB_PATH: &str = "data/fractalengine.db";

fn main() {
    // Captures the payload of any panic (esp. in-egui-pass panics that
    // otherwise only surface as "Encountered a panic in system" + abort) to
    // `data/panic.log`. Installed first so it covers every subsequent line.
    panic_log::install();

    // Durability: default SurrealKV's fsync mode unless the operator overrode it.
    // Valid values are `never` | `every` | a duration >100ms; `"true"` is invalid
    // and would brick startup. See src/AGENTS.md §durability. Must run before any
    // datastore opens (the DB thread below).
    if std::env::var("SURREAL_DATASTORE_SYNC_DATA").is_err() {
        std::env::set_var("SURREAL_DATASTORE_SYNC_DATA", "every");
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let ch = ChannelHandles::new();

    let _net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);

    // P2P Mycelium Phase A+B: local content-addressed blob store, shared with
    // the DB thread and the Bevy blob:// asset source.
    let blob_store: fe_database::BlobStoreHandle =
        std::sync::Arc::new(fe_sync::FsBlobStore::open_default().expect("open blob store"));

    // Secret store: OS keychain on desktop.
    let secret_store: Arc<dyn fe_identity::SecretStore> =
        Arc::new(fe_identity::OsKeystoreBackend::new());

    // Load or generate a persistent node keypair. The 32-byte seed is stored
    // in the secret store so the node identity survives across launches,
    // preserving P2P reconnection and invite verification.
    let node_kp = match fe_identity::load_or_generate_keypair(&secret_store, "node_keypair") {
        Ok(kp) => kp,
        Err(e) => {
            tracing::warn!(
                "Could not load/store keypair in secret store, generating ephemeral: {e}"
            );
            fe_identity::NodeKeypair::generate()
        }
    };
    let iroh_secret = node_kp.to_iroh_secret();
    let local_did = node_kp.to_did_key();
    let api_verifying_key = node_kp.verifying_key();

    // Phase F: create a second keypair from the same seed for the DB thread.
    // NodeKeypair is not Clone, so we recreate from the same seed bytes.
    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");

    // Phase E: replication channel (DB → sync bridge).
    // The DB thread emits ReplicationEvents; we bridge them to SyncCommand::WriteRowEntry
    // after the sync command sender is created below.
    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);

    // Scene change broadcast: DB thread emits CUD deltas, API thread fans out to WS clients.
    let (entity_change_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(256);

    // Node lifecycle seam (node_lifecycle_addressing_20260725 FR-6): the DB
    // thread emits create/promote/tombstone/reflow events onto `lifecycle_tx`;
    // the binary owns the receiver half (fe-database can't depend on fe-sync, so
    // it declares its own sender type and the binary bridges the two halves —
    // see fe-sync/src/AGENTS.md §lifecycle-forwarding).
    let (lifecycle_tx, lifecycle_rx) = fe_sync::lifecycle_channel(256);

    // Cloned for the seed-gate replay below — the original moves into the DB thread.
    let db_res_tx_replay = ch.db_res_tx.clone();
    let _db_thread = match fe_database::spawn_db_thread_with_sync_and_lifecycle(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
        Some(secret_store.clone()),
        Some(entity_change_tx.clone()),
        None, // use default db_path
        Some(lifecycle_tx),
    ) {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!("Failed to start database thread: {}", e);
            eprintln!("Fatal error: Could not initialize database.\n{}", e);
            eprintln!("Please ensure the 'data/' directory is writable and the path is valid.");
            std::process::exit(1);
        }
    };

    // `lifecycle_rx` rides into `BevyHandles` below: fe-runtime pumps it into
    // `Messages<LifecycleEvent>` so in-app consumers (fe-ui PathReflow re-flow,
    // T5 reporting) observe create/promote/tombstone/reflow (FR-6). The op-log
    // stays the durable source of truth.

    // Gate seed and preserve its DB results; see AGENTS.md §entity-store-analytics.
    if ch.db_cmd_tx.send(DbCommand::Seed).is_err() {
        tracing::error!("Database command channel closed before seed");
        eprintln!("Fatal error: database command channel closed before seed.");
        std::process::exit(1);
    }
    let mut startup_db_results = Vec::new();
    loop {
        match ch
            .db_res_rx
            .recv_timeout(std::time::Duration::from_secs(30))
        {
            Ok(result @ fe_runtime::messages::DbResult::Seeded { .. }) => {
                startup_db_results.push(result);
                break;
            }
            Ok(fe_runtime::messages::DbResult::Error(error)) => {
                tracing::error!(%error, "Database seed failed");
                eprintln!("Fatal error: database seed failed: {error}");
                std::process::exit(1);
            }
            Ok(result) => startup_db_results.push(result),
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                tracing::error!("Database seed did not complete within 30 seconds");
                eprintln!("Fatal error: database seed timed out during startup.");
                std::process::exit(1);
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                tracing::error!("Database result channel closed during seed");
                eprintln!("Fatal error: database result channel closed during startup.");
                std::process::exit(1);
            }
        }
    }
    for result in startup_db_results {
        if db_res_tx_replay.send(result).is_err() {
            tracing::error!("Database result channel closed while replaying startup result");
            eprintln!("Fatal error: database result channel closed during startup.");
            std::process::exit(1);
        }
    }

    // ---- P2P Mycelium Phase D: sync thread (iroh endpoint) ----
    let (sync_cmd_tx, sync_cmd_rx) = crossbeam::channel::bounded(256);
    let (sync_evt_tx, sync_evt_rx) = crossbeam::channel::bounded(256);
    tracing::info!(
        node_id = %iroh_secret.public(),
        "Starting sync thread"
    );

    let _sync_thread = fe_sync::spawn_sync_thread(
        iroh_secret,
        blob_store.clone(),
        sync_cmd_rx,
        sync_evt_tx,
        local_did,
    );

    // Phase E: bridge replication events from DB thread to sync thread.
    // try_send + drop counter so a stalled sync thread never blocks this hop
    // (see fe-database/src/AGENTS.md §replication-backpressure).
    {
        let sync_tx_for_repl = sync_cmd_tx.clone();
        std::thread::spawn(move || {
            let mut dropped: u64 = 0;
            while let Ok(evt) = repl_rx.recv() {
                match sync_tx_for_repl.try_send(fe_sync::SyncCommand::WriteRowEntry {
                    verse_id: evt.verse_id,
                    table: evt.table,
                    record_id: evt.record_id,
                    content_hash: evt.content_hash,
                }) {
                    Ok(()) => {}
                    Err(crossbeam::channel::TrySendError::Full(_)) => {
                        dropped += 1;
                        tracing::warn!(
                            dropped_total = dropped,
                            "DB→sync replication bridge full — dropping event"
                        );
                    }
                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => break,
                }
            }
        });
    }

    // Wire the on_miss callback: when BlobAssetReader can't find a blob
    // locally, it sends a FetchBlob command to the sync thread.
    let sync_cmd_for_miss = sync_cmd_tx.clone();
    let on_miss: fe_runtime::bevy_blob_reader::OnMissCallback = Arc::new(move |hash| {
        // We don't know the verse_id at the asset-reader level, so use a
        // placeholder. Phase F will route through VersePeers instead.
        sync_cmd_for_miss
            .send(fe_sync::SyncCommand::FetchBlob {
                hash,
                verse_id: String::new(),
            })
            .ok();
    });

    // Clone the blob-store handle for the API thread (asset endpoints) and the
    // asset-download bridge before it moves into BevyHandles below. All share one
    // content-addressed store.
    let blob_store_for_api = blob_store.clone();
    let blob_store_for_assets = blob_store.clone();

    let mut app = fe_runtime::app::build_app(BevyHandles {
        net_cmd_tx: ch.net_cmd_tx,
        net_evt_rx: ch.net_evt_rx,
        db_cmd_tx: ch.db_cmd_tx,
        db_res_rx: ch.db_res_rx,
        blob_store: Some(blob_store),
        on_blob_miss: Some(on_miss),
        lifecycle_rx: Some(lifecycle_rx),
    });

    // GUI-only plugins (removed from fe-runtime so the headless relay can skip them)
    app.add_plugins(bevy_egui::EguiPlugin::default());
    app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());

    // Insert sync resources into Bevy world
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Insert identity, peer, and secret store resources
    app.insert_resource(NodeIdentity::new(node_kp));
    app.insert_resource(PeerRegistry::default());
    app.insert_resource(LocalUserRole::default());
    app.insert_resource(fe_database::SecretStoreRes(secret_store));

    // ---- API Gateway thread ----
    // Channel: API thread sends ApiCommand -> tx, Bevy drains from rx.
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
    let (transform_broadcast_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::TransformUpdate>(1024);
    // Revocation broadcast: Bevy sends revoked JTIs, API thread updates its cache.
    let (revocation_tx, revocation_rx) = tokio::sync::broadcast::channel::<String>(64);

    // One shared cache; see AGENTS.md §entity-store-analytics.
    let entity_store = Arc::new(fe_entity_store::EntityStore::new());

    let (scene_change_tx_bevy, scene_change_rx_bevy) =
        crossbeam::channel::bounded::<fe_runtime::messages::SceneChange>(256);
    {
        let mut entity_change_rx = entity_change_tx.subscribe();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("scene change bridge runtime");
            rt.block_on(async move {
                tracing::info!("Scene change bridge started — feeding EntityStore");
                loop {
                    match entity_change_rx.recv().await {
                        Ok(change) => match scene_change_tx_bevy.try_send(change) {
                            Ok(()) => {}
                            Err(crossbeam::channel::TrySendError::Full(_)) => {
                                tracing::warn!("Scene change bridge: channel full — dropping");
                            }
                            Err(crossbeam::channel::TrySendError::Disconnected(_)) => break,
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Scene change bridge lagged by {n}");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        });
    }

    // Optional direct reader backs analytics auth/hydration; failure must not block the editor. See AGENTS.md §entity-store-analytics.
    let api_db_reader: Option<Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>> = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("api_db_reader runtime");
        match rt.block_on(async {
            let db =
                surrealdb::Surreal::new::<surrealdb::engine::local::SurrealKv>(DB_PATH).await?;
            db.use_ns("fractalengine").use_db("fractalengine").await?;
            Ok::<_, surrealdb::Error>(db)
        }) {
            Ok(db) => match rt.block_on(hydrate_entity_store(&db, &entity_store)) {
                Ok(node_count) => {
                    tracing::info!(
                        node_count,
                        "Opened read-only SurrealKV connection and hydrated EntityStore for API gateway"
                    );
                    Some(Arc::new(db))
                }
                Err(error) => {
                    tracing::error!(
                        %error,
                        "Could not hydrate EntityStore; analytics API will remain unavailable"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::error!(
                    "Could not open local API read connection; analytics API will remain unavailable: {e}"
                );
                None
            }
        }
    };

    // ---- Terrain tileset registry (local .hexon store) ----
    let tileset_registry: Option<Arc<fe_terrain::tiles::TilesetRegistry>> =
        match fe_terrain::tiles::HexonStore::new() {
            Ok(store) => {
                let registry = fe_terrain::tiles::TilesetRegistry::new(store);
                let loaded = registry.load_all();
                tracing::info!("Tileset registry loaded {} tileset(s)", loaded.len());
                Some(Arc::new(registry))
            }
            Err(e) => {
                tracing::warn!("Hexon store unavailable — terrain tileset wiring skipped: {e}");
                None
            }
        };

    let _api_thread = fe_api::spawn_api_thread(fe_api::ApiConfig {
        bind_addr: "127.0.0.1:8765".to_string(),
        api_cmd_tx: api_cmd_tx.clone(),
        transform_broadcast_tx: transform_broadcast_tx.clone(),
        verifying_key: api_verifying_key,
        revocation_rx,
        blob_store: Some(blob_store_for_api),
        cors_origins: None, // defaults to localhost-only
        entity_change_tx: entity_change_tx.clone(),
        api_db_reader,
        entity_store: Some(Arc::clone(&entity_store)),
        tileset_registry: tileset_registry.clone(),
        hexon_registry: None,
        announcement_store: None,
    });

    // ---- Entity Store (in-memory hot cache) ----
    app.insert_resource(SharedEntityStore(entity_store));

    app.insert_resource(SceneChangeReceiver(scene_change_rx_bevy));

    app.insert_resource(fe_runtime::app::RevocationBroadcastSender(revocation_tx));
    app.insert_resource(ApiCommandReceiver(Arc::new(Mutex::new(api_cmd_rx))));
    app.insert_resource(ApiCommandSender(api_cmd_tx));
    app.insert_resource(TransformBroadcastSender(transform_broadcast_tx.clone()));

    // Bridge: tokio broadcast → crossbeam channel so Bevy can poll inbound
    // API transform updates without a tokio runtime.
    let (inbound_tx, inbound_rx) =
        crossbeam::channel::bounded::<fe_runtime::messages::TransformUpdate>(256);
    {
        let mut rx = transform_broadcast_tx.subscribe();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("inbound transform bridge runtime");
            rt.block_on(async move {
                tracing::info!("Transform bridge started — listening for broadcasts");
                loop {
                    match rx.recv().await {
                        Ok(update) => {
                            tracing::debug!(
                                "Bridge recv: node={} pos=[{:.2},{:.2},{:.2}]",
                                update.node_id, update.position[0], update.position[1], update.position[2],
                            );
                            match inbound_tx.try_send(update) {
                                Ok(()) => {}
                                Err(crossbeam::channel::TrySendError::Full(_)) => {
                                    tracing::warn!("Bridge: inbound crossbeam channel full — dropping transform");
                                }
                                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                                    tracing::error!("Bridge: inbound crossbeam channel disconnected");
                                    break;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Bridge lagged by {n} transform broadcasts");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        });
    }
    app.insert_resource(fe_runtime::app::InboundTransformReceiver(inbound_rx));

    app.init_resource::<PendingApiRequests>();
    app.add_systems(
        bevy::prelude::Update,
        (
            fe_runtime::app::drain_api_commands,
            drain_scene_changes_to_store,
        ),
    );

    // Add 3D viewport (camera, grid, lighting, axis gizmo) and UI overlay
    app.add_plugins(fe_renderer::viewport::ViewportPlugin);
    app.add_plugins(fe_ui::plugin::GardenerConsolePlugin);
    // Terrain runtime: petal map assignments + tileset registry bridge.
    app.add_plugins(fe_terrain::terrain_plugin::TerrainPlugin);
    // Splat view: synthesized terrain splats + Mesh/Splats/Hybrid view modes.
    app.add_plugins(fe_terrain::splat::SplatPlugin);
    if let Some(ref registry) = tileset_registry {
        app.insert_resource(fe_terrain::petal_binding::SharedTilesetRegistry(
            registry.clone(),
        ));
    }
    app.add_systems(
        bevy::prelude::Update,
        (
            terrain_bridge::bridge_petal_terrain,
            terrain_bridge::drain_hexon_ops,
        ),
    );
    // Asset-download bridge: drains fe-ui's queued node-asset downloads, copying
    // resolved blobs into the user's downloads folder. See src/AGENTS.md §assets.
    app.insert_resource(asset_bridge::AssetBlobStore(blob_store_for_assets));
    app.add_systems(bevy::prelude::Update, asset_bridge::drain_asset_ops);
    // GPX import bridge: drains fe-ui's queued GPX imports into petal-bound
    // track + waypoint nodes. See src/AGENTS.md §gpx.
    app.init_resource::<gpx_bridge::PendingGpxImports>();
    app.add_systems(
        bevy::prelude::Update,
        (gpx_bridge::drain_gpx_ops, gpx_bridge::advance_gpx_imports),
    );
    // Path editor bridge: drains fe-ui's queued path edits (create/append/
    // remove/annotate/export/delete) and materializes persisted gpx_points
    // on petal load. See src/AGENTS.md §gpx and conductor/tracks/gpx_path_editor_20260711.
    app.init_resource::<gpx_bridge::PendingPathEdits>();
    app.add_systems(
        bevy::prelude::Update,
        (
            gpx_bridge::drain_path_ops,
            gpx_bridge::advance_path_edits,
            gpx_bridge::request_petal_gpx_materialization,
            gpx_bridge::advance_path_materialization,
            // Tag rendered track ribbons selectable (bridges fe-terrain's
            // GpxTrackLine → fe-ui's SpawnedNodeMarker for the viewport picker).
            gpx_bridge::tag_track_lines_selectable,
        ),
    );
    // WebView portal: inline wry overlay + petal portal lifecycle systems.
    app.add_plugins(fe_webview::plugin::WebViewPlugin);
    app.add_plugins(fe_webview::petal_portal::PetalPortalPlugin);

    app.run();
}

// ---------------------------------------------------------------------------
// EntityStore bridge: scene change receiver + drain system
// ---------------------------------------------------------------------------

#[derive(bevy::prelude::Resource)]
struct SceneChangeReceiver(crossbeam::channel::Receiver<fe_runtime::messages::SceneChange>);

/// Arc-backed Bevy handle for the API and scene-change hot cache.
#[derive(bevy::prelude::Resource, Clone)]
struct SharedEntityStore(Arc<fe_entity_store::EntityStore>);

/// Hydrate the shared analytics cache from every live local node before API startup.
async fn hydrate_entity_store(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    store: &fe_entity_store::EntityStore,
) -> anyhow::Result<usize> {
    let query = db.query(
        // `created_at` is selected only to satisfy SurrealDB 3.x, which rejects an ORDER BY
        // idiom that the projection does not carry. Rows land as `serde_json::Value` and
        // `snapshot_from_node_row` reads keys by name, so the extra column is inert.
        "SELECT node_id, petal_id, position, elevation, rotation, scale, properties, created_at \
         FROM node WHERE tombstone = NONE ORDER BY created_at ASC",
    );
    let mut response = tokio::time::timeout(std::time::Duration::from_secs(10), query)
        .await
        .map_err(|_| anyhow::anyhow!("local node hydration query timed out"))?
        .map_err(|error| anyhow::anyhow!("local node hydration query failed: {error}"))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("local node hydration response failed: {error}"))?;
    let hydrated_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    for row in &rows {
        store.upsert(snapshot_from_node_row(row, hydrated_at_ms)?);
    }
    Ok(rows.len())
}

/// Convert a validated local node row into the EntityStore's analytics shape.
fn snapshot_from_node_row(
    row: &serde_json::Value,
    updated_at_ms: u64,
) -> anyhow::Result<fe_entity_store::EntitySnapshot> {
    let node_id = required_row_string(row, "node_id")?;
    let petal_id = required_row_string(row, "petal_id")?;
    let coordinates = row
        .pointer("/position/coordinates")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("node {node_id} has no position.coordinates array"))?;
    let position = [
        required_row_number(coordinates, 0, "position.coordinates")?,
        row.get("elevation")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("node {node_id} has no elevation"))? as f32,
        required_row_number(coordinates, 1, "position.coordinates")?,
    ];
    let rotation = required_row_array3(row, "rotation")?;
    let scale = required_row_array3(row, "scale")?;
    let properties = match row.get("properties") {
        Some(value) if value.is_null() => None,
        Some(value) if value.is_object() => Some(value.clone()),
        Some(_) => anyhow::bail!("node {node_id} has non-object properties"),
        None => None,
    };

    Ok(fe_entity_store::EntitySnapshot {
        node_id,
        petal_id,
        position,
        rotation,
        scale,
        properties,
        updated_at_ms,
        node_log: Vec::new(),
    })
}

/// Read a nonempty string field from a direct local node row.
fn required_row_string(row: &serde_json::Value, field: &str) -> anyhow::Result<String> {
    let value = row
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("node row has no {field}"))?;
    Ok(value.to_string())
}

/// Read one finite f32 from a direct local node row array.
fn required_row_number(
    values: &[serde_json::Value],
    index: usize,
    field: &str,
) -> anyhow::Result<f32> {
    let value = values
        .get(index)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow::anyhow!("node row has invalid {field}[{index}]"))?;
    let value = value as f32;
    if !value.is_finite() {
        anyhow::bail!("node row has invalid {field}[{index}]");
    }
    Ok(value)
}

/// Read the three visible transform components used by the analytics snapshot.
fn required_row_array3(row: &serde_json::Value, field: &str) -> anyhow::Result<[f32; 3]> {
    let values = row
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("node row has no {field} array"))?;
    Ok([
        required_row_number(values, 0, field)?,
        required_row_number(values, 1, field)?,
        required_row_number(values, 2, field)?,
    ])
}

/// Bevy system: drain scene change events from the bridge channel into the
/// `EntityStore` hot cache each frame.
fn drain_scene_changes_to_store(
    receiver: bevy::prelude::Res<SceneChangeReceiver>,
    store: bevy::prelude::Res<SharedEntityStore>,
) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    while let Ok(change) = receiver.0.try_recv() {
        // Convert fe_runtime::messages::SceneChange to fe_entity_store::SceneChange
        let store_change = match change {
            fe_runtime::messages::SceneChange::NodeAdded { node } => {
                fe_entity_store::SceneChange::NodeAdded {
                    node: fe_entity_store::NodeSnapshot {
                        node_id: node.node_id,
                        petal_id: node.petal_id,
                        name: node.name,
                        position: node.position,
                        rotation: node.rotation,
                        scale: node.scale,
                        has_asset: node.has_asset,
                        asset_path: node.asset_path,
                    },
                }
            }
            fe_runtime::messages::SceneChange::NodeRemoved { node_id, .. } => {
                fe_entity_store::SceneChange::NodeRemoved { node_id }
            }
            fe_runtime::messages::SceneChange::NodeRenamed {
                node_id, new_name, ..
            } => fe_entity_store::SceneChange::NodeRenamed { node_id, new_name },
            fe_runtime::messages::SceneChange::NodeTransform {
                node_id,
                position,
                rotation,
                scale,
                ..
            } => fe_entity_store::SceneChange::NodeTransform {
                node_id,
                position,
                rotation,
                scale,
            },
            fe_runtime::messages::SceneChange::TransformFailed {
                node_id,
                position,
                rotation,
                scale,
                ..
            } => fe_entity_store::SceneChange::TransformFailed {
                node_id,
                position,
                rotation,
                scale,
            },
            fe_runtime::messages::SceneChange::PropertyChanged {
                node_id,
                key,
                value,
                ..
            } => fe_entity_store::SceneChange::PropertyChanged {
                node_id,
                key,
                value,
            },
        };
        store.0.apply_scene_change(&store_change, now_ms);
    }
}

#[cfg(test)]
mod analytics_hydration_tests {
    use super::*;

    #[test]
    fn node_row_hydrates_the_entity_store_shape() {
        let row = serde_json::json!({
            "node_id": "node-1",
            "petal_id": "petal-1",
            "position": { "coordinates": [1.0, 3.0] },
            "elevation": 2.0,
            "rotation": [0.1, 0.2, 0.3, 1.0],
            "scale": [1.0, 2.0, 3.0],
            "properties": { "kind": "marker" }
        });

        let snapshot = snapshot_from_node_row(&row, 42).expect("valid node row");

        assert_eq!(snapshot.node_id, "node-1");
        assert_eq!(snapshot.petal_id, "petal-1");
        assert_eq!(snapshot.position, [1.0, 2.0, 3.0]);
        assert_eq!(snapshot.rotation, [0.1, 0.2, 0.3]);
        assert_eq!(snapshot.scale, [1.0, 2.0, 3.0]);
        assert_eq!(
            snapshot.properties,
            Some(serde_json::json!({ "kind": "marker" }))
        );
        assert_eq!(snapshot.updated_at_ms, 42);
    }

    #[test]
    fn malformed_node_row_rejects_startup_hydration() {
        let row = serde_json::json!({
            "node_id": "node-1",
            "petal_id": "petal-1",
            "position": { "coordinates": [1.0] },
            "elevation": 2.0,
            "rotation": [0.0, 0.0, 0.0],
            "scale": [1.0, 1.0, 1.0]
        });

        assert!(snapshot_from_node_row(&row, 42).is_err());
    }

    #[tokio::test]
    async fn hydration_loads_each_live_node_before_api_startup() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        fe_database::schema::apply_all(&db)
            .await
            .expect("apply schema");
        db.query(
            "CREATE node CONTENT { \
             node_id: 'live-node', petal_id: 'petal-1', \
             position: <geometry<point>> [1.0, 3.0], elevation: 2.0, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
             interactive: true, created_at: '2026-08-08T00:00:00Z' }",
        )
        .await
        .expect("create live node")
        .check()
        .expect("live node query succeeded");
        db.query(
            "CREATE node CONTENT { \
             node_id: 'deleted-node', petal_id: 'petal-1', \
             position: <geometry<point>> [4.0, 6.0], elevation: 5.0, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
             interactive: true, created_at: '2026-08-08T00:00:00Z', \
             tombstone: { hlc: 1, source_did: 'did:key:test' } }",
        )
        .await
        .expect("create tombstoned node")
        .check()
        .expect("tombstoned node query succeeded");

        let store = fe_entity_store::EntityStore::new();
        let count = hydrate_entity_store(&db, &store)
            .await
            .expect("hydrate live nodes");

        assert_eq!(count, 1);
        assert_eq!(store.node_count(), 1);
        assert_eq!(store.get("live-node").unwrap().position, [1.0, 2.0, 3.0]);
        assert!(store.get("deleted-node").is_none());
    }
}
