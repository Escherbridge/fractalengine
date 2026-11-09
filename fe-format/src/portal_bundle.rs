//! Portal web-bundle `portal` section v1: typed model, pure validator, capability digest and increase (docs/spec/portal-bundle/README.md §5-§7).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::limits::{
    MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_UNCOMPRESSED_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES,
    MAX_JSON_ENTRY_BYTES,
};

/// Schema tag every v1 `portal` section carries.
pub const PORTAL_SCHEMA_V1: &str = "fe-portal-bundle/1";
/// `asset.content_type` of a stored bundle (README §8.1, OD-5).
pub const PORTAL_BUNDLE_MIME: &str = "application/vnd.fractalengine.portal-bundle+hexon";
/// Manifest v2 `kind` discriminator for a web bundle.
pub const WEB_BUNDLE_KIND: &str = "web-bundle";
/// Manifest member holding the manifest v2 document.
pub const EXTENSION_MANIFEST_PATH: &str = "plugin/extension.json";
/// Default archive-size cap (OD-4); callers pass the per-node override.
pub const DEFAULT_MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

/// Header members a web bundle may carry besides `web/**` (README §1.3).
const HEADER_MEMBERS: &[&str] = &[
    "manifest.json",
    "entries.json",
    "members.json",
    "license.json",
    EXTENSION_MANIFEST_PATH,
];
/// Runtime-capability keys manifest v2 forbids on a web bundle (README §1.8).
const ENVELOPE_KEYS: &[&str] = &[
    "contract", "binding", "scene", "readings", "routes", "tick", "ingress", "egress", "bus", "ui",
];
const MAX_ORIGINS: usize = 16;
const MAX_QUERIES: usize = 16;
const MAX_QUERY_PARAMS: usize = 16;
const MAX_QUERY_SQL_BYTES: usize = 8192;
const MAX_KEY_PATTERNS: usize = 32;
const MAX_INVOKES: usize = 16;
const MAX_HOOKS: usize = 16;
const MAX_STREAMS: usize = 8;
const MAX_CONSUMERS: usize = 16;
const MAX_UI_INTENTS: usize = 2;
const MAX_TITLE_CHARS: usize = 120;
const MAX_MEMBER_PATH_BYTES: usize = 512;
const MAX_MEMBER_SEGMENTS: usize = 16;
const MAX_SEGMENT_BYTES: usize = 128;
const MAX_STREAM_RATE_HZ: u32 = 10;
const MAX_STREAM_EVENT_BYTES: u32 = 16_384;
const QUERY_PARAM_TYPES: &[&str] = &["string", "integer", "number", "boolean", "node_id"];
const UI_INTENTS: &[&str] = &["focus_node", "select_node"];

// ---------------------------------------------------------------------------
// Typed section
// ---------------------------------------------------------------------------

/// The `portal` section of `plugin/extension.json` (every key required except the two `commands` members).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalSection {
    pub schema: String,
    pub entry: String,
    pub title: String,
    pub csp: PortalCsp,
    pub navigation: Vec<String>,
    pub subscriptions: PortalSubscriptions,
    pub commands: PortalCommands,
    pub hooks: Vec<PortalHook>,
    pub streams: Vec<PortalStream>,
    pub ui_intents: Vec<String>,
}

/// CSP additions from the closed vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalCsp {
    pub connect: Vec<String>,
    pub img: Vec<String>,
    pub font: Vec<String>,
    pub media: Vec<String>,
    pub frame: Vec<String>,
    pub wasm: bool,
    pub inline_styles: bool,
}

/// Host data the page may subscribe to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalSubscriptions {
    pub node: bool,
    pub readings: bool,
    pub queries: Vec<PortalQuery>,
}

/// One named guarded query: SQL text plus parameter types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalQuery {
    pub name: String,
    pub sql: String,
    pub params: BTreeMap<String, String>,
}

/// Write and invoke grants.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PortalCommands {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_property: Option<PortalSetProperty>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invoke: Vec<PortalInvoke>,
}

/// Property-key patterns the page may write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalSetProperty {
    pub keys: Vec<String>,
}

/// One extension command the page may call through the bus.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PortalInvoke {
    pub package: String,
    pub command: String,
}

/// One foreign extension stream the page consumes (`<package_id>/<stream>`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PortalHook {
    pub source: String,
}

/// One stream the page produces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalStream {
    pub id: String,
    pub max_rate_hz: u32,
    pub max_event_bytes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Refusal classes in README §6 order; the first failing check wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalClass {
    TooLarge,
    ArchiveLimit,
    Integrity,
    ManifestMissing,
    ManifestMalformed,
    KindNotWebBundle,
    EnvelopeKeyForbidden,
    IdMismatch,
    SchemaUnsupported,
    VocabularyUnknown,
    LimitExceeded,
    OriginInvalid,
    OriginBlocked,
    QueryInvalid,
    KeyPatternInvalid,
    TargetInvalid,
    Duplicate,
    ComponentPresent,
    MemberReserved,
    MemberPathInvalid,
    MemberTypeUnknown,
    EntryMissing,
    EntryNotHtml,
}

