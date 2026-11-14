use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use axum_extra::extract::Multipart;
use ed25519_dalek::SigningKey;
use fe_format::entitlement::{unix_now, IssuerKeyring, KeyringVerifier};
use fe_format::{AssetEntry, HexonManifest, HexonType};
use fe_hexon::entitlement_client::Licensee;
use fe_hexon::entitlement_gate::EntitlementGate;
use fe_hexon::manifest::InstalledHexon;
use fe_hexon::package::HexonPackage;
use fe_hexon::registry::{HexonRegistry, LoadEntryError, RegistryError};
use fe_identity::api_token::ApiClaims;
use serde::Deserialize;
use tracing::{info, warn};

use crate::auth::require_role;
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};
use fe_policy::{Action, AuthContext};

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
    hexon_id: String,
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
    tags: Vec<String>,
    description: Option<String>,
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
    hexon_id: String,
    /// `installed`, `installed-preview`, `entitlement-required` or `entitled`, as seen by a tokenless REST caller.
    entitlement: &'static str,
}

/// Envelope code of a licensed non-free entry refused to a caller without an entitlement.
const ENTITLEMENT_REQUIRED_CODE: &str = "entitlement_required";

/// The REST surface holds no licensee token: gates open tokenless against an empty keyring (see `src/AGENTS.md` §hexon-entitlement).
struct TokenlessEntitlement {
    identity: SigningKey,
    device_secret: [u8; 32],
    verifier: KeyringVerifier,
}

impl TokenlessEntitlement {
    fn new() -> Self {
        // Never consulted: with no token the gate decides from the licence terms alone.
        Self {
            identity: SigningKey::from_bytes(&[0u8; 32]),
            device_secret: [0u8; 32],
            verifier: KeyringVerifier::new(IssuerKeyring::new()),
        }
    }

    /// The gate a tokenless caller gets for one install.
    fn gate(&self, registry: &HexonRegistry, hexon_uri: &str, petal_id: &str) -> Result<EntitlementGate, RegistryError> {
        let licensee = Licensee { identity: &self.identity, device_secret: &self.device_secret };
        registry.gate_for(hexon_uri, petal_id, None, &licensee, &self.verifier)
    }

    /// Hexon Manager label of one install for a tokenless caller.
    fn label(&self, registry: &HexonRegistry, installed: &InstalledHexon) -> &'static str {
        match self.gate(registry, &installed.hexon_uri, &installed.petal_id) {
            Ok(gate) => gate.status_label_at(unix_now()),
            Err(_) => "entitlement-required",
        }
    }
}

fn to_manifest_dto(m: &HexonManifest) -> CrateManifestDto {
    CrateManifestDto {
        hexon_uri: fe_hexon::package::hexon_uri(m),
        hexon_id: m.hexon_id.clone(),
        name: m.name.clone(),
        description: m.description.clone().unwrap_or_default(),
        publisher_name: m.publisher_name.clone().unwrap_or_default(),
        publisher_did: m.publisher_did.clone(),
        version: m.version.clone(),
        hexon_type: m.hexon_type.to_string(),
        tags: m.tags.clone(),
        approx_size_bytes: m.approx_size_bytes.unwrap_or(0),
        min_engine_version: m.min_engine_version.clone().unwrap_or_default(),
        created_at: m.created_at.clone(),
    }
}

/// The snake_case wire name of an entry kind (fe-format's serde spelling).
fn entry_kind_name(kind: &fe_format::EntryKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_string(),
    }
}

fn to_entry_dto(e: &AssetEntry) -> CrateEntryDto {
    CrateEntryDto {
        entry_id: e.entry_id.clone(),
        kind: entry_kind_name(&e.kind),
        asset_hash: e.asset_hash.clone(),
        format: e.format.clone(),
        label: e.label.clone(),
        tags: e.tags.clone(),
        description: e.description.clone(),
        is_placeable: e.is_placeable,
    }
}

