//! Scoped extension bindings: record, point, mode, resolution states, doc-key codec, hook catalogue.
//! Contract: `docs/extensions/bindings.md` §3, §5.5, §6; notes in `fe-runtime/src/AGENTS.md` §extension-binding.

use fe_policy::{build_scope, parse_scope, ScopeParts};
use serde::{Deserialize, Serialize};

/// Current binding record schema version.
pub const BINDING_SCHEMA_VERSION: u32 = 1;
/// Doc-key prefix of a binding row.
pub const BINDING_DOC_PREFIX: &str = "binding";
/// Doc-key placeholder for a keyless hook.
pub const KEYLESS_PLACEHOLDER: &str = "_";
/// Canonical payload cap (bytes).
pub const MAX_BINDING_PAYLOAD_BYTES: usize = 32 * 1024;
/// `config` cap (bytes).
pub const MAX_BINDING_CONFIG_BYTES: usize = 16 * 1024;
/// Hook id length limit per side of the dot.
pub const MAX_HOOK_PART_LEN: usize = 32;
/// Key length limit.
pub const MAX_KEY_LEN: usize = 128;
/// Binding-started instance cap per installation per host (§5.6).
pub const MAX_BINDING_INSTANCES_PER_INSTALLATION: usize = 64;
/// Idle minutes before a lazy binding instance stops (§5.6, OQ-B9).
pub const LAZY_IDLE_STOP_MINUTES: u64 = 10;

/// How a scope's binding serves a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingMode {
    /// The named package serves the point.
    Extension,
    /// The built-in serves the point here and masks outer bindings.
    Builtin,
    /// Cleared: as if no row existed at this scope.
    Inherit,
}

/// The replicated binding row (§3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingRecord {
    pub schema_version: u32,
    pub scope: String,
    pub hook: String,
    #[serde(default)]
    pub key: String,
    pub mode: BindingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_req: Option<String>,
    #[serde(default = "empty_config")]
    pub config: serde_json::Value,
    #[serde(default)]
    pub runs_on: Option<String>,
    pub author_did: String,
    /// HLC as a decimal string (the wire range is u64).
    pub written_at: String,
}

fn empty_config() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// A hook point: the pair `(hook, key)`; `key` is `""` for keyless hooks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BindingPoint {
    pub hook: String,
    #[serde(default)]
    pub key: String,
}

impl BindingPoint {
    pub fn new(hook: impl Into<String>, key: impl Into<String>) -> Self {
        Self { hook: hook.into(), key: key.into() }
    }
}

impl BindingRecord {
    /// The point this row binds.
    pub fn point(&self) -> BindingPoint {
        BindingPoint::new(self.hook.clone(), self.key.clone())
    }

    /// The row's doc key, built from its own scope, point and author.
    pub fn doc_key(&self) -> Result<String, BindingCodecError> {
        let parts = parse_scope_string(&self.scope)?;
        encode_binding_doc_key(&parts, &self.point(), &self.author_did)
    }

    /// Shape checks that need no other state (§3 field rules); `config` content is not judged here.
    pub fn validate_shape(&self) -> Result<(), BindingCodecError> {
        if self.schema_version != BINDING_SCHEMA_VERSION {
            return Err(BindingCodecError::SchemaVersion(self.schema_version));
        }
        parse_scope_string(&self.scope)?;
        if !is_valid_hook_id(&self.hook) {
            return Err(BindingCodecError::HookId(self.hook.clone()));
        }
        if !is_valid_key(&self.key) {
            return Err(BindingCodecError::Key(self.key.clone()));
        }
        match self.mode {
            BindingMode::Extension => {
                if self.package_id.as_deref().is_none_or(str::is_empty) {
                    return Err(BindingCodecError::PackageRequired);
                }
            }
            BindingMode::Builtin | BindingMode::Inherit => {
                if self.package_id.is_some() || self.version_req.is_some() {
                    return Err(BindingCodecError::PackageForbidden);
                }
            }
        }
        if !self.config.is_object() {
            return Err(BindingCodecError::ConfigNotObject);
        }
        let config_len = serde_json::to_vec(&self.config).map_or(usize::MAX, |bytes| bytes.len());
        if config_len > MAX_BINDING_CONFIG_BYTES {
            return Err(BindingCodecError::ConfigTooLarge(config_len));
        }
        let payload_len = serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len());
        if payload_len > MAX_BINDING_PAYLOAD_BYTES {
            return Err(BindingCodecError::PayloadTooLarge(payload_len));
        }
        Ok(())
    }
}

