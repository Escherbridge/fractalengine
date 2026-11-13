//! Admin channel wire protocol v1: length-prefixed JSON frames (docs/extensions/admin-channel.md §3-§6).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Protocol version every frame carries in `v`.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame body in bytes (the 4-byte length prefix is extra).
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Longest request `id`.
pub const MAX_REQUEST_ID_BYTES: usize = 64;
/// Longest installation, binding, verse or source id.
pub const MAX_ID_BYTES: usize = 256;
/// Longest `install-file` path in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// The audit `channel` value of every action that arrives over this channel.
pub const AUDIT_CHANNEL: &str = "cli";

/// One request: `{"v":1,"id":"…","verb":"…","args":{…}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RequestWire", into = "RequestWire")]
pub struct AdminRequest {
    /// Protocol version; always [`PROTOCOL_VERSION`] once decoded.
    pub v: u32,
    /// Client correlation id, echoed in every reply frame.
    pub id: String,
    /// What to do.
    pub verb: AdminVerb,
}

/// The eleven verbs (admin-channel.md §4). Consent and local acceptance are deliberately absent: they are GUI-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", content = "args", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AdminVerb {
    /// Every installation on this host.
    List {},
    /// One installation.
    Status { installation_id: String },
    /// Verify and stage a package file; may raise the GUI consent dialog.
    InstallFile { path: String },
    /// Fetch, verify and stage a package from a configured registry source; may raise the GUI consent dialog.
    InstallRegistry {
        source_id: String,
        package_id: String,
        #[serde(default)]
        version: Option<String>,
    },
    /// Switch one binding on, or every binding of the installation when `binding_id` is absent.
    Enable {
        installation_id: String,
        #[serde(default)]
        binding_id: Option<String>,
    },
    /// Switch one binding off, or every binding of the installation when `binding_id` is absent.
    Disable {
        installation_id: String,
        #[serde(default)]
        binding_id: Option<String>,
    },
    /// Stage the newer version; a changed capability digest raises the GUI consent dialog.
    Update { installation_id: String },
    /// Withdraw this host's local acceptance of the installation in one verse.
    Unbind { installation_id: String, verse_id: String },
    /// Remove the installation.
    Uninstall { installation_id: String },
    /// The host-wide kill switch (governance §Host switch).
    DisableAll {},
    /// Clear the runtime kill switch.
    EnableAll {},
}

impl AdminVerb {
    /// The wire name, e.g. `install-file`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::List {} => "list",
            Self::Status { .. } => "status",
            Self::InstallFile { .. } => "install-file",
            Self::InstallRegistry { .. } => "install-registry",
            Self::Enable { .. } => "enable",
            Self::Disable { .. } => "disable",
            Self::Update { .. } => "update",
            Self::Unbind { .. } => "unbind",
            Self::Uninstall { .. } => "uninstall",
            Self::DisableAll {} => "disable-all",
            Self::EnableAll {} => "enable-all",
        }
    }

    /// The governance §Audit `action` this verb records; `None` for the read-only verbs.
    pub fn audit_action(&self) -> Option<&'static str> {
        match self {
            Self::List {} | Self::Status { .. } => None,
            Self::InstallFile { .. } | Self::InstallRegistry { .. } => Some("install"),
            Self::Enable { .. } | Self::EnableAll {} => Some("enable"),
            Self::Disable { .. } | Self::DisableAll {} => Some("disable"),
            Self::Update { .. } => Some("update"),
            Self::Unbind { .. } => Some("unbind"),
            Self::Uninstall { .. } => Some("uninstall"),
        }
    }

    /// Whether the host may answer `pending-consent` before the final reply.
    pub fn may_raise_consent(&self) -> bool {
        matches!(self, Self::InstallFile { .. } | Self::InstallRegistry { .. } | Self::Update { .. })
    }

    /// The installation the verb addresses, when it addresses one.
    pub fn installation_id(&self) -> Option<&str> {
        match self {
            Self::Status { installation_id }
            | Self::Enable { installation_id, .. }
            | Self::Disable { installation_id, .. }
            | Self::Update { installation_id }
            | Self::Unbind { installation_id, .. }
            | Self::Uninstall { installation_id } => Some(installation_id),
            _ => None,
        }
    }

    /// Argument checks beyond the shape (admin-channel.md §4 table); the message names the bad argument.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::List {} | Self::DisableAll {} | Self::EnableAll {} => Ok(()),
            Self::Status { installation_id }
            | Self::Update { installation_id }
            | Self::Uninstall { installation_id } => check_id("installation_id", installation_id),
            Self::Enable { installation_id, binding_id } | Self::Disable { installation_id, binding_id } => {
                check_id("installation_id", installation_id)?;
                binding_id.as_deref().map_or(Ok(()), |b| check_id("binding_id", b))
            }
            Self::Unbind { installation_id, verse_id } => {
                check_id("installation_id", installation_id)?;
                check_id("verse_id", verse_id)
            }
            Self::InstallFile { path } => check_path(path),
            Self::InstallRegistry { source_id, package_id, version } => {
                check_id("source_id", source_id)?;
                check_package_id(package_id)?;
                version.as_deref().map_or(Ok(()), check_version)
            }
        }
    }
}