impl RefusalClass {
    /// Stable wire name (`error` field of the 422 body).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::ArchiveLimit => "archive_limit",
            Self::Integrity => "integrity",
            Self::ManifestMissing => "manifest_missing",
            Self::ManifestMalformed => "manifest_malformed",
            Self::KindNotWebBundle => "kind_not_web_bundle",
            Self::EnvelopeKeyForbidden => "envelope_key_forbidden",
            Self::IdMismatch => "id_mismatch",
            Self::SchemaUnsupported => "schema_unsupported",
            Self::VocabularyUnknown => "vocabulary_unknown",
            Self::LimitExceeded => "limit_exceeded",
            Self::OriginInvalid => "origin_invalid",
            Self::OriginBlocked => "origin_blocked",
            Self::QueryInvalid => "query_invalid",
            Self::KeyPatternInvalid => "key_pattern_invalid",
            Self::TargetInvalid => "target_invalid",
            Self::Duplicate => "duplicate",
            Self::ComponentPresent => "component_present",
            Self::MemberReserved => "member_reserved",
            Self::MemberPathInvalid => "member_path_invalid",
            Self::MemberTypeUnknown => "member_type_unknown",
            Self::EntryMissing => "entry_missing",
            Self::EntryNotHtml => "entry_not_html",
        }
    }

    /// Ingest HTTP status: 413 for classes 1-2, 422 otherwise.
    pub fn http_status(self) -> u16 {
        match self {
            Self::TooLarge | Self::ArchiveLimit => 413,
            _ => 422,
        }
    }

    /// Chrome reason shown for a blocked bundle (admission.md §2).
    pub fn chrome_reason(self) -> &'static str {
        match self {
            Self::TooLarge | Self::ArchiveLimit => "too large",
            Self::Integrity => "integrity failure",
            _ => "malformed manifest",
        }
    }
}

impl std::fmt::Display for RefusalClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refused bundle: its class plus a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{class}: {detail}")]
pub struct Refusal {
    pub class: RefusalClass,
    pub detail: String,
}

fn refuse<T>(class: RefusalClass, detail: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal {
        class,
        detail: detail.into(),
    })
}

// ---------------------------------------------------------------------------
// Validator
// ---------------------------------------------------------------------------

/// One archive member as the central directory reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleMember {
    pub path: String,
    pub uncompressed_size: u64,
}

/// Header-phase facts about an archive; the validator never touches body bytes.
#[derive(Debug, Clone)]
pub struct BundleFacts<'a> {
    /// Compressed archive length.
    pub archive_bytes: u64,
    /// Archive-size cap (`FE_PORTAL_BUNDLE_MAX_BYTES` or [`DEFAULT_MAX_BUNDLE_BYTES`]).
    pub max_bytes: u64,
    /// Every central-directory entry.
    pub members: &'a [BundleMember],
    /// `Some(reason)` when the seal failed to verify (any `IntegrityError`).
    pub integrity_failure: Option<String>,
    /// Bytes of `plugin/extension.json`, if the archive has the member.
    pub extension_json: Option<&'a [u8]>,
    /// Hexon manifest `hexon_id`.
    pub hexon_id: &'a str,
    /// Hexon manifest `version`.
    pub hexon_version: &'a str,
}

/// A bundle that passed every check.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedBundle {
    pub package_id: String,
    pub version: String,
    pub portal: PortalSection,
    /// Lowercase-hex BLAKE3 of the JCS capability set (README §7.2).
    pub capability_digest: String,
    /// Each `web/` member with its served Content-Type.
    pub web_members: Vec<(String, &'static str)>,
}

/// Validate a bundle's header facts; `origin_allowed` is `fe_runtime::portal_url::is_url_allowed` on the parsed origin (injected so fe-format stays bevy-free).
pub fn validate_portal_bundle(
    facts: &BundleFacts<'_>,
    origin_allowed: &dyn Fn(&str) -> bool,
) -> Result<ValidatedBundle, Refusal> {
    check_sizes(facts)?;
    if let Some(reason) = &facts.integrity_failure {
        return refuse(RefusalClass::Integrity, reason.clone());
    }
    let Some(raw) = facts.extension_json else {
        return refuse(RefusalClass::ManifestMissing, "no plugin/extension.json");
    };
    let ext: Value = serde_json::from_slice(raw).map_err(|e| Refusal {
        class: RefusalClass::ManifestMalformed,
        detail: format!("extension.json is not JSON: {e}"),
    })?;
    let Some(ext_obj) = ext.as_object() else {
        return refuse(RefusalClass::ManifestMalformed, "extension.json is not an object");
    };
    let (Some(kind), Some(id), Some(version)) = (
        ext_obj.get("kind").and_then(Value::as_str),
        ext_obj.get("id").and_then(Value::as_str),
        ext_obj.get("version").and_then(Value::as_str),
    ) else {
        return refuse(RefusalClass::ManifestMalformed, "kind, id and version must be strings");
    };
    if kind != WEB_BUNDLE_KIND {
        return refuse(RefusalClass::KindNotWebBundle, format!("kind is {kind:?}"));
    }
    let portal_value = ext_obj.get("portal").cloned().unwrap_or(Value::Null);
    let portal: PortalSection = serde_json::from_value(portal_value.clone()).map_err(|e| Refusal {
        class: RefusalClass::ManifestMalformed,
        detail: format!("portal section: {e}"),
    })?;
    if let Some(key) = ENVELOPE_KEYS.iter().find(|k| ext_obj.contains_key(**k)) {
        return refuse(RefusalClass::EnvelopeKeyForbidden, format!("{key} is forbidden on a web bundle"));
    }
    if id != facts.hexon_id || version != facts.hexon_version {
        return refuse(
            RefusalClass::IdMismatch,
            format!("extension {id}@{version} vs hexon {}@{}", facts.hexon_id, facts.hexon_version),
        );
    }
    if portal.schema != PORTAL_SCHEMA_V1 {
        return refuse(RefusalClass::SchemaUnsupported, format!("schema {:?}", portal.schema));
    }
    check_vocabulary(&portal_value, &portal)?;
    check_limits(&portal)?;
    check_origins(&portal, origin_allowed)?;
    check_queries(&portal.subscriptions.queries)?;
    check_key_patterns(&portal.commands)?;
    check_targets(&portal)?;
    check_duplicates(&portal)?;
    let web_members = check_members(facts.members, &portal.entry)?;
    let capability_digest = capability_digest(&portal);
    Ok(ValidatedBundle {
        package_id: id.to_owned(),
        version: version.to_owned(),
        portal,
        capability_digest,
        web_members,
    })
}

