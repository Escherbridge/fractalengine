//! Extension governance DB plane (spec R18/R21/R23): the replicated per-verse policy row, its merge
//! with the host's local policy, and the local `ExtensionAudit` trail (consent, kill-list override).
//! Contract: docs/extensions/governance.md; rationale: src/handlers/AGENTS.md §extension-policy.

use std::collections::HashMap;

use fe_runtime::messages::ReplicatedRowOutcome;
use fe_sdk::manifest_v2::{Egress, ExtensionKind, ExtensionManifest, Ingress};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::op_log::{commit_operation, commit_operation_atomic_with};
use crate::repo::Db;
use crate::role_level::RoleLevel;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{BlobStoreHandle, ReplicationSender};

/// Replicated table holding one policy row per verse.
pub const POLICY_TABLE: &str = "extension_policy";
/// The only policy row schema this build reads; a newer one is denied inbound.
pub const POLICY_SCHEMA_VERSION: u32 = 1;
/// Largest serialized policy row.
pub const MAX_POLICY_BYTES: usize = 64 * 1024;
/// Most `allow` or `deny` entries in one policy.
pub const MAX_POLICY_ENTRIES: usize = 256;
/// Audit rows one read returns at most (mirrors fe-api `MAX_AUDIT_ROWS`).
pub const MAX_AUDIT_ROWS: usize = 500;
/// `op_log.op_type` text of every audit entry.
const AUDIT_OP_TYPE: &str = "ExtensionAudit";

/// Table definitions `schema::apply_all` runs (idempotent).
pub const SCHEMA_STATEMENTS: [&str; 2] = [
    "DEFINE TABLE IF NOT EXISTS extension_policy SCHEMALESS",
    "DEFINE INDEX IF NOT EXISTS idx_extension_policy_verse ON TABLE extension_policy FIELDS verse_id UNIQUE",
];

/// Define the policy table (called from `schema::apply_all`; safe to repeat).
pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    for statement in SCHEMA_STATEMENTS {
        db.query(statement)
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("{POLICY_TABLE} schema: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Policy contract (governance.md §Policy row)
// ---------------------------------------------------------------------------

/// Which packages a verse admits at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyMode {
    /// Signed by a `release` root (a `dev` root in a dev build). The default when no row exists.
    #[default]
    FirstPartyOnly,
    /// An `allow` entry matches, and the package is trusted.
    AllowList,
    /// Trusted by any loaded root.
    TrustRoot,
}

/// One `allow` entry: a package id plus EXACTLY one of an exact build hash or a SemVer requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowEntry {
    pub package_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<String>,
}

/// Network ceiling, ordered `none` < `loopback` < `declared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetCeiling {
    /// No endpoint of this direction.
    #[serde(rename = "none")]
    Closed,
    /// Only endpoints whose remote end is 127.0.0.1, [::1] or localhost.
    Loopback,
    /// Every declared endpoint.
    Declared,
}

/// Capability ceilings; an absent field never limits (the verse default for `bus_external` is closed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ceilings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_write: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readings: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress: Option<NetCeiling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress: Option<NetCeiling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus_external: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<ExtensionKind>>,
}

/// The per-verse policy a Manager+ writes (the body of `PUT .../extension-policy`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPolicy {
    /// Optional echo of the path's verse; must match when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verse_id: Option<String>,
    pub mode: PolicyMode,
    pub allow: Vec<AllowEntry>,
    /// Package ids, or 64-hex build digests matched under ANY package id.
    pub deny: Vec<String>,
    pub ceilings: Ceilings,
}

/// The host's own `extensions.toml` (beside the trust roots): deny list and ceilings, no mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalPolicy {
    pub deny: Vec<String>,
    pub ceilings: Ceilings,
}

/// The stored and replicated row: one per verse, newest `(written_at, author_did)` wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRow {
    pub schema_version: u32,
    pub verse_id: String,
    pub author_did: String,
    /// The writer's packed HLC (the op-log lamport of the write).
    pub written_at: u64,
    pub policy: ExtensionPolicy,
}

impl PolicyRow {
    /// Last-writer-wins order key; the policy bytes break an (impossible locally) HLC tie.
    fn order_key(&self) -> (u64, &str, String) {
        (
            self.written_at,
            self.author_did.as_str(),
            serde_json::to_string(&self.policy).unwrap_or_default(),
        )
    }
}

/// How a package is trusted (set by the caller from the verifying root's role).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustTier {
    /// A `release` root (first-party).
    Release,
    /// A `dev` root; exists only with the dev flag, so it counts as first-party in a dev build.
    Dev,
    /// Any other loaded root.
    Publisher,
    /// Unsigned under the debug-only dev flag.
    DevUnsigned,
}

/// What policy evaluation needs to know about one build.
#[derive(Debug, Clone, Copy)]
pub struct PolicyFacts<'a> {
    pub package_id: &'a str,
    pub version: &'a str,
    pub content_hash: &'a str,
    pub component_digest: Option<&'a str>,
    pub plaintext_digest: Option<&'a str>,
    pub trust: TrustTier,
    pub kind: ExtensionKind,
    /// The extension manifest; `None` for a web bundle (the portal contract owns its ceilings).
    pub manifest: Option<&'a ExtensionManifest>,
}

/// Which policy refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyLayer {
    Verse,
    Local,
}

/// Why a policy refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenialReason {
    DenyListed(String),
    NotFirstParty,
    NotAllowListed,
    Ceiling(&'static str),
}

