//! PAS-P5 S1 flows: seal a hexon, verify it, and refuse each class of tampering.

use std::io::{Cursor, Read, Write};

use ed25519_dalek::SigningKey;
use fe_format::integrity::{chunk_member_path, digest_hex, CHUNK_INDEX_PATH};
use fe_format::{
    seal, verify_archive, verifying_key_to_did, AssetEntry, BundleEntry, ElevationEncoding,
    EntryCatalog, EntryKind, HexonArchive, HexonManifest, HexonType, IntegrityError,
    IntegrityPolicy, IntegrityStatus, License, TilesetBundleIndex, TilesetMeta,
};
use zip::write::SimpleFileOptions;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn did(key: &SigningKey) -> String {
    verifying_key_to_did(&key.verifying_key())
}

fn manifest(hexon_type: HexonType) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: "flow-tileset".into(),
        hexon_type,
        publisher_did: "did:key:z6Mkplaceholder".into(),
        publisher_name: None,
        version: "1.0.0".into(),
        build_id: None,
        name: "Flow Tileset".into(),
        description: None,
        tags: vec![],
        created_at: "2026-10-14T00:00:00Z".into(),
        updated_at: "2026-10-14T00:00:00Z".into(),
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

fn meta(tile_count: u32) -> TilesetMeta {
    TilesetMeta {
        bounds: [45.0, -122.0, 46.0, -121.0],
        min_zoom: 10,
        max_zoom: 11,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: true,
        tile_count,
        satellite_tile_count: 1,
        region_name: "Flow Region".into(),
        ..Default::default()
    }
}

/// Every member's decompressed bytes, in archive order.
fn members(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (file.name().to_string(), data)
        })
        .collect()
}

