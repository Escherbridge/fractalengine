use fe_renderer::addressing::{content_address, validate_glb_magic};
use fe_renderer::ingester::{GltfIngester, AssetIngester, MAX_ASSET_SIZE_BYTES};

#[test]
fn test_glb_magic_valid() {
    let bytes = [0x67, 0x6C, 0x54, 0x46, 0x00, 0x00];
    assert!(validate_glb_magic(&bytes));
}

#[test]
fn test_glb_magic_invalid() {
    assert!(!validate_glb_magic(&[0, 0, 0, 0]));
}

#[test]
fn test_size_limit() {
    let big = vec![0u8; MAX_ASSET_SIZE_BYTES + 1];
    let ingester = GltfIngester;
    assert!(ingester.ingest(&big).is_err());
}

#[test]
fn test_content_address_deterministic() {
    let data = b"test asset";
    let id1 = content_address(data);
    let id2 = content_address(data);
    assert_eq!(id1.0, id2.0);
}

#[test]
fn test_content_address_unique() {
    let id1 = content_address(b"asset one");
    let id2 = content_address(b"asset two");
    assert_ne!(id1.0, id2.0);
}