impl AdminRequest {
    /// A v1 request.
    pub fn new(id: impl Into<String>, verb: AdminVerb) -> Self {
        Self { v: PROTOCOL_VERSION, id: id.into(), verb }
    }
}

/// The serde shape of [`AdminRequest`]; unknown top-level members (a token, an actor) are refused.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    v: u32,
    id: String,
    verb: String,
    #[serde(default)]
    args: Value,
}

impl TryFrom<RequestWire> for AdminRequest {
    type Error = String;

    fn try_from(wire: RequestWire) -> Result<Self, Self::Error> {
        let args = if wire.args.is_null() { json!({}) } else { wire.args };
        let verb: AdminVerb =
            serde_json::from_value(json!({ "verb": wire.verb, "args": args })).map_err(|error| error.to_string())?;
        Ok(Self { v: wire.v, id: wire.id, verb })
    }
}

impl From<AdminRequest> for RequestWire {
    fn from(request: AdminRequest) -> Self {
        let mut tagged = serde_json::to_value(&request.verb).unwrap_or(Value::Null);
        let args = tagged.get_mut("args").map(Value::take).unwrap_or_else(|| json!({}));
        Self { v: request.v, id: request.id, verb: request.verb.name().to_owned(), args }
    }
}

/// One reply frame: `{"v":1,"id":"…","status":"ok|pending-consent|refused",…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminReply {
    /// Protocol version.
    pub v: u32,
    /// The request's id; empty when the request was too malformed to carry one.
    pub id: String,
    /// What happened.
    #[serde(flatten)]
    pub outcome: ReplyOutcome,
}

/// The three reply kinds; `pending-consent` is interim and is followed by exactly one `ok` or `refused`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ReplyOutcome {
    /// Done; `result` lists the installations the verb touched (every one for `list` and the `*-all` verbs).
    Ok { result: AdminResult },
    /// The GUI consent dialog is open; the final reply follows once the operator answers it there.
    PendingConsent { consent: PendingConsent },
    /// Not done.
    Refused { code: RefusalCode, message: String },
}

impl AdminReply {
    /// A final success.
    pub fn ok(id: impl Into<String>, result: AdminResult) -> Self {
        Self { v: PROTOCOL_VERSION, id: id.into(), outcome: ReplyOutcome::Ok { result } }
    }

    /// The interim consent notice.
    pub fn pending_consent(id: impl Into<String>, consent: PendingConsent) -> Self {
        Self { v: PROTOCOL_VERSION, id: id.into(), outcome: ReplyOutcome::PendingConsent { consent } }
    }

    /// A final refusal.
    pub fn refused(id: impl Into<String>, code: RefusalCode, message: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            outcome: ReplyOutcome::Refused { code, message: message.into() },
        }
    }

    /// Whether this is the last frame for its request.
    pub fn is_final(&self) -> bool {
        !matches!(self.outcome, ReplyOutcome::PendingConsent { .. })
    }
}

/// Installations a verb touched, plus an optional human line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AdminResult {
    /// Affected installations after the verb ran.
    #[serde(default)]
    pub installations: Vec<InstallationSummary>,
    /// e.g. `Disabled 3 installations`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One installation as the CLI shows it (a projection of the Extensions panel row).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InstallationSummary {
    /// Local installation key.
    pub installation_id: String,
    /// Reverse-DNS package id.
    pub package_id: String,
    /// Display name.
    pub name: String,
    /// Running (or staged, before consent) version.
    pub version: String,
    /// The panel's status label, e.g. `Running`, `Needs consent`, `Blocked: revoked`.
    pub status: String,
    /// Newer version known from a registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<String>,
    /// Bindings of the installation.
    #[serde(default)]
    pub bindings: Vec<BindingSummary>,
}