/// A `policy-denied` refusal naming the layer (bindings.md §5.2 row 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDenial {
    pub layer: PolicyLayer,
    pub reason: DenialReason,
}

impl std::fmt::Display for PolicyDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let layer = match self.layer {
            PolicyLayer::Verse => "verse",
            PolicyLayer::Local => "local",
        };
        match &self.reason {
            DenialReason::DenyListed(entry) => write!(f, "{layer} policy: deny-listed ({entry})"),
            DenialReason::NotFirstParty => write!(f, "{layer} policy: first-party-only admits release-signed packages"),
            DenialReason::NotAllowListed => write!(f, "{layer} policy: no allow entry admits this build"),
            DenialReason::Ceiling(name) => write!(f, "{layer} policy: exceeds the {name} ceiling"),
        }
    }
}

impl std::error::Error for PolicyDenial {}

/// The four extension kinds a `kinds` ceiling may name (web bundles are governed by the portal contract).
const EXTENSION_KINDS: [ExtensionKind; 4] = [
    ExtensionKind::Simulation,
    ExtensionKind::DeviceAdapter,
    ExtensionKind::BiConnector,
    ExtensionKind::General,
];

impl ExtensionPolicy {
    /// Shape rules shared by the local write and the inbound row (strictest reader); `Err` is the reason.
    pub fn validate(&self, verse_id: &str) -> Result<(), String> {
        if self.verse_id.as_deref().is_some_and(|echo| echo != verse_id) {
            return Err("verse_id does not match the policy's verse".into());
        }
        if self.allow.len() > MAX_POLICY_ENTRIES || self.deny.len() > MAX_POLICY_ENTRIES {
            return Err(format!("allow and deny hold at most {MAX_POLICY_ENTRIES} entries each"));
        }
        for entry in &self.allow {
            if !is_package_id(&entry.package_id) {
                return Err(format!("allow: malformed package id '{}'", entry.package_id));
            }
            match (&entry.content_hash, &entry.versions) {
                (Some(hash), None) if is_digest(hash) => {}
                (None, Some(requirement)) if is_version_requirement(requirement) => {}
                (Some(_), None) => return Err(format!("allow: content_hash of '{}' is not 64 lowercase hex", entry.package_id)),
                (None, Some(_)) => return Err(format!("allow: versions of '{}' is not a SemVer requirement", entry.package_id)),
                _ => return Err(format!("allow: '{}' needs exactly one of content_hash or versions", entry.package_id)),
            }
        }
        if let Some(entry) = self.deny.iter().find(|entry| !is_package_id(entry) && !is_digest(entry)) {
            return Err(format!("deny: '{entry}' is neither a package id nor a 64-hex digest"));
        }
        if let Some(kinds) = &self.ceilings.kinds {
            if kinds.iter().any(|kind| !EXTENSION_KINDS.contains(kind)) {
                return Err("ceilings.kinds lists only extension kinds (simulation, device-adapter, bi-connector, general)".into());
            }
            if kinds.iter().enumerate().any(|(i, kind)| kinds[..i].contains(kind)) {
                return Err("ceilings.kinds has a duplicate".into());
            }
        }
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_POLICY_BYTES) {
            return Err(format!("policy exceeds {MAX_POLICY_BYTES} bytes"));
        }
        Ok(())
    }

    /// Whether `external: true` streams and commands may serve remote callers in this verse (verse default: no).
    pub fn bus_external(policy: Option<&Self>, local: &LocalPolicy) -> bool {
        policy.and_then(|p| p.ceilings.bus_external).unwrap_or(false)
            && local.ceilings.bus_external.unwrap_or(true)
    }
}

/// Precondition 7: the verse policy (default `first-party-only` when absent) AND the local policy admit
/// the build within ceilings; denies union and ceilings intersect (S-RELAY-ADVERTISE pattern).
pub fn evaluate(
    verse: Option<&ExtensionPolicy>,
    local: &LocalPolicy,
    facts: &PolicyFacts<'_>,
) -> Result<(), PolicyDenial> {
    let fallback = ExtensionPolicy::default();
    let verse = verse.unwrap_or(&fallback);
    check_deny(PolicyLayer::Verse, &verse.deny, facts)?;
    check_deny(PolicyLayer::Local, &local.deny, facts)?;
    check_mode(verse, facts)?;
    check_ceilings(PolicyLayer::Verse, &verse.ceilings, facts)?;
    check_ceilings(PolicyLayer::Local, &local.ceilings, facts)
}

fn deny(layer: PolicyLayer, reason: DenialReason) -> Result<(), PolicyDenial> {
    Err(PolicyDenial { layer, reason })
}

/// A digest entry matches the build's content hash, component digest or plaintext digest under any id.
fn check_deny(layer: PolicyLayer, entries: &[String], facts: &PolicyFacts<'_>) -> Result<(), PolicyDenial> {
    let hashes = [Some(facts.content_hash), facts.component_digest, facts.plaintext_digest];
    for entry in entries {
        let hit = if is_digest(entry) {
            hashes.iter().flatten().any(|hash| hash.eq_ignore_ascii_case(entry))
        } else {
            entry == facts.package_id
        };
        if hit {
            return deny(layer, DenialReason::DenyListed(entry.clone()));
        }
    }
    Ok(())
}