/// Why a row, key or scope string was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingCodecError {
    SchemaVersion(u32),
    Scope(String),
    HookId(String),
    Key(String),
    Author(String),
    DocKey(String),
    PackageRequired,
    PackageForbidden,
    ConfigNotObject,
    ConfigTooLarge(usize),
    PayloadTooLarge(usize),
}

impl std::fmt::Display for BindingCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaVersion(v) => write!(f, "unsupported binding schema_version {v}"),
            Self::Scope(s) => write!(f, "invalid binding scope: {s}"),
            Self::HookId(s) => write!(f, "invalid hook id: {s}"),
            Self::Key(s) => write!(f, "invalid binding key: {s}"),
            Self::Author(s) => write!(f, "invalid binding author: {s}"),
            Self::DocKey(s) => write!(f, "malformed binding doc key: {s}"),
            Self::PackageRequired => write!(f, "mode extension requires package_id"),
            Self::PackageForbidden => write!(f, "package_id/version_req only valid for mode extension"),
            Self::ConfigNotObject => write!(f, "config must be a JSON object"),
            Self::ConfigTooLarge(n) => write!(f, "config is {n} bytes (cap {MAX_BINDING_CONFIG_BYTES})"),
            Self::PayloadTooLarge(n) => write!(f, "payload is {n} bytes (cap {MAX_BINDING_PAYLOAD_BYTES})"),
        }
    }
}

impl std::error::Error for BindingCodecError {}

/// Hook id grammar `^[a-z][a-z0-9-]{0,31}\.[a-z][a-z0-9-]{0,31}$`.
pub fn is_valid_hook_id(id: &str) -> bool {
    let Some((family, point)) = id.split_once('.') else {
        return false;
    };
    let part_ok = |part: &str| {
        let mut chars = part.chars();
        part.len() <= MAX_HOOK_PART_LEN
            && chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    part_ok(family) && part_ok(point)
}

/// Key grammar: `""` or `^[a-z0-9][a-z0-9._+@/-]{0,127}$`.
pub fn is_valid_key(key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    let mut chars = key.chars();
    key.len() <= MAX_KEY_LEN
        && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '@' | '/' | '-')
        })
}

fn parse_scope_string(scope: &str) -> Result<ScopeParts, BindingCodecError> {
    let parts = parse_scope(scope).map_err(|e| BindingCodecError::Scope(e.to_string()))?;
    // A '/' in an id would break the doc-key split; fe-policy ids are ULIDs, so refuse it outright.
    let ids = [Some(&parts.verse_id), parts.fractal_id.as_ref(), parts.petal_id.as_ref()];
    if ids.into_iter().flatten().any(|id| id.contains('/')) {
        return Err(BindingCodecError::Scope(scope.to_string()));
    }
    Ok(parts)
}

/// Build the scope string for parsed parts; `None` when a petal lacks its fractal.
pub fn scope_string(parts: &ScopeParts) -> Option<String> {
    if parts.petal_id.is_some() && parts.fractal_id.is_none() {
        return None;
    }
    Some(build_scope(&parts.verse_id, parts.fractal_id.as_deref(), parts.petal_id.as_deref()))
}

/// Scope level of parsed parts, nearest = deepest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingScopeLevel {
    Verse,
    Fractal,
    Petal,
}

impl BindingScopeLevel {
    pub fn of(parts: &ScopeParts) -> Self {
        if parts.petal_id.is_some() {
            Self::Petal
        } else if parts.fractal_id.is_some() {
            Self::Fractal
        } else {
            Self::Verse
        }
    }
}

/// Resolution chain for a petal, nearest first: petal, fractal, verse (§5.1).
pub fn resolution_scopes(verse_id: &str, fractal_id: &str, petal_id: &str) -> [String; 3] {
    [
        build_scope(verse_id, Some(fractal_id), Some(petal_id)),
        build_scope(verse_id, Some(fractal_id), None),
        build_scope(verse_id, None, None),
    ]
}

/// Percent-encode every byte outside `[a-z0-9._+-]`; the empty key writes `_`.
pub fn encode_key_component(key: &str) -> String {
    if key.is_empty() {
        return KEYLESS_PLACEHOLDER.to_string();
    }
    let mut out = String::with_capacity(key.len());
    for byte in key.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'+' | b'-') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Inverse of [`encode_key_component`]; `_` decodes to the keyless `""`.
