//! Scenario 16: Two-Peer Tileset Stream — see fe-test-harness/src/AGENTS.md §scenarios.
//!
//! Campaign plan `p2p_asset_streaming_20260718` S4: a real end-to-end proof
//! that a peer can fetch a tileset from another peer as a verified HashSeq of
//! bundles, and that a resumed download needs no provider once it is already
//! complete on disk. Exercises the real iroh transport (loopback, `TestPeer`)
//! rather than the in-memory `InMemoryTransport` S2's own tests use.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
use fe_format::{HexonArchiveData, SchemaDefinition};
use fe_runtime::messages::{DbCommand, DbResult};
use fe_sync::messages::{SyncCommand, SyncEvent};
use fe_terrain::tiles::{bundle_tileset, HexonStore, TilesetRegistry, DEFAULT_BUNDLE_BYTES};

use crate::peer::TestPeer;
use crate::TestResult;

const NAME: &str = "two_peer_tileset_stream";
const TILESET_ID: &str = "s4-harness-stream-tileset";
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Same shape as `fe-terrain/tests/tileset_bundles_test.rs`'s fixture,
/// shrunk to 2 tiles — the harness proves host wiring over a real network,
/// not the bundler's exhaustiveness (that's S1's job).
fn synthetic_tileset() -> HexonArchiveData {
    let elevation_tiles = vec![
        ("10/0/0".to_string(), vec![1u8, 2, 3, 4, 5, 6, 7, 8]),
        ("10/0/1".to_string(), vec![9u8, 10, 11, 12, 13, 14, 15, 16]),
    ];
    let meta = TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 10,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: false,
        tile_count: elevation_tiles.len() as u32,
        satellite_tile_count: 0,
        region_name: "Harness Stream Test".into(),
        parent_tileset: None,
        chunk_index: None,
        native_scale: None,
        ground_sample_distance_m: None,
        crs: None,
        scale_bounds: None,
    };
    HexonArchiveData {
        manifest: HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: TILESET_ID.into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:zharness".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Harness Stream Test".into(),
            description: None,
            tags: vec!["terrain".into(), "tileset".into()],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        },
        entries: vec![],
        license: None,
        nodes: vec![],
        field_defs: vec![],
        schema: SchemaDefinition { field_defs: vec![] },
        assets: vec![],
        terrain_config: None,
        gpx_files: vec![],
        geojson_overlays: vec![],
        tileset_meta: Some(meta),
        elevation_tiles,
        satellite_tiles: vec![],
        integrity: fe_format::IntegrityStatus::Unsealed,
    }
}

/// Bundle the synthetic tileset to `dir`, returning `(index_path, bundle_paths)`.
fn bundle_to_disk(dir: &std::path::Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    std::fs::create_dir_all(dir)?;
    let bundles = bundle_tileset(&synthetic_tileset(), DEFAULT_BUNDLE_BYTES)?;
    let index_path = dir.join("index.json");
    std::fs::write(&index_path, &bundles.index_json)?;
    let mut bundle_paths = Vec::with_capacity(bundles.bundles.len());
    for (seq, bytes) in bundles.bundles.iter().enumerate() {
        let path = dir.join(format!("{seq}.bundle"));
        std::fs::write(&path, bytes)?;
        bundle_paths.push(path);
    }
    Ok((index_path, bundle_paths))
}

