//! The runtime blob store (fe-sync) and the hexon registry's blob store share
//! one canonical on-disk layout, so a blob written by either is read by the
//! other from the same root (fe-runtime/src/AGENTS.md §blob-store).

use fe_runtime::blob_store::{hash_to_hex, BlobStore};

#[test]
fn runtime_written_blob_is_loadable_by_the_hexon_store() {
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = fe_sync::FsBlobStore::new(root.path().to_path_buf()).expect("runtime store");
    let hexon = fe_hexon::registry::FsBlobStore::new(root.path().to_path_buf());

    let bytes = b"glb bytes ingested by the runtime";
    let hash = runtime.add_blob(bytes).expect("add_blob");

    let hex = hash_to_hex(&hash);
    assert!(hexon.has(&hex));
    assert_eq!(hexon.load(&hex).expect("hexon load"), bytes);
}

#[test]
fn hexon_stored_blob_is_servable_by_the_runtime_store() {
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = fe_sync::FsBlobStore::new(root.path().to_path_buf()).expect("runtime store");
    let hexon = fe_hexon::registry::FsBlobStore::new(root.path().to_path_buf());

    let bytes = b"texture bytes installed from a hexon crate";
    let hex = fe_hexon::signature::asset_hash(bytes);
    assert!(hexon.store(&hex, bytes).expect("hexon store"));

    let hash = fe_runtime::blob_store::hash_from_hex(&hex).expect("hex hash");
    assert!(runtime.has_blob(&hash));
    let path = runtime.get_blob_path(&hash).expect("runtime path");
    assert_eq!(std::fs::read(path).expect("read blob"), bytes);
}

#[test]
fn hexon_store_refuses_data_that_does_not_match_its_hash() {
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = fe_sync::FsBlobStore::new(root.path().to_path_buf()).expect("runtime store");
    let hexon = fe_hexon::registry::FsBlobStore::new(root.path().to_path_buf());

    let claimed = fe_hexon::signature::asset_hash(b"the bytes the manifest promised");
    hexon
        .store(&claimed, b"different bytes")
        .expect_err("a hash/data mismatch must be refused");

    let hash = fe_runtime::blob_store::hash_from_hex(&claimed).expect("hex hash");
    assert!(
        !runtime.has_blob(&hash),
        "nothing was written under the claimed hash"
    );
    assert!(!hexon.has(&claimed));
}
