//! Scoped extension bindings, DB plane (docs/extensions/bindings.md §3-§4): stored per-author versions, the
//! local write path, inbound admission and admissibility; rationale in src/AGENTS.md §extension-binding.

use std::collections::{BTreeMap, BTreeSet};

use fe_policy::{EffectiveRoles, RoleStatement};
use fe_runtime::extension_binding::{
    decode_binding_doc_key, hook_spec, is_valid_hook_id, key_allowed, scope_string,
    BindingCodecError, BindingMode, BindingRecord, HookKeyKind, HookTier, BINDING_DOC_PREFIX,
    BINDING_SCHEMA_VERSION, MAX_BINDING_PAYLOAD_BYTES,
};
use fe_runtime::messages::{BindingRefusal, ReplicatedRowOutcome, StoredBinding, VerseBindings};
use serde_json::{json, Value};

use super::extension_policy::{self, AuditAction, AuditEntry, AuditOutcome, PolicyMode};
use crate::op_log::commit_operation_atomic_with;
use crate::repo::Db;
use crate::role_level::RoleLevel;
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{BlobStoreHandle, ReplicationSender, ScopeParts};

/// DB table holding every stored binding version (one live version per doc key).
pub const BINDING_TABLE: &str = "extension_binding";
/// Doc table binding rows replicate under: `binding/{scope}/{hook}/{key}/{author_did}`.
pub const BINDING_DOC_TABLE: &str = BINDING_DOC_PREFIX;
/// Hooks whose built-in reads a `builtin` binding's `config` (§6: `petal.map` takes a TerrainConfig).
const BUILTIN_CONFIG_HOOKS: [&str; 1] = ["petal.map"];
/// Longest `runs_on` DID a local write accepts.
const MAX_DID_LEN: usize = 256;
/// The widest `written_at` (u64::MAX), so the size check before stamping can only over-count.
const WIDEST_CLOCK: &str = "18446744073709551615";

/// Table definitions `schema::apply_all` runs (idempotent).
pub const SCHEMA_STATEMENTS: [&str; 4] = [
    "DEFINE TABLE IF NOT EXISTS extension_binding SCHEMALESS",
    "DEFINE INDEX IF NOT EXISTS idx_extension_binding_key ON TABLE extension_binding FIELDS binding_key UNIQUE",
    "DEFINE INDEX IF NOT EXISTS idx_extension_binding_scope ON TABLE extension_binding FIELDS verse_id, scope",
    "DEFINE INDEX IF NOT EXISTS idx_extension_binding_author ON TABLE extension_binding FIELDS author_did",
];

/// Define the binding table (called from `schema::apply_all`; safe to repeat).
pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    for statement in SCHEMA_STATEMENTS {
        db.query(statement)
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("{BINDING_TABLE} schema: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors and small helpers
// ---------------------------------------------------------------------------

/// Why a binding command was refused; [`BindingCommandError::code`] is the bindings.md §4.2 error id.
#[derive(Debug, thiserror::Error)]
pub enum BindingCommandError {
    /// Shape rules (422 `invalid-binding`).
    #[error("invalid-binding: {0}")]
    Invalid(String),
    /// A hook this build does not serve, stretch hooks included (422 `unknown-hook`).
    #[error("unknown-hook: {0}")]
    UnknownHook(String),
    /// Unknown or foreign scope, or a remote caller with no role there (the uniform 404).
    #[error("not-found")]
    NotFound,
    /// The caller or the local DID is below Manager at the scope (403 `role-denied`).
    #[error("role-denied: {0}")]
    RoleDenied(String),
    /// The replicated verse policy refuses the package (403 `policy-denied`).
    #[error("policy-denied: {0}")]
    PolicyDenied(String),
    #[error("binding command failed: {0:#}")]
    Db(#[from] anyhow::Error),
}

impl BindingCommandError {
    /// Stable error id for REST/MCP replies.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid-binding",
            Self::UnknownHook(_) => "unknown-hook",
            Self::NotFound => "not-found",
            Self::RoleDenied(_) => "role-denied",
            Self::PolicyDenied(_) => "policy-denied",
            Self::Db(_) => "failed",
        }
    }

    /// The typed reply the DB loop sends back on a binding command's channel.
    pub fn refusal(&self) -> BindingRefusal {
        BindingRefusal {
            code: self.code().to_string(),
            message: self.to_string(),
        }
    }
}

/// A version's replicated record id: its doc key without the `binding/` table prefix.
pub fn binding_record_id(record: &BindingRecord) -> Result<String, BindingCodecError> {
    let doc_key = record.doc_key()?;
    doc_key
        .strip_prefix(BINDING_DOC_TABLE)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_string)
        .ok_or(BindingCodecError::DocKey(doc_key.clone()))
}