/// Assemble a completed download into a fresh store and return every
/// elevation tile byte-for-byte, keyed by cache key — for comparing against
/// the synthetic source.
fn assemble_and_read_back(
    store_dir: &std::path::Path,
    index_path: &std::path::Path,
    bundle_paths: &[PathBuf],
) -> Result<Vec<(String, Vec<u8>)>> {
    let index_json = std::fs::read(index_path)?;
    let store = HexonStore::with_dir(store_dir.to_path_buf())?;
    fe_terrain::tiles::assemble_bundles_from_index_json(
        &store,
        TILESET_ID,
        &index_json,
        bundle_paths,
    )?;
    let registry = TilesetRegistry::new(store);
    registry.reload(TILESET_ID)?;
    let mut out = Vec::new();
    for (key, _) in synthetic_tileset().elevation_tiles {
        let (_, x, y) = {
            let mut parts = key.splitn(3, '/');
            (
                parts.next().unwrap().parse::<u8>().unwrap(),
                parts.next().unwrap().parse::<u32>().unwrap(),
                parts.next().unwrap().parse::<u32>().unwrap(),
            )
        };
        let tile = registry
            .get_tile(TILESET_ID, 10, x, y)
            .ok_or_else(|| anyhow::anyhow!("missing tile {key} after assemble"))?;
        out.push((key, tile));
    }
    Ok(out)
}

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;

    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

    let alice_addr = alice.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!("alice is offline — this scenario requires a real online endpoint")
    })?;
    bob.sync_node_addr.clone().ok_or_else(|| {
        anyhow::anyhow!("bob is offline — this scenario requires a real online endpoint")
    })?;

    // 1. Alice creates a verse (durable row + namespace secret) purely as the
    //    gossip topic tileset ads ride on (S3: ads share the verse topic,
    //    tagged `kind` so they never collide with compute envelopes).
    alice.send(DbCommand::CreateVerse {
        name: "Tileset Stream Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        Duration::from_secs(30),
    )?;
    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, .. } => id.clone(),
        _ => unreachable!(),
    };

    let ns_secret_hex = alice
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow::anyhow!("verse secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    // 2. Bob joins the namespace (dialing alice), alice opens hers passively —
    //    this is also what subscribes BOTH to the verse's gossip topic
    //    (`subscribe_to_verse_gossip_topic` runs before the replica open, see
    //    fe-sync/src/AGENTS.md §gossip-bootstrap), which is what tileset ads
    //    ride on.
    bob.sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex.clone(),
            namespace_secret: Some(ns_secret_hex.clone()),
            bootstrap_peers: vec![alice_addr.clone()],
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to bob: {e}"))?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex,
            namespace_secret: Some(ns_secret_hex),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow::anyhow!("failed to send OpenVerseReplica to alice: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));

    // 3. Alice bundles a synthetic tileset (256 KiB default cap — these tiny
    //    tiles ship as one bundle, S1's `a_tiny_tileset_ships_as_one_bundle`
    //    invariant) and publishes it.
    let (alice_index_path, alice_bundle_paths) = bundle_to_disk(&tmp.path().join("alice-bundles"))?;
    alice
        .sync_cmd_tx
        .send(SyncCommand::PublishTileset {
            tileset_id: TILESET_ID.to_string(),
            index_path: alice_index_path,
            bundle_paths: alice_bundle_paths,
        })
        .map_err(|e| anyhow::anyhow!("failed to send PublishTileset to alice: {e}"))?;
    alice.wait_sync_event(
        |e| matches!(e, SyncEvent::TilesetPublished { tileset_id, .. } if tileset_id == TILESET_ID),
        SYNC_TIMEOUT,
    )?;

    // 4. Bob waits for the advertisement over the real gossip transport
    //    (not a manual copy — this is the one difference from the
    //    pre-existing two_peer_* scenarios, which simulate the network leg).
    bob.wait_sync_event(
        |e| {
            matches!(e, SyncEvent::PeerTilesetAdvertisement { advertisements_json, .. }
                if advertisements_json.contains(TILESET_ID))
        },
        SYNC_TIMEOUT,
    )?;

    // 5. Bob downloads: meta resolution + every bundle, driven entirely by
    //    the sync thread (S3) — the harness never ping-pongs RequestChunk.
    bob.sync_cmd_tx
        .send(SyncCommand::DownloadTileset {
            tileset_id: TILESET_ID.to_string(),
            provider: Some(alice_addr.clone()),
        })
        .map_err(|e| anyhow::anyhow!("failed to send DownloadTileset to bob: {e}"))?;
    let complete = bob.wait_sync_event(
        |e| matches!(e, SyncEvent::TilesetDownloadComplete { tileset_id, .. } if tileset_id == TILESET_ID),
        SYNC_TIMEOUT,
    )?;
    let (index_path, bundle_paths) = match complete {
        SyncEvent::TilesetDownloadComplete {
            index_path,
            bundle_paths,
            ..
        } => (index_path, bundle_paths),
        _ => unreachable!(),
    };

    // 6. Assemble into a fresh store and verify every tile byte-equals
    //    alice's source.
    let source = synthetic_tileset();
    let bob_store_dir = tmp.path().join("bob-hexons");
    let read_back = assemble_and_read_back(&bob_store_dir, &index_path, &bundle_paths)?;
    for (key, bytes) in &read_back {
        let expected = source
            .elevation_tiles
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| anyhow::anyhow!("unexpected tile key {key}"))?;
        if bytes != &expected {
            return Ok(TestResult::fail(
                NAME,
                &format!("tile {key} bytes differ after assemble"),
            ));
        }
    }
    if read_back.len() != source.elevation_tiles.len() {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "expected {} tiles, assembled {}",
                source.elevation_tiles.len(),
                read_back.len()
            ),
        ));
    }

    // 7. Cross-restart: alice and bob both shut down, bob respawns on the
    //    SAME `p2p` dir (`TestPeer::spawn` derives it from `temp_dir/name`)
    //    and reissues `DownloadTileset` with alice offline. The persisted
    //    ledger (`TilesetTransfer`'s `downloads/<id>/download.json`) already
    //    verified every bundle complete before the restart, so the
    //    startup `resume_all` pass reports it resumed with NOTHING missing —
    //    proving the install path depends on local verified state, not a
    //    live provider.
    alice.shutdown();
    bob.shutdown();

    let bob2 = TestPeer::spawn("bob", tmp.path())?;
    // A best-effort reissue, exactly as a real UI would on reopening the
    // Downloads tab — honestly fails (no advertised root with alice down)
    // but must never panic or corrupt the persisted ledger.
    let _ = bob2.sync_cmd_tx.send(SyncCommand::DownloadTileset {
        tileset_id: TILESET_ID.to_string(),
        provider: None,
    });
    let resumed = bob2.wait_sync_event(
        |e| matches!(e, SyncEvent::TilesetDownloadResumed { tileset_id, .. } if tileset_id == TILESET_ID),
        SYNC_TIMEOUT,
    )?;
    match resumed {
        SyncEvent::TilesetDownloadResumed {
            received, total, ..
        } => {
            if received != total || total == 0 {
                return Ok(TestResult::fail(
                    NAME,
                    &format!(
                        "expected a fully-resumed download after restart, got {received}/{total}"
                    ),
                ));
            }
        }
        _ => unreachable!(),
    }

    bob2.shutdown();
    tracing::info!(
        "Two-peer tileset stream verified: {} tiles, cross-restart resumed with alice offline",
        read_back.len()
    );
    Ok(TestResult::pass(NAME))
}
