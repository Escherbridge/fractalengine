//! Extensions API (spec R11/R12/R15, bus.md §8): status, audit, the GUI-only administration refusal,
//! commands, ingress mount and extension streams; rationale in `fe-api/AGENTS.md` §extensions.
//!
//! fe-api never links the plugin host: the binary implements [`ExtensionHost`] over `fe_plugin`'s
//! `Bus`/`Net`/lifecycle, and this module owns the HTTP-facing rules (remote rate gate, uniform
//! not-found, credential-header stripping, status mapping). REST, MCP and `/ws` call the same
//! core functions so each rule lives once.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use fe_identity::api_token::ApiClaims;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

use crate::rest::{authorized_petal_scope, NOT_FOUND_OR_DENIED};
use crate::server::ApiState;
use crate::types::ApiResponse;

/// Largest command `args` (bus.md §6).
pub const MAX_COMMAND_ARGS_BYTES: usize = 16 * 1024;
/// Largest ingress body (matches `fe_plugin::net::INGRESS_BODY_CEILING`).
pub const MAX_INGRESS_BODY_BYTES: usize = 1024 * 1024;
/// A command or ingress reply later than this answers 504 (bus.md §4: 5 s, plus the broker's own timeout margin).
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(6_000);
/// Audit rows one `GET .../audit` returns at most.
pub const MAX_AUDIT_ROWS: usize = 500;

/// Credential and host-authenticating headers are never forwarded to a guest, whatever the manifest declares.
pub use fe_sdk::manifest_v2::is_credential_header;

// ---------------------------------------------------------------------------
// Host seam
// ---------------------------------------------------------------------------

/// Which external surface a call came through (bus.md §4 `caller.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiCallerKind {
    Rest,
    Mcp,
}

/// The authenticated caller; `did` is the token's `sub`, `role_ceiling` its `max_role`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiCaller {
    pub did: String,
    pub kind: ApiCallerKind,
    /// Every node-minted token carries the node DID, so the host caps the DID's role by this (AGENTS.md §extensions).
    pub role_ceiling: fe_policy::RoleLevel,
}

/// Why the broker refused an external call (mirrors `fe_plugin::bus::BusRefusal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRefusal {
    NotFound,
    NotAvailable(String),
    CapabilityDenied(String),
    RateLimited(String),
    InvalidInput(String),
    LimitReached(String),
}

/// A guest or host failure after an admitted command (mirrors `CommandOutcome::{Guest, Host}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandFailure {
    InvalidInput(String),
    Refused(String),
    Failed(String),
    UnknownOutcome(String),
    NotAvailable(String),
}

/// The eventual answer of an admitted command: the guest's JSON text or a failure.
pub type CommandReply = oneshot::Receiver<Result<String, CommandFailure>>;

/// Lifecycle verbs the API refuses for every caller (GUI and local `fe-ext admin` only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminVerb {
    Install,
    Consent,
    Bind,
    Unbind,
    Enable,
    Disable,
    Update,
    Uninstall,
    DisableAll,
    EnableAll,
    /// The verse Owner's audited kill-list override on this host (governance.md precondition 5);
    /// params `{verse_id, package_id, content_hash, confirm}` with `confirm` = `package_id`.
    KillListOverride,
}

impl AdminVerb {
    /// The audit/log text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Consent => "consent",
            Self::Bind => "bind",
            Self::Unbind => "unbind",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
            Self::DisableAll => "disable-all",
            Self::EnableAll => "enable-all",
            Self::KillListOverride => "kill-list-override",
        }
    }

    /// Verbs addressed at one installation (`POST /extensions/{installation}/{verb}`).
    pub fn parse_installation_verb(verb: &str) -> Option<Self> {
        match verb {
            "consent" => Some(Self::Consent),
            "bind" => Some(Self::Bind),
            "unbind" => Some(Self::Unbind),
            "enable" => Some(Self::Enable),
            "disable" => Some(Self::Disable),
            "update" => Some(Self::Update),
            _ => None,
        }
    }
}

/// One administration request after the local-identity check.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminOp {
    pub verb: AdminVerb,
    /// `None` for `install`, `disable-all` and `enable-all`.
    pub installation: Option<String>,
    /// The verb's JSON body (`package_id`, `petal_id`, `digest`, ...); `{}` when empty.
    pub params: Value,
}

/// An activation precondition that failed (governance.md "Activation preconditions"), or a bad request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRefusal {
    /// The stable code: `host-off`, `package-invalid`, `untrusted-signer`, `revoked`,
    /// `entitlement-required`, `policy-denied`, `role-denied`, `consent-missing`, `consent-mismatch`,
    /// `invalid-policy`, `confirmation-mismatch`, `not-available`, `not-found`, `invalid-input`.
    pub code: String,
    pub message: String,
}

/// The eventual answer of a host call that touches the database (policy reads and writes).
pub type AdminReply = oneshot::Receiver<Result<Value, AdminRefusal>>;

