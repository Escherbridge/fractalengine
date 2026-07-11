//! Bridges fe-ui's queued node-asset download ops into the local blob store.
//! See src/AGENTS.md §assets for the resolve → copy → status-writeback contract.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use fe_database::{hash_from_hex, BlobStoreHandle};
use fe_ui::asset_ops::{AssetDownloadStatus, AssetOp, PendingAssetOps};
use fe_ui::verse_manager::VerseManager;

/// Blob-store handle exposed to the asset-download bridge system.
#[derive(Resource, Clone)]
pub struct AssetBlobStore(pub BlobStoreHandle);

/// Drains `PendingAssetOps` each frame: resolve the node's `blob://{hash}.{ext}`
/// asset via the blob store and copy it into the user's downloads folder,
/// recording the outcome in `AssetDownloadStatus` for the UI to surface.
pub fn drain_asset_ops(
    mut ops: ResMut<PendingAssetOps>,
    verse_mgr: Res<VerseManager>,
    blob_store: Option<Res<AssetBlobStore>>,
    mut status: ResMut<AssetDownloadStatus>,
) {
    if ops.0.is_empty() {
        return;
    }
    let Some(blob_store) = blob_store else {
        tracing::warn!("Asset ops dropped — blob store unavailable");
        ops.0.clear();
        return;
    };

    for op in ops.0.drain(..) {
        let AssetOp::Download { node_id } = op;
        let (saved_path, error) = match resolve_and_copy(&verse_mgr, &blob_store.0, &node_id) {
            Ok(path) => {
                tracing::info!(node_id = %node_id, path = %path.display(), "Asset downloaded");
                (Some(path.to_string_lossy().to_string()), None)
            }
            Err(e) => {
                tracing::warn!(node_id = %node_id, "Asset download failed: {e}");
                (None, Some(e))
            }
        };
        *status = AssetDownloadStatus {
            node_id: Some(node_id),
            saved_path,
            error,
        };
    }
}

/// Resolve a node's asset blob and copy it to `<downloads>/fractalengine/`.
fn resolve_and_copy(
    verse_mgr: &VerseManager,
    blob_store: &BlobStoreHandle,
    node_id: &str,
) -> Result<PathBuf, String> {
    let node = verse_mgr
        .all_nodes()
        .find(|n| n.id == node_id)
        .ok_or_else(|| "node not found".to_string())?;

    let asset_path = node
        .asset_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "node has no asset".to_string())?;

    // Never build the on-disk path from user input: the hash is parsed from the
    // `blob://{hash}.{ext}` URI and the path comes only from the content-addressed
    // store keyed by that parsed hash.
    let (hash_hex, ext) = parse_blob_uri(asset_path)?;
    let hash = hash_from_hex(&hash_hex).map_err(|e| format!("invalid asset hash: {e}"))?;
    let blob_path = blob_store
        .get_blob_path(&hash)
        .ok_or_else(|| "asset blob not available locally".to_string())?;

    let downloads =
        dirs::download_dir().ok_or_else(|| "no downloads directory on this platform".to_string())?;
    let dest_dir = downloads.join("fractalengine");
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("could not create download dir: {e}"))?;

    let file_name = sanitized_file_name(&node.name, &hash_hex, &ext);
    let dest = unique_path(&dest_dir, &file_name);

    std::fs::copy(&blob_path, &dest).map_err(|e| format!("copy failed: {e}"))?;
    Ok(dest)
}

/// Split a `blob://{hash}.{ext}` URI into `(hash_hex, ext)`. Rejects embedded
/// path separators so the URI must be a bare filename. Empty extension when
/// the URI has no dot.
fn parse_blob_uri(uri: &str) -> Result<(String, String), String> {
    let rest = uri.strip_prefix("blob://").unwrap_or(uri);
    if rest.is_empty() || rest.contains('/') || rest.contains('\\') {
        return Err(format!("malformed blob uri: {uri}"));
    }
    match rest.rsplit_once('.') {
        Some((stem, ext)) => Ok((stem.to_string(), ext.to_string())),
        None => Ok((rest.to_string(), String::new())),
    }
}

/// Build a safe destination filename from the node name (preferred) or the
/// content hash, plus the asset extension. Strips path separators and control
/// characters so a hostile node name can never escape the downloads directory.
fn sanitized_file_name(node_name: &str, hash_hex: &str, ext: &str) -> String {
    let sanitized: String = node_name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = sanitized.trim().trim_matches('.').trim();
    let base = if trimmed.is_empty() { hash_hex } else { trimmed };
    if ext.is_empty() {
        base.to_string()
    } else {
        format!("{base}.{ext}")
    }
}

/// Return a non-colliding path in `dir` for `file_name`, appending `-1`, `-2`, …
/// before the extension on collision.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let name = match ext {
            Some(ext) => format!("{stem}-{n}.{ext}"),
            None => format!("{stem}-{n}"),
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blob_uri_extracts_hash_and_ext() {
        let (hash, ext) = parse_blob_uri("blob://abc123.glb").unwrap();
        assert_eq!(hash, "abc123");
        assert_eq!(ext, "glb");
    }

    #[test]
    fn parse_blob_uri_handles_no_extension() {
        let (hash, ext) = parse_blob_uri("blob://abc123").unwrap();
        assert_eq!(hash, "abc123");
        assert_eq!(ext, "");
    }

    #[test]
    fn parse_blob_uri_rejects_path_separators() {
        assert!(parse_blob_uri("blob://../etc/passwd").is_err());
        assert!(parse_blob_uri("blob://a/b.glb").is_err());
        assert!(parse_blob_uri("blob://a\\b.glb").is_err());
    }

    #[test]
    fn sanitized_file_name_strips_separators() {
        let name = sanitized_file_name("../evil/name", "deadbeef", "glb");
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
        assert!(name.ends_with(".glb"));
    }

    #[test]
    fn sanitized_file_name_falls_back_to_hash() {
        let name = sanitized_file_name("   ", "deadbeef", "glb");
        assert_eq!(name, "deadbeef.glb");
    }

    #[test]
    fn sanitized_file_name_omits_empty_ext() {
        let name = sanitized_file_name("model", "deadbeef", "");
        assert_eq!(name, "model");
    }
}