/// One switchable binding.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BindingSummary {
    /// Id `enable` / `disable` take.
    pub binding_id: String,
    /// Human label.
    pub label: String,
    /// Whether it runs.
    pub enabled: bool,
}

/// What the GUI dialog is asking about, so the CLI can say which window to look at.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PendingConsent {
    /// Installation the consent will be recorded on.
    pub installation_id: String,
    /// Reverse-DNS package id.
    pub package_id: String,
    /// Display name.
    pub name: String,
    /// Version awaiting consent.
    pub version: String,
    /// Digest the dialog asks consent for.
    pub capability_digest: String,
    /// An update (the running version keeps running until consent).
    pub is_update: bool,
}

/// Stable refusal codes: the governance codes plus this channel's own (admin-channel.md §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum RefusalCode {
    HostOff,
    PackageInvalid,
    UntrustedSigner,
    Revoked,
    EntitlementRequired,
    PolicyDenied,
    RoleDenied,
    ConsentMissing,
    NotAvailable,
    NotFound,
    InvalidInput,
    BadRequest,
    UnsupportedVersion,
    FrameTooLarge,
    ReplyTooLarge,
    ConsentNeedsGui,
    ConsentDeclined,
    ConsentTimeout,
    Busy,
    Internal,
    /// A code this build does not know; the text is kept for display.
    Other(String),
}

impl RefusalCode {
    /// The wire text.
    pub fn as_str(&self) -> &str {
        match self {
            Self::HostOff => "host-off",
            Self::PackageInvalid => "package-invalid",
            Self::UntrustedSigner => "untrusted-signer",
            Self::Revoked => "revoked",
            Self::EntitlementRequired => "entitlement-required",
            Self::PolicyDenied => "policy-denied",
            Self::RoleDenied => "role-denied",
            Self::ConsentMissing => "consent-missing",
            Self::NotAvailable => "not-available",
            Self::NotFound => "not-found",
            Self::InvalidInput => "invalid-input",
            Self::BadRequest => "bad-request",
            Self::UnsupportedVersion => "unsupported-version",
            Self::FrameTooLarge => "frame-too-large",
            Self::ReplyTooLarge => "reply-too-large",
            Self::ConsentNeedsGui => "consent-needs-gui",
            Self::ConsentDeclined => "consent-declined",
            Self::ConsentTimeout => "consent-timeout",
            Self::Busy => "busy",
            Self::Internal => "internal",
            Self::Other(code) => code,
        }
    }
}

impl From<String> for RefusalCode {
    fn from(code: String) -> Self {
        match code.as_str() {
            "host-off" => Self::HostOff,
            "package-invalid" => Self::PackageInvalid,
            "untrusted-signer" => Self::UntrustedSigner,
            "revoked" => Self::Revoked,
            "entitlement-required" => Self::EntitlementRequired,
            "policy-denied" => Self::PolicyDenied,
            "role-denied" => Self::RoleDenied,
            "consent-missing" => Self::ConsentMissing,
            "not-available" => Self::NotAvailable,
            "not-found" => Self::NotFound,
            "invalid-input" => Self::InvalidInput,
            "bad-request" => Self::BadRequest,
            "unsupported-version" => Self::UnsupportedVersion,
            "frame-too-large" => Self::FrameTooLarge,
            "reply-too-large" => Self::ReplyTooLarge,
            "consent-needs-gui" => Self::ConsentNeedsGui,
            "consent-declined" => Self::ConsentDeclined,
            "consent-timeout" => Self::ConsentTimeout,
            "busy" => Self::Busy,
            "internal" => Self::Internal,
            _ => Self::Other(code),
        }
    }
}

impl From<RefusalCode> for String {
    fn from(code: RefusalCode) -> Self {
        code.as_str().to_owned()
    }
}

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The declared or encoded body exceeds [`MAX_FRAME_BYTES`]; nothing past the prefix was read.
    #[error("frame of {0} bytes exceeds the 65536-byte limit")]
    TooLarge(usize),
    /// A zero-length frame.
    #[error("empty frame")]
    Empty,
    /// The peer closed mid-frame.
    #[error("connection closed mid-frame")]
    Truncated,
    /// Transport failure.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Read one frame; `Ok(None)` is a clean close before any byte of a new frame. The cap is checked before allocating.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        let read = reader.read(&mut prefix[filled..]).await?;
        if read == 0 {
            return if filled == 0 { Ok(None) } else { Err(FrameError::Truncated) };
        }
        filled += read;
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 {
        return Err(FrameError::Empty);
    }
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.map_err(|error| match error.kind() {
        io::ErrorKind::UnexpectedEof => FrameError::Truncated,
        _ => FrameError::Io(error),
    })?;
    Ok(Some(body))
}