/// Classes 1-2: archive-size cap and fe-format archive limits.
fn check_sizes(facts: &BundleFacts<'_>) -> Result<(), Refusal> {
    let declared_total: u64 = facts.members.iter().map(|m| m.uncompressed_size).sum();
    if facts.archive_bytes > facts.max_bytes {
        return refuse(RefusalClass::TooLarge, format!("{} bytes over cap {}", facts.archive_bytes, facts.max_bytes));
    }
    if declared_total > facts.max_bytes.saturating_mul(8) {
        return refuse(RefusalClass::TooLarge, format!("{declared_total} declared bytes over 8x cap"));
    }
    if facts.members.len() > MAX_ARCHIVE_ENTRIES {
        return refuse(RefusalClass::ArchiveLimit, format!("{} entries", facts.members.len()));
    }
    if declared_total > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
        return refuse(RefusalClass::ArchiveLimit, "archive over 1 GiB uncompressed");
    }
    for m in facts.members {
        if m.uncompressed_size > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return refuse(RefusalClass::ArchiveLimit, format!("{} over 64 MiB", m.path));
        }
        if m.path.to_ascii_lowercase().ends_with(".json") && m.uncompressed_size > MAX_JSON_ENTRY_BYTES {
            return refuse(RefusalClass::ArchiveLimit, format!("{} is a .json member over 8 MiB", m.path));
        }
    }
    Ok(())
}

/// Class 10: closed vocabulary at every level of `portal`.
fn check_vocabulary(value: &Value, portal: &PortalSection) -> Result<(), Refusal> {
    fn only(obj: &Value, allowed: &[&str], at: &str) -> Result<(), Refusal> {
        if let Some(map) = obj.as_object() {
            if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
                return refuse(RefusalClass::VocabularyUnknown, format!("unknown key {at}.{key}"));
            }
        }
        Ok(())
    }
    only(
        value,
        &["schema", "entry", "title", "csp", "navigation", "subscriptions", "commands", "hooks", "streams", "ui_intents"],
        "portal",
    )?;
    only(&value["csp"], &["connect", "img", "font", "media", "frame", "wasm", "inline_styles"], "portal.csp")?;
    only(&value["subscriptions"], &["node", "readings", "queries"], "portal.subscriptions")?;
    for q in value["subscriptions"]["queries"].as_array().into_iter().flatten() {
        only(q, &["name", "sql", "params"], "portal.subscriptions.queries[]")?;
    }
    only(&value["commands"], &["set_property", "invoke"], "portal.commands")?;
    only(&value["commands"]["set_property"], &["keys"], "portal.commands.set_property")?;
    for i in value["commands"]["invoke"].as_array().into_iter().flatten() {
        only(i, &["package", "command"], "portal.commands.invoke[]")?;
    }
    for h in value["hooks"].as_array().into_iter().flatten() {
        only(h, &["source"], "portal.hooks[]")?;
    }
    for s in value["streams"].as_array().into_iter().flatten() {
        only(s, &["id", "max_rate_hz", "max_event_bytes", "consumers"], "portal.streams[]")?;
    }
    if let Some(intent) = portal.ui_intents.iter().find(|i| !UI_INTENTS.contains(&i.as_str())) {
        return refuse(RefusalClass::VocabularyUnknown, format!("unknown ui_intent {intent:?}"));
    }
    Ok(())
}

