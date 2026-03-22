use fe_network::AssetId;
use bevy::prelude::{AssetServer, Handle};
use bevy::gltf::Gltf;
use crate::ingester::cache_path;

pub fn load_to_bevy(asset_id: AssetId, asset_server: &AssetServer) -> Handle<Gltf> {
    let path = cache_path(asset_id);
    if path.exists() {
        asset_server.load(path)
    } else {
        // CROSS-CRATE: fe_network::iroh_blobs::fetch_asset(asset_id) — wired in Sprint 5B
        asset_server.load("assets/placeholder.glb")
    }
}