pub fn decode_key_component(component: &str) -> Result<String, BindingCodecError> {
    if component == KEYLESS_PLACEHOLDER {
        return Ok(String::new());
    }
    let bytes = component.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = component
                .get(i + 1..i + 3)
                .ok_or_else(|| BindingCodecError::Key(component.to_string()))?;
            let value = u8::from_str_radix(hex, 16).map_err(|_| BindingCodecError::Key(component.to_string()))?;
            raw.push(value);
            i += 3;
        } else {
            raw.push(bytes[i]);
            i += 1;
        }
    }
    let key = String::from_utf8(raw).map_err(|_| BindingCodecError::Key(component.to_string()))?;
    if !is_valid_key(&key) {
        return Err(BindingCodecError::Key(key));
    }
    Ok(key)
}

/// `binding/{scope}/{hook}/{key}/{author_did}`.
pub fn encode_binding_doc_key(
    scope: &ScopeParts,
    point: &BindingPoint,
    author_did: &str,
) -> Result<String, BindingCodecError> {
    let scope_text = scope_string(scope).ok_or_else(|| BindingCodecError::Scope("petal without fractal".into()))?;
    if !is_valid_hook_id(&point.hook) {
        return Err(BindingCodecError::HookId(point.hook.clone()));
    }
    if !is_valid_key(&point.key) {
        return Err(BindingCodecError::Key(point.key.clone()));
    }
    if author_did.is_empty() || author_did.contains('/') {
        return Err(BindingCodecError::Author(author_did.to_string()));
    }
    Ok(format!(
        "{BINDING_DOC_PREFIX}/{scope_text}/{}/{}/{author_did}",
        point.hook,
        encode_key_component(&point.key)
    ))
}

/// Parse a binding doc key into `(scope, point, author_did)`.
pub fn decode_binding_doc_key(doc_key: &str) -> Result<(ScopeParts, BindingPoint, String), BindingCodecError> {
    let bad = || BindingCodecError::DocKey(doc_key.to_string());
    let segments: Vec<&str> = doc_key.split('/').collect();
    if segments.len() != 5 || segments[0] != BINDING_DOC_PREFIX {
        return Err(bad());
    }
    let scope = parse_scope_string(segments[1])?;
    if !is_valid_hook_id(segments[2]) {
        return Err(BindingCodecError::HookId(segments[2].to_string()));
    }
    let key = decode_key_component(segments[3])?;
    if segments[4].is_empty() {
        return Err(BindingCodecError::Author(String::new()));
    }
    Ok((scope, BindingPoint::new(segments[2], key), segments[4].to_string()))
}

/// Why a point does not run extension code on this peer (§5.2 row states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnresolvedReason {
    HostOff,
    UnknownHook,
    NotInstalled,
    VersionUnsatisfied,
    PackageInvalid,
    UntrustedSigner,
    EntitlementRequired,
    Revoked,
    PolicyDenied,
    NotProvided,
    InvalidConfig,
    RoleDenied,
    ConsentMissing,
    NotAccepted,
    Disabled,
    Degraded,
    Capacity,
}

impl UnresolvedReason {
    /// The §5.2 check order, first failure wins.
    pub const CHECK_ORDER: [UnresolvedReason; 17] = [
        Self::HostOff,
        Self::UnknownHook,
        Self::NotInstalled,
        Self::VersionUnsatisfied,
        Self::PackageInvalid,
        Self::UntrustedSigner,
        Self::EntitlementRequired,
        Self::Revoked,
        Self::PolicyDenied,
        Self::NotProvided,
        Self::InvalidConfig,
        Self::RoleDenied,
        Self::ConsentMissing,
        Self::NotAccepted,
        Self::Disabled,
        Self::Degraded,
        Self::Capacity,
    ];

    /// The single local-DID action offered with this state, if any.
    pub fn offer(self) -> Option<BindingOffer> {
        match self {
            Self::NotInstalled => Some(BindingOffer::Install),
            Self::VersionUnsatisfied | Self::NotProvided => Some(BindingOffer::Update),
            Self::Revoked => Some(BindingOffer::Override),
            Self::InvalidConfig => Some(BindingOffer::EditBinding),
            Self::ConsentMissing => Some(BindingOffer::ReviewPermissions),
            Self::NotAccepted => Some(BindingOffer::AllowInVerse),
            Self::Disabled => Some(BindingOffer::Enable),
            _ => None,
        }
    }
}

/// The one action a state shows; never executed without a local-DID action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingOffer {
    Install,
    Update,
    /// Verse Owner's own host only.
    Override,
    /// Manager+ only.
    EditBinding,
    ReviewPermissions,
    AllowInVerse,
    Enable,
}