fn check_mode(policy: &ExtensionPolicy, facts: &PolicyFacts<'_>) -> Result<(), PolicyDenial> {
    match policy.mode {
        PolicyMode::FirstPartyOnly => match facts.trust {
            TrustTier::Release | TrustTier::Dev | TrustTier::DevUnsigned => Ok(()),
            TrustTier::Publisher => deny(PolicyLayer::Verse, DenialReason::NotFirstParty),
        },
        PolicyMode::TrustRoot => Ok(()),
        PolicyMode::AllowList => {
            let admitted = policy.allow.iter().any(|entry| {
                entry.package_id == facts.package_id
                    && match (&entry.content_hash, &entry.versions) {
                        (Some(hash), _) => hash.eq_ignore_ascii_case(facts.content_hash),
                        (None, Some(requirement)) => version_matches(requirement, facts.version),
                        (None, None) => false,
                    }
            });
            if admitted {
                Ok(())
            } else {
                deny(PolicyLayer::Verse, DenialReason::NotAllowListed)
            }
        }
    }
}

fn check_ceilings(layer: PolicyLayer, ceilings: &Ceilings, facts: &PolicyFacts<'_>) -> Result<(), PolicyDenial> {
    if facts.kind != ExtensionKind::WebBundle {
        if let Some(kinds) = &ceilings.kinds {
            if !kinds.contains(&facts.kind) {
                return deny(layer, DenialReason::Ceiling("kinds"));
            }
        }
    }
    let Some(manifest) = facts.manifest else { return Ok(()) };
    let writes_scene = manifest
        .scene
        .as_ref()
        .and_then(|scene| scene.write.as_ref())
        .is_some_and(|kinds| !kinds.is_empty());
    if writes_scene && ceilings.scene_write == Some(false) {
        return deny(layer, DenialReason::Ceiling("scene_write"));
    }
    if manifest.readings.is_some() && ceilings.readings == Some(false) {
        return deny(layer, DenialReason::Ceiling("readings"));
    }
    let ingress_need = manifest.ingress.iter().map(ingress_reach).max().unwrap_or(NetCeiling::Closed);
    if ceilings.ingress.is_some_and(|ceiling| ingress_need > ceiling) {
        return deny(layer, DenialReason::Ceiling("ingress"));
    }
    let egress_need = manifest.egress.iter().map(egress_reach).max().unwrap_or(NetCeiling::Closed);
    if ceilings.egress.is_some_and(|ceiling| egress_need > ceiling) {
        return deny(layer, DenialReason::Ceiling("egress"));
    }
    Ok(())
}

/// Accept-mode ingress listens on the API server, so it is reachable beyond loopback.
fn ingress_reach(ingress: &Ingress) -> NetCeiling {
    match ingress {
        Ingress::HttpServer(_) | Ingress::WebsocketAccept(_) => NetCeiling::Declared,
        Ingress::WebsocketDial(dial) => url_reach(&dial.url),
        Ingress::Mqtt(mqtt) => url_reach(&mqtt.broker),
    }
}

fn egress_reach(egress: &Egress) -> NetCeiling {
    match egress {
        Egress::Http(http) => url_reach(&http.url),
        Egress::Mqtt(mqtt) => url_reach(&mqtt.broker),
    }
}

/// Loopback when the URL's host is 127.0.0.1, [::1] or localhost (manifest v2's loopback rule).
fn url_reach(url: &str) -> NetCeiling {
    // The authority ends at the first `/ \ ? #` (WHATWG, as the network client parses it): splitting on `/`
    // alone read `https://evil.example?@localhost/` as loopback, past a `loopback` ceiling (G2).
    let authority = url
        .split_once("://")
        .map_or("", |(_, rest)| rest.split(['/', '\\', '?', '#']).next().unwrap_or(""));
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if hostport.starts_with('[') {
        hostport.split_inclusive(']').next().unwrap_or(hostport)
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    if matches!(host, "127.0.0.1" | "[::1]" | "localhost") {
        NetCeiling::Loopback
    } else {
        NetCeiling::Declared
    }
}

/// Store-client package id grammar: `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`.
fn is_package_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && !id.ends_with(".json")
}

/// 64 lowercase hex characters (a BLAKE3 digest as package-format.md writes it).
fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---------------------------------------------------------------------------
// SemVer requirements (Cargo syntax; comma = AND)
// ---------------------------------------------------------------------------

/// A requirement this matcher understands: `*`, or comma-separated `[op]MAJOR[.MINOR[.PATCH]]`.
pub fn is_version_requirement(requirement: &str) -> bool {
    let requirement = requirement.trim();
    requirement.len() <= 128
        && (requirement == "*"
            || requirement
                .split(',')
                .all(|part| parse_comparator(part.trim()).is_some()))
}

/// Whether `version` (exact `X.Y.Z`) satisfies `requirement`; pre-release and build versions never match (fail closed).
pub fn version_matches(requirement: &str, version: &str) -> bool {
    let Some(version) = parse_release(version) else { return false };
    let requirement = requirement.trim();
    if requirement == "*" {
        return true;
    }
    requirement.split(',').all(|part| {
        parse_comparator(part.trim()).is_some_and(|(op, major, minor, patch)| comparator_holds(op, major, minor, patch, version))
    })
}

fn parse_number(text: &str) -> Option<u64> {
    let ok = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && (text == "0" || !text.starts_with('0'));
    ok.then(|| text.parse().ok()).flatten()
}

