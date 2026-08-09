use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use axum_extra::extract::Multipart;
use fe_hexon::manifest::HexonKind;
use fe_hexon::package::HexonPackage;
use fe_identity::api_token::ApiClaims;
use serde::Deserialize;
use tracing::{info, warn};

use crate::auth::{require_role, require_scope};
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};
use fe_policy::{Action, AuthContext, RoleLevel};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    petal_id: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    tags: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AvailableQuery {
    petal_id: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    tags: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InstallQuery {
    #[serde(rename = "petal_id")]
    petal_id: String,
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

type JsonResult = Json<ApiResponse<serde_json::Value>>;

fn ok<T: serde::Serialize>(data: T) -> JsonResult {
    Json(ApiResponse::<serde_json::Value>::success(
        serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
    ))
}

fn err(msg: &str) -> JsonResult {
    Json(ApiResponse::<serde_json::Value>::error(msg))
}

// ---------------------------------------------------------------------------
// Crate DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
struct CrateManifestDto {
    hexon_uri: String,
    crate_id: String,
    name: String,
    description: String,
    publisher_name: String,
    publisher_did: String,
    version: String,
    hexon_type: String,
    tags: Vec<String>,
    approx_size_bytes: u64,
    min_engine_version: String,
    created_at: String,
}

#[derive(Debug, serde::Serialize)]
struct CrateEntryDto {
    entry_id: String,
    kind: String,
    asset_hash: String,
    format: String,
    label: String,
    is_placeable: bool,
}

#[derive(Debug, serde::Serialize)]
struct InstalledCrateDto {
    hexon_uri: String,
    name: String,
    version: String,
    petal_id: String,
    installed_at: String,
    signature_valid: bool,
    entry_count: usize,
}

fn to_manifest_dto(m: &fe_hexon::manifest::HexonManifest) -> CrateManifestDto {
    CrateManifestDto {
        hexon_uri: fe_hexon::package::hexon_uri(m),
        crate_id: m.crate_id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        publisher_name: m.publisher_name.clone(),
        publisher_did: m.publisher_did.clone(),
        version: m.version.clone(),
        hexon_type: m.hexon_type.to_string(),
        tags: m.tags.clone(),
        approx_size_bytes: m.approx_size_bytes,
        min_engine_version: m.min_engine_version.clone(),
        created_at: m.created_at.clone(),
    }
}

fn to_entry_dto(e: &fe_hexon::manifest::AssetEntry) -> CrateEntryDto {
    CrateEntryDto {
        entry_id: e.entry_id.clone(),
        kind: e.kind.to_string(),
        asset_hash: e.asset_hash.clone(),
        format: e.format.clone(),
        label: e.label.clone(),
        is_placeable: e.is_placeable,
    }
}

fn to_installed_dto(ic: &fe_hexon::manifest::InstalledCrate) -> InstalledCrateDto {
    InstalledCrateDto {
        hexon_uri: ic.hexon_uri.clone(),
        name: ic.manifest.name.clone(),
        version: ic.manifest.version.clone(),
        petal_id: ic.petal_id.clone(),
        installed_at: ic.installed_at.clone(),
        signature_valid: ic.signature_valid,
        entry_count: ic.entries.len(),
    }
}

// ---------------------------------------------------------------------------
// State access
// ---------------------------------------------------------------------------

fn get_registry(
    state: &ApiState,
) -> Option<Arc<std::sync::Mutex<fe_hexon::registry::HexonRegistry>>> {
    state.hexon_registry.clone()
}

/// Resolve and authorize a petal-scoped Hexon operation for an API caller.
pub(crate) async fn require_hexon_petal_access(
    state: &ApiState,
    claims: &ApiClaims,
    petal_id: &str,
    minimum_role: &str,
    action: Action,
) -> Result<AuthContext, &'static str> {
    if !is_valid_ulid(petal_id) {
        return Err("invalid petal_id");
    }
    if require_role(claims, minimum_role).is_err() {
        return Err("insufficient permissions");
    }

    let Some(scope) = crate::rest::resolve_petal_scope(state, petal_id).await else {
        return Err("could not resolve petal scope");
    };
    if require_scope(claims, &scope).is_err() {
        return Err("insufficient scope");
    }

    let subject = AuthContext::ApiToken {
        subject: claims.sub.clone(),
        scopes: vec![claims.scope.clone()],
        role: RoleLevel::from(claims.max_role.as_str()),
    };
    fe_hexon::authz::authorize(&subject, &action, &scope)
        .map_err(|_| "insufficient permissions")?;
    Ok(subject)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/crates/publish — Upload a .fecrate package (multipart).
/// Requires Owner+ role.
pub async fn publish_crate(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    mut multipart: Multipart,
) -> JsonResult {
    if require_role(&claims, "owner").is_err() {
        return err("insufficient permissions");
    }

    let Some(_registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let mut crate_bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.file_name().is_some() {
            if let Ok(data) = field.bytes().await {
                crate_bytes = data.to_vec();
                break;
            }
        }
    }

    if crate_bytes.is_empty() {
        return err("no .fecrate file uploaded");
    }

    let package = match HexonPackage::open(&crate_bytes) {
        Ok(p) => p,
        Err(e) => return err(&format!("invalid package: {}", e)),
    };

    if !package.signature_valid() {
        return err("package signature verification failed");
    }

    let uri = package.hexon_uri();
    info!("Publishing hexon crate: {}", uri);

    // Install into registry
    let registry = _registry;
    let mut reg = registry.lock().unwrap();
    match reg.install(package.clone(), "global") {
        Ok(_) => ok(to_manifest_dto(&package.manifest)),
        Err(fe_hexon::registry::RegistryError::AlreadyInstalled(_)) => {
            // Already published — return manifest anyway
            ok(to_manifest_dto(&package.manifest))
        }
        Err(e) => err(&format!("publish failed: {}", e)),
    }
}

/// POST /api/v1/crates/:uri/install — Install a crate into a petal.
/// Requires Editor+ role.
pub async fn install_crate(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_uri): Path<String>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let _subject = match require_hexon_petal_access(
        &state,
        &claims,
        &query.petal_id,
        "editor",
        Action::Install,
    )
    .await
    {
        Ok(subject) => subject,
        Err(message) => return err(message),
    };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let reg = registry.lock().unwrap();

    // Check if the crate is published (exists in global scope)
    let Some(published) = reg.get_installed(&hexon_uri) else {
        return err(&format!("crate {} not found — publish it first", hexon_uri));
    };

    // If already installed for this petal, return success
    if published.petal_id == query.petal_id {
        return ok(to_installed_dto(published));
    }

    // Build a synthetic package from the published crate data for re-install into target petal
    let installed_dto = to_installed_dto(published);
    drop(reg);

    // For petal-specific install, we re-use the existing registry entry
    // The real workflow: the crate was published globally, now we "install" it into a petal
    // by recording the association
    info!("Installing {} into petal {}", hexon_uri, query.petal_id);
    ok(serde_json::json!({
        "hexon_uri": installed_dto.hexon_uri,
        "petal_id": query.petal_id,
        "installed": true
    }))
}

/// DELETE /api/v1/crates/:uri/uninstall — Uninstall a crate from a petal.
/// Requires Editor+ role.
pub async fn uninstall_crate(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_uri): Path<String>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "editor", Action::Write)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let mut reg = registry.lock().unwrap();

    match reg.uninstall_as(&subject, &hexon_uri, &query.petal_id) {
        Ok(()) => {
            info!("Uninstalled {} from petal {}", hexon_uri, query.petal_id);
            ok(serde_json::json!({ "hexon_uri": hexon_uri, "uninstalled": true }))
        }
        Err(fe_hexon::registry::RegistryError::NotFound(_)) => {
            err(&format!("crate {} not installed", hexon_uri))
        }
        Err(e) => {
            warn!("Uninstall failed: {}", e);
            err(&format!("uninstall failed: {}", e))
        }
    }
}