/// Last-writer-wins order at one point: HLC, then canonical payload bytes (fe-policy `latest_per_grantor`).
pub fn version_order(record: &BindingRecord) -> (u64, Vec<u8>) {
    (
        record.written_at.parse().unwrap_or(0),
        serde_json::to_vec(record).unwrap_or_default(),
    )
}

/// Manifest v2 `id` grammar (fe-sdk `is_package_id`): two or more lowercase labels, at most 128 bytes.
fn is_manifest_package_id(id: &str) -> bool {
    let label = |label: &str| {
        let mut chars = label.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    id.len() <= 128 && id.split('.').count() >= 2 && id.split('.').all(label)
}

/// A DID the doc-key codec can carry.
fn is_did(did: &str) -> bool {
    did.starts_with("did:")
        && did.len() <= MAX_DID_LEN
        && !did.contains('/')
        && !did.chars().any(char::is_whitespace)
}

fn mode_text(mode: BindingMode) -> &'static str {
    match mode {
        BindingMode::Extension => "extension",
        BindingMode::Builtin => "builtin",
        BindingMode::Inherit => "inherit",
    }
}

/// One-line audit reason naming the point, the choice and the scope.
fn describe(record: &BindingRecord) -> String {
    let point = if record.key.is_empty() {
        record.hook.clone()
    } else {
        format!("{}:{}", record.hook, record.key)
    };
    match (record.mode, record.package_id.as_deref()) {
        (BindingMode::Extension, Some(package_id)) => {
            format!("binding {point} -> {package_id} at {}", record.scope)
        }
        (BindingMode::Builtin, _) => format!("binding {point} -> built-in at {}", record.scope),
        _ => format!("binding {point} cleared at {}", record.scope),
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The stored row: index columns plus the canonical payload, which is the source of truth on read.
fn stored_row(verse_id: &str, record_id: &str, record: &BindingRecord) -> Value {
    let mut row = json!({
        "binding_key": record_id,
        "verse_id": verse_id,
        "scope": record.scope,
        "hook": record.hook,
        "author_did": record.author_did,
        "written_at": record.written_at,
        "mode": mode_text(record.mode),
        "payload": serde_json::to_string(record).unwrap_or_default(),
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(package_id) = &record.package_id {
        row["package_id"] = json!(package_id);
    }
    row
}

async fn select_records(
    db: &Db,
    sql: &str,
    binds: Vec<(&'static str, String)>,
) -> anyhow::Result<Vec<BindingRecord>> {
    let mut query = db.query(sql.to_string());
    for bind in binds {
        query = query.bind(bind);
    }
    let mut response = query
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("{BINDING_TABLE} read failed: {e}"))?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let parsed = row
                .get("payload")
                .and_then(Value::as_str)
                .and_then(|payload| serde_json::from_str::<BindingRecord>(payload).ok());
            if parsed.is_none() {
                tracing::warn!(?row, "skipping malformed {BINDING_TABLE} row");
            }
            parsed
        })
        .collect())
}

/// The live version stored under one record id.
async fn stored_version(db: &Db, record_id: &str) -> anyhow::Result<Option<BindingRecord>> {
    let sql =
        format!("SELECT payload FROM {BINDING_TABLE} WHERE binding_key = $binding_key LIMIT 1");
    Ok(
        select_records(db, &sql, vec![("binding_key", record_id.to_string())])
            .await?
            .into_iter()
            .next(),
    )
}

/// Make `record` its doc key's one live version (one transaction).
async fn write_version(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    record: &BindingRecord,
) -> anyhow::Result<()> {
    db.query(format!(
        "BEGIN TRANSACTION;\n\
         DELETE {BINDING_TABLE} WHERE binding_key = $binding_key;\n\
         CREATE {BINDING_TABLE} CONTENT $binding_row;\n\
         COMMIT TRANSACTION;"
    ))
    .bind(("binding_key", record_id.to_string()))
    .bind(("binding_row", stored_row(verse_id, record_id, record)))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("writing {BINDING_TABLE} version {record_id} failed: {e}"))?;
    Ok(())
}

