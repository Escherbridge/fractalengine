//! The `blob.mirror` after-commit tap: committed local imports feed the mirror queue (src/AGENTS.md §plugin-host).

use std::path::PathBuf;

use bevy::prelude::{MessageReader, Res};
use fe_plugin::bindings::egress::MirrorJob;
use fe_runtime::blob_store::{hash_from_hex, BlobHash};
use fe_runtime::messages::DbResult;

use crate::plugin_bridge::{ExtensionBlobMirror, ExtensionBlobSlot};

/// Content type of every blob a GLB import commits.
const GLB_CONTENT_TYPE: &str = "model/gltf-binary";
const BLOB_SCHEME: &str = "blob://";
const GLB_SUFFIX: &str = ".glb";
const HEX_HASH_LENGTH: usize = 64;

/// The mirror job for one committed GLB import; `None` for anything else (legacy, failed, non-blob, file missing).
pub(crate) fn mirror_job_for(
    result: &DbResult,
    blob_path: impl Fn(&BlobHash) -> Option<PathBuf>,
) -> Option<MirrorJob> {
    let DbResult::GltfImported { asset_id, petal_id, asset_path, materialization: Some(_), .. } = result else {
        return None;
    };
    let hex = asset_path.strip_prefix(BLOB_SCHEME)?.strip_suffix(GLB_SUFFIX)?;
    if hex.len() != HEX_HASH_LENGTH {
        return None;
    }
    let hash = hash_from_hex(hex).ok()?;
    let metadata = std::fs::metadata(blob_path(&hash)?).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(MirrorJob {
        petal_id: petal_id.clone(),
        asset_id: asset_id.clone(),
        hash,
        size: metadata.len(),
        content_type: GLB_CONTENT_TYPE.to_owned(),
    })
}

/// Queue every committed local import for `blob.mirror`; a full queue drops and counts (never blocks).
pub(crate) fn feed_blob_mirror(
    mut results: MessageReader<DbResult>,
    mirror: Res<ExtensionBlobMirror>,
    blobs: Res<ExtensionBlobSlot>,
) {
    let (Some(queue), Some(store)) = (mirror.queue(), blobs.get()) else {
        // Not built yet: drain so late-built queues do not replay old imports.
        results.clear();
        return;
    };
    for result in results.read() {
        if let Some(job) = mirror_job_for(result, |hash| store.get_blob_path(hash)) {
            queue.after_commit(job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, Update};
    use fe_plugin::bindings::egress::blob_mirror_channel;
    use fe_runtime::blob_store::{hash_to_hex, BlobStore, BlobStoreHandle};
    use fe_runtime::messages::{CommittedNodeMaterialization, NodeDto};

    /// Resolves one fixed hash to a real temp file.
    struct FixedBlobs {
        hash: BlobHash,
        path: PathBuf,
    }

    impl BlobStore for FixedBlobs {
        fn add_blob(&self, _bytes: &[u8]) -> anyhow::Result<BlobHash> {
            Ok(self.hash)
        }
        fn get_blob_path(&self, hash: &BlobHash) -> Option<PathBuf> {
            (*hash == self.hash).then(|| self.path.clone())
        }
        fn has_blob(&self, hash: &BlobHash) -> bool {
            *hash == self.hash
        }
        fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn imported(asset_path: &str, materialized: bool) -> DbResult {
        let node = NodeDto {
            node_id: "node-1".into(),
            petal_id: "petal-1".into(),
            name: "model".into(),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            has_asset: true,
            asset_path: Some(asset_path.into()),
        };
        DbResult::GltfImported {
            request: None,
            node_id: "node-1".into(),
            asset_id: "asset-1".into(),
            petal_id: "petal-1".into(),
            name: "model".into(),
            asset_path: asset_path.into(),
            position: [0.0; 3],
            materialization: materialized.then(|| CommittedNodeMaterialization {
                node,
                rotation_euler: [0.0; 3],
                asset_id: Some("asset-1".into()),
                properties: serde_json::Value::Null,
                history_version: 1,
            }),
            correlation_id: None,
        }
    }

    #[test]
    fn committed_local_imports_queue_one_job_and_nothing_else_does() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("model.glb");
        std::fs::write(&file, [7u8; 12]).unwrap();
        let hash: BlobHash = [9u8; 32];
        let blob_path_path = format!("blob://{}.glb", hash_to_hex(&hash));
        let lookup = |wanted: &BlobHash| (*wanted == hash).then(|| file.clone());

        let job = mirror_job_for(&imported(&blob_path_path, true), lookup).expect("committed import");
        assert_eq!(
            job,
            MirrorJob {
                petal_id: "petal-1".into(),
                asset_id: "asset-1".into(),
                hash,
                size: 12,
                content_type: "model/gltf-binary".into(),
            }
        );

        let other_hash = format!("blob://{}.glb", hash_to_hex(&[1u8; 32]));
        let refused = [
            imported(&blob_path_path, false),
            imported("assets/model.glb", true),
            imported(&format!("blob://{}.bin", hash_to_hex(&hash)), true),
            imported("blob://abc.glb", true),
            imported(&other_hash, true),
            DbResult::ImportGltfFailed { request: None, message: "bad".into() },
        ];
        for result in &refused {
            assert_eq!(mirror_job_for(result, lookup), None, "{result:?}");
        }
        let gone = |_: &BlobHash| Some(dir.path().join("missing.glb"));
        assert_eq!(mirror_job_for(&imported(&blob_path_path, true), gone), None);

        // App leg: the system reads the message and the mirror receives exactly one job.
        let store: BlobStoreHandle = std::sync::Arc::new(FixedBlobs { hash, path: file.clone() });
        let (queue, mirror) = blob_mirror_channel(store.clone());
        let slot = ExtensionBlobSlot::default();
        slot.bind(store);
        let published = ExtensionBlobMirror::default();
        assert!(published.publish(queue));
        let mut app = App::new();
        app.add_message::<DbResult>()
            .insert_resource(slot)
            .insert_resource(published)
            .add_systems(Update, feed_blob_mirror);
        app.world_mut().write_message(imported(&blob_path_path, true));
        app.world_mut().write_message(imported(&blob_path_path, false));
        app.world_mut().write_message(DbResult::ImportGltfFailed { request: None, message: "bad".into() });
        app.update();
        assert_eq!(mirror.pending(), 1);
    }
}