fn parse_release(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let release = [parse_number(parts.next()?)?, parse_number(parts.next()?)?, parse_number(parts.next()?)?];
    parts.next().is_none().then_some(release)
}

type Comparator<'a> = (&'a str, u64, Option<u64>, Option<u64>);

fn parse_comparator(text: &str) -> Option<Comparator<'_>> {
    let (op, rest) = [">=", "<=", ">", "<", "=", "^", "~"]
        .iter()
        .find_map(|op| text.strip_prefix(op).map(|rest| (*op, rest)))
        .unwrap_or(("^", text));
    let mut parts = rest.trim().split('.');
    let major = parse_number(parts.next()?)?;
    let minor = parts.next().map(parse_number);
    let patch = parts.next().map(parse_number);
    if parts.next().is_some() || minor == Some(None) || patch == Some(None) || (minor.is_none() && patch.is_some()) {
        return None;
    }
    Some((op, major, minor.flatten(), patch.flatten()))
}

fn comparator_holds(op: &str, major: u64, minor: Option<u64>, patch: Option<u64>, v: [u64; 3]) -> bool {
    let floor = [major, minor.unwrap_or(0), patch.unwrap_or(0)];
    let next_major = [major.saturating_add(1), 0, 0];
    let next_minor = |minor: u64| [major, minor.saturating_add(1), 0];
    match op {
        "=" => match (minor, patch) {
            (Some(_), Some(_)) => v == floor,
            (Some(n), None) => v >= floor && v < next_minor(n),
            _ => v >= floor && v < next_major,
        },
        ">" => match (minor, patch) {
            (Some(_), Some(_)) => v > floor,
            (Some(n), None) => v >= next_minor(n),
            _ => v >= next_major,
        },
        ">=" => v >= floor,
        "<" => v < floor,
        "<=" => match (minor, patch) {
            (Some(_), Some(_)) => v <= floor,
            (Some(n), None) => v < next_minor(n),
            _ => v < next_major,
        },
        "~" => v >= floor && v < minor.map_or(next_major, next_minor),
        _ => {
            // Caret (also a bare version): the leftmost non-zero component may not change.
            let ceiling = match (major, minor, patch) {
                (0, Some(0), Some(p)) => [0, 0, p.saturating_add(1)],
                (0, Some(n), _) => next_minor(n),
                _ => next_major,
            };
            v >= floor && v < ceiling
        }
    }
}

// ---------------------------------------------------------------------------
// Policy row: local write, read, inbound admission
// ---------------------------------------------------------------------------

