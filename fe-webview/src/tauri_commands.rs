//! Asset helpers retained for a future Tauri shell; see src/AGENTS.md section tauri-commands.

#[cfg(feature = "backend-tauri")]
use fe_runtime::shared_node::validate_asset_path;
#[cfg(feature = "backend-tauri")]
use tauri::Manager;

/// Resolve an asset path and return the file contents.
///
/// This provides the asset:// protocol functionality for serving local files
/// from the petal's asset directory with path traversal protection.
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn resolve_asset(
    petal_id: String,
    asset_path: String,
    app: tauri::AppHandle,
) -> Result<Vec<u8>, String> {
    // First-line defense: lexical traversal checks (no filesystem access)
    validate_asset_path(&petal_id, &asset_path)?;

    // Get the app data directory
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    // Build the full path: {data_dir}/verses/{petal_id}/assets/{asset_path}
    let base = data_dir.join("verses").join(&petal_id).join("assets");

    let resolved = base.join(&asset_path);

    // Canonicalize both paths so symlinks cannot escape the base dir
    let canonical_base =
        std::fs::canonicalize(&base).map_err(|e| format!("Failed to resolve asset base: {e}"))?;
    let canonical_resolved =
        std::fs::canonicalize(&resolved).map_err(|e| format!("Failed to resolve asset: {e}"))?;
    if !canonical_resolved.starts_with(&canonical_base) {
        return Err("Path traversal blocked".to_string());
    }

    std::fs::read(&canonical_resolved).map_err(|e| format!("Failed to read asset: {e}"))
}

/// Get the base URL for serving assets via asset:// protocol.
///
/// Returns the base path that assets are served from.
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn get_asset_base_url(app: tauri::AppHandle) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let base = data_dir.join("verses");

    Ok(base.to_string_lossy().to_string())
}