/// Class 11: every list and numeric bound of README §5.
fn check_limits(p: &PortalSection) -> Result<(), Refusal> {
    let title_chars = p.title.chars().count();
    if title_chars == 0 || title_chars > MAX_TITLE_CHARS {
        return refuse(RefusalClass::LimitExceeded, "title must be 1-120 characters");
    }
    let origin_lists: [(&str, &Vec<String>); 6] = [
        ("csp.connect", &p.csp.connect),
        ("csp.img", &p.csp.img),
        ("csp.font", &p.csp.font),
        ("csp.media", &p.csp.media),
        ("csp.frame", &p.csp.frame),
        ("navigation", &p.navigation),
    ];
    for (name, list) in origin_lists {
        if list.len() > MAX_ORIGINS {
            return refuse(RefusalClass::LimitExceeded, format!("{name} has {} entries", list.len()));
        }
    }
    let bounds: [(&str, usize, usize); 6] = [
        ("subscriptions.queries", p.subscriptions.queries.len(), MAX_QUERIES),
        ("commands.invoke", p.commands.invoke.len(), MAX_INVOKES),
        ("hooks", p.hooks.len(), MAX_HOOKS),
        ("streams", p.streams.len(), MAX_STREAMS),
        ("ui_intents", p.ui_intents.len(), MAX_UI_INTENTS),
        (
            "commands.set_property.keys",
            p.commands.set_property.as_ref().map_or(0, |s| s.keys.len()),
            MAX_KEY_PATTERNS,
        ),
    ];
    for (name, len, max) in bounds {
        if len > max {
            return refuse(RefusalClass::LimitExceeded, format!("{name} has {len} entries (max {max})"));
        }
    }
    for q in &p.subscriptions.queries {
        if q.params.len() > MAX_QUERY_PARAMS {
            return refuse(RefusalClass::LimitExceeded, format!("query {} has {} params", q.name, q.params.len()));
        }
    }
    for s in &p.streams {
        if !(1..=MAX_STREAM_RATE_HZ).contains(&s.max_rate_hz)
            || !(1..=MAX_STREAM_EVENT_BYTES).contains(&s.max_event_bytes)
        {
            return refuse(RefusalClass::LimitExceeded, format!("stream {} rate or event size out of range", s.id));
        }
        if let Some(c) = &s.consumers {
            if c.is_empty() || c.len() > MAX_CONSUMERS {
                return refuse(RefusalClass::LimitExceeded, format!("stream {} consumers 1-16", s.id));
            }
        }
    }
    Ok(())
}

/// Classes 12-13: exact https origins that the host's URL policy allows.
fn check_origins(p: &PortalSection, origin_allowed: &dyn Fn(&str) -> bool) -> Result<(), Refusal> {
    let all = [&p.csp.connect, &p.csp.img, &p.csp.font, &p.csp.media, &p.csp.frame, &p.navigation];
    for list in all {
        for origin in list {
            if !is_exact_https_origin(origin) {
                return refuse(RefusalClass::OriginInvalid, format!("{origin:?} is not an exact https origin"));
            }
        }
    }
    for list in all {
        for origin in list {
            if !origin_allowed(origin) {
                return refuse(RefusalClass::OriginBlocked, format!("{origin} is blocked by URL policy"));
            }
        }
    }
    Ok(())
}

/// `https://host[:port]`: lowercase DNS labels (at least two), no path, query, fragment, userinfo or wildcard.
fn is_exact_https_origin(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("https://") else { return false };
    if s.len() > 255 {
        return false;
    }
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if let Some(p) = port {
        let digits_ok = !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit());
        if !digits_ok || p.parse::<u32>().map_or(true, |n| n == 0 || n > 65_535) {
            return false;
        }
    }
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            let b = l.as_bytes();
            !b.is_empty()
                && b.len() <= 63
                && b[0] != b'-'
                && b[b.len() - 1] != b'-'
                && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        })
}

/// Class 14: query name, SQL shape and parameter grammar.
fn check_queries(queries: &[PortalQuery]) -> Result<(), Refusal> {
    for q in queries {
        if !is_ident(&q.name, 48, "") {
            return refuse(RefusalClass::QueryInvalid, format!("query name {:?}", q.name));
        }
        let sql = q.sql.trim_start();
        let select_then_space = sql.len() > 6
            && sql.is_char_boundary(6)
            && sql[..6].eq_ignore_ascii_case("select")
            && sql[6..].starts_with(|c: char| c.is_whitespace());
        if q.sql.len() < 6 || q.sql.len() > MAX_QUERY_SQL_BYTES || !select_then_space || q.sql.contains(';') {
            return refuse(RefusalClass::QueryInvalid, format!("query {} must be one SELECT of at most 8 KiB with no ';'", q.name));
        }
        for (param, ty) in &q.params {
            if !is_ident(param, 32, "") || param.starts_with("fe_") || !QUERY_PARAM_TYPES.contains(&ty.as_str()) {
                return refuse(RefusalClass::QueryInvalid, format!("query {} param {param:?}: {ty:?}", q.name));
            }
        }
    }
    Ok(())
}

/// `[a-z][a-z0-9_<extra>]{0,max-1}`.
fn is_ident(s: &str, max: usize, extra: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || extra.as_bytes().contains(c))
}

/// Package id grammar: `[a-z0-9][a-z0-9._-]{0,127}`.
fn is_package_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'.' | b'_' | b'-'))
}