/// Write one frame (prefix and body in one write) and flush.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> Result<(), FrameError> {
    if body.is_empty() {
        return Err(FrameError::Empty);
    }
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Decode and validate one request body; a refusal carries the request id when one could be read.
pub fn decode_request(body: &[u8]) -> Result<AdminRequest, AdminReply> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| AdminReply::refused("", RefusalCode::BadRequest, "the request is not a JSON object"))?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_request_id(id))
        .unwrap_or_default()
        .to_owned();
    match value.get("v").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        Some(v) => {
            return Err(AdminReply::refused(
                id,
                RefusalCode::UnsupportedVersion,
                format!("this app speaks admin protocol v{PROTOCOL_VERSION}, the request is v{v}"),
            ))
        }
        None => return Err(AdminReply::refused(id, RefusalCode::BadRequest, "missing protocol version `v`")),
    }
    if id.is_empty() {
        return Err(AdminReply::refused(
            "",
            RefusalCode::BadRequest,
            format!("`id` must be 1-{MAX_REQUEST_ID_BYTES} characters of A-Z a-z 0-9 _ -"),
        ));
    }
    let request: AdminRequest =
        serde_json::from_value(value).map_err(|error| AdminReply::refused(&id, RefusalCode::BadRequest, error.to_string()))?;
    request
        .verb
        .validate()
        .map_err(|message| AdminReply::refused(&id, RefusalCode::InvalidInput, message))?;
    Ok(request)
}

/// Encode a request body (the client side).
pub fn encode_request(request: &AdminRequest) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(request).map_err(|error| FrameError::Io(io::Error::other(error)))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    Ok(body)
}

/// Encode a reply body; a reply over the cap becomes a `reply-too-large` refusal, so a reply is always sendable.
pub fn encode_reply(reply: &AdminReply) -> Vec<u8> {
    match serde_json::to_vec(reply) {
        Ok(body) if body.len() <= MAX_FRAME_BYTES => body,
        Ok(body) => fallback_reply(&reply.id, RefusalCode::ReplyTooLarge, &format!("the reply ({} bytes) exceeds the frame limit; ask `status` per installation", body.len())),
        Err(error) => fallback_reply(&reply.id, RefusalCode::Internal, &error.to_string()),
    }
}

/// Decode a reply body (the client side).
pub fn decode_reply(body: &[u8]) -> Result<AdminReply, serde_json::Error> {
    serde_json::from_slice(body)
}

fn fallback_reply(id: &str, code: RefusalCode, message: &str) -> Vec<u8> {
    serde_json::to_vec(&AdminReply::refused(id, code, message)).unwrap_or_else(|_| b"{}".to_vec())
}

/// `id`: 1-64 characters of `[A-Za-z0-9_-]`.
pub fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REQUEST_ID_BYTES
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn check_id(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("`{field}` must be 1-{MAX_ID_BYTES} bytes with no spaces or control characters"));
    }
    Ok(())
}

fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return Err(format!("`path` must be 1-{MAX_PATH_BYTES} bytes"));
    }
    if !Path::new(path).is_absolute() {
        return Err("`path` must be absolute (the app does not share the CLI's working directory)".into());
    }
    Ok(())
}

/// Store-client contract v1 package id: `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`.
fn check_package_id(package_id: &str) -> Result<(), String> {
    let mut bytes = package_id.bytes();
    let first_ok = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric());
    let rest_ok = bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if !first_ok || !rest_ok || package_id.len() > 128 || package_id.to_ascii_lowercase().ends_with(".json") {
        return Err("`package_id` must match ^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$ and not end in .json".into());
    }
    Ok(())
}

fn check_version(version: &str) -> Result<(), String> {
    match semver::Version::parse(version) {
        Ok(parsed) if parsed.build.is_empty() => Ok(()),
        _ => Err("`version` must be a SemVer version without build metadata".into()),
    }
}