/// One peer's answer for one point at one petal (§5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum BindingResolution {
    /// An instance of `package_id` serves the point.
    Resolved { package_id: String, scope: String },
    /// The built-in serves it (no binding, or an explicit `builtin` row at `scope`).
    Builtin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// Placement names another peer; built-in locally, no offer.
    Elsewhere { runs_on: String },
    /// Extension binding that cannot run here; the built-in serves meanwhile.
    Unresolved {
        package_id: String,
        scope: String,
        reason: UnresolvedReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offer: Option<BindingOffer>,
    },
}

impl BindingResolution {
    /// Build an unresolved answer with the reason's offer attached.
    pub fn unresolved(package_id: impl Into<String>, scope: impl Into<String>, reason: UnresolvedReason) -> Self {
        Self::Unresolved { package_id: package_id.into(), scope: scope.into(), reason, offer: reason.offer() }
    }

    /// True when extension code serves the point on this peer.
    pub fn is_extension(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }
}

/// Hook family (§6 Family column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookFamily {
    Lifecycle,
    MapTerrain,
    Api,
    SceneOps,
    Ingest,
    Ui,
    Egress,
    Files,
    Storage,
    Network,
    PortalWeb,
    Renderer,
}

/// What the key of a keyed hook names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookKeyKind {
    /// Keyless hook; the key is `""`.
    None,
    Name,
    DeviceFormatId,
    MimeType,
    PathPrefix,
}

/// How the triggering operation meets the guest call (§6 Call column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookCallKind {
    /// The triggering operation waits for the answer, up to the deadline.
    Waits,
    /// Computed off the frame and reused until an input changes.
    Cached,
    /// Runs after the triggering write is durable.
    AfterCommit,
    /// Runs after a peer fetch miss or failure.
    AfterMiss,
    /// Per child of a tileset download.
    PerChild,
    /// Switches on surfaces the instance already has; one `configure` call.
    Service,
}

/// Release tier of a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookTier {
    V1,
    /// Declared in the catalogue but unknown to hosts until built (`unknown-hook`).
    Stretch,
}

/// One row of the hook catalogue (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    pub id: &'static str,
    pub family: HookFamily,
    pub key_kind: HookKeyKind,
    pub call: HookCallKind,
    /// Wait deadline in ms; `None` for service hooks and calls without one.
    pub deadline_ms: Option<u32>,
    /// Largest body the host hands the guest, bytes; `None` when no body.
    pub body_cap_bytes: Option<u32>,
    /// Starts as soon as it resolves (else lazily, §5.6).
    pub eager: bool,
    /// `runs_on: null` reads as the author's DID (§5.4).
    pub one_peer: bool,
    pub tier: HookTier,
}

const KIB: u32 = 1024;
const MIB: u32 = 1024 * 1024;