/// Why a local policy write was refused.
#[derive(Debug, thiserror::Error)]
pub enum PolicyWriteError {
    /// The policy failed the shape rules (422 `invalid-policy`).
    #[error("invalid-policy: {0}")]
    Invalid(String),
    /// The local DID (the row's author) is below Manager at the verse (403 `role-denied`).
    #[error("role-denied: {0}")]
    RoleDenied(String),
    #[error("extension policy write failed: {0:#}")]
    Db(#[from] anyhow::Error),
}

/// The verse's stored policy row, or `None` (no row: `first-party-only`).
pub async fn get_policy(db: &Db, verse_id: &str) -> anyhow::Result<Option<PolicyRow>> {
    let mut response = db
        .query("SELECT * OMIT id FROM extension_policy WHERE verse_id = $policy_verse LIMIT 1")
        .bind(("policy_verse", verse_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("{POLICY_TABLE} read failed: {e}"))?;
    let rows: Vec<Value> = response.take(0)?;
    rows.into_iter()
        .next()
        .map(|row| serde_json::from_value::<PolicyRow>(row).map_err(|e| anyhow::anyhow!("stored {POLICY_TABLE} row: {e}")))
        .transpose()
}

/// Every stored policy row (host startup: hydrate the policy mirror).
pub async fn all_policies(db: &Db) -> anyhow::Result<Vec<PolicyRow>> {
    let mut response = db
        .query("SELECT * OMIT id FROM extension_policy")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("{POLICY_TABLE} read failed: {e}"))?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(rows.into_iter().filter_map(|row| serde_json::from_value(row).ok()).collect())
}

/// [`set_policy_as`] with the node itself as the actor (local callers and tests).
pub async fn set_policy_with_replication(
    db: &Db,
    local_did: &str,
    verse_id: &str,
    policy: ExtensionPolicy,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<PolicyRow, PolicyWriteError> {
    set_policy_as(db, local_did, local_did, verse_id, policy, blob_store, repl_tx).await
}

/// Write the verse policy as `local_did` (log-first, one `policy-change` audit op and the row in ONE
/// transaction), then publish the row. The API already checked the caller's own Manager+ and scope;
/// the audit names `actor_did` (who asked), the row names `local_did` (who wrote). Empty actor = `local_did`.
pub async fn set_policy_as(
    db: &Db,
    local_did: &str,
    actor_did: &str,
    verse_id: &str,
    mut policy: ExtensionPolicy,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<PolicyRow, PolicyWriteError> {
    let actor_did = if actor_did.is_empty() { local_did } else { actor_did };
    policy.validate(verse_id).map_err(PolicyWriteError::Invalid)?;
    policy.verse_id = None;
    let verse_scope = crate::scope::build_scope(verse_id, None, None);
    let role = crate::role_manager::resolve_role(db, local_did, &verse_scope).await?;
    if !role.is_at_least(RoleLevel::Manager) {
        let reason = format!("this node holds {role} at {verse_scope}; the policy row needs Manager+");
        append_audit(db, &AuditEntry::verse(AuditAction::PolicyChange, actor_did, verse_id, AuditOutcome::Refused, Some(reason.clone()))).await?;
        return Err(PolicyWriteError::RoleDenied(reason));
    }

    let mut audit = AuditEntry::verse(AuditAction::PolicyChange, actor_did, verse_id, AuditOutcome::Ok, None);
    audit.at = now_rfc3339();
    let entry = audit_op(&audit)?;
    // The row's `written_at` is the op's own HLC, so the closure builds it from the stamped clock.
    let mut row = PolicyRow {
        schema_version: POLICY_SCHEMA_VERSION,
        verse_id: verse_id.to_string(),
        author_did: local_did.to_string(),
        written_at: 0,
        policy,
    };
    let template = row.clone();
    let (lamport, _) = commit_operation_atomic_with(db, entry, move |hlc| {
        let stamped = PolicyRow { written_at: hlc, ..template };
        let verse = json!(stamped.verse_id);
        let value = serde_json::to_value(&stamped).unwrap_or(Value::Null);
        (
            vec![
                "DELETE extension_policy WHERE verse_id = $policy_verse".to_string(),
                "CREATE extension_policy CONTENT $policy_row".to_string(),
            ],
            vec![("policy_verse".to_string(), verse), ("policy_row".to_string(), value)],
        )
    })
    .await?;
    row.written_at = lamport;

    // Durable first, then publish (§device-payload precedent).
    match (blob_store, serde_json::to_vec(&row)) {
        (Some(store), Ok(bytes)) => crate::replicate_row(repl_tx, store, verse_id, POLICY_TABLE, verse_id, &bytes),
        (None, _) if repl_tx.is_some() => {
            tracing::warn!(verse_id, "extension policy written: blob store missing, not published");
        }
        (_, Err(e)) => tracing::warn!(verse_id, "extension policy written but not serialised for replication: {e}"),
        _ => {}
    }
    Ok(row)
}

/// Strict parse of a replicated row plus its identity rules; `Err` is the deny reason.
fn parse_inbound(verse_id: &str, record_id: &str, row_bytes: &[u8]) -> Result<PolicyRow, String> {
    if row_bytes.len() > MAX_POLICY_BYTES + 1024 {
        return Err("extension_policy row is over the size cap".into());
    }
    let row: PolicyRow = serde_json::from_slice(row_bytes).map_err(|e| format!("extension_policy row does not parse: {e}"))?;
    if row.schema_version != POLICY_SCHEMA_VERSION {
        return Err(format!("extension_policy schema_version {} is not {POLICY_SCHEMA_VERSION}", row.schema_version));
    }
    if row.verse_id != verse_id || record_id != verse_id {
        return Err("extension_policy row names another verse".into());
    }
    row.policy.validate(verse_id)?;
    Ok(row)
}

/// Referential confinement for `replicated_row::confine`: `Ok` gates the author at `Floor::ManageVerse`.
pub fn confine_inbound(verse_id: &str, record_id: &str, row_bytes: &[u8]) -> Result<(), String> {
    parse_inbound(verse_id, record_id, row_bytes).map(|_| ())
}

/// Row-mode apply (after the Manager+-now gate): the payload's author must be the attested author and
/// held Manager+ when it wrote (role-at-write); the newest `(written_at, author_did)` wins.
pub async fn apply_replicated_policy_row(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<ReplicatedRowOutcome> {
    if row_bytes.is_empty() {
        // Clearing writes `mode: first-party-only`; a `del` marker has no meaning here.
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let row = match parse_inbound(verse_id, record_id, row_bytes) {
        Ok(row) => row,
        Err(reason) => {
            tracing::warn!(verse_id, author = author_did, %reason, "inbound extension_policy row denied");
            return Ok(ReplicatedRowOutcome::Denied);
        }
    };
    if row.author_did != author_did {
        tracing::warn!(verse_id, author = author_did, claimed = %row.author_did, "extension_policy author differs from the attested author");
        return Ok(ReplicatedRowOutcome::Denied);
    }
    // An author stamps its own clock: a far-future `written_at` would win last-writer-wins forever.
    let Ok(clock) = crate::op_log::admit_scene_hlc(row.written_at) else {
        tracing::warn!(verse_id, author = author_did, written_at = row.written_at, "extension_policy clock outside the five-minute future bound");
        return Ok(ReplicatedRowOutcome::Denied);
    };
    if !written_while_manager(db, verse_id, author_did, row.written_at).await? {
        tracing::warn!(verse_id, author = author_did, "extension_policy row written before its author held Manager+");
        return Ok(ReplicatedRowOutcome::Denied);
    }
    if let Some(current) = get_policy(db, verse_id).await? {
        if current.order_key() >= row.order_key() {
            return Ok(ReplicatedRowOutcome::Applied);
        }
    }
    db.query(
        "BEGIN TRANSACTION;\n\
         DELETE extension_policy WHERE verse_id = $policy_verse;\n\
         CREATE extension_policy CONTENT $policy_row;\n\
         COMMIT TRANSACTION;",
    )
    .bind(("policy_verse", verse_id.to_string()))
    .bind(("policy_row", serde_json::to_value(&row)?))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("replicated {POLICY_TABLE} write failed: {e}"))?;
    // The next local write orders after the row it replaces.
    crate::op_log::observe_scene_hlc(clock);
    Ok(ReplicatedRowOutcome::Applied)
}

async fn written_while_manager(db: &Db, verse_id: &str, author: &str, hlc: u64) -> anyhow::Result<bool> {
    let scope = crate::scope::build_scope(verse_id, None, None);
    held_at_write(db, verse_id, author, &scope, RoleLevel::Manager, hlc).await
}

/// Role-at-write for governance rows (§write-time-role, generalised from `written_while_authorized`'s fixed
/// Editor): false iff `author`'s current `level` at `scope` comes from admitted statements that did not yet
/// grant it at `hlc`. Policy rows use Manager at the verse; binding rows (bindings.md §4.4) reuse it per scope.
pub async fn held_at_write(
    db: &Db,
    verse_id: &str,
    author: &str,
    scope: &str,
    level: RoleLevel,
    hlc: u64,
) -> anyhow::Result<bool> {
    let Some(owner) = crate::role_grants::verse_owner(db, verse_id).await? else {
        return Ok(true);
    };
    if owner == author {
        return Ok(true);
    }
    let admitted = crate::role_grants::load_admitted(db, verse_id).await?;
    if !fe_policy::materialize(&owner, admitted.iter()).role_at(author, scope).is_at_least(level) {
        // Authority not derived from statements is the current-role gate's call.
        return Ok(true);
    }
    Ok(fe_policy::materialize_at(&owner, admitted.iter(), hlc)
        .role_at(author, scope)
        .is_at_least(level))
}

// ---------------------------------------------------------------------------
// Audit (governance.md §Audit): local op-log only, never replicated (OD-E7)
// ---------------------------------------------------------------------------

/// The audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditAction {
    Install,
    Update,
    Consent,
    ConsentRevoke,
    /// Also local acceptance in a verse (bindings.md §5.3), with `verse_id` set.
    Bind,
    Unbind,
    /// `target: "*"` for Disable all.
    Enable,
    Disable,
    Uninstall,
    Revocation,
    PolicyChange,
    BusSubscribe,
    BusUnsubscribe,
    CommandCall,
    Denial,
    KillListOverride,
}

/// Outcome of an audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditOutcome {
    Ok,
    Refused,
    Denied,
}

/// One `ExtensionAudit` op-log payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub action: AuditAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub actor_did: String,
    pub target: String,
    pub outcome: AuditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verse_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_digest: Option<String>,
    /// RFC 3339; stamped by [`append_audit`] when empty.
    #[serde(default)]
    pub at: String,
}

impl AuditEntry {
    /// A verse-level entry (policy change, kill-list override) with no installation.
    pub fn verse(action: AuditAction, actor_did: &str, verse_id: &str, outcome: AuditOutcome, reason: Option<String>) -> Self {
        Self {
            action,
            installation: None,
            package_id: None,
            content_hash: None,
            actor_did: actor_did.to_string(),
            target: verse_id.to_string(),
            outcome,
            reason,
            args_digest: None,
            verse_id: Some(verse_id.to_string()),
            capability_digest: None,
            at: String::new(),
        }
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn audit_op(entry: &AuditEntry) -> anyhow::Result<OpLogEntry> {
    let subject = entry.installation.as_deref().unwrap_or(&entry.target);
    Ok(OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(format!("ext:{subject}")),
        op_type: OpType::ExtensionAudit,
        payload: serde_json::to_value(entry)?,
        sig: "00".repeat(64),
    })
}

/// Append one audit entry (`commit_operation`, no materialization: the entry is its own fact); returns its lamport.
pub async fn append_audit(db: &Db, entry: &AuditEntry) -> anyhow::Result<u64> {
    let mut entry = entry.clone();
    if entry.at.is_empty() {
        entry.at = now_rfc3339();
    }
    let op = audit_op(&entry)?;
    commit_operation(db, op, |lamport| async move { Ok(lamport) }).await
}

/// Which audit rows to read.
#[derive(Debug, Clone, Copy)]
pub enum AuditFilter<'a> {
    Installation(&'a str),
    Verse(&'a str),
}

/// The newest `limit` audit entries (clamped to [`MAX_AUDIT_ROWS`]), newest first, each with its `lamport_clock`.
pub async fn list_audit(db: &Db, filter: AuditFilter<'_>, limit: usize) -> anyhow::Result<Vec<Value>> {
    let limit = limit.clamp(1, MAX_AUDIT_ROWS);
    let (field, value) = match filter {
        AuditFilter::Installation(id) => ("installation", id),
        AuditFilter::Verse(id) => ("verse_id", id),
    };
    let sql = format!(
        "SELECT lamport_clock, payload FROM op_log WHERE op_type = '{AUDIT_OP_TYPE}' AND payload.{field} = $audit_subject \
         ORDER BY lamport_clock DESC LIMIT {limit}"
    );
    let mut response = db
        .query(sql)
        .bind(("audit_subject", value.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("extension audit read failed: {e}"))?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|mut row| {
            let lamport = row["lamport_clock"].clone();
            let mut payload = row["payload"].take();
            if let Some(object) = payload.as_object_mut() {
                object.insert("lamport_clock".into(), lamport);
            }
            payload
        })
        .collect())
}

/// Audit payloads of `actions` with outcome ok, oldest first.
async fn ok_entries(db: &Db, actions: &[AuditAction]) -> anyhow::Result<Vec<AuditEntry>> {
    let actions: Vec<Value> = actions.iter().map(|a| serde_json::to_value(a).unwrap_or(Value::Null)).collect();
    let mut response = db
        .query(format!(
            "SELECT lamport_clock, payload FROM op_log WHERE op_type = '{AUDIT_OP_TYPE}' \
             AND payload.outcome = 'ok' AND payload.action IN $audit_actions ORDER BY lamport_clock ASC"
        ))
        .bind(("audit_actions", actions))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("extension audit read failed: {e}"))?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|mut row| serde_json::from_value(row["payload"].take()).ok())
        .collect())
}