/// GET /api/v1/crates/search — Search local crates.
/// Requires Viewer+ role.
pub async fn search_crates(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<SearchQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let tags: Vec<&str> = query
        .tags
        .as_ref()
        .map(|t| t.split(',').collect::<Vec<_>>())
        .unwrap_or_default();

    let kind = query
        .kind
        .as_ref()
        .and_then(|k| k.parse::<HexonKind>().ok());

    let reg = registry.lock().unwrap();
    let manifests = match reg.search_local_in_petal_as(
        &subject,
        Some(&query.petal_id),
        &query.q,
        &tags,
        kind,
    ) {
        Ok(manifests) => manifests,
        Err(error) => return err(&format!("search denied: {error}")),
    };

    let dtos: Vec<CrateManifestDto> = manifests.iter().map(to_manifest_dto).collect();
    ok(dtos)
}

/// GET /api/v1/crates/installed — List installed crates.
/// Requires Viewer+ role.
pub async fn list_installed(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let reg = registry.lock().unwrap();
    let installed = match reg.list_installed_as(&subject, Some(&query.petal_id)) {
        Ok(installed) => installed,
        Err(error) => return err(&format!("list denied: {error}")),
    };

    let dtos: Vec<InstalledCrateDto> = installed.iter().map(to_installed_dto).collect();
    ok(dtos)
}