/// The verse exists, the fractal belongs to it and the petal to the fractal (no interim verse reaches here).
async fn scope_is_known(db: &Db, parts: &ScopeParts) -> anyhow::Result<bool> {
    let mut response = db
        .query(
            "SELECT verse_id FROM verse WHERE verse_id = $binding_verse LIMIT 1; \
             SELECT verse_id FROM fractal WHERE fractal_id = $binding_fractal LIMIT 1; \
             SELECT fractal_id FROM petal WHERE petal_id = $binding_petal LIMIT 1;",
        )
        .bind(("binding_verse", parts.verse_id.clone()))
        .bind((
            "binding_fractal",
            parts.fractal_id.clone().unwrap_or_default(),
        ))
        .bind(("binding_petal", parts.petal_id.clone().unwrap_or_default()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("binding scope lookup failed: {e}"))?;
    let verse: Vec<Value> = response.take(0)?;
    let fractal: Vec<Value> = response.take(1)?;
    let petal: Vec<Value> = response.take(2)?;
    let field = |rows: &[Value], name: &str| {
        rows.first()
            .and_then(|row| row.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Ok(!verse.is_empty()
        && (parts.fractal_id.is_none()
            || field(&fractal, "verse_id").as_deref() == Some(parts.verse_id.as_str()))
        && (parts.petal_id.is_none() || field(&petal, "fractal_id") == parts.fractal_id))
}

/// Petals of the verse whose own `terrain` column is set: an implicit petal-scope `builtin` for `petal.map` (OQ-B6).
async fn stored_terrain_petals(db: &Db, verse_id: &str) -> anyhow::Result<BTreeSet<String>> {
    let mut response = db
        .query("SELECT VALUE fractal_id FROM fractal WHERE verse_id = $binding_verse")
        .bind(("binding_verse", verse_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("binding fractal lookup failed: {e}"))?;
    let fractal_ids: Vec<Value> = response.take(0)?;
    if fractal_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut response = db
        .query("SELECT VALUE petal_id FROM petal WHERE fractal_id IN $binding_fractals AND terrain != NONE AND terrain != NULL")
        .bind(("binding_fractals", fractal_ids))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("binding terrain lookup failed: {e}"))?;
    let petal_ids: Vec<Value> = response.take(0)?;
    Ok(petal_ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

// ---------------------------------------------------------------------------
// Admissibility (§4.4)
// ---------------------------------------------------------------------------

/// §4.4 per version: admissible iff the author held Manager+ at the scope when it wrote AND holds it now; the
/// newest admissible version per (scope, point) is effective (an effective `inherit` means no binding there).
/// `written_authorized` is the write-time half, true when the current check alone decides (owner, no statements).
pub fn judge_versions(
    owner: Option<&str>,
    roles: Option<&EffectiveRoles>,
    admitted: &[RoleStatement],
    records: Vec<BindingRecord>,
) -> Vec<StoredBinding> {
    let mut versions: Vec<StoredBinding> = records
        .into_iter()
        .map(|record| {
            let (written_authorized, held_now) = match (owner, roles) {
                (Some(owner), Some(roles)) => (
                    crate::role_grants::authorized_at_write(
                        owner,
                        roles,
                        admitted,
                        &record.author_did,
                        &record.scope,
                        RoleLevel::Manager,
                        record.written_at.parse::<u64>().unwrap_or(0),
                    ),
                    roles
                        .authority(&record.author_did, &record.scope)
                        .is_at_least(RoleLevel::Manager),
                ),
                // Pre-manifest nobody holds authority.
                _ => (false, false),
            };
            StoredBinding {
                admissible: written_authorized && held_now,
                written_authorized,
                effective: false,
                record,
            }
        })
        .collect();
    let mut newest: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for (index, version) in versions
        .iter()
        .enumerate()
        .filter(|(_, version)| version.admissible)
    {
        let point = (
            version.record.scope.clone(),
            version.record.hook.clone(),
            version.record.key.clone(),
        );
        let newer = newest.get(&point).is_none_or(|&best| {
            version_order(&version.record) > version_order(&versions[best].record)
        });
        if newer {
            newest.insert(point, index);
        }
    }
    for index in newest.into_values() {
        versions[index].effective = true;
    }
    versions
}

/// Judge `records` of one verse against its current statements.
async fn judge_verse(
    db: &Db,
    verse_id: &str,
    records: Vec<BindingRecord>,
) -> anyhow::Result<(Option<EffectiveRoles>, Vec<StoredBinding>)> {
    let owner = crate::role_grants::verse_owner(db, verse_id).await?;
    let admitted = crate::role_grants::load_admitted(db, verse_id).await?;
    let roles = owner
        .as_deref()
        .map(|owner| fe_policy::materialize(owner, admitted.iter()));
    let versions = judge_versions(owner.as_deref(), roles.as_ref(), &admitted, records);
    Ok((roles, versions))
}

/// One verse's binding plane for the per-peer resolver (bindings.md §5): every version judged, the current
/// roles and the stored-terrain petals. Re-run per re-resolution trigger, never per frame.
pub async fn load_binding_snapshot(db: &Db, verse_id: &str) -> anyhow::Result<VerseBindings> {
    let sql = format!("SELECT payload FROM {BINDING_TABLE} WHERE verse_id = $binding_verse");
    let records = select_records(db, &sql, vec![("binding_verse", verse_id.to_string())]).await?;
    let (roles, versions) = judge_verse(db, verse_id, records).await?;
    Ok(VerseBindings {
        verse_id: verse_id.to_string(),
        versions,
        roles,
        stored_terrain_petals: stored_terrain_petals(db, verse_id).await?,
    })
}

/// Every stored version at exactly `scope`, judged (bindings.md §9 `GET /bindings`, withdrawn versions
/// included). A remote caller (`Some`) needs Viewer+ there; `None` is the local DID. Otherwise the uniform 404.
pub async fn list_bindings(
    db: &Db,
    scope: &str,
    caller_did: Option<&str>,
) -> Result<Vec<StoredBinding>, BindingCommandError> {
    let parts = crate::parse_scope(scope).map_err(|_| BindingCommandError::NotFound)?;
    if scope_string(&parts).as_deref() != Some(scope)
        || crate::role_grants::verse_of_scope(scope).is_none()
        || !scope_is_known(db, &parts).await?
    {
        return Err(BindingCommandError::NotFound);
    }
    if let Some(caller) = caller_did {
        if !crate::role_manager::resolve_role(db, caller, scope)
            .await?
            .is_at_least(RoleLevel::Viewer)
        {
            return Err(BindingCommandError::NotFound);
        }
    }
    let sql = format!("SELECT payload FROM {BINDING_TABLE} WHERE verse_id = $binding_verse AND scope = $binding_scope");
    let records = select_records(
        db,
        &sql,
        vec![
            ("binding_verse", parts.verse_id.clone()),
            ("binding_scope", scope.to_string()),
        ],
    )
    .await?;
    Ok(judge_verse(db, &parts.verse_id, records).await?.1)
}

// ---------------------------------------------------------------------------
// Local write path (§4.2)
// ---------------------------------------------------------------------------

/// Write one binding as `local_did` (§4.2): shape, scope and role, verse policy, then one atomic op-log + row
/// commit and the publish. `template.author_did`/`written_at` are ignored (host-stamped); `caller_did` is the
/// REST/MCP token DID (`None` = the local DID on the GUI). `mode: inherit` clears. A local write never defers.
pub async fn put_binding(
    db: &Db,
    local_did: &str,
    caller_did: Option<&str>,
    template: BindingRecord,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<BindingRecord, BindingCommandError> {
    let (parts, record) = shape_local_write(local_did, template)?;
    if !scope_is_known(db, &parts).await? {
        return Err(BindingCommandError::NotFound);
    }
    if let Some(caller) = caller_did.filter(|caller| *caller != local_did) {
        let role = crate::role_manager::resolve_role(db, caller, &record.scope).await?;
        if !role.is_at_least(RoleLevel::Viewer) {
            return Err(BindingCommandError::NotFound);
        }
        if !role.is_at_least(RoleLevel::Manager) {
            return refuse_role(db, caller, &parts.verse_id, &record, role).await;
        }
    }
    // The local DID authors the row, so it must hold Manager+ too (the `SetPetalTerrain { grant }` pattern).
    let node_role = crate::role_manager::resolve_role(db, local_did, &record.scope).await?;
    if !node_role.is_at_least(RoleLevel::Manager) {
        return refuse_role(db, local_did, &parts.verse_id, &record, node_role).await;
    }
    if let (BindingMode::Extension, Some(package_id)) = (record.mode, record.package_id.as_deref())
    {
        check_verse_policy(db, &parts.verse_id, package_id).await?;
    }
    commit_local(db, &parts, record, blob_store, repl_tx).await
}

/// §4.2 step 1 on a template: the record to stamp, plus its parsed scope.
fn shape_local_write(
    local_did: &str,
    template: BindingRecord,
) -> Result<(ScopeParts, BindingRecord), BindingCommandError> {
    let invalid = BindingCommandError::Invalid;
    let parts = crate::parse_scope(&template.scope)
        .map_err(|e| invalid(format!("scope '{}': {e}", template.scope)))?;
    if scope_string(&parts).as_deref() != Some(template.scope.as_str()) {
        return Err(invalid(format!(
            "scope '{}' is not a canonical verse, fractal or petal scope",
            template.scope
        )));
    }
    // An interim or `#`-smuggling id names no hierarchy scope here: the uniform 404, never a shape hint.
    if crate::role_grants::verse_of_scope(&template.scope).is_none() {
        return Err(BindingCommandError::NotFound);
    }
    let Some(spec) = hook_spec(&template.hook).filter(|spec| spec.tier == HookTier::V1) else {
        return Err(if is_valid_hook_id(&template.hook) {
            BindingCommandError::UnknownHook(template.hook)
        } else {
            invalid(format!("hook id '{}'", template.hook))
        });
    };
    if !key_allowed(spec, &template.key) {
        return Err(invalid(format!(
            "key '{}' does not fit hook {}",
            template.key, spec.id
        )));
    }
    if spec.key_kind != HookKeyKind::None && template.key.is_empty() {
        return Err(invalid(format!("hook {} needs a key", spec.id)));
    }
    let config_empty = template
        .config
        .as_object()
        .is_some_and(|object| object.is_empty());
    match template.mode {
        BindingMode::Extension => {
            let package_id = template.package_id.as_deref().unwrap_or_default();
            if !is_manifest_package_id(package_id) {
                return Err(invalid(format!(
                    "package_id '{package_id}' is not a manifest v2 id"
                )));
            }
            if let Some(requirement) = template.version_req.as_deref() {
                if !extension_policy::is_version_requirement(requirement) {
                    return Err(invalid(format!(
                        "version_req '{requirement}' is not a SemVer requirement"
                    )));
                }
            }
        }
        BindingMode::Builtin if !config_empty && !BUILTIN_CONFIG_HOOKS.contains(&spec.id) => {
            return Err(invalid(format!("the built-in {} takes no config", spec.id)));
        }
        BindingMode::Inherit if !config_empty || template.runs_on.is_some() => {
            return Err(invalid(
                "a clear (mode inherit) carries no config or runs_on".to_string(),
            ));
        }
        BindingMode::Builtin | BindingMode::Inherit => {}
    }
    if let Some(did) = template.runs_on.as_deref() {
        if !is_did(did) {
            return Err(invalid(format!("runs_on '{did}' is not a DID")));
        }
    }
    let record = BindingRecord {
        schema_version: BINDING_SCHEMA_VERSION,
        author_did: local_did.to_string(),
        written_at: WIDEST_CLOCK.to_string(),
        ..template
    };
    record
        .validate_shape()
        .map_err(|e| invalid(e.to_string()))?;
    record.doc_key().map_err(|e| invalid(e.to_string()))?;
    Ok((parts, record))
}

/// §4.2 step 3, package level only: a replicated deny entry, or an allow-list with no entry for the package.
/// Build- and host-dependent checks (trust roots, digests, kinds, local policy) wait for resolution.
async fn check_verse_policy(
    db: &Db,
    verse_id: &str,
    package_id: &str,
) -> Result<(), BindingCommandError> {
    let Some(row) = extension_policy::get_policy(db, verse_id).await? else {
        return Ok(());
    };
    if row.policy.deny.iter().any(|entry| entry == package_id) {
        return Err(BindingCommandError::PolicyDenied(format!(
            "the verse policy denies {package_id}"
        )));
    }
    if row.policy.mode == PolicyMode::AllowList
        && !row
            .policy
            .allow
            .iter()
            .any(|entry| entry.package_id == package_id)
    {
        return Err(BindingCommandError::PolicyDenied(format!(
            "the verse allow-list has no entry for {package_id}"
        )));
    }
    Ok(())
}

/// Audit a Manager+ refusal (security-relevant, governance §Audit) and return it.
async fn refuse_role<T>(
    db: &Db,
    actor: &str,
    verse_id: &str,
    record: &BindingRecord,
    role: RoleLevel,
) -> Result<T, BindingCommandError> {
    let reason = format!(
        "{actor} holds {role} at {}; a binding needs Manager+",
        record.scope
    );
    let mut audit = AuditEntry::verse(
        AuditAction::PolicyChange,
        actor,
        verse_id,
        AuditOutcome::Refused,
        Some(reason.clone()),
    );
    audit.target = record.doc_key().unwrap_or_else(|_| record.scope.clone());
    audit.package_id = record.package_id.clone();
    extension_policy::append_audit(db, &audit).await?;
    Err(BindingCommandError::RoleDenied(reason))
}

/// §4.2 step 4: one `ExtensionAudit` op and the row in ONE transaction (`written_at` = the op's HLC), then publish.
async fn commit_local(
    db: &Db,
    parts: &ScopeParts,
    record: BindingRecord,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<BindingRecord, BindingCommandError> {
    let record_id =
        binding_record_id(&record).map_err(|e| BindingCommandError::Invalid(e.to_string()))?;
    let mut audit = AuditEntry::verse(
        AuditAction::PolicyChange,
        &record.author_did,
        &parts.verse_id,
        AuditOutcome::Ok,
        Some(describe(&record)),
    );
    audit.target = format!("{BINDING_DOC_TABLE}/{record_id}");
    audit.package_id = record.package_id.clone();
    audit.at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(format!("ext:{}", audit.target)),
        op_type: OpType::ExtensionAudit,
        payload: serde_json::to_value(&audit).map_err(anyhow::Error::from)?,
        sig: "00".repeat(64),
    };
    let verse_id = parts.verse_id.clone();
    let key = record_id.clone();
    let template = record.clone();
    let (lamport, _) = commit_operation_atomic_with(db, entry, move |hlc| {
        let stamped = BindingRecord {
            written_at: hlc.to_string(),
            ..template
        };
        (
            vec![
                format!("DELETE {BINDING_TABLE} WHERE binding_key = $binding_key"),
                format!("CREATE {BINDING_TABLE} CONTENT $binding_row"),
            ],
            vec![
                ("binding_key".to_string(), json!(key)),
                (
                    "binding_row".to_string(),
                    stored_row(&verse_id, &key, &stamped),
                ),
            ],
        )
    })
    .await?;
    let stamped = BindingRecord {
        written_at: lamport.to_string(),
        ..record
    };
    // Durable first, then publish (§device-payload precedent).
    match (blob_store, serde_json::to_vec(&stamped)) {
        (Some(store), Ok(bytes)) => crate::replicate_row_with_petal(
            repl_tx,
            store,
            &parts.verse_id,
            BINDING_DOC_TABLE,
            &record_id,
            &bytes,
            parts.petal_id.clone(),
        ),
        (None, _) if repl_tx.is_some() => {
            tracing::warn!(verse_id = %parts.verse_id, record_id = %record_id, "binding written: blob store missing, not published");
        }
        (_, Err(e)) => {
            tracing::warn!(record_id = %record_id, "binding written but not serialised for replication: {e}")
        }
        _ => {}
    }
    Ok(stamped)
}

/// **Keep binding** (§4.5): `version` re-authored as the clicking Manager, `runs_on` pinned to the version's
/// effective placement (a one-peer hook reads `null` as the original author), so placement never moves.
pub fn kept_template(version: &BindingRecord) -> BindingRecord {
    let one_peer = hook_spec(&version.hook).is_some_and(|spec| spec.one_peer);
    let runs_on = match (version.mode, &version.runs_on) {
        (BindingMode::Inherit, _) => None,
        (_, Some(did)) => Some(did.clone()),
        (_, None) => one_peer.then(|| version.author_did.clone()),
    };
    BindingRecord {
        runs_on,
        author_did: String::new(),
        written_at: String::new(),
        ..version.clone()
    }
}

/// Re-author the stored version `record_id` as `local_did` through [`put_binding`] (every §4.2 check runs).
pub async fn keep_binding(
    db: &Db,
    local_did: &str,
    caller_did: Option<&str>,
    record_id: &str,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<BindingRecord, BindingCommandError> {
    let Some(version) = stored_version(db, record_id).await? else {
        return Err(BindingCommandError::NotFound);
    };
    put_binding(
        db,
        local_did,
        caller_did,
        kept_template(&version),
        blob_store,
        repl_tx,
    )
    .await
}

/// Re-publish every live version this node authored (iroh dedupes identical bytes): the second chance for a
/// publish dropped on a full bridge, run beside the role-statement re-emit at startup and replica open.
pub async fn reemit_local_bindings(
    db: &Db,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    local_did: &str,
    verse_id: Option<&str>,
) -> anyhow::Result<usize> {
    if repl_tx.is_none() {
        return Ok(0);
    }
    let records = match verse_id {
        Some(verse) => {
            let sql = format!("SELECT payload FROM {BINDING_TABLE} WHERE verse_id = $binding_verse AND author_did = $binding_author");
            select_records(
                db,
                &sql,
                vec![
                    ("binding_verse", verse.to_string()),
                    ("binding_author", local_did.to_string()),
                ],
            )
            .await?
        }
        None => {
            let sql =
                format!("SELECT payload FROM {BINDING_TABLE} WHERE author_did = $binding_author");
            select_records(db, &sql, vec![("binding_author", local_did.to_string())]).await?
        }
    };
    let mut emitted = 0;
    for record in &records {
        let (Ok(record_id), Some(verse), Ok(bytes)) = (
            binding_record_id(record),
            crate::role_grants::verse_of_scope(&record.scope),
            serde_json::to_vec(record),
        ) else {
            continue;
        };
        crate::replicate_row(
            repl_tx,
            blob_store,
            &verse,
            BINDING_DOC_TABLE,
            &record_id,
            &bytes,
        );
        emitted += 1;
    }
    Ok(emitted)
}

// ---------------------------------------------------------------------------
// Inbound admission (§4.3)
// ---------------------------------------------------------------------------

/// Structural admission of an inbound row (§4.3 `Denied`): the key and payload parse and agree, the payload's
/// author is the attested author, the scope is a hierarchy scope of this verse; `Err` is the deny reason.
pub fn confine_inbound(
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> Result<BindingRecord, String> {
    if row_bytes.len() > MAX_BINDING_PAYLOAD_BYTES {
        return Err(format!(
            "binding row is over the {MAX_BINDING_PAYLOAD_BYTES}-byte cap"
        ));
    }
    let doc_key = format!("{BINDING_DOC_TABLE}/{record_id}");
    let (parts, _, key_author) =
        decode_binding_doc_key(&doc_key).map_err(|e| format!("binding key: {e}"))?;
    let record: BindingRecord = serde_json::from_slice(row_bytes)
        .map_err(|e| format!("binding row does not parse: {e}"))?;
    record
        .validate_shape()
        .map_err(|e| format!("binding row: {e}"))?;
    // Equal doc keys: the payload's scope, hook, key and author are the key's own (canonical encoding only).
    if record.doc_key().ok().as_deref() != Some(doc_key.as_str()) {
        return Err("binding payload differs from its doc key".to_string());
    }
    if key_author != author_did {
        return Err(format!(
            "binding author {key_author} is not the attested author {author_did}"
        ));
    }
    if parts.verse_id != verse_id
        || crate::role_grants::verse_of_scope(&record.scope).as_deref() != Some(verse_id)
    {
        return Err("binding scope is not a hierarchy scope of this verse".to_string());
    }
    if !record.written_at.parse::<u64>().is_ok_and(|hlc| hlc > 0) {
        return Err("binding written_at is not a positive u64".to_string());
    }
    Ok(record)
}

/// Row-mode apply after the Manager+-now gate (§4.3 `Applied`): bounded author clock, then one live version per
/// doc key, newest by [`version_order`]. Admissibility is judged on read (§4.4), so a version is stored even when
/// its hook or package is unknown here. An empty `del` entry is NotApplicable (clearing writes `inherit`).
pub async fn apply_replicated_binding_row(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<ReplicatedRowOutcome> {
    if row_bytes.is_empty() {
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let record = match confine_inbound(verse_id, record_id, row_bytes, author_did) {
        Ok(record) => record,
        Err(reason) => {
            tracing::warn!(verse_id, record_id, author = author_did, %reason, "inbound binding row denied");
            return Ok(ReplicatedRowOutcome::Denied);
        }
    };
    // An author stamps its own clock: a far-future version would win last-writer-wins at its point forever.
    let Ok(clock) = crate::op_log::admit_scene_hlc(record.written_at.parse().unwrap_or(0)) else {
        tracing::warn!(verse_id, record_id, author = author_did, written_at = %record.written_at, "binding clock outside the five-minute future bound; denied");
        return Ok(ReplicatedRowOutcome::Denied);
    };
    if let Some(current) = stored_version(db, record_id).await? {
        if version_order(&current) >= version_order(&record) {
            return Ok(ReplicatedRowOutcome::Applied);
        }
    }
    write_version(db, verse_id, record_id, &record).await?;
    // The next local write orders after the version it may replace.
    crate::op_log::observe_scene_hlc(clock);
    Ok(ReplicatedRowOutcome::Applied)
}