// ---------------------------------------------------------------------------
// Consent records (governance.md §Consent): derived from the audit trail
// ---------------------------------------------------------------------------

/// The local consent record of one installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub installation_id: String,
    pub package_id: String,
    pub content_hash: String,
    pub capability_digest: String,
    pub actor_did: String,
    pub at: String,
}

/// Record consent (one `consent` audit op); the record IS the audit entry, so it cannot drift from it.
pub async fn record_consent(db: &Db, record: &ConsentRecord) -> anyhow::Result<u64> {
    append_audit(
        db,
        &AuditEntry {
            action: AuditAction::Consent,
            installation: Some(record.installation_id.clone()),
            package_id: Some(record.package_id.clone()),
            content_hash: Some(record.content_hash.clone()),
            actor_did: record.actor_did.clone(),
            target: record.installation_id.clone(),
            outcome: AuditOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: Some(record.capability_digest.clone()),
            at: record.at.clone(),
        },
    )
    .await
}

/// Every installation's current consent (latest `consent` not followed by a `consent-revoke`).
pub async fn load_consents(db: &Db) -> anyhow::Result<HashMap<String, ConsentRecord>> {
    let mut consents = HashMap::new();
    for entry in ok_entries(db, &[AuditAction::Consent, AuditAction::ConsentRevoke]).await? {
        let Some(installation) = entry.installation.clone() else { continue };
        if entry.action == AuditAction::ConsentRevoke {
            consents.remove(&installation);
            continue;
        }
        if let (Some(package_id), Some(content_hash), Some(capability_digest)) =
            (entry.package_id, entry.content_hash, entry.capability_digest)
        {
            consents.insert(
                installation.clone(),
                ConsentRecord {
                    installation_id: installation,
                    package_id,
                    content_hash,
                    capability_digest,
                    actor_did: entry.actor_did,
                    at: entry.at,
                },
            );
        }
    }
    Ok(consents)
}