/// A reply for a host that has not wired the call yet.
fn not_wired(what: &str) -> AdminReply {
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(Err(AdminRefusal {
        code: "not-available".into(),
        message: format!("{what} is not wired on this host"),
    }));
    rx
}

/// A request on the ingress mount, after bearer auth and credential-header stripping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressCall {
    pub petal_id: String,
    pub package_id: String,
    pub endpoint: String,
    pub caller_did: String,
    /// The token's `max_role`: the host caps the DID's role by this (a delegated token of this node stays remote, G1).
    pub role_ceiling: fe_policy::RoleLevel,
    pub method: String,
    pub sub_path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Why the mount refused (mirrors `fe_plugin::net::IngressRefusal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressRefusal {
    NotFound,
    TooManyRequests,
    PayloadTooLarge,
    MethodNotAllowed,
    UnsupportedMediaType,
}

/// A guest's HTTP answer to an ingress request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressAnswer {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// A websocket upgrade on an accept-mode ingress endpoint, after bearer auth. Carries no headers at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsAcceptCall {
    pub petal_id: String,
    pub package_id: String,
    pub endpoint: String,
    pub caller_did: String,
    /// The token's `max_role` (see [`IngressCall::role_ceiling`]).
    pub role_ceiling: fe_policy::RoleLevel,
}

/// One websocket data frame, either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsData {
    Text(String),
    Binary(Vec<u8>),
}

/// What the connection's writer sends: a frame, or a close the host ordered (1008 demotion, 1001 stop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsOut {
    Frame(WsData),
    Close { code: u16, reason: String },
}

/// An accepted websocket: feed inbound frames to `websocket_frame`, write `outbound`, then `websocket_closed`.
#[derive(Debug)]
pub struct WsHostSession {
    pub connection: u64,
    pub outbound: tokio::sync::mpsc::Receiver<WsOut>,
}

/// What the binary provides: the plugin host behind the extension routes.
pub trait ExtensionHost: Send + Sync {
    /// The host's local DID (the identity extensions write as).
    fn local_did(&self) -> String;
    /// bus.md §4 check 0: a remote DID's request rate on extension routes.
    fn admit_remote(&self, did: &str) -> bool;
    /// One status document per installation (R11 states and counters).
    fn list(&self) -> Vec<Value>;
    /// One installation's status document.
    fn status(&self, installation: &str) -> Option<Value>;
    /// The newest `limit` audit rows of one installation, newest first.
    fn audit(&self, installation: &str, limit: usize) -> Option<Vec<Value>>;
    /// Run a lifecycle verb as the local DID; reached by the GUI and the local admin channel, never by REST/MCP/WS.
    fn administer(&self, actor_did: &str, op: AdminOp) -> Result<Value, AdminRefusal>;
    /// Record one `denial` audit row for a non-local administration attempt (R12).
    fn record_denied_admin(&self, actor_did: &str, target: &str);
    /// bus.md §4 checks 1-8; remote callers only ever see [`HostRefusal::NotFound`].
    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        args: &Value,
    ) -> Result<CommandReply, HostRefusal>;
    /// Route one request to the accept-mode ingress mount.
    fn ingress(&self, call: IngressCall) -> Result<oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal>;
    /// Admit a websocket on an accept-mode endpoint (role floor, `max_connections` -> `TooManyRequests`).
    fn open_websocket(&self, _call: WsAcceptCall) -> Result<WsHostSession, IngressRefusal> {
        Err(IngressRefusal::NotFound)
    }
    /// One inbound frame; the host drops and counts over-size or over-rate frames (never an error to the client).
    fn websocket_frame(&self, _connection: u64, _frame: WsData) {}
    /// The connection ended (client close, error or a host close already sent).
    fn websocket_closed(&self, _connection: u64) {}
    /// Open an extension-stream client for a REST/WS/MCP connection.
    fn open_client(&self, caller: &ApiCaller) -> (u64, Arc<Notify>);
    /// Subscribe a client at a petal to `<package_id>/<stream>`.
    fn client_subscribe(
        &self,
        client: u64,
        petal_id: &str,
        source: &str,
        params: &Value,
    ) -> Result<u64, HostRefusal>;
    /// End one subscription.
    fn client_unsubscribe(&self, client: u64, subscription: u64);
    /// Up to `max` queued events as bus.md §5 envelope JSON texts.
    fn take_client_events(&self, client: u64, max: usize) -> Vec<String>;
    /// Close a client and all its subscriptions.
    fn close_client(&self, client: u64);
    /// The verse's replicated extension policy row (`{"policy": null}` when none: `first-party-only`).
    fn verse_policy(&self, _verse_id: &str) -> AdminReply {
        not_wired("the verse extension policy")
    }
    /// Write the verse policy AS THE LOCAL DID (fe-database `set_policy_with_replication`); the caller's
    /// own Manager+ and scope were checked here and `actor_did` (the token `sub`) rides `SetExtensionPolicy`
    /// into the `policy-change` audit entry, which is written either way.
    fn set_verse_policy(&self, _actor_did: &str, _verse_id: &str, _policy: Value) -> AdminReply {
        not_wired("the verse extension policy")
    }
}

/// The host as an axum `Extension`.
#[derive(Clone)]
pub struct ExtensionHostHandle(pub Arc<dyn ExtensionHost>);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A failure with its HTTP status, stable code and message; every surface renders this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ExtError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    /// The uniform not-found: identical for unknown, foreign, forbidden and non-local (fe-api §existence-oracle).
    pub fn not_found() -> Self {
        Self::new(404, "not-found", NOT_FOUND_OR_DENIED)
    }

    /// Remote request rate spent.
    pub fn rate_limited(detail: impl Into<String>) -> Self {
        Self::new(429, "rate-limited", detail)
    }

    /// Bad arguments.
    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::new(400, "invalid-input", detail)
    }

    /// The HTTP status as a typed code.
    pub fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_GATEWAY)
    }
}

impl From<HostRefusal> for ExtError {
    fn from(refusal: HostRefusal) -> Self {
        match refusal {
            HostRefusal::NotFound => Self::not_found(),
            HostRefusal::NotAvailable(detail) => Self::new(404, "not-available", detail),
            HostRefusal::CapabilityDenied(detail) => Self::new(403, "capability-denied", detail),
            HostRefusal::RateLimited(detail) => Self::new(429, "rate-limited", detail),
            HostRefusal::InvalidInput(detail) => Self::new(400, "invalid-input", detail),
            HostRefusal::LimitReached(detail) => Self::new(429, "limit-reached", detail),
        }
    }
}

impl From<CommandFailure> for ExtError {
    fn from(failure: CommandFailure) -> Self {
        match failure {
            CommandFailure::InvalidInput(detail) => Self::new(400, "invalid-input", detail),
            CommandFailure::Refused(detail) => Self::new(409, "refused", detail),
            CommandFailure::Failed(detail) => Self::new(502, "failed", detail),
            CommandFailure::UnknownOutcome(detail) => Self::new(504, "unknown-outcome", detail),
            CommandFailure::NotAvailable(detail) => Self::new(404, "not-available", detail),
        }
    }
}

impl From<AdminRefusal> for ExtError {
    fn from(refusal: AdminRefusal) -> Self {
        let (status, code) = match refusal.code.as_str() {
            "host-off" => (503, "host-off"),
            "package-invalid" => (422, "package-invalid"),
            "untrusted-signer" => (422, "untrusted-signer"),
            "revoked" => (409, "revoked"),
            "entitlement-required" => (403, "entitlement-required"),
            "policy-denied" => (403, "policy-denied"),
            "role-denied" => (403, "role-denied"),
            "consent-missing" => (409, "consent-missing"),
            "consent-mismatch" => (409, "consent-mismatch"),
            "invalid-policy" => (422, "invalid-policy"),
            "confirmation-mismatch" => (409, "confirmation-mismatch"),
            "not-available" => (503, "not-available"),
            // R12: a non-local actor is indistinguishable from an unknown target.
            "not-found" | "not-local-identity" => return Self::not_found(),
            _ => (400, "invalid-input"),
        };
        Self::new(status, code, refusal.message)
    }
}

impl From<IngressRefusal> for ExtError {
    fn from(refusal: IngressRefusal) -> Self {
        match refusal {
            IngressRefusal::NotFound => Self::not_found(),
            IngressRefusal::TooManyRequests => Self::rate_limited("ingress rate limit"),
            IngressRefusal::PayloadTooLarge => Self::new(413, "payload-too-large", "body too large"),
            IngressRefusal::MethodNotAllowed => Self::new(405, "method-not-allowed", "method not allowed"),
            IngressRefusal::UnsupportedMediaType => {
                Self::new(415, "unsupported-media-type", "unsupported media type")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Core (shared by REST, MCP and /ws)
// ---------------------------------------------------------------------------

/// The caller as the host sees it.
pub fn caller_of(claims: &ApiClaims, kind: ApiCallerKind) -> ApiCaller {
    ApiCaller { did: claims.sub.clone(), kind, role_ceiling: fe_policy::RoleLevel::from(claims.max_role.as_str()) }
}

/// The local identity (R12): the host's DID under an Owner token. fe-identity mints EVERY token with the
/// node DID as `sub`, so the DID alone would make each delegated viewer or editor token "local".
pub fn is_local(host: &dyn ExtensionHost, claims: &ApiClaims) -> bool {
    claims.sub == host.local_did() && crate::auth::require_role_level(claims, fe_policy::RoleLevel::Owner).is_ok()
}

/// The per-caller key of the remote rate gate: a delegated token of this node is limited per token (`jti`).
fn rate_subject(host: &dyn ExtensionHost, claims: &ApiClaims) -> String {
    if claims.sub == host.local_did() {
        format!("{}#{}", claims.sub, claims.jti)
    } else {
        claims.sub.clone()
    }
}

/// Every extension route runs this FIRST: a remote DID's per-DID request rate, before any lookup (not audited).
pub fn gate(host: &dyn ExtensionHost, claims: &ApiClaims) -> Result<(), ExtError> {
    if is_local(host, claims) || host.admit_remote(&rate_subject(host, claims)) {
        Ok(())
    } else {
        Err(ExtError::rate_limited("extension request rate limit"))
    }
}

/// Local-only reads (status, audit, list): everyone else gets the uniform not-found.
fn require_local(host: &dyn ExtensionHost, claims: &ApiClaims) -> Result<(), ExtError> {
    gate(host, claims)?;
    if is_local(host, claims) {
        Ok(())
    } else {
        Err(ExtError::not_found())
    }
}

/// `GET /api/v1/extensions` and MCP `list_extensions`.
pub fn list_extensions(host: &dyn ExtensionHost, claims: &ApiClaims) -> Result<Value, ExtError> {
    require_local(host, claims)?;
    Ok(json!({ "extensions": host.list() }))
}

/// `GET /api/v1/extensions/{installation}`.
pub fn extension_status(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    installation: &str,
) -> Result<Value, ExtError> {
    require_local(host, claims)?;
    host.status(installation).ok_or_else(ExtError::not_found)
}

/// `GET /api/v1/extensions/{installation}/audit`; `limit` is clamped to [`MAX_AUDIT_ROWS`].
pub fn extension_audit(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    installation: &str,
    limit: Option<usize>,
) -> Result<Value, ExtError> {
    require_local(host, claims)?;
    let limit = limit.unwrap_or(100).clamp(1, MAX_AUDIT_ROWS);
    let rows = host.audit(installation, limit).ok_or_else(ExtError::not_found)?;
    Ok(json!({ "installation": installation, "entries": rows }))
}

/// Longest installation id a denial audit row records.
const MAX_AUDIT_TARGET_CHARS: usize = 128;

/// The refusal every administration verb gets on REST, MCP and `/ws` (user decision 2026-11-09).
pub const ADMIN_GUI_ONLY_MESSAGE: &str = "extension administration is not available over the API; use the Extensions panel or fe-ext admin";

/// Install, consent, bind, enable, disable, update, uninstall and the kill-list override are GUI-only plus the
/// local `fe-ext admin` channel: this door refuses them for EVERY caller, the verse Owner included, and writes one
/// `denial` audit row. A non-local actor keeps the uniform not-found (R12: no existence oracle).
pub fn administer(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    op: AdminOp,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    // The path segment is caller-sized: the denial row keeps a bounded, control-free prefix.
    let target = match &op.installation {
        Some(installation) => {
            let installation: String =
                installation.chars().filter(|c| !c.is_control()).take(MAX_AUDIT_TARGET_CHARS).collect();
            format!("{}@{installation}", op.verb.as_str())
        }
        None => op.verb.as_str().to_string(),
    };
    host.record_denied_admin(&claims.sub, &target);
    if is_local(host, claims) {
        Err(ExtError::new(403, "admin-gui-only", ADMIN_GUI_ONLY_MESSAGE))
    } else {
        Err(ExtError::not_found())
    }
}

/// Wait for a database-backed host answer under the reply deadline.
async fn await_admin(reply: AdminReply) -> Result<Value, ExtError> {
    match tokio::time::timeout(REPLY_TIMEOUT, reply).await {
        Ok(Ok(result)) => result.map_err(ExtError::from),
        Ok(Err(_)) => Err(ExtError::new(503, "not-available", "the extension host stopped before answering")),
        Err(_) => Err(ExtError::new(504, "unknown-outcome", "the extension host did not answer in time")),
    }
}

/// `GET /api/v1/verses/{verse_id}/extension-policy`: any token whose scope covers the verse (Viewer+).
/// `verse_authorized` is that scope check; false is the uniform not-found.
pub async fn get_verse_policy(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    verse_authorized: bool,
    verse_id: &str,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    read_verse_policy(host, verse_authorized, verse_id).await
}

/// [`get_verse_policy`] after the rate gate (the route gates once, before its scope lookup).
async fn read_verse_policy(host: &dyn ExtensionHost, verse_authorized: bool, verse_id: &str) -> Result<Value, ExtError> {
    if !verse_authorized {
        return Err(ExtError::not_found());
    }
    await_admin(host.verse_policy(verse_id)).await
}

/// `PUT /api/v1/verses/{verse_id}/extension-policy` (R18): a Manager+ token in the verse; the host writes the
/// row as its local DID, which must hold Manager+ there too (the `device_format` rule). A refused role leaves
/// one `denial` audit entry.
pub async fn put_verse_policy(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    verse_authorized: bool,
    verse_id: &str,
    policy: Value,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    write_verse_policy(host, claims, verse_authorized, verse_id, policy).await
}

/// [`put_verse_policy`] after the rate gate.
async fn write_verse_policy(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    verse_authorized: bool,
    verse_id: &str,
    policy: Value,
) -> Result<Value, ExtError> {
    if !verse_authorized {
        return Err(ExtError::not_found());
    }
    if crate::auth::require_role_level(claims, fe_policy::RoleLevel::Manager).is_err() {
        host.record_denied_admin(&claims.sub, &format!("policy-change@{verse_id}"));
        return Err(ExtError::new(403, "role-denied", "setting a verse extension policy needs Manager+"));
    }
    if !policy.is_object() {
        return Err(ExtError::invalid("the policy must be a JSON object"));
    }
    await_admin(host.set_verse_policy(&claims.sub, verse_id, policy)).await
}

/// Call `<package_id>/<command>` at a petal. `petal_authorized` is the caller's token-scope check on the
/// petal (false = out of scope or unknown): it yields the same not-found as an uninstalled package.
#[allow(clippy::too_many_arguments)] // public entry point called by the REST and MCP routes; a params struct would churn both callers
pub async fn command(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    kind: ApiCallerKind,
    petal_authorized: bool,
    petal_id: &str,
    package_id: &str,
    command_id: &str,
    args: Value,
) -> Result<Value, ExtError> {
    gate(host, claims)?;
    if !petal_authorized {
        return Err(ExtError::not_found());
    }
    if !args.is_object() {
        return Err(ExtError::invalid("args must be a JSON object"));
    }
    if args.to_string().len() > MAX_COMMAND_ARGS_BYTES {
        return Err(ExtError::invalid("args exceed 16 KiB"));
    }
    let caller = caller_of(claims, kind);
    let reply = host
        .call_command(&caller, petal_id, package_id, command_id, &args)
        .map_err(ExtError::from)?;
    match tokio::time::timeout(REPLY_TIMEOUT, reply).await {
        Ok(Ok(Ok(text))) => serde_json::from_str(&text)
            .map_err(|e| ExtError::new(502, "failed", format!("command result is not JSON: {e}"))),
        Ok(Ok(Err(failure))) => Err(failure.into()),
        Ok(Err(_)) => Err(ExtError::new(504, "unknown-outcome", "the instance stopped before answering")),
        Err(_) => Err(ExtError::new(504, "unknown-outcome", "the command did not answer in time")),
    }
}

/// Request parts the ingress mount hands over (before stripping).
#[derive(Debug, Clone)]
pub struct IngressRequestParts {
    pub petal_id: String,
    pub package_id: String,
    pub endpoint: String,
    pub method: String,
    pub sub_path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Mount a request on an extension's `http-server` endpoint. The HOST strips credential headers here,
/// whatever the manifest's `forward_headers` says, so a guest never sees the API token (steer 2026-11-08 20:20).
pub async fn ingress(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_authorized: bool,
    parts: IngressRequestParts,
) -> Result<IngressAnswer, ExtError> {
    gate(host, claims)?;
    admitted_ingress(host, claims, petal_authorized, parts).await
}

/// [`ingress`] after the rate gate: a handler (or the bound alias) that already gated calls this, so a request is charged once.
pub(crate) async fn admitted_ingress(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_authorized: bool,
    parts: IngressRequestParts,
) -> Result<IngressAnswer, ExtError> {
    if !petal_authorized {
        return Err(ExtError::not_found());
    }
    if parts.body.len() > MAX_INGRESS_BODY_BYTES {
        return Err(IngressRefusal::PayloadTooLarge.into());
    }
    let headers = parts
        .headers
        .into_iter()
        .filter(|(name, _)| !is_credential_header(name))
        .collect();
    let call = IngressCall {
        petal_id: parts.petal_id,
        package_id: parts.package_id,
        endpoint: parts.endpoint,
        caller_did: claims.sub.clone(),
        role_ceiling: fe_policy::RoleLevel::from(claims.max_role.as_str()),
        method: parts.method.to_ascii_uppercase(),
        sub_path: parts.sub_path,
        query: parts.query,
        headers,
        content_type: parts.content_type,
        body: parts.body,
    };
    let reply = host.ingress(call).map_err(ExtError::from)?;
    match tokio::time::timeout(REPLY_TIMEOUT, reply).await {
        Ok(Ok(Some(answer))) => Ok(answer),
        Ok(Ok(None)) => Err(ExtError::new(204, "no-reply", "the endpoint sent no reply")),
        Ok(Err(_)) => Err(ExtError::new(502, "failed", "the instance stopped before answering")),
        Err(_) => Err(ExtError::new(504, "unknown-outcome", "the endpoint did not answer in time")),
    }
}

/// Admit a websocket upgrade on an extension's accept endpoint: the same gate and petal scope as HTTP ingress,
/// and no request header (credentials included) ever reaches the host.
pub fn open_ingress_websocket(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_authorized: bool,
    petal_id: &str,
    package_id: &str,
    endpoint: &str,
) -> Result<WsHostSession, ExtError> {
    gate(host, claims)?;
    admitted_websocket(host, claims, petal_authorized, petal_id, package_id, endpoint)
}

/// [`open_ingress_websocket`] after the rate gate (the mount handler gates once per request).
fn admitted_websocket(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_authorized: bool,
    petal_id: &str,
    package_id: &str,
    endpoint: &str,
) -> Result<WsHostSession, ExtError> {
    if !petal_authorized {
        return Err(ExtError::not_found());
    }
    host.open_websocket(WsAcceptCall {
        petal_id: petal_id.to_string(),
        package_id: package_id.to_string(),
        endpoint: endpoint.to_string(),
        caller_did: claims.sub.clone(),
        role_ceiling: fe_policy::RoleLevel::from(claims.max_role.as_str()),
    })
    .map_err(ExtError::from)
}

/// Reports `websocket_closed` when dropped: the pump ending, or an admitted upgrade that never completes (the
/// connection would otherwise hold a `max_connections` slot and the guest would never hear `closed`).
struct ClosedOnDrop {
    host: Arc<dyn ExtensionHost>,
    connection: u64,
}

impl Drop for ClosedOnDrop {
    fn drop(&mut self) {
        self.host.websocket_closed(self.connection);
    }
}

/// Pump one accepted socket: client frames to the host, the host's outbound frames to the client, until either ends.
async fn run_ingress_socket(closed: ClosedOnDrop, mut socket: WebSocket, mut session: WsHostSession) {
    let (host, connection) = (closed.host.clone(), closed.connection);
    loop {
        tokio::select! {
            outbound = session.outbound.recv() => match outbound {
                Some(WsOut::Frame(WsData::Text(text))) => {
                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                }
                Some(WsOut::Frame(WsData::Binary(bytes))) => {
                    if socket.send(Message::Binary(bytes.into())).await.is_err() { break; }
                }
                Some(WsOut::Close { code, reason }) => {
                    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                    break;
                }
                None => break,
            },
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => host.websocket_frame(connection, WsData::Text(text.as_str().to_string())),
                Some(Ok(Message::Binary(bytes))) => host.websocket_frame(connection, WsData::Binary(bytes.to_vec())),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            },
        }
    }
    drop(closed);
}

/// MCP `extension_command {petal_id, package_id, command, args}`; the caller resolves `petal_authorized`.
pub async fn extension_command_tool(
    host: &dyn ExtensionHost,
    claims: &ApiClaims,
    petal_authorized: bool,
    arguments: &Value,
) -> Result<Value, ExtError> {
    let text = |key: &str| arguments.get(key).and_then(Value::as_str);
    let (Some(petal_id), Some(package_id), Some(command_id)) =
        (text("petal_id"), text("package_id"), text("command"))
    else {
        return Err(ExtError::invalid("petal_id, package_id and command are required"));
    };
    let args = arguments.get("args").cloned().unwrap_or_else(|| json!({}));
    command(host, claims, ApiCallerKind::Mcp, petal_authorized, petal_id, package_id, command_id, args).await
}

/// One `/ws` connection's extension-stream subscriptions; closes the host client on drop.
pub struct ExtensionStreams {
    host: Arc<dyn ExtensionHost>,
    claims: ApiClaims,
    client: u64,
    notify: Arc<Notify>,
    subscriptions: Vec<u64>,
}

impl ExtensionStreams {
    /// Open a client for this connection's token.
    pub fn open(host: Arc<dyn ExtensionHost>, claims: ApiClaims) -> Self {
        let (client, notify) = host.open_client(&caller_of(&claims, ApiCallerKind::Rest));
        Self { host, claims, client, notify, subscriptions: Vec::new() }
    }

    /// Wakes when events are queued; then call [`Self::drain`].
    pub fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// `bus_subscribe`: host streams are not reachable here, and an unauthorized petal is a uniform not-found.
    pub fn subscribe(
        &mut self,
        petal_authorized: bool,
        petal_id: &str,
        source: &str,
        params: &Value,
    ) -> Result<u64, ExtError> {
        gate(self.host.as_ref(), &self.claims)?;
        if !petal_authorized || source.starts_with("host/") {
            return Err(ExtError::not_found());
        }
        let subscription = self
            .host
            .client_subscribe(self.client, petal_id, source, params)
            .map_err(ExtError::from)?;
        self.subscriptions.push(subscription);
        Ok(subscription)
    }

    /// `bus_unsubscribe`; ignores ids this connection does not own.
    pub fn unsubscribe(&mut self, subscription: u64) {
        if let Some(index) = self.subscriptions.iter().position(|s| *s == subscription) {
            self.subscriptions.swap_remove(index);
            self.host.client_unsubscribe(self.client, subscription);
        }
    }

    /// Up to `max` queued `bus_event` envelopes (JSON texts).
    pub fn drain(&self, max: usize) -> Vec<String> {
        self.host.take_client_events(self.client, max)
    }
}

impl Drop for ExtensionStreams {
    fn drop(&mut self) {
        self.host.close_client(self.client);
    }
}

// ---------------------------------------------------------------------------
// REST
// ---------------------------------------------------------------------------

/// All extension routes, to merge into the authenticated router when the host provides a plugin host.
pub fn routes(host: Arc<dyn ExtensionHost>) -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/extensions", get(get_list).post(post_install))
        .route("/api/v1/extensions/disable-all", post(post_disable_all))
        .route("/api/v1/extensions/enable-all", post(post_enable_all))
        .route("/api/v1/extensions/kill-list-overrides", post(post_kill_list_override))
        .route(
            "/api/v1/verses/{verse_id}/extension-policy",
            get(get_policy_route).put(put_policy_route),
        )
        .route("/api/v1/extensions/{installation}", get(get_status).delete(delete_uninstall))
        .route("/api/v1/extensions/{installation}/audit", get(get_audit))
        .route("/api/v1/extensions/{installation}/{verb}", post(post_installation_verb))
        .route(
            "/api/v1/petals/{petal_id}/extensions/{package_id}/commands/{command}",
            post(post_command),
        )
        .route(
            "/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{endpoint}",
            any(any_ingress_root),
        )
        .route(
            "/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{endpoint}/{*sub_path}",
            any(any_ingress),
        )
        .layer(Extension(ExtensionHostHandle(host)))
}

/// `ApiResponse` envelope with a real status.
fn respond(result: Result<Value, ExtError>) -> Response {
    match result {
        Ok(data) => (StatusCode::OK, Json(ApiResponse::success(data))).into_response(),
        Err(e) => error_response(e),
    }
}

fn error_response(e: ExtError) -> Response {
    (e.status_code(), Json(ApiResponse::<Value>::error(e.message))).into_response()
}

/// Lenient JSON body: empty is `{}`, anything else must be an object.
fn parse_body(bytes: &[u8]) -> Result<Value, ExtError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) if value.is_object() => Ok(value),
        _ => Err(ExtError::invalid("body must be a JSON object")),
    }
}

async fn get_list(Extension(host): Extension<ExtensionHostHandle>, Extension(claims): Extension<ApiClaims>) -> Response {
    respond(list_extensions(host.0.as_ref(), &claims))
}

async fn get_status(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(installation): Path<String>,
) -> Response {
    respond(extension_status(host.0.as_ref(), &claims, &installation))
}

/// `?limit=` for the audit route.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn get_audit(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(installation): Path<String>,
    Query(query): Query<AuditQuery>,
) -> Response {
    respond(extension_audit(host.0.as_ref(), &claims, &installation, query.limit))
}

fn admin_response(
    host: &ExtensionHostHandle,
    claims: &ApiClaims,
    verb: AdminVerb,
    installation: Option<String>,
    body: &[u8],
) -> Response {
    // Refused whatever the body: a malformed body never answers differently from a well-formed one.
    let params = parse_body(body).unwrap_or_else(|_| json!({}));
    respond(administer(host.0.as_ref(), claims, AdminOp { verb, installation, params }))
}

async fn post_install(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    admin_response(&host, &claims, AdminVerb::Install, None, &body)
}

async fn post_disable_all(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    admin_response(&host, &claims, AdminVerb::DisableAll, None, &body)
}

async fn post_enable_all(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    admin_response(&host, &claims, AdminVerb::EnableAll, None, &body)
}

async fn post_kill_list_override(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    admin_response(&host, &claims, AdminVerb::KillListOverride, None, &body)
}

/// Whether the token's scope covers the verse (unknown and foreign collapse to false).
async fn verse_authorized(state: &ApiState, claims: &ApiClaims, verse_id: &str) -> bool {
    crate::rest::covered_scope(claims, crate::rest::resolve_verse_scope(state, verse_id).await).is_some()
}

async fn get_policy_route(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return error_response(e);
    }
    // Gated once above (a remote token is charged one rate unit per request, before any lookup).
    let authorized = verse_authorized(&state, &claims, &verse_id).await;
    respond(read_verse_policy(host.0.as_ref(), authorized, &verse_id).await)
}

async fn put_policy_route(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return error_response(e);
    }
    let policy = match parse_body(&body) {
        Ok(policy) => policy,
        Err(e) => return error_response(e),
    };
    let authorized = verse_authorized(&state, &claims, &verse_id).await;
    respond(write_verse_policy(host.0.as_ref(), &claims, authorized, &verse_id, policy).await)
}

async fn delete_uninstall(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path(installation): Path<String>,
) -> Response {
    admin_response(&host, &claims, AdminVerb::Uninstall, Some(installation), b"")
}

async fn post_installation_verb(
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path((installation, verb)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let Some(verb) = AdminVerb::parse_installation_verb(&verb) else {
        // An unknown verb is indistinguishable from an unknown route for a remote caller.
        return match gate(host.0.as_ref(), &claims) {
            Ok(()) => error_response(ExtError::not_found()),
            Err(e) => error_response(e),
        };
    };
    admin_response(&host, &claims, verb, Some(installation), &body)
}

/// `POST .../commands/{command}` body.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CommandRequest {
    #[serde(default = "empty_object")]
    pub args: Value,
}

fn empty_object() -> Value {
    json!({})
}

async fn post_command(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    Path((petal_id, package_id, command_id)): Path<(String, String, String)>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return error_response(e);
    }
    let args = match parse_body(&body) {
        Ok(value) => value.get("args").cloned().unwrap_or_else(empty_object),
        Err(e) => return error_response(e),
    };
    let authorized = authorized_petal_scope(&state, &claims, &petal_id).await.is_some();
    respond(
        command(
            host.0.as_ref(),
            &claims,
            ApiCallerKind::Rest,
            authorized,
            &petal_id,
            &package_id,
            &command_id,
            args,
        )
        .await,
    )
}

async fn any_ingress_root(
    state: State<Arc<ApiState>>,
    host: Extension<ExtensionHostHandle>,
    claims: Extension<ApiClaims>,
    Path((petal_id, package_id, endpoint)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    mount_ingress(state, host, claims, petal_id, package_id, endpoint, String::new(), request).await
}

async fn any_ingress(
    state: State<Arc<ApiState>>,
    host: Extension<ExtensionHostHandle>,
    claims: Extension<ApiClaims>,
    Path((petal_id, package_id, endpoint, sub_path)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    mount_ingress(state, host, claims, petal_id, package_id, endpoint, sub_path, request).await
}

#[allow(clippy::too_many_arguments)]
async fn mount_ingress(
    State(state): State<Arc<ApiState>>,
    Extension(host): Extension<ExtensionHostHandle>,
    Extension(claims): Extension<ApiClaims>,
    petal_id: String,
    package_id: String,
    endpoint: String,
    sub_path: String,
    request: Request,
) -> Response {
    if let Err(e) = gate(host.0.as_ref(), &claims) {
        return error_response(e);
    }
    let wants_upgrade = request
        .headers()
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if wants_upgrade {
        let authorized = authorized_petal_scope(&state, &claims, &petal_id).await.is_some();
        // Gated once above; the core below does not charge the remote rate again.
        let session = match admitted_websocket(host.0.as_ref(), &claims, authorized, &petal_id, &package_id, &endpoint) {
            Ok(session) => session,
            Err(e) => return error_response(e),
        };
        let closed = ClosedOnDrop { host: host.0.clone(), connection: session.connection };
        let upgrade = match WebSocketUpgrade::from_request(request, &()).await {
            Ok(upgrade) => upgrade,
            // `closed` drops here and reports the close.
            Err(rejection) => return rejection.into_response(),
        };
        // Both limits: tungstenite refuses an over-ceiling frame from its header, before buffering it (default 16 MiB).
        return upgrade
            .max_message_size(MAX_INGRESS_BODY_BYTES)
            .max_frame_size(MAX_INGRESS_BODY_BYTES)
            .on_upgrade(move |socket| run_ingress_socket(closed, socket, session));
    }
    let (head, body) = request.into_parts();
    let content_type = head
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let headers: Vec<(String, String)> = head
        .headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
        .collect();
    let query = head.uri.query().map(url_pairs).unwrap_or_default();
    let body = match read_limited(body).await {
        Ok(bytes) => bytes,
        Err(e) => return error_response(e),
    };
    let authorized = authorized_petal_scope(&state, &claims, &petal_id).await.is_some();
    let parts = IngressRequestParts {
        petal_id,
        package_id,
        endpoint,
        method: head.method.as_str().to_string(),
        sub_path,
        query,
        headers,
        content_type,
        body,
    };
    match admitted_ingress(host.0.as_ref(), &claims, authorized, parts).await {
        Ok(answer) => answer_response(answer),
        Err(e) if e.code == "no-reply" => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error_response(e),
    }
}

/// Read the body, refusing anything over the ingress ceiling (413).
async fn read_limited(body: Body) -> Result<Vec<u8>, ExtError> {
    match axum::body::to_bytes(body, MAX_INGRESS_BODY_BYTES + 1).await {
        Ok(bytes) if bytes.len() <= MAX_INGRESS_BODY_BYTES => Ok(bytes.to_vec()),
        Ok(_) => Err(IngressRefusal::PayloadTooLarge.into()),
        Err(_) => Err(IngressRefusal::PayloadTooLarge.into()),
    }
}

/// Split `a=b&c=d` (no percent-decoding beyond `+` and `%XX`).
fn url_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A guest's answer as an HTTP response; a status outside 200-599 is a 502. Guest bytes share the API
/// origin, so they are never sniffed or run as an active document.
fn answer_response(answer: IngressAnswer) -> Response {
    let status = StatusCode::from_u16(answer.status)
        .ok()
        .filter(|status| (200..=599).contains(&status.as_u16()))
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, answer.body).into_response();
    let headers = response.headers_mut();
    if let Some(content_type) = answer.content_type.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
        headers.insert(HeaderName::from_static("content-type"), content_type);
    }
    headers.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    headers.insert(HeaderName::from_static("content-security-policy"), HeaderValue::from_static("sandbox"));
    response
}
