//! Bridges fe-ui's queued node-asset download ops into the local blob store.
//! See src/AGENTS.md §assets for the resolve → dialog → copy → status-writeback contract.

use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;

use bevy::prelude::*;

use fe_database::{hash_from_hex, BlobStoreHandle};
use fe_ui::asset_ops::{AssetDownloadStatus, AssetOp, PendingAssetOps};
use fe_ui::verse_manager::VerseManager;

/// Blob-store handle exposed to the asset-download bridge system.
#[derive(Resource, Clone)]
pub struct AssetBlobStore(pub BlobStoreHandle);

/// A node's asset resolved to a real on-disk blob path + a suggested save
/// name. Once this is built, only the copy itself can still fail.
#[derive(Debug)]
struct ResolvedAsset {
    blob_path: PathBuf,
    suggested_name: String,
}

/// Drains `PendingAssetOps` each frame: resolve the node's `blob://{hash}.{ext}`
/// asset via the blob store, prompt a native save dialog (suggested name from
/// the node, default dir Downloads), copy on confirm, and record the outcome
/// in `AssetDownloadStatus` for the UI to surface. The dialog lives here
/// rather than in fe-ui because only the bridge has the resolved node name +
/// real extension needed to suggest a sane filename.
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
        match resolve_asset(&verse_mgr, &blob_store.0, &node_id) {
            Ok(resolved) => match prompt_and_copy(&resolved) {
                Some(Ok(dest)) => {
                    tracing::info!(node_id = %node_id, path = %dest.display(), "Asset downloaded");
                    *status = AssetDownloadStatus {
                        node_id: Some(node_id),
                        saved_path: Some(dest.to_string_lossy().to_string()),
                        error: None,
                    };
                }
                Some(Err(e)) => {
                    tracing::warn!(node_id = %node_id, "Asset download failed: {e}");
                    *status = AssetDownloadStatus {
                        node_id: Some(node_id),
                        saved_path: None,
                        error: Some(e),
                    };
                }
                None => {
                    // User cancelled the save dialog: no status change, no toast noise.
                    tracing::debug!(node_id = %node_id, "Asset download cancelled by user");
                }
            },
            Err(e) => {
                tracing::warn!(node_id = %node_id, "Asset download failed: {e}");
                *status = AssetDownloadStatus {
                    node_id: Some(node_id),
                    saved_path: None,
                    error: Some(e),
                };
            }
        }
    }
}

/// Resolve a node's asset to a real blob path + suggested filename. No
/// filesystem writes, no dialogs — this is the part the FR-1 integration
/// test exercises directly against a real `BlobStore`.
fn resolve_asset(
    verse_mgr: &VerseManager,
    blob_store: &BlobStoreHandle,
    node_id: &str,
) -> Result<ResolvedAsset, String> {
    let node = verse_mgr
        .all_nodes()
        .find(|n| n.id == node_id)
        .ok_or_else(|| "node not found".to_string())?;

    let asset_path = node.asset_path.as_deref().filter(|p| !p.is_empty());
    let asset_path = match asset_path {
        Some(p) => p,
        // `has_asset` can be true while `asset_path` hasn't been cached locally
        // yet (e.g. DB row synced but blob metadata not yet round-tripped) —
        // distinguish this from "no asset at all" so the UI shows why.
        None if node.has_asset => {
            return Err(
                "Asset is not cached locally yet — reopen this node or re-sync to fetch it"
                    .to_string(),
            )
        }
        None => return Err("This node has no asset".to_string()),
    };

    // Never build the on-disk path from user input: the hash is parsed from the
    // `blob://{hash}.{ext}` URI and the path comes only from the content-addressed
    // store keyed by that parsed hash.
    let (hash_hex, ext) = parse_blob_uri(asset_path)?;
    let hash = hash_from_hex(&hash_hex).map_err(|e| format!("invalid asset hash: {e}"))?;
    let blob_path = blob_store
        .get_blob_path(&hash)
        .ok_or_else(|| "asset blob not available locally".to_string())?;

    Ok(ResolvedAsset {
        blob_path,
        suggested_name: sanitized_file_name(&node.name, &hash_hex, &ext),
    })
}