// ---------------------------------------------------------------------------
// Local acceptance and the install offer (bindings.md §5.3, §5.5)
// ---------------------------------------------------------------------------

/// This host's record that one installation may run in one verse; local, never replicated (R12).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AcceptanceRecord {
    pub installation_id: String,
    pub verse_id: String,
    /// The readings class picked when the manifest lists several (governance Consent "Bind").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_class: Option<String>,
    /// The identity that accepted (R-GOV-31); the host honours only its own DID's records (R12).
    #[serde(default)]
    pub actor_did: String,
}

const SOURCE_CLASS_PREFIX: &str = "source_class=";

fn acceptance_entry(action: AuditAction, actor_did: &str, record: &AcceptanceRecord, package_id: Option<&str>) -> AuditEntry {
    let mut entry = AuditEntry::verse(action, actor_did, &record.verse_id, AuditOutcome::Ok, None);
    entry.installation = Some(record.installation_id.clone());
    entry.package_id = package_id.map(str::to_string);
    entry.target = format!("{}@{}", record.installation_id, record.verse_id);
    entry.reason = record.source_class.as_ref().map(|class| format!("{SOURCE_CLASS_PREFIX}{class}"));
    entry
}

/// Record "Allow in <verse>" for one installation (one `bind` audit op with `verse_id`).
pub async fn record_acceptance(db: &Db, actor_did: &str, record: &AcceptanceRecord, package_id: &str) -> anyhow::Result<u64> {
    if record.installation_id.is_empty() || record.verse_id.is_empty() {
        anyhow::bail!("invalid-input: acceptance needs an installation and a verse");
    }
    append_audit(db, &acceptance_entry(AuditAction::Bind, actor_did, record, Some(package_id))).await
}

/// Withdraw an acceptance (one `unbind` op); instances of the installation in that verse deactivate `unbound`.
pub async fn revoke_acceptance(db: &Db, actor_did: &str, record: &AcceptanceRecord) -> anyhow::Result<u64> {
    append_audit(db, &acceptance_entry(AuditAction::Unbind, actor_did, record, None)).await
}

/// Every acceptance in force: the latest `bind` per (installation, verse, actor) not followed by that actor's
/// `unbind`; the caller keeps only its own DID's records (an identity's word never carries over, R12).
pub async fn load_acceptances(db: &Db) -> anyhow::Result<Vec<AcceptanceRecord>> {
    let mut accepted: HashMap<(String, String, String), AcceptanceRecord> = HashMap::new();
    for entry in ok_entries(db, &[AuditAction::Bind, AuditAction::Unbind]).await? {
        let (Some(installation_id), Some(verse_id)) = (entry.installation.clone(), entry.verse_id.clone()) else {
            continue; // a per-petal bind without a verse is not an acceptance
        };
        let key = (installation_id.clone(), verse_id.clone(), entry.actor_did.clone());
        if entry.action == AuditAction::Unbind {
            accepted.remove(&key);
            continue;
        }
        let source_class = entry.reason.as_deref().and_then(|reason| reason.strip_prefix(SOURCE_CLASS_PREFIX)).map(str::to_string);
        accepted.insert(key, AcceptanceRecord { installation_id, verse_id, source_class, actor_did: entry.actor_did });
    }
    Ok(accepted.into_values().collect())
}