/// Re-zip `members` (a tamperer rewriting the archive with fresh CRCs).
fn zip_of(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in members {
        zip.start_file(name.as_str(), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Apply `edit` to the decoded member list and re-zip.
fn rewrite(bytes: &[u8], edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> Vec<u8> {
    let mut list = members(bytes);
    edit(&mut list);
    zip_of(&list)
}

fn flip_first_byte_of(bytes: &[u8], path: &str) -> Vec<u8> {
    rewrite(bytes, |list| {
        let (_, data) = list.iter_mut().find(|(n, _)| n == path).expect(path);
        data[0] ^= 0xff;
    })
}

const ASSET: &[u8] = b"preview-model-bytes";

/// An unsealed tileset with tiles, satellite imagery, a license and one catalogued asset.
fn unsealed_tileset() -> Vec<u8> {
    let elevation = vec![
        ("10/1/2".to_string(), vec![1u8; 700]),
        ("10/1/3".to_string(), vec![2u8; 900]),
        (
            "11/2/4".to_string(),
            (0..=255u8).cycle().take(4000).collect(),
        ),
    ];
    let satellite = vec![("10/1/2".to_string(), vec![9u8; 1200])];
    let exported = HexonArchive::export_tileset(
        manifest(HexonType::TerrainTileset),
        &meta(elevation.len() as u32),
        &elevation,
        &satellite,
        Some(License::default()),
    )
    .unwrap();
    let hash = digest_hex(ASSET);
    let catalog = EntryCatalog {
        entries: vec![AssetEntry {
            entry_id: "preview".into(),
            kind: EntryKind::Model,
            asset_hash: hash.clone(),
            format: "glb".into(),
            label: "Preview".into(),
            tags: vec![],
            description: None,
            is_placeable: false,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }],
    };
    rewrite(&exported, |list| {
        let entries = list.iter_mut().find(|(n, _)| n == "entries.json").unwrap();
        entries.1 = serde_json::to_vec_pretty(&catalog).unwrap();
        list.push((format!("assets/{hash}"), ASSET.to_vec()));
    })
}

fn require(pin: Option<String>) -> IntegrityPolicy {
    IntegrityPolicy::RequireSealed { pin }
}

#[test]
fn sealed_tileset_verifies_and_names_its_publisher() {
    let publisher = key(1);
    let sealed = seal(&unsealed_tileset(), &publisher).expect("seal");

    let status = verify_archive(&sealed, &require(None)).expect("sealed archive verifies");
    assert_eq!(
        status,
        IntegrityStatus::Sealed {
            publisher_did: did(&publisher)
        }
    );
    assert_eq!(
        verify_archive(&sealed, &require(Some(did(&publisher)))),
        Ok(status.clone()),
        "pinning the signer passes"
    );
    assert_eq!(
        verify_archive(&sealed, &IntegrityPolicy::VerifyIfSealed),
        Ok(status)
    );
}

#[test]
fn tampering_is_refused_per_class() {
    let publisher = key(1);
    let sealed = seal(&unsealed_tileset(), &publisher).expect("seal");
    let asset_path = format!("assets/{}", digest_hex(ASSET));
    let stray = b"stray-blob".to_vec();
    let stray_hash = digest_hex(&stray);

    type Mutation = Box<dyn Fn(&[u8]) -> Vec<u8>>;
    let tile = "terrain/tiles/10/1/2.png";
    let rows: Vec<(&str, Mutation, IntegrityError)> = vec![
        (
            "flip a byte in a tile",
            Box::new(move |b: &[u8]| flip_first_byte_of(b, tile)),
            IntegrityError::DigestMismatch { path: tile.into() },
        ),
        (
            "flip a byte in entries.json",
            Box::new(|b: &[u8]| flip_first_byte_of(b, "entries.json")),
            IntegrityError::DigestMismatch {
                path: "entries.json".into(),
            },
        ),
        (
            "flip a byte in tileset_meta.json",
            Box::new(|b: &[u8]| flip_first_byte_of(b, "terrain/tileset_meta.json")),
            IntegrityError::DigestMismatch {
                path: "terrain/tileset_meta.json".into(),
            },
        ),
        (
            "flip a byte in members.json",
            Box::new(|b: &[u8]| flip_first_byte_of(b, "members.json")),
            IntegrityError::DigestMismatch {
                path: "members.json".into(),
            },
        ),
        (
            "flip a byte in an assets/{h} blob",
            Box::new({
                let path = asset_path.clone();
                move |b: &[u8]| flip_first_byte_of(b, &path)
            }),
            IntegrityError::DigestMismatch {
                path: asset_path.clone(),
            },
        ),
        (
            "append an uncovered member after sealing (the splat-append hazard)",
            Box::new(|b: &[u8]| {
                rewrite(b, |list| {
                    list.push(("splats/baked.ply".into(), b"ply".to_vec()))
                })
            }),
            IntegrityError::Uncovered {
                path: "splats/baked.ply".into(),
            },
        ),
        (
            "add a content-addressed blob the signed catalog never names",
            Box::new({
                let (hash, blob) = (stray_hash.clone(), stray.clone());
                move |b: &[u8]| {
                    rewrite(b, |list| {
                        list.push((format!("assets/{hash}"), blob.clone()))
                    })
                }
            }),
            IntegrityError::AssetNotReferenced {
                hash: stray_hash.clone(),
            },
        ),
        (
            "delete a committed member",
            Box::new(move |b: &[u8]| rewrite(b, |list| list.retain(|(n, _)| n != tile))),
            IntegrityError::MissingMember { path: tile.into() },
        ),
        (
            "edit a signed manifest field without re-signing",
            Box::new(|b: &[u8]| {
                rewrite(b, |list| {
                    let manifest = list.iter_mut().find(|(n, _)| n == "manifest.json").unwrap();
                    let text = String::from_utf8(manifest.1.clone()).unwrap();
                    manifest.1 = text.replace("Flow Tileset", "Evil Tileset").into_bytes();
                })
            }),
            IntegrityError::BadSignature,
        ),
        (
            "re-sign with a different key while the publisher is pinned",
            Box::new(|b: &[u8]| seal(b, &key(2)).unwrap()),
            IntegrityError::PublisherMismatch {
                expected: did(&key(1)),
                found: did(&key(2)),
            },
        ),
    ];

    let pinned = require(Some(did(&publisher)));
    for (description, mutate, expected) in rows {
        let tampered = mutate(&sealed);
        assert_eq!(
            verify_archive(&tampered, &pinned),
            Err(expected),
            "row: {description}"
        );
    }
}

#[test]
fn legacy_unsealed_archive_passes_verify_if_sealed_and_fails_require_sealed() {
    let legacy = unsealed_tileset();
    assert_eq!(
        verify_archive(&legacy, &IntegrityPolicy::VerifyIfSealed),
        Ok(IntegrityStatus::Unsealed)
    );
    assert_eq!(
        verify_archive(&legacy, &require(None)),
        Err(IntegrityError::NotSealed)
    );
    // A legacy archive still imports exactly as before.
    let data = HexonArchive::import(&legacy).expect("legacy import");
    assert_eq!(data.elevation_tiles.len(), 3);
}

#[test]
fn sealing_preserves_every_member_byte() {
    let original = unsealed_tileset();
    let sealed = seal(&original, &key(1)).expect("seal");

    let before = HexonArchive::import(&original).unwrap();
    let after = HexonArchive::import(&sealed).unwrap();
    assert_eq!(after.elevation_tiles, before.elevation_tiles);
    assert_eq!(after.satellite_tiles, before.satellite_tiles);
    assert_eq!(after.assets, before.assets);
    assert_eq!(after.tileset_meta, before.tileset_meta);
    assert_eq!(after.entries.len(), before.entries.len());
    assert_eq!(after.manifest.hexon_id, before.manifest.hexon_id);
    assert!(after.license.is_some());

    // Raw copy: every non-manifest member keeps its bytes, CRC and compression.
    let mut sealed_zip = zip::ZipArchive::new(Cursor::new(sealed.as_slice())).unwrap();
    let mut original_zip = zip::ZipArchive::new(Cursor::new(original.as_slice())).unwrap();
    for (name, data) in members(&original) {
        if name == "manifest.json" {
            continue;
        }
        {
            let ours = sealed_zip.by_name(&name).expect("member survives sealing");
            let theirs = original_zip.by_name(&name).unwrap();
            assert_eq!(ours.crc32(), theirs.crc32(), "{name}");
            assert_eq!(ours.compressed_size(), theirs.compressed_size(), "{name}");
            assert_eq!(ours.compression(), theirs.compression(), "{name}");
        }
        let mut copied = Vec::new();
        sealed_zip
            .by_name(&name)
            .unwrap()
            .read_to_end(&mut copied)
            .unwrap();
        assert_eq!(copied, data, "{name}");
    }

    // Re-sealing a sealed archive replaces (never duplicates) members.json.
    let resealed = seal(&sealed, &key(1)).expect("re-seal");
    let names: Vec<String> = members(&resealed).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names.iter().filter(|n| *n == "members.json").count(), 1);
    assert!(verify_archive(&resealed, &require(Some(did(&key(1))))).is_ok());
}

#[test]
fn container_chunk_table_is_bound_to_its_chunk_members() {
    let chunks: Vec<Vec<u8>> = (0..3u8)
        .map(|i| {
            let tile = vec![(format!("10/{i}/0"), vec![i; 256])];
            let chunk = HexonArchive::export_tileset(
                manifest(HexonType::TerrainTileset),
                &meta(1),
                &tile,
                &[],
                None,
            )
            .unwrap();
            seal(&chunk, &key(1)).unwrap()
        })
        .collect();
    let index_for = |digests: Vec<String>| TilesetBundleIndex {
        schema: 1,
        tileset_id: "flow-tileset".into(),
        tileset_meta: meta(3),
        bundles: digests
            .into_iter()
            .enumerate()
            .map(|(seq, blake3_hex)| BundleEntry {
                seq: seq as u32,
                blake3_hex,
                byte_len: chunks[seq].len() as u64,
                tile_count: 1,
                zoom: 10,
                x_range: [seq as u32, seq as u32],
                y_range: [0, 0],
                hilbert_range: [seq as u64, seq as u64],
            })
            .collect(),
    };
    let container = |index: &TilesetBundleIndex| {
        let mut list = members(
            &HexonArchive::export_tileset(
                manifest(HexonType::TerrainTileset),
                &meta(3),
                &[],
                &[],
                None,
            )
            .unwrap(),
        );
        list.push((CHUNK_INDEX_PATH.into(), serde_json::to_vec(index).unwrap()));
        for (seq, chunk) in chunks.iter().enumerate() {
            list.push((chunk_member_path(seq as u32), chunk.clone()));
        }
        zip_of(&list)
    };

    let honest = index_for(chunks.iter().map(|c| digest_hex(c)).collect());
    let sealed = seal(&container(&honest), &key(1)).expect("container seals");
    assert!(verify_archive(&sealed, &require(Some(did(&key(1))))).is_ok());

    // Swapping a chunk for another validly sealed one is caught by the signed table.
    let swapped = rewrite(&sealed, |list| {
        let first = list
            .iter()
            .find(|(n, _)| n == "chunks/0.hexon")
            .unwrap()
            .1
            .clone();
        list.iter_mut()
            .find(|(n, _)| n == "chunks/1.hexon")
            .unwrap()
            .1 = first;
    });
    assert_eq!(
        verify_archive(&swapped, &require(None)),
        Err(IntegrityError::DigestMismatch {
            path: "chunks/1.hexon".into()
        })
    );

    // A producer whose table disagrees with its chunks cannot seal.
    let mut lying = honest.clone();
    lying.bundles[2].blake3_hex = digest_hex(b"not chunk 2");
    let err = seal(&container(&lying), &key(1)).expect_err("inconsistent table must not seal");
    assert_eq!(
        err.downcast_ref::<IntegrityError>(),
        Some(&IntegrityError::DigestMismatch {
            path: "chunks/2.hexon".into()
        })
    );
}