/// Open a native save dialog (suggested filename from `resolved`, default dir
/// Downloads) and copy the blob to the chosen destination on confirm.
/// `None` means the user cancelled — caller must not touch `status` in that case.
fn prompt_and_copy(resolved: &ResolvedAsset) -> Option<Result<PathBuf, String>> {
    let mut dialog = rfd::FileDialog::new().set_file_name(resolved.suggested_name.clone());
    if let Some(dir) = dirs::download_dir() {
        dialog = dialog.set_directory(dir);
    }
    let dest = dialog.save_file()?;
    Some(
        std::fs::copy(&resolved.blob_path, &dest)
            .map(|_| dest)
            .map_err(|e| format!("copy failed: {e}")),
    )
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
/// before the extension on collision. Only used by the dialog-free test/legacy
/// path below — the native save dialog handles overwrite prompts itself.
#[cfg(test)]
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

/// Resolve + copy into an explicit `dest_dir` — the dialog-free path exercised
/// directly by the FR-1 integration test (a real save dialog can't run headless).
#[cfg(test)]
fn resolve_and_copy(
    verse_mgr: &VerseManager,
    blob_store: &BlobStoreHandle,
    node_id: &str,
    dest_dir: &Path,
) -> Result<PathBuf, String> {
    let resolved = resolve_asset(verse_mgr, blob_store, node_id)?;
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("could not create dest dir: {e}"))?;
    let dest = unique_path(dest_dir, &resolved.suggested_name);
    std::fs::copy(&resolved.blob_path, &dest).map_err(|e| format!("copy failed: {e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_database::BlobStore as _;
    use fe_ui::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
    use std::sync::Arc;

    fn verse_with_node(node: NodeEntry) -> VerseManager {
        VerseManager::from_verses(vec![VerseEntry {
            id: "v1".to_string(),
            name: "V1".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![FractalEntry {
                id: "f1".to_string(),
                name: "F1".to_string(),
                expanded: true,
                petals: vec![PetalEntry {
                    id: "p1".to_string(),
                    name: "P1".to_string(),
                    expanded: true,
                    nodes: vec![node],
                }],
            }],
        }])
    }

    /// FR-1 root-cause evidence: a node whose `asset_path` points at a blob
    /// actually present in a real (filesystem-backed) `BlobStore` resolves and
    /// copies to a real destination — the resolve→copy path itself works.
    #[test]
    fn resolve_and_copy_lands_real_blob_at_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store =
            fe_sync::FsBlobStore::new(tmp.path().join("blobs")).expect("open real blob store");
        let bytes = b"a real glb payload, not a mock";
        let hash = store.add_blob(bytes).expect("add blob");
        let hash_hex = fe_database::hash_to_hex(&hash);
        let blob_store: BlobStoreHandle = Arc::new(store);

        let node = NodeEntry {
            id: "node-1".to_string(),
            name: "My Cool Model".to_string(),
            has_asset: true,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: Some(format!("blob://{hash_hex}.glb")),
        };
        let verse_mgr = verse_with_node(node);
        let dest_dir = tmp.path().join("out");

        let dest = resolve_and_copy(&verse_mgr, &blob_store, "node-1", &dest_dir)
            .expect("resolve_and_copy must succeed for a node backed by a real stored blob");

        assert!(dest.exists(), "copied file must exist at the destination");
        assert_eq!(
            std::fs::read(&dest).expect("read copied file"),
            bytes,
            "copied bytes must match the stored blob"
        );
        assert!(dest.to_string_lossy().contains("My Cool Model"));
        assert!(dest.extension().and_then(|e| e.to_str()) == Some("glb"));
    }

    /// FR-4: a node with `has_asset == true` but no cached `asset_path` must
    /// fail with a clear, distinguishing error instead of the generic
    /// "node has no asset" message (which contradicts `has_asset == true`).
    #[test]
    fn resolve_asset_reports_clear_error_when_has_asset_but_no_cached_path() {
        let node = NodeEntry {
            id: "node-2".to_string(),
            name: "Uncached Model".to_string(),
            has_asset: true,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: None,
        };
        let verse_mgr = verse_with_node(node);
        let blob_store: BlobStoreHandle =
            Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());

        let err = resolve_asset(&verse_mgr, &blob_store, "node-2").unwrap_err();
        assert_ne!(
            err, "node has no asset",
            "must not reuse the confusing generic message when has_asset is true"
        );
        let lower = err.to_lowercase();
        assert!(
            lower.contains("not cached") || lower.contains("re-sync") || lower.contains("reopen"),
            "error should explain the asset isn't cached locally yet, got: {err}"
        );
    }

    /// A node with no asset at all still gets a plain "no asset" error.
    #[test]
    fn resolve_asset_reports_no_asset_for_node_without_one() {
        let node = NodeEntry {
            id: "node-3".to_string(),
            name: "Empty".to_string(),
            has_asset: false,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: None,
        };
        let verse_mgr = verse_with_node(node);
        let blob_store: BlobStoreHandle =
            Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());

        let err = resolve_asset(&verse_mgr, &blob_store, "node-3").unwrap_err();
        assert!(err.to_lowercase().contains("no asset"));
    }

    #[test]
    fn resolve_asset_reports_missing_node() {
        let verse_mgr = VerseManager::default();
        let blob_store: BlobStoreHandle =
            Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let err = resolve_asset(&verse_mgr, &blob_store, "does-not-exist").unwrap_err();
        assert_eq!(err, "node not found");
    }

    /// The blob hash is a valid `blob://{hash}.ext` reference, but the blob
    /// itself was never written to this store (e.g. GC'd or never fetched).
    #[test]
    fn resolve_asset_reports_missing_blob() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = fe_sync::FsBlobStore::new(tmp.path().join("blobs")).expect("open store");
        let blob_store: BlobStoreHandle = Arc::new(store);
        let missing_hash = "0".repeat(64);

        let node = NodeEntry {
            id: "node-4".to_string(),
            name: "Ghost".to_string(),
            has_asset: true,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: Some(format!("blob://{missing_hash}.glb")),
        };
        let verse_mgr = verse_with_node(node);

        let err = resolve_asset(&verse_mgr, &blob_store, "node-4").unwrap_err();
        assert!(err.contains("not available locally"));
    }

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