/// Local id grammar: `[a-z][a-z0-9_-]{0,47}`.
fn is_local_id(s: &str) -> bool {
    is_ident(s, 48, "-")
}

/// Class 15: key-pattern grammar plus the reserved `node_kind` / `portal.*` keys (README §5.1).
fn check_key_patterns(commands: &PortalCommands) -> Result<(), Refusal> {
    let Some(set) = &commands.set_property else { return Ok(()) };
    if set.keys.is_empty() {
        return refuse(RefusalClass::KeyPatternInvalid, "set_property.keys is empty");
    }
    for key in &set.keys {
        let (base, _) = split_pattern(key);
        let segments_ok = !base.is_empty()
            && key.len() <= 128
            && base.split('.').all(|seg| {
                !seg.is_empty() && seg.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            });
        if !segments_ok {
            return refuse(RefusalClass::KeyPatternInvalid, format!("key pattern {key:?}"));
        }
        let first = base.split('.').next().unwrap_or_default();
        if base == "node_kind" || first == "portal" {
            return refuse(RefusalClass::KeyPatternInvalid, format!("{key:?} covers a host-reserved key"));
        }
    }
    Ok(())
}

/// `(base, is_wildcard)` for `a.b` or `a.b.*`.
fn split_pattern(key: &str) -> (&str, bool) {
    match key.strip_suffix(".*") {
        Some(base) => (base, true),
        None => (key, false),
    }
}

/// Class 16: invoke, hook and stream-id grammar.
fn check_targets(p: &PortalSection) -> Result<(), Refusal> {
    for i in &p.commands.invoke {
        if !is_package_id(&i.package) || !is_local_id(&i.command) {
            return refuse(RefusalClass::TargetInvalid, format!("invoke {}/{}", i.package, i.command));
        }
    }
    for h in &p.hooks {
        let ok = h
            .source
            .split_once('/')
            .is_some_and(|(pkg, stream)| pkg != "host" && is_package_id(pkg) && is_local_id(stream));
        if !ok {
            return refuse(RefusalClass::TargetInvalid, format!("hook source {:?}", h.source));
        }
    }
    for s in &p.streams {
        if !is_local_id(&s.id) {
            return refuse(RefusalClass::TargetInvalid, format!("stream id {:?}", s.id));
        }
        if let Some(c) = &s.consumers {
            if let Some(bad) = c.iter().find(|pkg| !is_package_id(pkg)) {
                return refuse(RefusalClass::TargetInvalid, format!("stream consumer {bad:?}"));
            }
        }
    }
    Ok(())
}

/// Class 17: no duplicate origins, query names, key patterns, invoke targets, hook sources, stream ids, consumers or intents.
fn check_duplicates(p: &PortalSection) -> Result<(), Refusal> {
    fn unique<'a>(what: &str, items: impl Iterator<Item = &'a str>) -> Result<(), Refusal> {
        let mut seen = BTreeSet::new();
        for item in items {
            if !seen.insert(item) {
                return refuse(RefusalClass::Duplicate, format!("duplicate {what} {item:?}"));
            }
        }
        Ok(())
    }
    let origin_lists: [(&str, &Vec<String>); 6] = [
        ("csp.connect", &p.csp.connect),
        ("csp.img", &p.csp.img),
        ("csp.font", &p.csp.font),
        ("csp.media", &p.csp.media),
        ("csp.frame", &p.csp.frame),
        ("navigation", &p.navigation),
    ];
    for (name, list) in origin_lists {
        unique(name, list.iter().map(String::as_str))?;
    }
    unique("query name", p.subscriptions.queries.iter().map(|q| q.name.as_str()))?;
    if let Some(set) = &p.commands.set_property {
        unique("key pattern", set.keys.iter().map(String::as_str))?;
    }
    let invokes: Vec<String> = p.commands.invoke.iter().map(|i| format!("{}/{}", i.package, i.command)).collect();
    unique("invoke target", invokes.iter().map(String::as_str))?;
    unique("hook source", p.hooks.iter().map(|h| h.source.as_str()))?;
    unique("stream id", p.streams.iter().map(|s| s.id.as_str()))?;
    for s in &p.streams {
        if let Some(c) = &s.consumers {
            unique("stream consumer", c.iter().map(String::as_str))?;
        }
    }
    unique("ui_intent", p.ui_intents.iter().map(String::as_str))
}