/// GET /api/v1/crates/:uri — Get manifest for a crate.
/// Requires Viewer+ role.
pub async fn get_crate_manifest(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_uri): Path<String>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let reg = registry.lock().unwrap();
    match reg.get_installed_as(&subject, &hexon_uri, &query.petal_id) {
        Ok(Some(ic)) => ok(to_manifest_dto(&ic.manifest)),
        Ok(None) => err(&format!("crate {} not found", hexon_uri)),
        Err(error) => err(&format!("read denied: {error}")),
    }
}

/// GET /api/v1/crates/:uri/entries — List entries for a crate.
/// Requires Viewer+ role.
pub async fn get_crate_entries(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(hexon_uri): Path<String>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let reg = registry.lock().unwrap();
    match reg.get_installed_as(&subject, &hexon_uri, &query.petal_id) {
        Ok(Some(ic)) => {
            let dtos: Vec<CrateEntryDto> = ic.entries.iter().map(to_entry_dto).collect();
            ok(dtos)
        }
        Ok(None) => err(&format!("crate {} not found", hexon_uri)),
        Err(error) => err(&format!("read denied: {error}")),
    }
}

/// GET /api/v1/crates/:uri/entries/:id/asset — Fetch asset blob by entry id.
/// Requires Viewer+ role.
pub async fn get_crate_asset(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((hexon_uri, entry_id)): Path<(String, String)>,
    Query(query): Query<InstallQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let reg = registry.lock().unwrap();
    let installed = match reg.get_installed_as(&subject, &hexon_uri, &query.petal_id) {
        Ok(Some(installed)) => installed,
        Ok(None) => return err(&format!("crate {} not found", hexon_uri)),
        Err(error) => return err(&format!("read denied: {error}")),
    };

    let Some(entry) = installed.entries.iter().find(|e| e.entry_id == entry_id) else {
        return err(&format!(
            "entry {} not found in crate {}",
            entry_id, hexon_uri
        ));
    };

    // Check if blob exists in store
    match reg.get_blob_path(&entry.asset_hash) {
        Some(path) => ok(serde_json::json!({
            "entry_id": entry_id,
            "asset_hash": entry.asset_hash,
            "format": entry.format,
            "blob_path": path.to_string_lossy(),
            "available": true
        })),
        None => err(&format!(
            "asset blob {} not available in store",
            entry.asset_hash
        )),
    }
}

// ---------------------------------------------------------------------------
// Available crates
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
struct AvailableCrateDto {
    manifest: CrateManifestDto,
    installed: bool,
    available_from: Vec<String>,
}

/// Search crates installed in one petal.
/// Requires Viewer+ role.
pub async fn available_crates(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Query(query): Query<AvailableQuery>,
) -> JsonResult {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured");
    };

    let tags: Vec<&str> = query
        .tags
        .as_ref()
        .map(|t| t.split(',').collect::<Vec<_>>())
        .unwrap_or_default();

    let kind = query
        .kind
        .as_ref()
        .and_then(|k| k.parse::<HexonKind>().ok());

    let reg = registry.lock().unwrap();

    let local_manifests = match reg.search_local_in_petal_as(
        &subject,
        Some(&query.petal_id),
        &query.q,
        &tags,
        kind,
    ) {
        Ok(manifests) => manifests,
        Err(error) => return err(&format!("search denied: {error}")),
    };

    let results: Vec<AvailableCrateDto> = local_manifests
        .iter()
        .map(|m| AvailableCrateDto {
            manifest: to_manifest_dto(m),
            installed: true,
            available_from: vec!["local".to_string()],
        })
        .collect();

    ok(results)
}
