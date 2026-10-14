//! Flow tests: store-level integrity enforcement, atomic install, and sealed
//! chunked-container install/reassembly (campaign plan p2p_asset_streaming
//! S6). See `fe-terrain/src/AGENTS.md` §chunked-containers.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::SigningKey;

use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
use fe_format::{
    seal, verify_archive, ChunkInput, HexonArchive, HexonArchiveIndex, IntegrityError,
    IntegrityPolicy, IntegrityStatus,
};
use fe_terrain::tiles::distribution::assemble_sealed_container;
use fe_terrain::tiles::store::UNSAFE_HEXON_ID_ERROR;
use fe_terrain::tiles::{HexonStore, TileCoord};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_store() -> (HexonStore, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hexon_integrity_flow_{}_{nanos}",
        std::process::id()
    ));
    let store = HexonStore::with_dir(dir.clone()).expect("with_dir failed");
    (store, dir)
}

fn manifest(id: &str) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6Mktest".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: "Flow Test".into(),
        description: None,
        tags: vec![],
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
    }
}

fn tileset_meta() -> TilesetMeta {
    TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 10,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        tile_count: 1,
        region_name: "Flow Region".into(),
        ..Default::default()
    }
}

fn tileset_bytes(id: &str, tiles: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut meta = tileset_meta();
    meta.tile_count = tiles.len() as u32;
    HexonArchive::export_tileset(manifest(id), &meta, tiles, &[], None).expect("export_tileset")
}

// ---------------------------------------------------------------------------
// install_tileset: integrity + path safety
// ---------------------------------------------------------------------------