/// What one confirmation of the install offer records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferConfirmation {
    pub installation_id: String,
    pub package_id: String,
    pub content_hash: String,
    /// Digest consented to; for a web bundle the portal capability digest (portal-bundle README §7.2, A-10).
    pub capability_digest: String,
    pub verse_id: String,
    pub source_class: Option<String>,
}

/// One confirmation, three audit entries in order: `install`, `consent`, `bind` (bindings.md §5.5 step 3).
pub async fn record_offer_confirmation(db: &Db, actor_did: &str, confirmation: &OfferConfirmation) -> anyhow::Result<()> {
    let mut install = AuditEntry::verse(AuditAction::Install, actor_did, &confirmation.verse_id, AuditOutcome::Ok, None);
    install.installation = Some(confirmation.installation_id.clone());
    install.package_id = Some(confirmation.package_id.clone());
    install.content_hash = Some(confirmation.content_hash.clone());
    install.target = confirmation.installation_id.clone();
    append_audit(db, &install).await?;
    record_consent(
        db,
        &ConsentRecord {
            installation_id: confirmation.installation_id.clone(),
            package_id: confirmation.package_id.clone(),
            content_hash: confirmation.content_hash.clone(),
            capability_digest: confirmation.capability_digest.clone(),
            actor_did: actor_did.to_string(),
            at: String::new(),
        },
    )
    .await?;
    let record = AcceptanceRecord {
        installation_id: confirmation.installation_id.clone(),
        verse_id: confirmation.verse_id.clone(),
        source_class: confirmation.source_class.clone(),
        actor_did: actor_did.to_string(),
    };
    record_acceptance(db, actor_did, &record, &confirmation.package_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Kill-list override (bindings.md §5.7; OD-E1 user decision 2026-11-08)
// ---------------------------------------------------------------------------

/// The verse Owner's request to run one kill-listed build in one verse on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillListOverrideRequest {
    pub verse_id: String,
    pub package_id: String,
    pub content_hash: String,
    /// The typed confirmation: must equal `package_id` exactly.
    pub confirm: String,
}

/// A recorded override, keyed `(verse_id, content_hash)` on this host only; never replicated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KillListOverride {
    pub verse_id: String,
    pub content_hash: String,
    /// The Owner identity that recorded it; hydration keeps only the host's current DID (a rotated key never carries over).
    #[serde(default)]
    pub actor_did: String,
}

/// Why an override was refused.
#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    /// Malformed request (400 `invalid-input`); not audited.
    #[error("invalid-input: {0}")]
    Invalid(String),
    /// The confirmation does not equal the package id (409 `confirmation-mismatch`); audited `refused`.
    #[error("confirmation-mismatch: type the package id exactly to override the kill list")]
    ConfirmationMismatch,
    /// The local DID is not the verse Owner (403 `role-denied`); audited `refused`.
    #[error("role-denied: only the verse Owner, on the Owner's own host, may override the kill list")]
    NotOwner,
    #[error("kill-list override failed: {0:#}")]
    Db(#[from] anyhow::Error),
}

/// Record the Owner's override: `local_did` must be the verse's `owner_did` (`verse.created_by`).
pub async fn record_kill_list_override(
    db: &Db,
    local_did: &str,
    request: &KillListOverrideRequest,
) -> Result<KillListOverride, OverrideError> {
    if !is_package_id(&request.package_id) || !is_digest(&request.content_hash) {
        return Err(OverrideError::Invalid("package_id or content_hash is malformed".into()));
    }
    // An empty node DID would match a verse whose `created_by` is empty: no identity, no override.
    if local_did.is_empty() {
        return Err(OverrideError::NotOwner);
    }
    let mut entry = AuditEntry::verse(AuditAction::KillListOverride, local_did, &request.verse_id, AuditOutcome::Ok, None);
    entry.package_id = Some(request.package_id.clone());
    entry.content_hash = Some(request.content_hash.clone());
    entry.target = format!("{}@{}", request.package_id, request.verse_id);
    let refusal = if request.confirm != request.package_id {
        Some(OverrideError::ConfirmationMismatch)
    } else if crate::role_grants::verse_owner(db, &request.verse_id).await?.as_deref() != Some(local_did) {
        Some(OverrideError::NotOwner)
    } else {
        None
    };
    if let Some(refusal) = refusal {
        entry.outcome = AuditOutcome::Refused;
        entry.reason = Some(refusal.to_string());
        append_audit(db, &entry).await?;
        return Err(refusal);
    }
    append_audit(db, &entry).await?;
    Ok(KillListOverride {
        verse_id: request.verse_id.clone(),
        content_hash: request.content_hash.clone(),
        actor_did: local_did.to_string(),
    })
}

/// Every override recorded on this host, with the identity that recorded it (startup hydration filters by it).
pub async fn load_kill_list_overrides(db: &Db) -> anyhow::Result<Vec<KillListOverride>> {
    Ok(ok_entries(db, &[AuditAction::KillListOverride])
        .await?
        .into_iter()
        .filter_map(|entry| {
            Some(KillListOverride { verse_id: entry.verse_id?, content_hash: entry.content_hash?, actor_did: entry.actor_did })
        })
        .collect())
}