/// Classes 18-23: member directory checks, one pass per class in README order.
fn check_members(members: &[BundleMember], entry: &str) -> Result<Vec<(String, &'static str)>, Refusal> {
    let is_header = |p: &str| HEADER_MEMBERS.contains(&p) || p.ends_with('/');
    if members.iter().any(|m| m.path == "plugin/component.wasm") {
        return refuse(RefusalClass::ComponentPresent, "plugin/component.wasm in a web bundle");
    }
    for m in members.iter().filter(|m| !is_header(&m.path)) {
        if !m.path.starts_with("web/") || m.path == "web/__fe" || m.path.starts_with("web/__fe/") {
            return refuse(RefusalClass::MemberReserved, format!("member {:?} is outside web/ or reserved", m.path));
        }
    }
    let body: Vec<&BundleMember> = members.iter().filter(|m| !is_header(&m.path)).collect();
    for m in &body {
        if !is_member_path(&m.path) {
            return refuse(RefusalClass::MemberPathInvalid, format!("member path {:?}", m.path));
        }
    }
    let mut typed = Vec::with_capacity(body.len());
    for m in &body {
        match mime_for_path(&m.path) {
            Some(mime) => typed.push((m.path.clone(), mime)),
            None => return refuse(RefusalClass::MemberTypeUnknown, format!("member {:?} has no known type", m.path)),
        }
    }
    if !body.iter().any(|m| m.path == entry) {
        return refuse(RefusalClass::EntryMissing, format!("entry {entry:?} is not a member"));
    }
    if !matches!(extension_of(entry).as_deref(), Some("html" | "htm")) {
        return refuse(RefusalClass::EntryNotHtml, format!("entry {entry:?} is not .html/.htm"));
    }
    Ok(typed)
}

/// README §3.1: `web/` plus 1-16 unreserved segments, none dot-leading, at most 512 bytes.
pub fn is_member_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("web/") else { return false };
    if path.len() > MAX_MEMBER_PATH_BYTES {
        return false;
    }
    let segments: Vec<&str> = rest.split('/').collect();
    segments.len() <= MAX_MEMBER_SEGMENTS
        && segments.iter().all(|seg| {
            let b = seg.as_bytes();
            let unreserved = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'~' | b'-');
            !b.is_empty() && b.len() <= MAX_SEGMENT_BYTES && unreserved(b[0]) && b.iter().all(|c| unreserved(*c) || *c == b'.')
        })
}

fn extension_of(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

/// Closed MIME table (README §4.1); `None` means `member_type_unknown`.
pub fn mime_for_path(path: &str) -> Option<&'static str> {
    Some(match extension_of(path)?.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "geojson" => "application/geo+json",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "glb" => "model/gltf-binary",
        "bin" => "application/octet-stream",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Capability digest and increase
// ---------------------------------------------------------------------------

/// BLAKE3 (lowercase hex) over the JCS canonical JSON of the section minus `schema`, `entry`, `title` (README §7.1-7.2).
pub fn capability_digest(portal: &PortalSection) -> String {
    let mut value = serde_json::to_value(portal).expect("PortalSection serializes");
    if let Some(map) = value.as_object_mut() {
        for key in ["schema", "entry", "title"] {
            map.remove(key);
        }
    }
    let mut canonical = String::new();
    write_jcs(&value, &mut canonical);
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

/// RFC 8785 writer: sorted keys, no whitespace; the section holds only strings, booleans and unsigned integers.
fn write_jcs(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s).expect("string serializes")),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_jcs(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            // JCS sorts by UTF-16 code units; keys here are ASCII so byte order is identical.
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("key serializes"));
                out.push(':');
                write_jcs(item, out);
            }
            out.push('}');
        }
    }
}

/// True when build `y` grants anything build `x` did not (README §7.3).
pub fn capability_increase(x: &PortalSection, y: &PortalSection) -> bool {
    let subset = |new: &[String], old: &[String]| new.iter().all(|v| old.contains(v));
    let origin_pairs = [
        (&y.csp.connect, &x.csp.connect),
        (&y.csp.img, &x.csp.img),
        (&y.csp.font, &x.csp.font),
        (&y.csp.media, &x.csp.media),
        (&y.csp.frame, &x.csp.frame),
        (&y.navigation, &x.navigation),
    ];
    if origin_pairs.iter().any(|(new, old)| !subset(new, old)) {
        return true;
    }
    if (y.csp.wasm && !x.csp.wasm)
        || (y.csp.inline_styles && !x.csp.inline_styles)
        || (y.subscriptions.node && !x.subscriptions.node)
        || (y.subscriptions.readings && !x.subscriptions.readings)
    {
        return true;
    }
    // A query is unchanged only when name, SQL and params all match: consent showed the SQL.
    if y.subscriptions.queries.iter().any(|q| !x.subscriptions.queries.contains(q)) {
        return true;
    }
    let x_keys: &[String] = x.commands.set_property.as_ref().map(|s| s.keys.as_slice()).unwrap_or(&[]);
    let y_keys: &[String] = y.commands.set_property.as_ref().map(|s| s.keys.as_slice()).unwrap_or(&[]);
    if y_keys.iter().any(|new| !x_keys.iter().any(|old| pattern_contains(old, new))) {
        return true;
    }
    if y.commands.invoke.iter().any(|i| !x.commands.invoke.contains(i)) || y.hooks.iter().any(|h| !x.hooks.contains(h)) {
        return true;
    }
    if y.ui_intents.iter().any(|i| !x.ui_intents.contains(i)) {
        return true;
    }
    y.streams.iter().any(|ys| match x.streams.iter().find(|xs| xs.id == ys.id) {
        None => true,
        Some(xs) => {
            ys.max_rate_hz > xs.max_rate_hz
                || ys.max_event_bytes > xs.max_event_bytes
                || match (&xs.consumers, &ys.consumers) {
                    (None, _) => false,
                    (Some(_), None) => true,
                    (Some(old), Some(new)) => !subset(new, old),
                }
        }
    })
}