/// The hook catalogue, one row per §6 hook including stretch rows.
pub const HOOK_CATALOGUE: &[HookSpec] = &[
    HookSpec { id: "petal.activate", family: HookFamily::Lifecycle, key_kind: HookKeyKind::Name, call: HookCallKind::Service, deadline_ms: None, body_cap_bytes: None, eager: true, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "petal.map", family: HookFamily::MapTerrain, key_kind: HookKeyKind::None, call: HookCallKind::Cached, deadline_ms: Some(5_000), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "api.routes", family: HookFamily::Api, key_kind: HookKeyKind::Name, call: HookCallKind::Waits, deadline_ms: Some(5_000), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "scene.create", family: HookFamily::SceneOps, key_kind: HookKeyKind::None, call: HookCallKind::Waits, deadline_ms: Some(250), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "scene.transform", family: HookFamily::SceneOps, key_kind: HookKeyKind::None, call: HookCallKind::Waits, deadline_ms: Some(250), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "scene.property", family: HookFamily::SceneOps, key_kind: HookKeyKind::None, call: HookCallKind::Waits, deadline_ms: Some(250), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "ingest.decode", family: HookFamily::Ingest, key_kind: HookKeyKind::DeviceFormatId, call: HookCallKind::AfterCommit, deadline_ms: Some(5_000), body_cap_bytes: Some(MIB), eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "ui.panel", family: HookFamily::Ui, key_kind: HookKeyKind::Name, call: HookCallKind::Service, deadline_ms: None, body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "egress.connector", family: HookFamily::Egress, key_kind: HookKeyKind::Name, call: HookCallKind::Service, deadline_ms: None, body_cap_bytes: None, eager: true, one_peer: true, tier: HookTier::V1 },
    HookSpec { id: "egress.snippet", family: HookFamily::Egress, key_kind: HookKeyKind::Name, call: HookCallKind::Cached, deadline_ms: Some(1_000), body_cap_bytes: Some(16 * KIB), eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "asset.import", family: HookFamily::Files, key_kind: HookKeyKind::MimeType, call: HookCallKind::Waits, deadline_ms: Some(30_000), body_cap_bytes: Some(16 * MIB), eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "blob.mirror", family: HookFamily::Storage, key_kind: HookKeyKind::Name, call: HookCallKind::AfterCommit, deadline_ms: None, body_cap_bytes: Some(8 * MIB), eager: false, one_peer: true, tier: HookTier::V1 },
    HookSpec { id: "blob.source", family: HookFamily::Storage, key_kind: HookKeyKind::Name, call: HookCallKind::AfterMiss, deadline_ms: Some(30_000), body_cap_bytes: None, eager: false, one_peer: true, tier: HookTier::V1 },
    HookSpec { id: "sync.tileset-source", family: HookFamily::Network, key_kind: HookKeyKind::Name, call: HookCallKind::PerChild, deadline_ms: Some(30_000), body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "portal.open", family: HookFamily::PortalWeb, key_kind: HookKeyKind::Name, call: HookCallKind::Service, deadline_ms: None, body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::V1 },
    HookSpec { id: "asset.export", family: HookFamily::Files, key_kind: HookKeyKind::Name, call: HookCallKind::Waits, deadline_ms: None, body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::Stretch },
    HookSpec { id: "render.asset-source", family: HookFamily::Renderer, key_kind: HookKeyKind::PathPrefix, call: HookCallKind::Cached, deadline_ms: None, body_cap_bytes: None, eager: false, one_peer: false, tier: HookTier::Stretch },
];

/// Look up a catalogue row by hook id.
pub fn hook_spec(id: &str) -> Option<&'static HookSpec> {
    HOOK_CATALOGUE.iter().find(|spec| spec.id == id)
}

/// Hook ids this build serves (v1 tier); stretch rows are `unknown-hook` until built.
pub fn served_hook_ids() -> impl Iterator<Item = &'static str> {
    HOOK_CATALOGUE.iter().filter(|spec| spec.tier == HookTier::V1).map(|spec| spec.id)
}

