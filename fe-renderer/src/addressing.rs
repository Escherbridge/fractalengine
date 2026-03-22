use fe_network::AssetId;

pub fn content_address(bytes: &[u8]) -> AssetId {
    let hash = blake3::hash(bytes);
    AssetId(*hash.as_bytes())
}

pub fn validate_glb_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0x67, 0x6C, 0x54, 0x46]
}