/// True when every key matched by `inner` is matched by `outer` (pattern containment, not string equality).
fn pattern_contains(outer: &str, inner: &str) -> bool {
    let (outer_base, outer_wild) = split_pattern(outer);
    let (inner_base, inner_wild) = split_pattern(inner);
    match (outer_wild, inner_wild) {
        (false, false) => outer_base == inner_base,
        (false, true) => false,
        // `a.*` matches keys under `a.`; an inner base equal to `a` or below it stays inside.
        (true, _) => inner_base == outer_base || inner_base.starts_with(&format!("{outer_base}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_portal() -> Value {
        json!({
            "schema": PORTAL_SCHEMA_V1,
            "entry": "web/index.html",
            "title": "Flow dashboard",
            "csp": { "connect": [], "img": [], "font": ["https://fonts.example.net"], "media": [], "frame": [], "wasm": false, "inline_styles": false },
            "navigation": ["https://docs.example.org"],
            "subscriptions": {
                "node": true, "readings": true,
                "queries": [{ "name": "hourly_flow", "sql": "SELECT metric FROM iot_reading WHERE metric = $metric", "params": { "metric": "string" } }]
            },
            "commands": { "set_property": { "keys": ["fleet.*", "console.note"] }, "invoke": [{ "package": "org.example.fleet-sim", "command": "start" }] },
            "hooks": [{ "source": "org.example.fleet-sim/positions" }],
            "streams": [{ "id": "selection", "max_rate_hz": 4, "max_event_bytes": 2048 }],
            "ui_intents": ["focus_node"]
        })
    }

    fn extension(portal: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "kind": "web-bundle", "id": "org.example.flow", "version": "1.0.0", "portal": portal })).unwrap()
    }

    fn member(path: &str) -> BundleMember {
        BundleMember { path: path.to_owned(), uncompressed_size: 100 }
    }

    fn base_members() -> Vec<BundleMember> {
        ["manifest.json", "entries.json", "plugin/extension.json", "web/index.html", "web/app.js"].into_iter().map(member).collect()
    }

    /// Run the validator on mutated inputs; the origin policy blocks `https://blocked.example.org`.
    fn run(
        mutate_portal: impl FnOnce(&mut Value),
        mutate_ext: impl FnOnce(&mut Value),
        members: Vec<BundleMember>,
        tweak: impl FnOnce(&mut BundleFacts<'_>),
    ) -> Result<ValidatedBundle, Refusal> {
        let mut portal = valid_portal();
        mutate_portal(&mut portal);
        let mut ext: Value = serde_json::from_slice(&extension(portal)).unwrap();
        mutate_ext(&mut ext);
        let raw = serde_json::to_vec(&ext).unwrap();
        let mut facts = BundleFacts {
            archive_bytes: 1000,
            max_bytes: DEFAULT_MAX_BUNDLE_BYTES,
            members: &members,
            integrity_failure: None,
            extension_json: Some(&raw),
            hexon_id: "org.example.flow",
            hexon_version: "1.0.0",
        };
        tweak(&mut facts);
        validate_portal_bundle(&facts, &|origin| origin != "https://blocked.example.org")
    }

    fn portal_case(f: impl FnOnce(&mut Value)) -> Result<ValidatedBundle, Refusal> {
        run(f, |_| {}, base_members(), |_| {})
    }

    #[test]
    fn validator_refuses_each_class_in_spec_order_and_admits_the_valid_bundle() {
        let ok = run(|_| {}, |_| {}, base_members(), |_| {}).expect("valid bundle");
        assert_eq!(ok.package_id, "org.example.flow");
        assert_eq!(ok.capability_digest.len(), 64);
        assert!(ok.web_members.contains(&("web/app.js".to_owned(), "text/javascript; charset=utf-8")));

        let mut over_json = base_members();
        over_json.push(BundleMember { path: "web/big.json".into(), uncompressed_size: MAX_JSON_ENTRY_BYTES + 1 });
        let mut no_ext = base_members();
        no_ext.retain(|m| m.path != "plugin/extension.json");

        let cases: Vec<(RefusalClass, Result<ValidatedBundle, Refusal>)> = vec![
            (RefusalClass::TooLarge, run(|_| {}, |_| {}, base_members(), |f| f.archive_bytes = f.max_bytes + 1)),
            (RefusalClass::ArchiveLimit, run(|_| {}, |_| {}, over_json, |_| {})),
            (RefusalClass::Integrity, run(|_| {}, |_| {}, base_members(), |f| f.integrity_failure = Some("bad root".into()))),
            (RefusalClass::ManifestMissing, run(|_| {}, |_| {}, no_ext, |f| f.extension_json = None)),
            (RefusalClass::ManifestMalformed, run(|_| {}, |_| {}, base_members(), |f| f.extension_json = Some(&b"{not json"[..]))),
            (RefusalClass::KindNotWebBundle, run(|_| {}, |e| e["kind"] = json!("extension"), base_members(), |_| {})),
            (RefusalClass::EnvelopeKeyForbidden, run(|_| {}, |e| e["tick"] = json!({}), base_members(), |_| {})),
            (RefusalClass::IdMismatch, run(|_| {}, |e| e["id"] = json!("org.other"), base_members(), |_| {})),
            (RefusalClass::SchemaUnsupported, portal_case(|p| p["schema"] = json!("fe-portal-bundle/2"))),
            (RefusalClass::VocabularyUnknown, portal_case(|p| p["csp"]["script"] = json!(["https://cdn.example.org"]))),
            (RefusalClass::LimitExceeded, portal_case(|p| p["ui_intents"] = json!(["focus_node", "select_node", "focus_node"]))),
            (RefusalClass::OriginInvalid, portal_case(|p| p["navigation"] = json!(["https://docs.example.org/path"]))),
            (RefusalClass::OriginBlocked, portal_case(|p| p["navigation"] = json!(["https://blocked.example.org"]))),
            (RefusalClass::QueryInvalid, portal_case(|p| p["subscriptions"]["queries"][0]["sql"] = json!("SELECT 1; DROP TABLE x"))),
            (RefusalClass::KeyPatternInvalid, portal_case(|p| p["commands"]["set_property"]["keys"] = json!(["portal.*"]))),
            (RefusalClass::TargetInvalid, portal_case(|p| p["hooks"] = json!([{ "source": "host/readings" }]))),
            (RefusalClass::Duplicate, portal_case(|p| p["navigation"] = json!(["https://docs.example.org", "https://docs.example.org"]))),
            (RefusalClass::ComponentPresent, run(|_| {}, |_| {}, [base_members(), vec![member("plugin/component.wasm")]].concat(), |_| {})),
            (RefusalClass::MemberReserved, run(|_| {}, |_| {}, [base_members(), vec![member("web/__fe/bridge.js")]].concat(), |_| {})),
            (RefusalClass::MemberPathInvalid, run(|_| {}, |_| {}, [base_members(), vec![member("web/.hidden.js")]].concat(), |_| {})),
            (RefusalClass::MemberTypeUnknown, run(|_| {}, |_| {}, [base_members(), vec![member("web/run.exe")]].concat(), |_| {})),
            (RefusalClass::EntryMissing, portal_case(|p| p["entry"] = json!("web/missing.html"))),
            (RefusalClass::EntryNotHtml, portal_case(|p| p["entry"] = json!("web/app.js"))),
        ];
        assert_eq!(cases.len(), 23, "one row per README §6 class");
        for (expected, got) in cases {
            let refusal = got.expect_err(expected.as_str());
            assert_eq!(refusal.class, expected, "{refusal}");
        }
        assert_eq!(RefusalClass::TooLarge.http_status(), 413);
        assert_eq!(RefusalClass::Integrity.http_status(), 422);
    }

    fn section(mutate: impl FnOnce(&mut Value)) -> PortalSection {
        let mut v = valid_portal();
        mutate(&mut v);
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn capability_digest_ignores_title_and_entry_and_tracks_grants() {
        let base = section(|_| {});
        let retitled = section(|p| {
            p["title"] = json!("Renamed");
            p["entry"] = json!("web/other.html");
        });
        let widened = section(|p| p["csp"]["wasm"] = json!(true));
        assert_eq!(capability_digest(&base), capability_digest(&retitled));
        assert_ne!(capability_digest(&base), capability_digest(&widened));
    }

    #[test]
    fn capability_increase_follows_the_readme_table() {
        let base = section(|_| {});
        // (mutation to build Y, expected increase)
        let cases: Vec<(Box<dyn Fn(&mut Value)>, bool)> = vec![
            (Box::new(|_| {}), false),
            (Box::new(|p| p["title"] = json!("Renamed")), false),
            (Box::new(|p| p["navigation"] = json!([])), false),
            (Box::new(|p| p["csp"]["connect"] = json!(["https://api.example.org"])), true),
            (Box::new(|p| p["csp"]["inline_styles"] = json!(true)), true),
            (Box::new(|p| p["subscriptions"]["queries"][0]["sql"] = json!("SELECT value FROM iot_reading")), true),
            (Box::new(|p| p["commands"]["set_property"]["keys"] = json!(["fleet.*"])), false),
            (Box::new(|p| p["commands"]["set_property"]["keys"] = json!(["fleet.a.*", "console.note"])), false),
            (Box::new(|p| p["commands"]["set_property"]["keys"] = json!(["fleet.*", "console.*"])), true),
            (Box::new(|p| p["streams"][0]["max_rate_hz"] = json!(8)), true),
            (Box::new(|p| p["streams"][0]["consumers"] = json!(["org.example.viewer"])), false),
            (Box::new(|p| p["ui_intents"] = json!(["focus_node", "select_node"])), true),
        ];
        for (i, (mutate, expected)) in cases.iter().enumerate() {
            let y = section(|p| mutate(p));
            assert_eq!(capability_increase(&base, &y), *expected, "case {i}");
        }
        // Dropping a consumer restriction is an increase; adding one is not.
        let restricted = section(|p| p["streams"][0]["consumers"] = json!(["org.example.viewer"]));
        assert!(capability_increase(&restricted, &base));
        assert!(!capability_increase(&base, &restricted));
    }
}