/// True when the hook's key kind is `None`, so any non-empty key is invalid for it.
pub fn key_allowed(spec: &HookSpec, key: &str) -> bool {
    (spec.key_kind != HookKeyKind::None || key.is_empty()) && is_valid_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(scope: &str, mode: BindingMode) -> BindingRecord {
        BindingRecord {
            schema_version: 1,
            scope: scope.into(),
            hook: "ingest.decode".into(),
            key: "acme/v2+raw".into(),
            mode,
            package_id: (mode == BindingMode::Extension).then(|| "org.example.decoder".to_string()),
            version_req: None,
            config: serde_json::json!({}),
            runs_on: None,
            author_did: "did:key:z6MkAuthor".into(),
            written_at: "1762631400000000123".into(),
        }
    }

    // Round-trips every scope level and a key needing percent-encoding and the keyless placeholder.
    #[test]
    fn doc_key_codec_round_trips_scopes_and_keys() {
        let scopes = ["VERSE#v-1", "VERSE#v-1-FRACTAL#f-2", "VERSE#v-1-FRACTAL#f-2-PETAL#p-3"];
        for scope in scopes {
            for key in ["", "acme/v2+raw", "a.b_c-d"] {
                let parts = parse_scope(scope).unwrap();
                let point = BindingPoint::new("ingest.decode", key);
                let doc_key = encode_binding_doc_key(&parts, &point, "did:key:z6MkAuthor").unwrap();
                assert_eq!(doc_key.split('/').count(), 5, "key must stay five segments: {doc_key}");
                let (back_scope, back_point, back_author) = decode_binding_doc_key(&doc_key).unwrap();
                assert_eq!(scope_string(&back_scope).as_deref(), Some(scope));
                assert_eq!(back_point, point);
                assert_eq!(back_author, "did:key:z6MkAuthor");
            }
        }
        assert!(encode_binding_doc_key(&parse_scope("VERSE#v").unwrap(), &BindingPoint::new("petal.map", ""), "d")
            .unwrap()
            .ends_with("/petal.map/_/d"));
    }

    // Table of malformed rows and keys, each of which must be refused.
    #[test]
    fn malformed_doc_keys_and_records_are_refused() {
        for bad in ["binding/VERSE#v/petal.map/_", "row/VERSE#v/petal.map/_/d", "binding/VERSE#v/Petal.Map/_/d", "binding/VERSE#v/petal.map/%ZZ/d", "binding/FRACTAL#f/petal.map/_/d"] {
            assert!(decode_binding_doc_key(bad).is_err(), "{bad}");
        }
        let mut record = sample("VERSE#v", BindingMode::Extension);
        assert_eq!(record.validate_shape(), Ok(()));
        record.package_id = None;
        assert_eq!(record.validate_shape(), Err(BindingCodecError::PackageRequired));
        let mut builtin = sample("VERSE#v", BindingMode::Builtin);
        builtin.package_id = Some("org.example.x".into());
        assert_eq!(builtin.validate_shape(), Err(BindingCodecError::PackageForbidden));
        let mut huge = sample("VERSE#v", BindingMode::Extension);
        huge.config = serde_json::json!({ "blob": "x".repeat(MAX_BINDING_CONFIG_BYTES) });
        assert!(matches!(huge.validate_shape(), Err(BindingCodecError::ConfigTooLarge(_))));
    }

    // Record JSON uses the documented lowercase mode and omits absent package fields.
    #[test]
    fn record_serde_matches_the_documented_shape() {
        let record = sample("VERSE#v-FRACTAL#f", BindingMode::Builtin);
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["mode"], "builtin");
        assert!(value.get("package_id").is_none());
        let back: BindingRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back, record);
        assert_eq!(record.doc_key().unwrap(), "binding/VERSE#v-FRACTAL#f/ingest.decode/acme%2Fv2+raw/did:key:z6MkAuthor");
    }

    // Catalogue ids pinned to manifest-v2.md's 2.0 list and to fe_sdk's KNOWN_HOOKS / KEYED_HOOKS (R-HOOKS-12).
    #[test]
    fn catalogue_matches_the_documented_hook_set() {
        let served: Vec<&str> = served_hook_ids().collect();
        assert_eq!(
            served,
            [
                "petal.activate", "petal.map", "api.routes", "scene.create", "scene.transform", "scene.property",
                "ingest.decode", "ui.panel", "egress.connector", "egress.snippet", "asset.import", "blob.mirror",
                "blob.source", "sync.tileset-source", "portal.open",
            ]
        );
        // A manifest may `provide` exactly the served hooks a package can serve; a web bundle binds `portal.open` bare.
        let providable: Vec<&str> = served.iter().copied().filter(|id| *id != "portal.open").collect();
        assert_eq!(providable, fe_sdk::manifest_v2::KNOWN_HOOKS, "catalogue and manifest v2 hook ids drifted");
        // Only a keyed catalogue row may narrow `keys` in `provides` (R-BND-30: `petal.activate` is keyed by name).
        let keyed: Vec<&str> = HOOK_CATALOGUE
            .iter()
            .filter(|spec| spec.tier == HookTier::V1 && spec.key_kind != HookKeyKind::None && spec.id != "portal.open")
            .map(|spec| spec.id)
            .collect();
        assert_eq!(keyed, fe_sdk::manifest_v2::KEYED_HOOKS, "catalogue key column and manifest v2 KEYED_HOOKS drifted");
        for spec in HOOK_CATALOGUE {
            assert!(is_valid_hook_id(spec.id), "{}", spec.id);
        }
        let mut ids: Vec<&str> = HOOK_CATALOGUE.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), HOOK_CATALOGUE.len());
        assert!(hook_spec("asset.export").is_some_and(|s| s.tier == HookTier::Stretch));
        assert!(key_allowed(hook_spec("petal.map").unwrap(), "") && !key_allowed(hook_spec("petal.map").unwrap(), "x"));
    }

    // Each §5.2 state offers at most the documented single action.
    #[test]
    fn unresolved_states_carry_their_single_offer() {
        let resolution = BindingResolution::unresolved("org.example.city-map", "VERSE#v", UnresolvedReason::NotInstalled);
        assert!(matches!(resolution, BindingResolution::Unresolved { offer: Some(BindingOffer::Install), .. }));
        assert_eq!(UnresolvedReason::Revoked.offer(), Some(BindingOffer::Override));
        assert_eq!(UnresolvedReason::PolicyDenied.offer(), None);
        assert_eq!(UnresolvedReason::CHECK_ORDER.len(), 17);
    }
}
