//! Tauri IPC Command Handlers for the FractalEngine.
//!
//! These commands provide the bridge between the Tauri webview (JS) and Bevy (Rust).
//! They enable node data queries, interaction notifications, and asset resolution.

#[cfg(feature = "backend-tauri")]
use fe_runtime::shared_node::{validate_asset_path, SharedNode, WebViewInteraction};
#[cfg(feature = "backend-tauri")]
use tauri::Manager;

/// Get node data by ID (unwired stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn get_node_data(node_id: String) -> Result<SharedNode, String> {
    tracing::warn!(
        "get_node_data called with node_id: {} - needs VerseManager integration",
        node_id
    );

    Ok(SharedNode {
        node_id,
        verse_id: "default-verse".to_string(),
        fractal_id: "default-fractal".to_string(),
        petal_id: "default-petal".to_string(),
        position: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        webpage_url: None,
        asset_path: None,
        properties: std::collections::HashMap::new(),
    })
}

/// Notify the system of a webview interaction (logging stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn notify_interaction(interaction: WebViewInteraction) -> Result<(), String> {
    tracing::debug!("Received webview interaction: {:?}", interaction);

    match &interaction {
        WebViewInteraction::NodeSelected { node } => {
            tracing::info!("Node selected in webview: {}", node.node_id);
        }
        WebViewInteraction::NodeDeselected { node_id } => {
            tracing::info!("Node deselected in webview: {}", node_id);
        }
        WebViewInteraction::TransformChanged {
            node_id,
            position,
            rotation,
            scale,
        } => {
            tracing::info!(
                "Transform changed for node {}: pos={:?}, rot={:?}, scale={:?}",
                node_id,
                position,
                rotation,
                scale
            );
        }
        WebViewInteraction::PropertyChanged {
            node_id,
            key,
            value,
        } => {
            tracing::info!(
                "Property changed for node {}: {} = {:?}",
                node_id,
                key,
                value
            );
        }
        WebViewInteraction::UrlChanged { node_id, url } => {
            tracing::info!("URL changed for node {}: {}", node_id, url);
        }
    }

    Ok(())
}

/// List all nodes for a petal (unwired stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn list_nodes_for_petal(petal_id: String) -> Result<Vec<SharedNode>, String> {
    tracing::debug!("Listing nodes for petal: {}", petal_id);

    Ok(vec![])
}

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

/// Update a node's transform from the webview (unwired stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn update_node_transform(
    node_id: String,
    position: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
) -> Result<(), String> {
    tracing::info!(
        "Update transform for node {}: pos={:?}, rot={:?}, scale={:?}",
        node_id,
        position,
        rotation,
        scale
    );

    Ok(())
}

/// Update a node's URL from the webview (unwired stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn update_node_url(node_id: String, url: String) -> Result<(), String> {
    tracing::info!("Update URL for node {}: {}", node_id, url);

    Ok(())
}

/// Get the list of available petals (unwired stub — see src/AGENTS.md §tauri-commands).
#[cfg(feature = "backend-tauri")]
#[tauri::command]
pub fn list_petals() -> Result<Vec<PetalInfo>, String> {
    Ok(vec![])
}

/// Information about a petal (returned to the webview).
#[cfg(feature = "backend-tauri")]
#[derive(serde::Serialize)]
pub struct PetalInfo {
    pub petal_id: String,
    pub name: String,
    pub node_count: usize,
}