fn to_installed_dto(entitlement: &TokenlessEntitlement, registry: &HexonRegistry, ic: &InstalledHexon) -> InstalledCrateDto {
    InstalledCrateDto {
        entitlement: entitlement.label(registry, ic),
        hexon_uri: ic.hexon_uri.clone(),
        name: ic.manifest.name.clone(),
        version: ic.manifest.version.clone(),
        petal_id: ic.petal_id.clone(),
        installed_at: ic.installed_at.clone(),
        signature_valid: ic.signature_valid,
        entry_count: ic.entries.len(),
        hexon_id: ic.manifest.hexon_id.clone(),
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

    let Some(scope) = crate::rest::authorized_petal_scope(state, claims, petal_id).await else {
        return Err(crate::rest::NOT_FOUND_OR_DENIED);
    };

    let subject = crate::auth::api_token_context(claims);
    fe_hexon::authz::authorize(&subject, &action, &scope)
        .map_err(|_| "insufficient permissions")?;
    Ok(subject)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/crates/publish — Upload a .hexon package (multipart).
/// Requires Owner+ role.
pub async fn publish_crate(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    mut multipart: Multipart,
) -> JsonResult {
    if require_role(&claims, "owner").is_err() {
        return err("insufficient permissions");
    }

    let Some(registry) = get_registry(&state) else {
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
        return err("no .hexon file uploaded");
    }

    let package = match HexonPackage::open(&crate_bytes) {
        Ok(p) => p,
        Err(e) => return err(&format!("invalid package: {}", e)),
    };

    if !package.signature_valid() {
        return err("package signature verification failed");
    }

    let uri = package.hexon_uri();
    info!("Publishing hexon: {}", uri);

    let manifest_dto = to_manifest_dto(&package.manifest);
    let mut reg = registry.lock().unwrap();
    let subject = crate::auth::api_token_context(&claims);
    match reg.publish_as(&subject, package) {
        // Already published: return the manifest anyway.
        Ok(_) | Err(RegistryError::AlreadyInstalled(_)) => {
            // Entitlement checks need the content hash recomputed from these exact archive bytes.
            if let Err(error) = reg.bind_published_archive(&uri, &crate_bytes) {
                warn!("Published {} without an entitlement content hash: {}", uri, error);
            }
            ok(manifest_dto)
        }
        Err(fe_hexon::registry::RegistryError::PermissionDenied(_)) => {
            err("insufficient permissions")
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
    let subject = match require_hexon_petal_access(
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

    let mut reg = registry.lock().unwrap();
    let entitlement = TokenlessEntitlement::new();
    match reg.install_published_as(&subject, &hexon_uri, &query.petal_id) {
        Ok(installed) => {
            info!("Installed {} into petal {}", hexon_uri, query.petal_id);
            ok(to_installed_dto(&entitlement, &reg, &installed))
        }
        // Idempotent: a repeat install returns the existing record.
        Err(fe_hexon::registry::RegistryError::AlreadyInstalled(_)) => {
            match reg.get_in_petal(&hexon_uri, &query.petal_id) {
                Some(existing) => ok(to_installed_dto(&entitlement, &reg, existing)),
                None => err(&format!("crate {} not found", hexon_uri)),
            }
        }
        Err(fe_hexon::registry::RegistryError::PermissionDenied(_)) => {
            err("insufficient permissions")
        }
        Err(fe_hexon::registry::RegistryError::NotFound(_)) => {
            err(&format!("crate {} not found — publish it first", hexon_uri))
        }
        Err(e) => {
            warn!("Install failed: {}", e);
            err(&format!("install failed: {}", e))
        }
    }
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
        .and_then(|k| k.parse::<HexonType>().ok());

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

    let entitlement = TokenlessEntitlement::new();
    let dtos: Vec<InstalledCrateDto> = installed
        .iter()
        .map(|installed| to_installed_dto(&entitlement, &reg, installed))
        .collect();
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

/// GET /api/v1/crates/:uri/entries/:id/asset — Serve an entry's bytes through its entitlement gate.
/// Requires Viewer+ role; a licensed non-free entry is HTTP 403 `entitlement_required` (no REST token yet).
pub async fn get_crate_asset(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path((hexon_uri, entry_id)): Path<(String, String)>,
    Query(query): Query<InstallQuery>,
) -> Response {
    let subject =
        match require_hexon_petal_access(&state, &claims, &query.petal_id, "viewer", Action::Read)
            .await
        {
            Ok(subject) => subject,
            Err(message) => return err(message).into_response(),
        };

    let Some(registry) = get_registry(&state) else {
        return err("hexon registry not configured").into_response();
    };

    // Blob reads hit the disk: run them off the async workers.
    let petal_id = query.petal_id;
    let loaded = tokio::task::spawn_blocking(move || {
        let reg = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match reg.get_installed_as(&subject, &hexon_uri, &petal_id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(err(&format!("crate {} not found", hexon_uri)).into_response()),
            Err(error) => return Err(err(&format!("read denied: {error}")).into_response()),
        }
        let gate = TokenlessEntitlement::new()
            .gate(&reg, &hexon_uri, &petal_id)
            .map_err(|_| err(&format!("crate {} not found", hexon_uri)).into_response())?;
        reg.load_entry(&hexon_uri, &petal_id, &entry_id, &gate)
            .map(std::borrow::Cow::into_owned)
            .map_err(|error| asset_refusal(&hexon_uri, &entry_id, error))
    })
    .await;

    match loaded {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"))],
            bytes,
        )
            .into_response(),
        Ok(Err(refusal)) => refusal,
        Err(error) => {
            warn!("crate asset read task failed: {error}");
            err("asset read failed").into_response()
        }
    }
}

/// Response for an entry the registry did not serve; only an entitlement refusal changes the HTTP status.
fn asset_refusal(hexon_uri: &str, entry_id: &str, error: LoadEntryError) -> Response {
    use fe_hexon::entitlement_gate::GateRefusal;
    if error.is_entitlement_required() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "ok": false,
                "code": ENTITLEMENT_REQUIRED_CODE,
                "error": format!("entry {entry_id} needs an entitlement"),
            })),
        )
            .into_response();
    }
    let message = match error {
        LoadEntryError::NotInstalled(_) => format!("crate {hexon_uri} not found"),
        LoadEntryError::Gate(GateRefusal::UnknownEntry(_)) => {
            format!("entry {entry_id} not found in crate {hexon_uri}")
        }
        LoadEntryError::Gate(GateRefusal::MissingContent(_)) => {
            format!("asset of entry {entry_id} not available in store")
        }
        other => {
            warn!("Crate asset {} / {} refused: {}", hexon_uri, entry_id, other);
            format!("entry {entry_id} could not be loaded")
        }
    };
    err(&message).into_response()
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
        .and_then(|k| k.parse::<HexonType>().ok());

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