/// S6 flow test: a sealed tileset whose tile bytes were tampered after
/// sealing is refused by `install_tileset`, and nothing lands in the store.
#[test]
fn tampered_sealed_tileset_install_is_refused_and_store_is_unchanged() {
    let key = SigningKey::from_bytes(&[21u8; 32]);
    let (store, dir) = temp_store();

    let marker = vec![7u8; 256];
    let bytes = tileset_bytes("tamper-install", &[("10/1/2".into(), marker.clone())]);
    let mut sealed = seal(&bytes, &key).expect("seal");
    let pos = sealed
        .windows(marker.len())
        .position(|w| w == marker.as_slice())
        .expect("tile bytes must appear verbatim (Stored compression)");
    sealed[pos] ^= 0xFF;

    let err = store
        .install_tileset(&sealed)
        .expect_err("tampered install must be refused");
    assert!(
        matches!(
            err.downcast_ref::<IntegrityError>(),
            Some(IntegrityError::DigestMismatch { .. })
        ),
        "{err:#}"
    );

    assert!(store.list_installed().is_empty());
    assert!(!dir.join("tamper-install.hexon").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// S6 flow test: every path-escaping / device-stem `hexon_id` is refused
/// before any path join, and no file reaches the filesystem.
#[test]
fn install_refuses_path_escaping_hexon_ids() {
    let (store, dir) = temp_store();
    for bad_id in ["../x", "a/b", "a\\b", "C:x", "..", ""] {
        let bytes = tileset_bytes(bad_id, &[("10/0/0".into(), vec![1u8; 16])]);
        let err = store
            .install_tileset(&bytes)
            .expect_err(&format!("{bad_id:?} must be refused"));
        assert!(
            err.to_string().contains(UNSAFE_HEXON_ID_ERROR),
            "{bad_id:?}: {err:#}"
        );
    }
    assert!(store.list_installed().is_empty());
    // Nothing named after any of the bad ids (or escaping the store dir) exists.
    let outside = dir.parent().expect("temp dir has a parent");
    assert!(
        !outside.join("x.hexon").exists(),
        "a traversing id must never write outside the store"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// S6 flow test: a legacy (unsealed) archive still installs locally, flagged
/// `verified: false` — the gis-tile-etl / sample-hexon compatibility path.
#[test]
fn legacy_unsealed_tileset_installs_flagged_unverified() {
    let (store, dir) = temp_store();
    let bytes = tileset_bytes("legacy-unsealed", &[("10/0/0".into(), vec![3u8; 32])]);

    let installed = store.install_tileset(&bytes).expect("install failed");
    assert!(!installed.verified);
    assert!(!store.list_installed()[0].verified);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Critic AC-21b/c: a reinstall of the SAME id, while an older
/// `HexonTileSource` from the first install is still alive and has already
/// cached a tile, never fails (the atomic temp-then-rename replaces `std::fs::
/// write`'s old in-place truncate, so a torn read is no longer observable
/// even though `LazyFile`'s own handle only stays open for the duration of
/// one `read_member` call — §lazy-tiles "Reinstall is atomic"), the old
/// reader's already-cached bytes are unaffected by the swap, and a fresh
/// `load_tileset` afterwards sees the NEW file.
#[test]
fn reinstall_keeps_an_older_readers_cached_bytes_and_new_opens_see_the_new_file() {
    let (store, dir) = temp_store();
    let v1 = tileset_bytes("swap", &[("10/0/0".into(), vec![1u8; 64])]);
    store.install_tileset(&v1).expect("first install");

    // Open a reader against v1 and cache its one tile.
    let reader = store.load_tileset("swap").expect("load v1");
    let v1_tile = reader
        .load_tile_blocking(TileCoord::new(0, 0, 10))
        .expect("v1 read must not error")
        .expect("v1 tile present");
    assert_eq!(v1_tile.as_ref(), vec![1u8; 64].as_slice());

    // Reinstall the SAME id while the older reader is still alive.
    let v2 = tileset_bytes("swap", &[("10/0/0".into(), vec![2u8; 64])]);
    store
        .install_tileset(&v2)
        .expect("atomic reinstall must succeed with an older reader still alive");

    // The old reader's already-cached bytes are unaffected by the swap.
    assert_eq!(
        reader
            .get_tile(TileCoord::new(0, 0, 10))
            .expect("cached tile stays resident")
            .as_ref(),
        v1_tile.as_ref()
    );

    // A fresh open sees the NEW file.
    let fresh = store.load_tileset("swap").expect("load v2");
    let v2_tile = fresh
        .load_tile_blocking(TileCoord::new(0, 0, 10))
        .expect("v2 read must not error")
        .expect("v2 tile present");
    assert_eq!(v2_tile.as_ref(), vec![2u8; 64].as_slice());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Chunked containers
// ---------------------------------------------------------------------------

fn three_chunks(tag: &str) -> Vec<ChunkInput> {
    (0..3u32)
        .map(|seq| {
            let tiles = vec![(format!("10/{seq}/0"), vec![(seq + 1) as u8; 48])];
            let meta = TilesetMeta {
                bounds: [45.0, -122.0, 46.0, -121.0],
                min_zoom: 10,
                max_zoom: 10,
                tile_size: 256,
                elevation_encoding: ElevationEncoding::TerrainRgb,
                tile_count: 1,
                region_name: "Flow Region".into(),
                ..Default::default()
            };
            let archive_bytes = HexonArchive::export_tileset(
                manifest(&format!("{tag}-chunk-{seq}")),
                &meta,
                &tiles,
                &[],
                None,
            )
            .expect("export chunk");
            ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [seq as u64, seq as u64],
                archive_bytes,
            }
        })
        .collect()
}

/// S6 flow test: `HexonStore::install_tileset` accepts the container layout
/// and the installed source serves tiles from every chunk.
#[test]
fn chunked_container_install_serves_tiles_from_every_chunk() {
    let (store, dir) = temp_store();
    let chunks = three_chunks("served");
    let mut outer_meta = tileset_meta();
    outer_meta.tile_count = 3;
    let container_bytes =
        HexonArchive::export_chunked(manifest("served-container"), &outer_meta, &chunks)
            .expect("export_chunked");

    let installed = store
        .install_tileset(&container_bytes)
        .expect("container install failed");
    assert_eq!(installed.tile_count, 3);

    let source = store
        .load_tileset(&installed.hexon_id)
        .expect("load container");
    for seq in 0..3u32 {
        let coord = TileCoord::new(seq, 0, 10);
        let data = source
            .load_tile_blocking(coord)
            .expect("blocking read must not error")
            .unwrap_or_else(|| panic!("chunk {seq} tile must be served"));
        assert_eq!(data.as_ref(), vec![(seq + 1) as u8; 48].as_slice());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// S6 flow test: a container reconstructed from a verified header plus
/// per-chunk bytes (what PAS-P5 S4/S5/S7a's `FetchStrategy` will hand
/// `assemble_sealed_container` once they land) installs as a file that
/// STILL passes `verify_archive(RequireSealed)` — the publisher's seal
/// survived the reassembly, so the install can be re-seeded unmodified.
#[test]
fn assembled_p2p_download_installs_as_a_reseedable_sealed_container() {
    let key = SigningKey::from_bytes(&[33u8; 32]);
    let (store, dir) = temp_store();

    let chunks = three_chunks("reseed");
    let mut outer_meta = tileset_meta();
    outer_meta.tile_count = 3;
    let unsealed = HexonArchive::export_chunked(manifest("reseed-container"), &outer_meta, &chunks)
        .expect("export_chunked");
    let published = seal(&unsealed, &key).expect("seal container");

    // Simulate what a verified HTTP/P2P fetch hands back: the header (via
    // the lazy index's header phase) plus each chunk's bytes, already
    // checked against the signed `chunks/index.json` by the real
    // `FetchStrategy` this function will eventually be called from.
    let published_path = dir.join("published.hexon");
    std::fs::write(&published_path, &published).unwrap();
    let index = HexonArchiveIndex::open(&published_path).expect("open published");
    let header = index.header_doc();
    let fetched_chunks: Vec<(u32, Vec<u8>)> = (0..3u32)
        .map(|seq| {
            let bytes = index
                .chunk(seq)
                .expect("chunk read must not error")
                .expect("chunk present");
            (seq, bytes)
        })
        .collect();

    let installed = assemble_sealed_container(&store, &header, &fetched_chunks)
        .expect("reassembly + install failed");
    assert!(
        installed.verified,
        "a correctly reassembled container must still verify as sealed"
    );

    let on_disk = std::fs::read(dir.join(format!("{}.hexon", installed.hexon_id))).unwrap();
    let status = verify_archive(&on_disk, &IntegrityPolicy::RequireSealed { pin: None })
        .expect("installed file must still pass RequireSealed");
    assert_eq!(
        status,
        IntegrityStatus::Sealed {
            publisher_did: fe_format::verifying_key_to_did(&key.verifying_key())
        }
    );

    // Reseedable: the installed file serves every chunk's tile again.
    let source = store
        .load_tileset(&installed.hexon_id)
        .expect("load reassembled");
    for seq in 0..3u32 {
        let data = source
            .load_tile_blocking(TileCoord::new(seq, 0, 10))
            .expect("read must not error")
            .unwrap_or_else(|| panic!("chunk {seq} tile must be served"));
        assert_eq!(data.as_ref(), vec![(seq + 1) as u8; 48].as_slice());
    }

    let _ = std::fs::remove_dir_all(&dir);
}
