use fe_runtime::messages::{DbCommand, DbResult};
use surrealdb::engine::local::SurrealKv;

/// Error type for database thread initialization failures.
#[derive(Debug)]
pub enum DbInitError {
    RuntimeBuild(std::io::Error),
    SurrealOpen {
        path: String,
        source: surrealdb::Error,
    },
    SurrealNsDb(surrealdb::Error),
    SchemaApply(String),
    ApiTokenSchemaApply(String),
    SceneClockRecovery(String),
}

impl std::fmt::Display for DbInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbInitError::RuntimeBuild(e) => write!(f, "Failed to build Tokio runtime: {e}"),
            DbInitError::SurrealOpen { path, source } => {
                write!(f, "Failed to open SurrealDB at {path}: {source}")
            }
            DbInitError::SurrealNsDb(e) => {
                write!(f, "Failed to set SurrealDB namespace/database: {e}")
            }
            DbInitError::SceneClockRecovery(error) => write!(f,"Scene clock recovery failed: {error}"),
            DbInitError::SchemaApply(e) => write!(f, "Schema application failed: {e}"),
            DbInitError::ApiTokenSchemaApply(e) => {
                write!(f, "API token schema application failed: {e}")
            }
        }
    }
}

impl std::error::Error for DbInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbInitError::RuntimeBuild(e) => Some(e),
            DbInitError::SurrealOpen { source, .. } => Some(source),
            DbInitError::SurrealNsDb(e) => Some(e),
            _ => None,
        }
    }
}

// P2P Mycelium Phase A: re-export blob store types so callers (fe-ui, the
// binary crate, and tests) can depend only on `fe-database` for the full
// asset-pipeline surface.
pub use fe_runtime::blob_store::{
    hash_from_hex, hash_to_hex, BlobHash, BlobStore, BlobStoreHandle,
};
/// M2/F6: canonical timeseries settings types (fe-runtime is the lowest
/// common dependency — the same split as `RoleLevel`).
pub use fe_runtime::timeseries::{
    TimeseriesMode, VerseTimeseriesSettings, DEFAULT_BUCKET_WIDTH_MS,
};

pub mod admin;
pub mod api_auth;

/// SQL guard statements plus their named bindings, appended inside one transaction.
pub type GuardedStatements = (Vec<String>, Vec<(String, serde_json::Value)>);
pub mod api_token_store;
pub mod atlas;
pub mod canon_log;
pub mod handlers;
pub mod invite;
mod join_authority;
pub mod join_delegation;
pub mod join_invite;
pub mod join_ticket;
pub mod merge;
pub mod migration;
pub mod model_url_meta;
pub mod op_log;
pub mod queries;
pub(crate) mod query_helpers;
pub mod rbac;
pub mod reconcile;
pub mod replication_mode;
pub mod repo;
pub mod role_grants;
pub mod role_level;
pub mod role_manager;
pub mod schema;
pub mod scope;
pub mod scoped_invite;
pub mod session_cache;
pub mod space_manager;
pub mod types;
pub mod verse;

pub use atlas::*;
pub use handlers::seed::seed_default_data;
pub use model_url_meta::ModelUrlMeta;
pub use replication_mode::ReplicationMode;
pub use role_level::RoleLevel;
// `scope`'s parser now lives in fe-policy (see fe-policy/AGENTS.md §scope-parser); these names are unchanged.
pub use scope::{build_scope, parent_scope, parse_scope, scope_contains, ScopeParts};
pub use types::*;

/// Legacy `fractalengine/assets/` subdirectory of pre-blob-store GLB imports —
/// source for the one-time startup backfill only (src/AGENTS.md §asset-resolution).
pub const IMPORTED_ASSETS_SUBDIR: &str = "imported";

/// Legacy imports directory (relative to CWD), read only by the one-time
/// blob-store backfill; nothing writes or serves from it any more.
pub fn imported_assets_dir() -> std::path::PathBuf {
    std::path::Path::new("fractalengine")
        .join("assets")
        .join(IMPORTED_ASSETS_SUBDIR)
}

#[derive(bevy::prelude::Resource, Clone)]
pub struct DbHandle(pub std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>);

/// Bevy resource wrapping an `Arc<dyn SecretStore>` so ECS systems can
/// access the secret store (e.g. for namespace secret lookups).
#[derive(bevy::prelude::Resource, Clone)]
pub struct SecretStoreRes(pub std::sync::Arc<dyn fe_identity::SecretStore>);

/// A replication event emitted by the DB thread when a replicated row is written.
/// Defined here (not in fe-sync) to avoid a circular dependency.
/// The binary crate bridges these into `SyncCommand::WriteRowEntry`.
///
/// `petal_id` is INFORMATIONAL at the bridge (F23 disclosure): both binaries'
/// DB→sync bridges forward only `{verse_id, table, record_id, content_hash}`
/// because `WriteRowEntry` carries no `petal_id` — downstream consumers
/// re-derive the petal from the row payload (the shard model reads it off the
/// row bytes), so nothing breaks, but no consumer may assume event-borne
/// petal routing works. Threading it through `WriteRowEntry` is the follow-up
/// shape if a future feature needs it (recorded in the F23 handoff).
#[derive(Debug, Clone)]
pub struct ReplicationEvent {
    pub verse_id: String,
    pub table: String,
    pub record_id: String,
    pub content_hash: BlobHash,
    pub petal_id: Option<String>,
}

/// Sender half for replication events.
pub type ReplicationSender = crossbeam::channel::Sender<ReplicationEvent>;

/// Events dropped because the replication bridge was full (see AGENTS.md §replication-backpressure).
static REPLICATION_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total replication events dropped due to a full bridge channel.
pub fn replication_drop_count() -> u64 {
    REPLICATION_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Count one replication event dropped on a full bridge; returns the new total.
pub(crate) fn note_replication_drop() -> u64 {
    REPLICATION_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Serialise row data as JSON, write to blob store, and send a
/// `ReplicationEvent` to be bridged to the sync thread.
///
/// # Arguments
/// * `repl_tx` — optional replication sender (None = replication disabled)
/// * `blob_store` — shared blob store for content-addressed row data
/// * `verse_id` — the verse owning this row
/// * `table` — SurrealDB table name
/// * `record_id` — the row's ULID
/// * `row_json` — serialised row as JSON bytes
pub fn replicate_row(
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_json: &[u8],
) {
    replicate_row_with_petal(
        repl_tx, blob_store, verse_id, table, record_id, row_json, None,
    )
}

/// Like [`replicate_row`] but attaches the row's `petal_id` to the event —
/// informational at the bridge (see [`ReplicationEvent`]); the emission
/// seam's tests assert it, and a future petal-routed transport consumes it.
pub fn replicate_row_with_petal(
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_json: &[u8],
    petal_id: Option<String>,
) {
    let Some(tx) = repl_tx else { return };

    let hash = match blob_store.add_blob(row_json) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(
                "replicate_row: blob_store.add_blob failed for {table}/{record_id}: {e}"
            );
            return;
        }
    };

    // Never block the DB thread on a stalled sync thread: drop-and-count on Full
    // (see AGENTS.md §replication-backpressure).
    match tx.try_send(ReplicationEvent {
        verse_id: verse_id.to_string(),
        table: table.to_string(),
        record_id: record_id.to_string(),
        content_hash: hash,
        petal_id,
    }) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            let dropped = REPLICATION_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            tracing::warn!(
                table,
                record_id,
                dropped_total = dropped,
                "replication bridge full — dropping event"
            );
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {}
    }
}

// ---------------------------------------------------------------------------
// Node-lifecycle dispatch helpers (node_lifecycle_addressing_20260725)
// ---------------------------------------------------------------------------

/// Map a role-bearing `CallerAuth` to a `fe_policy::AuthContext` for enforcement
/// (N-5: authz lives in fe-policy/fe-database, never the UI). The role string is
/// parsed through the canonical `RoleLevel::from` (unknown/empty → `None`).
/// `Local` carries no role — it is scope-resolved asynchronously by
/// `lifecycle_auth_context`; a `Local` reaching this synchronous mapper is
/// unresolved, so it maps deny-safe to `Anonymous`.
fn caller_auth_to_context(auth: &fe_runtime::messages::CallerAuth) -> fe_policy::AuthContext {
    use fe_runtime::messages::CallerAuth;
    match auth {
        CallerAuth::Identified { did, role } => fe_policy::AuthContext::Did {
            did: did.clone(),
            role: fe_policy::RoleLevel::from(role.as_str()),
        },
        CallerAuth::Local | CallerAuth::Anonymous => fe_policy::AuthContext::Anonymous,
    }
}

/// Resolve the caller's `AuthContext` for a lifecycle mutation at `scope`.
/// A `Local` caller asserts no role, so the DB thread resolves the local
/// user's real role at the scope (N-5: no UI-side role assertion). An
/// `Identified` (API) caller's already-validated token role is capped at the
/// node's own converged role — `min(token, role_at(local_did, scope))`, AC-46
/// — so a token minted before a remote demotion cannot keep writing rows every
/// peer defers. A failed lookup is `none` → denied. See
/// fe-database/src/AGENTS.md §Authorization (N-5).
async fn lifecycle_auth_context(
    db: &crate::repo::Db,
    local_did: &str,
    auth: &fe_runtime::messages::CallerAuth,
    scope: &str,
) -> fe_policy::AuthContext {
    use fe_runtime::messages::CallerAuth;
    match auth {
        CallerAuth::Local => fe_policy::AuthContext::Did {
            did: local_did.to_string(),
            role: node_role_at(db, local_did, scope).await,
        },
        // AC-46: an API token's role is capped by the node's own converged role.
        CallerAuth::Identified { did, role } => fe_policy::AuthContext::Did {
            did: did.clone(),
            role: fe_policy::RoleLevel::from(role.as_str())
                .min(node_role_at(db, local_did, scope).await),
        },
        CallerAuth::Anonymous => caller_auth_to_context(auth),
    }
}

/// This node's own converged role at `scope` (`role_at` over the materialized
/// cache) — what every peer's A3 gate judges the rows this node publishes by.
/// The ONE helper behind the replicated-write cap (role propagation AC-46,
/// src/AGENTS.md §handlers "replicated_row"). A failed lookup is `None`.
async fn node_role_at(db: &crate::repo::Db, local_did: &str, scope: &str) -> fe_policy::RoleLevel {
    match handlers::rbac::resolve_local_role_handler(db, local_did, scope).await {
        Ok(role) => fe_policy::RoleLevel::from(role.as_str()),
        Err(e) => {
            tracing::warn!("node role resolution failed for {scope}: {e}");
            fe_policy::RoleLevel::None
        }
    }
}

/// AC-46 for the role-less replicated-write doors (IoT readings, device
/// payloads, device formats): the API validated the token's role upstream; the
/// effective role is `min(token, node)`, so the write also needs the NODE to
/// clear `floor` at `scope` — otherwise it would apply locally while every peer
/// defers it. `Err` carries the refusal for the caller's reply.
async fn require_node_role(
    db: &crate::repo::Db,
    local_did: &str,
    scope: &str,
    floor: fe_policy::RoleLevel,
    what: &str,
) -> Result<(), String> {
    let role = node_role_at(db, local_did, scope).await;
    if role.is_at_least(floor) {
        Ok(())
    } else {
        Err(format!(
            "{what} refused: this node holds {role} at {scope}, below {floor} — peers would defer its rows"
        ))
    }
}

/// AC-46 for the granted object doors (QA-0122): an API token's role claim is
/// capped by THIS node's stored role at `scope`, exactly like a Local command
/// (which its handler judges); the claim alone never authorizes a write.
async fn require_granted_node_role(
    db: &crate::repo::Db,
    grant: &fe_runtime::node_history::NodeAccessGrant,
    local_did: &str,
    scope: Option<String>,
    floor: fe_policy::RoleLevel,
    what: &str,
) -> Result<(), String> {
    if matches!(grant, fe_runtime::node_history::NodeAccessGrant::Local) {
        return Ok(());
    }
    let scope = scope.ok_or_else(|| format!("{what} refused: not found or not permitted"))?;
    require_node_role(db, local_did, &scope, floor, what).await
}

/// Local command actor is the resolved application identity, never the supplied label.
fn object_command_actor<'a>(
    grant: &fe_runtime::node_history::NodeAccessGrant,
    local_did: &'a str,
    asserted_actor: &'a str,
) -> &'a str {
    match grant {
        fe_runtime::node_history::NodeAccessGrant::Local => local_did,
        fe_runtime::node_history::NodeAccessGrant::Scoped(_) => asserted_actor,
    }
}

/// The scope a replicated petal write lands in, when the petal is known.
async fn replicated_petal_scope(
    db: &crate::repo::Db,
    verse_id: Option<&str>,
    petal_id: &str,
) -> Option<String> {
    verse_id?;
    handlers::crud::resolve_petal_scope_handler(db, petal_id)
        .await
        .ok()
        .flatten()
}

/// Build the stable `fe://verse/fractal/petal/node` URI (FR-4) from a petal
/// scope string + node id. Mirrors `fe_entity_store::NodeAddress::to_uri`
/// byte-for-byte (kept local to avoid a new crate dependency; the two encoders
/// must stay in lock-step so T5's round-trip holds).
fn lifecycle_uri(scope: &str, node_id: &str) -> Option<String> {
    let rest = scope.strip_prefix("VERSE#")?;
    let vi = rest.find("-FRACTAL#")?;
    let (verse, after_v) = (&rest[..vi], &rest[vi + "-FRACTAL#".len()..]);
    let fi = after_v.find("-PETAL#")?;
    let (fractal, petal) = (&after_v[..fi], &after_v[fi + "-PETAL#".len()..]);
    if verse.is_empty() || fractal.is_empty() || petal.is_empty() || node_id.is_empty() {
        return None;
    }
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '%' => out.push_str("%25"),
                '/' => out.push_str("%2F"),
                '#' => out.push_str("%23"),
                o => out.push(o),
            }
        }
        out
    }
    Some(format!(
        "fe://{}/{}/{}/{}",
        enc(verse),
        enc(fractal),
        enc(petal),
        enc(node_id)
    ))
}

/// Forward one lifecycle event on the replication seam, non-blocking (N-5).
fn emit_lifecycle(
    lifecycle_tx: &Option<crossbeam::channel::Sender<fe_runtime::messages::LifecycleEvent>>,
    event: fe_runtime::messages::LifecycleEvent,
) {
    if let Some(tx) = lifecycle_tx {
        if let Err(e) = tx.try_send(event) {
            tracing::warn!("lifecycle event dropped at DB seam: {e}");
        }
    }
}

#[allow(clippy::result_large_err)] // DbInitError is large only on the rare init-failure path
pub fn spawn_db_thread(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
) -> Result<std::thread::JoinHandle<()>, DbInitError> {
    spawn_db_thread_with_sync(rx, tx, blob_store, None, None, None, None, None)
}

/// Spawn the DB thread with optional replication, keypair, secret store,
/// entity-change broadcast for scene streaming, and custom DB path.
///
/// If `db_path` is `None`, defaults to `"data/fractalengine.db"`.
/// Backwards-compatible shim: delegates to
/// [`spawn_db_thread_with_sync_and_lifecycle`] with no lifecycle-event seam.
#[allow(clippy::too_many_arguments, clippy::result_large_err)] // thread-spawn seam; see spawn_db_thread
pub fn spawn_db_thread_with_sync(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
    repl_tx: Option<ReplicationSender>,
    keypair: Option<fe_identity::NodeKeypair>,
    secret_store: Option<std::sync::Arc<dyn fe_identity::SecretStore>>,
    entity_change_tx: Option<tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>>,
    db_path: Option<String>,
) -> Result<std::thread::JoinHandle<()>, DbInitError> {
    spawn_db_thread_with_sync_and_lifecycle(
        rx,
        tx,
        blob_store,
        repl_tx,
        keypair,
        secret_store,
        entity_change_tx,
        db_path,
        None,
    )
}

/// Sender half for node lifecycle events (FR-6) emitted by the DB thread onto
/// the sync/replication seam. Mirrors `fe_sync::LifecycleEventSender` (the
/// concrete type is defined in fe-sync, which wraps this same channel in its
/// `LifecycleForwarder`) — fe-database can't depend on fe-sync (cycle), so the
/// seam is expressed with the shared fe-runtime event type + crossbeam here.
pub type LifecycleEventSender = crossbeam::channel::Sender<fe_runtime::messages::LifecycleEvent>;

/// Spawn the DB thread, additionally forwarding node lifecycle events
/// (create / promote / delete-tombstone / reflow) onto `lifecycle_tx` (FR-6).
/// The binary wires `lifecycle_tx` to `fe_sync::LifecycleForwarder`'s channel so
/// peers + reporting (T5) observe the same truth. `None` disables emission.
#[allow(clippy::too_many_arguments, clippy::result_large_err)] // thread-spawn seam; see spawn_db_thread
pub fn spawn_db_thread_with_sync_and_lifecycle(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
    repl_tx: Option<ReplicationSender>,
    keypair: Option<fe_identity::NodeKeypair>,
    secret_store: Option<std::sync::Arc<dyn fe_identity::SecretStore>>,
    entity_change_tx: Option<tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>>,
    db_path: Option<String>,
    lifecycle_tx: Option<LifecycleEventSender>,
) -> Result<std::thread::JoinHandle<()>, DbInitError> {
    spawn_db_thread_shared(
        rx,
        tx,
        blob_store,
        repl_tx,
        keypair,
        secret_store,
        entity_change_tx,
        db_path,
        lifecycle_tx,
    )
    .map(|db_thread| db_thread.join_handle)
}

/// A running DB thread plus a clone of its one datastore handle.
/// See src/AGENTS.md §shared-handle.
pub struct DbThread {
    /// Joins when the DB thread exits (after `DbCommand::Shutdown` or channel close).
    pub join_handle: std::thread::JoinHandle<()>,
    /// Clone of the DB thread's own SurrealKV handle (ns/db already selected) — the
    /// only way for another thread to read the store: a second open is refused by
    /// SurrealKV's file lock (Windows os error 33).
    pub shared_db: repo::Db,
}

/// [`spawn_db_thread_with_sync_and_lifecycle`] that also hands back a clone of
/// the DB thread's datastore handle for in-process readers (the API gateway).
/// See src/AGENTS.md §shared-handle.
#[allow(clippy::too_many_arguments, clippy::result_large_err)] // thread-spawn seam; see spawn_db_thread
pub fn spawn_db_thread_shared(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
    repl_tx: Option<ReplicationSender>,
    keypair: Option<fe_identity::NodeKeypair>,
    secret_store: Option<std::sync::Arc<dyn fe_identity::SecretStore>>,
    entity_change_tx: Option<tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>>,
    db_path: Option<String>,
    lifecycle_tx: Option<LifecycleEventSender>,
) -> Result<DbThread, DbInitError> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_db_thread must not be called from within a Tokio runtime"
    );
    let (startup_tx, startup_rx) =
        std::sync::mpsc::sync_channel::<Result<repo::Db, DbInitError>>(0);
    let development_reset_permit = admin::DevelopmentResetPermit::capture();
    let handle = std::thread::spawn(move || {
        // The blob store handle is owned by the DB thread so future handlers
        // (Phase B: import refactor, Phase C: migration) can write to it
        // without crossing channel boundaries. Held live for thread lifetime.
        let blob_store: BlobStoreHandle = blob_store;
        // Multi-thread: the SurrealDB router task must keep serving the shared
        // handle while this thread blocks in `rx.recv()` (src/AGENTS.md §shared-handle).
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fe-db-engine")
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = startup_tx.send(Err(DbInitError::RuntimeBuild(e)));
                return;
            }
        };
        rt.block_on(async {
            let db_path = db_path.unwrap_or_else(|| "data/fractalengine.db".to_string());
            tracing::info!("Database thread started, initialising SurrealDB at {db_path}");
            let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
                match surrealdb::Surreal::new::<SurrealKv>(&db_path).await {
                    Ok(db) => db,
                    Err(e) => {
                        let _ = startup_tx.send(Err(DbInitError::SurrealOpen {
                            path: db_path,
                            source: e,
                        }));
                        return;
                    }
                };
            if let Err(e) = db.use_ns("fractalengine").use_db("fractalengine").await {
                let _ = startup_tx.send(Err(DbInitError::SurrealNsDb(e)));
                return;
            }
            if let Err(e) = rbac::apply_schema(&db).await {
                let _ = startup_tx.send(Err(DbInitError::SchemaApply(e.to_string())));
                return;
            }
            if let Err(e) = api_token_store::apply_api_token_schema(&db).await {
                let _ = startup_tx.send(Err(DbInitError::ApiTokenSchemaApply(e.to_string())));
                return;
            }
            // Initialize the Hybrid Logical Clock from the max persisted value
            // so clocks survive restarts.
            {
                let max_lc = match db
                    .query("SELECT math::max(lamport_clock) AS max_lc FROM op_log GROUP ALL")
                    .await
                {
                    Ok(mut hlc_res) => {
                        match hlc_res.take::<Vec<serde_json::Value>>(0) {
                            Ok(rows) => rows
                                .first()
                                .and_then(|r| r["max_lc"].as_u64())
                                .unwrap_or(0),
                            Err(_) => 0,
                        }
                    }
                    Err(e) => {
                        tracing::warn!("HLC init query failed: {e}, starting from 0");
                        0
                    }
                };
                let scene_hlc=match handlers::scene_publication::persisted_hlc(&db).await {
                    Ok(clock)=>clock,
                    Err(error)=>{let _=startup_tx.send(Err(DbInitError::SceneClockRecovery(error.to_string())));return;}
                };
                op_log::init_hlc(max_lc.max(scene_hlc));
            }

            // The shared handle is ready only after fatal clock recovery has succeeded.
            let _ = startup_tx.send(Ok(db.clone()));

            // Derive the local DID from the node keypair (falls back to
            // "local-node" when no keypair is available).
            let local_did = keypair.as_ref()
                .map(|kp| kp.to_did_key())
                .unwrap_or_else(|| "local-node".to_string());

            // Declarative reconciliation — runs every startup, converges to correct state.
            // Fixes ownership, backfills defaults, no tracking table needed.
            match reconcile::reconcile(&db, &local_did).await {
                Ok(n) if n > 0 => tracing::info!("Reconciliation applied {n} fix(es)"),
                Ok(_) => {}
                Err(e) => tracing::error!("Reconciliation failed: {e}"),
            }

            // Phase C: eagerly migrate any legacy base64 asset rows to the blob store.
            // Migration failure must NOT prevent startup — log and continue.
            match migrate_base64_assets_to_blob_store(&db, &blob_store).await {
                Ok((count, bytes)) => {
                    if count > 0 {
                        tracing::info!(
                            "Migrated {count} base64 assets to blob store ({bytes} bytes)"
                        );
                    }
                }
                Err(e) => tracing::error!("Asset migration failed: {e}"),
            }

            // Legacy imported/ files → blob store, before the first LoadHierarchy
            // (src/AGENTS.md §asset-resolution). Failure must not prevent startup.
            if let Err(e) = handlers::asset_backfill::backfill_imported_assets(
                &db,
                &blob_store,
                &imported_assets_dir(),
            )
            .await
            {
                tracing::error!("Legacy imported/ asset backfill failed: {e}");
            }

            // RP-2: one-shot legacy role migration, then repartition every verse (crash
            // repair) and re-emit this node's own role statements (AC-42) — see
            // src/AGENTS.md §role-propagation. Failure must not prevent startup.
            if let Err(e) =
                role_grants::startup(&db, &local_did, repl_tx.as_ref(), &blob_store).await
            {
                tracing::error!("Role statement startup failed: {e:#}");
            }

            tracing::info!("SurrealDB ready");
            if tx.send(DbResult::Started).is_err() {
                tracing::warn!("Result channel closed — UI may have shut down");
            }
            use handlers::send_result;
            // RP-3: the deferred-admission queue for inbound content rows — lives for the
            // loop, retried in-line (src/AGENTS.md §inbound-admissions).
            let mut admissions = handlers::inbound_admissions::InboundAdmissions::for_local_publisher(&local_did);

            #[allow(clippy::while_let_loop)]
            loop {
                let cmd = rx.recv();
                let cmd_start = std::time::Instant::now();
                match cmd {
                    Ok(DbCommand::PluginRead {admission,request,reply}) => {
                        let result=handlers::plugin_host::read(&db,&admission,request,&local_did).await.map_err(|error|error.to_string());
                        if let Err(error)=reply.send(result) {tracing::warn!(%error,"plugin read receiver closed");}
                    }
                    Ok(DbCommand::PluginCommit {admission,ops,reply}) => {
                        let result=match handlers::plugin_host::commit(&db,&admission,&ops,&local_did).await {
                            Ok(outcome)=>{
                                for (node_id,_,_,_) in &outcome.nodes {
                                    handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,node_id).await;
                                }
                                handlers::plugin_host::publish(&outcome,handlers::plugin_host::EffectSinks {
                                    results:&tx,scene:entity_change_tx.as_ref(),lifecycle:&lifecycle_tx,
                                });
                                Ok(fe_runtime::plugin_host::PluginReply::Committed(outcome.receipt))
                            }
                            Err(error)=>Err(error.to_string()),
                        };
                        if let Err(error)=reply.send(result) {tracing::warn!(%error,"plugin commit receiver closed; outcome may require reconciliation");}
                    }
                    Ok(DbCommand::Ping) => {
                        send_result(&tx, DbResult::Pong);
                    }
                    Ok(DbCommand::Seed) => match handlers::seed::seed_default_data(&db, &blob_store, &local_did).await {
                        Ok((petal_name, rooms)) => send_result(&tx, DbResult::Seeded { petal_name, rooms }),
                        Err(e) => {
                            tracing::error!("Seed failed: {e}");
                            send_result(&tx, DbResult::Error(format!("Seed failed: {e}")));
                        }
                    },
                    Ok(DbCommand::CreateVerse { name }) => {
                        match handlers::crud::create_verse_handler(&db, &blob_store, repl_tx.as_ref(), &name, &local_did, secret_store.as_ref()).await {
                            Ok((id, ns_id_hex)) => send_result(&tx, DbResult::VerseCreated { id, name, namespace_id: Some(ns_id_hex) }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create verse failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreateFractal { verse_id, name }) => {
                        match handlers::crud::create_fractal_handler(&db, &blob_store, repl_tx.as_ref(), &verse_id, &name, &local_did).await {
                            Ok(id) => send_result(&tx, DbResult::FractalCreated { id, verse_id, name }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create fractal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreatePetal { fractal_id, name }) => {
                        match handlers::crud::create_petal_handler(&db, &fractal_id, &name, &local_did).await {
                            Ok(id) => {
                                if let Err(error) = handlers::local_catalog::publish_petal(
                                    &db, &blob_store, repl_tx.as_ref(), &local_did, &id,
                                ).await {
                                    tracing::warn!(petal_id = %id, "committed Petal catalog publish failed: {error:#}");
                                }
                                send_result(&tx, DbResult::PetalCreated { id, fractal_id, name });
                            },
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create petal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreateNode { petal_id, name, position, correlation_id }) => {
                        match handlers::crud::create_node_as(&db, &petal_id, &name, position, &local_did).await {
                            Ok(id) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeAdded {
                                        node: fe_runtime::messages::NodeDto {
                                            node_id: id.clone(),
                                            petal_id: petal_id.clone(),
                                            name: name.clone(),
                                            position,
                                            rotation: [0.0, 0.0, 0.0, 1.0],
                                            scale: [1.0, 1.0, 1.0],
                                            has_asset: false,
                                            asset_path: None,
                                        },
                                    });
                                }
                                // FR-6: emit a NodeCreated lifecycle event carrying the stable address.
                                if let Ok(Some(scope)) = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await {
                                    if let Some(uri) = lifecycle_uri(&scope, &id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeCreated {
                                            address: uri, node_id: id.clone(),
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::NodeCreated { id, petal_id, name, has_asset: false, correlation_id, position });
                            }
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id,format!("Create node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ImportGltf { request, petal_id, name, file_path, position, file_lease }) => {
                        let result = handlers::crud::import_gltf_handler(&db, &blob_store, &petal_id, &name, &file_path, position, &local_did).await;
                        drop(file_lease);
                        match result {
                            Ok((node_id, asset_id, asset_path, materialization)) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeAdded {
                                        node: materialization.node.clone(),
                                    });
                                }
                                // FR-6: emit a NodeCreated lifecycle event for the imported node.
                                if let Ok(Some(scope)) = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await {
                                    if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeCreated {
                                            address: uri, node_id: node_id.clone(),
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::GltfImported { request, node_id, asset_id, petal_id, name, asset_path, position, materialization: Some(materialization), correlation_id: None });
                            }
                            Err(e) => send_result(&tx, DbResult::ImportGltfFailed { request, message: format!("GLTF import failed: {e}") }),
                        }
                    }
                    // F13 API asset ingest: metadata only — bytes were stored API-side.
                    Ok(DbCommand::CreateAsset { name, content_type, size_bytes, content_hash, owner_did, petal_id, correlation_id }) => {
                        // AC-46/TC-7: an anchored asset row replicates (device-payload emission),
                        // so the NODE must clear Editor at the anchor petal; an anchor that does
                        // not resolve is refused (fail closed). Un-anchored rows never replicate.
                        let refused = match petal_id.as_deref() {
                            None => None,
                            Some(anchor) => match handlers::crud::resolve_petal_scope_handler(&db, anchor).await {
                                Ok(Some(scope)) => require_node_role(&db, &local_did, &scope, fe_policy::RoleLevel::Editor, "CreateAsset").await.err(),
                                Ok(None) => Some(format!("CreateAsset refused: anchor petal {anchor} is not known locally")),
                                Err(e) => Some(format!("CreateAsset refused: anchor petal scope lookup failed: {e}")),
                            },
                        };
                        if let Some(refusal) = refused {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, refusal));
                        } else {
                            let registration = handlers::crud::AssetRegistration { name, content_type, size_bytes, content_hash, owner_did, petal_id };
                            send_result(&tx, handlers::crud::create_asset_reply(&db, &blob_store, registration, correlation_id).await);
                        }
                    }
                    // Fire-and-forget maintenance (src/AGENTS.md §handlers asset_gc): a
                    // failure is only logged — an uncorrelated `Error` would reach the
                    // oldest API waiter.
                    Ok(DbCommand::GcUnreferencedAssets { grace_secs }) => {
                        match handlers::asset_gc::gc_unreferenced_assets(&db, &blob_store, grace_secs).await {
                            Ok(report) => send_result(&tx, DbResult::AssetsGarbageCollected {
                                removed_assets: report.removed_assets,
                                removed_blobs: report.removed_blobs,
                            }),
                            Err(e) => tracing::warn!("asset GC pass failed: {e}"),
                        }
                    }
                    Ok(DbCommand::CreateNodeWithAsset { petal_id, name, asset_id, position, rotation, scale, correlation_id }) => {
                        match handlers::crud::create_node_with_asset_as(&db, &petal_id, &name, &asset_id, position, rotation, scale, &local_did).await {
                            Ok((node_id, asset_path, materialization)) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeAdded {
                                        node: materialization.node.clone(),
                                    });
                                }

                                if let Ok(Some(scope)) = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await {
                                    if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeCreated {
                                            address: uri, node_id: node_id.clone(),
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::GltfImported { request: None, node_id, asset_id, petal_id, name, asset_path, position, materialization: Some(materialization), correlation_id });
                            }
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Place asset failed: {e}"))),
                        }
                    }
                    Ok(command @ (DbCommand::LoadHierarchy | DbCommand::ReadHierarchy)) => {
                        match handlers::crud::load_hierarchy_handler(&db, &blob_store).await {
                            Ok(verses) if matches!(command, DbCommand::ReadHierarchy) => send_result(&tx, DbResult::HierarchyRead { verses }),
                            Ok(verses) => send_result(&tx, DbResult::HierarchyLoaded { verses }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Load hierarchy failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GenerateVerseInvite { verse_id, include_write_cap, expiry_hours }) => {
                        match handlers::invite::generate_verse_invite_handler(&db, &verse_id, include_write_cap, expiry_hours, keypair.as_ref(), secret_store.as_ref()).await {
                            Ok(invite_string) => send_result(&tx, DbResult::VerseInviteGenerated { verse_id, invite_string }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Generate invite failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::JoinVerseByInvite { invite_string }) => {
                        match handlers::invite::join_verse_by_invite_handler(&db, &invite_string, &local_did, secret_store.as_ref()).await {
                            Ok((verse_id, verse_name)) => send_result(&tx, DbResult::VerseJoined { verse_id, verse_name }),
                            // polish_lane_20261031 M-03: typed failure so the Join
                            // dialog can show it inline instead of a bare status-bar Error.
                            Err(e) => send_result(&tx, DbResult::JoinVerseFailed { error: format!("Join verse failed: {e}") }),
                        }
                    }
                    Ok(DbCommand::TerminalDevelopmentReset { request_id, confirmation, proof }) => {
                        let permit = if confirmation == "RESET" {
                            development_reset_permit.after_quiescence(&proof)
                        } else {
                            Err(anyhow::anyhow!("Terminal reset requires exact RESET confirmation"))
                        };
                        let (cleared, error) = match permit {
                            Ok(permit) => {
                                let outcome = handlers::admin::reset_database_handler(&db, &blob_store, &local_did, &permit).await;
                                (outcome.cleared, outcome.seed.err().map(|error| error.to_string()))
                            }
                            Err(error) => (false, Some(error.to_string())),
                        };
                        send_result(&tx, DbResult::TerminalDevelopmentReset { request_id, cleared, error });
                        break;
                    }
                    Ok(command @ (DbCommand::ResetDatabase | DbCommand::ResetDevelopmentDatabase { .. })) => {
                        let request_id = match command {
                            DbCommand::ResetDevelopmentDatabase { request_id } => Some(request_id),
                            _ => None,
                        };
                        let outcome = handlers::admin::reset_database_handler(&db, &blob_store, &local_did, &development_reset_permit).await;
                        let (petal_name, rooms, error) = match outcome.seed {
                            Ok((name, rooms)) => (name, rooms, None),
                            Err(error) => (String::new(), Vec::new(), Some(error.to_string())),
                        };
                        if outcome.cleared {
                            send_result(&tx, DbResult::DatabaseReset { petal_name, rooms });
                        }
                        if let Some(request_id) = request_id {
                            send_result(&tx, DbResult::DevelopmentDatabaseReset { request_id, cleared: outcome.cleared, error });
                        } else if let Some(error) = error {
                            send_result(&tx, DbResult::Error(format!("Database reset failed: {error}")));
                        }
                    }
                    Ok(DbCommand::UpdateNodeTransform { node_id, position, rotation, scale }) => {
                        match handlers::transform::update_node_transform_as(&db, &node_id, position, rotation, scale, &local_did).await {
                            Ok(()) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    match handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        Ok(Some(petal_id)) => {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::NodeTransform {
                                                node_id: node_id.clone(),
                                                petal_id,
                                                position,
                                                rotation,
                                                scale,
                                            });
                                        }
                                        Ok(None) => tracing::warn!(node_id, "omitting transform scene change without petal scope"),
                                        Err(e) => tracing::warn!(node_id, "omitting transform scene change after petal lookup failure: {e}"),
                                    }
                                }

                            }
                            Err(e) => {
                                tracing::error!("UpdateNodeTransform failed for {node_id}: {e}");
                                // Emit rollback with last-known-good values from DB
                                if let Some(ref ect) = entity_change_tx {
                                    match handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        Ok(Some(petal_id)) => {
                                            let rollback = match handlers::crud::get_node_transform_handler(&db, &node_id).await {
                                                Ok(Some((pos, rot, sc))) => {
                                                    fe_runtime::messages::SceneChange::TransformFailed {
                                                        node_id: node_id.clone(),
                                                        petal_id,
                                                        position: pos,
                                                        rotation: rot,
                                                        scale: sc,
                                                    }
                                                }
                                                _ => {
                                                    // Can't read old values — send back zeros as a signal
                                                    fe_runtime::messages::SceneChange::TransformFailed {
                                                        node_id: node_id.clone(),
                                                        petal_id,
                                                        position: [0.0; 3],
                                                        rotation: [0.0; 3],
                                                        scale: [1.0, 1.0, 1.0],
                                                    }
                                                }
                                            };
                                            let _ = ect.send(rollback);
                                        }
                                        Ok(None) => tracing::warn!(node_id, "omitting transform rollback without petal scope"),
                                        Err(lookup_error) => tracing::warn!(node_id, "omitting transform rollback after petal lookup failure: {lookup_error}"),
                                    }
                                }
                            }
                        }
                    }
                    Ok(DbCommand::UpdateNodeUrl { node_id, url, request_id, expected_scope }) => {
                        let (outcome, asset_id) = match handlers::transform::update_node_url_handler(&db, &node_id, url, &expected_scope, &local_did).await {
                            Ok(row) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                (Ok(row["external_url"].as_str().map(str::to_owned)), row["asset_id"].as_str().map(str::to_owned))
                            }
                            Err(error) => { tracing::warn!(node_id, %error, "UpdateNodeUrl refused or failed"); (Err("The URL was not confirmed: this model is missing, shared, changed, or you no longer have Editor access. Reload before retrying.".to_owned()), None) }
                        };
                        send_result(&tx, DbResult::NodeUrlUpdated { node_id, request_id, expected_scope, outcome, asset_id });
                    }
                    Ok(DbCommand::ReadNodeUrl { node_id, expected_scope, expected_asset_id, request_id }) => {
                        let outcome = handlers::transform::read_node_url_handler(&db, &node_id, &expected_scope, expected_asset_id.as_deref(), &local_did).await
                            .map_err(|error| {
                                tracing::warn!(node_id, %error, "ReadNodeUrl refused or failed");
                                "The URL is unavailable: this node/model changed, is missing, exceeds the read limit, or Viewer access is unavailable.".to_owned()
                            });
                        send_result(&tx, DbResult::NodeUrlRead { node_id, expected_scope, request_id, outcome });
                    }
                    Ok(DbCommand::ReadNativePetalView { request_id, target }) => {
                        let request_valid = ulid::Ulid::from_string(&request_id)
                            .is_ok_and(|id| id.to_string() == request_id);
                        let outcome = if request_valid {
                            tokio::time::timeout(
                                std::time::Duration::from_secs(1),
                                handlers::native_navigation::read(&db, &target, &local_did),
                            ).await.ok().and_then(Result::ok)
                                .ok_or(fe_runtime::native_navigation::Unavailable::Target)
                        } else { Err(fe_runtime::native_navigation::Unavailable::Target) };
                        send_result(&tx, DbResult::NativePetalViewRead { request_id, target, outcome });
                    }
                    Ok(DbCommand::GetEntityDetails { entity_type, entity_id, request_id }) => {
                        let details = handlers::entity::get_entity_details_handler(&db, entity_type, &entity_id, &local_did).await
                            .map_err(|error| {
                                tracing::warn!(%error, "GetEntityDetails refused or failed");
                                "Settings are unavailable: this entry is missing, changed, or you no longer have Viewer access.".to_string()
                            });
                        send_result(&tx, DbResult::EntityDetailsLoaded { entity_type, entity_id, request_id, details });
                    }
                    Ok(DbCommand::RenameEntity { entity_type, entity_id, new_name }) => {
                        match handlers::entity::rename_entity_handler(&db, entity_type, &entity_id, &new_name, &local_did).await {
                            Ok(()) => {
                                // EntityType has no Node variant, so no node-scoped scene change exists.
                                send_result(&tx, DbResult::EntityRenamed { entity_type, entity_id, new_name });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(e)),
                        }
                    }
                    Ok(DbCommand::SetVerseDefaultAccess { verse_id, default_access }) => {
                        match handlers::verse_settings::set_verse_default_access_handler(
                            &db,
                            &blob_store,
                            repl_tx.as_ref(),
                            &local_did,
                            &verse_id,
                            &default_access,
                        )
                        .await
                        {
                            Ok(()) => send_result(&tx, DbResult::VerseDefaultAccessSet { verse_id, default_access }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Set default access failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::SetVerseTimeseriesSettings { verse_id, mode, replication_factor, bucket_width_ms }) => {
                        match handlers::verse_settings::set_verse_timeseries_settings_handler(
                            &db,
                            &blob_store,
                            repl_tx.as_ref(),
                            &local_did,
                            &verse_id,
                            &mode,
                            replication_factor,
                            bucket_width_ms,
                        )
                        .await
                        {
                            Ok(s) => send_result(
                                &tx,
                                DbResult::VerseTimeseriesSettingsSet {
                                    verse_id,
                                    mode: s.mode.as_str().to_string(),
                                    replication_factor: s.replication_factor,
                                    bucket_width_ms: s.bucket_width_ms,
                                },
                            ),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Set verse timeseries settings failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::UpdateFractalDescription { fractal_id, description }) => {
                        match handlers::entity::update_fractal_description_handler(&db, &fractal_id, &description, &local_did).await {
                            Ok(()) => send_result(&tx, DbResult::FractalDescriptionUpdated { fractal_id, description }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("{e}"))),
                        }
                    }
                    Ok(DbCommand::DeleteEntity { entity_type, entity_id }) => {
                        match handlers::entity::delete_entity_handler(&db, entity_type, &entity_id, &local_did).await {
                            Ok(()) => {
                                // EntityType covers Verse/Fractal/Petal only (no Node variant).
                                // Cascade deletes remove child nodes — clients should
                                // re-subscribe after structural deletes. Individual NodeRemoved
                                // events for each cascaded node are deferred to a future track.
                                send_result(&tx, DbResult::EntityDeleted { entity_type, entity_id });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(e)),
                        }
                    }
                    Ok(DbCommand::PrepareJoinInvite { target, recipient_did, role, expiry_hours, request_id }) => {
                        let result = handlers::addressed_join_v2::prepare(&db,&local_did,&target,&recipient_did,&role,expiry_hours).await;
                        send_result(&tx,DbResult::JoinInvitePrepared {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::IssueJoinInvite { source, ticket, request_id }) => {
                        let result = handlers::addressed_join_v2::issue(&db,&local_did,keypair.as_ref(),&source,&ticket,&blob_store,repl_tx.as_ref()).await;
                        if result.is_ok() {
                            if let Some(verse) = role_grants::verse_of_scope(&source.target.scope) {
                                handlers::scene_publication::echo_applied(&db,&blob_store,&local_did,&tx,admissions.retry_author_roles(&db,&verse,entity_change_tx.as_ref()).await).await;
                            }
                        }
                        send_result(&tx,DbResult::JoinInviteIssued {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::AdmitJoinInvite { credential, request_id }) => {
                        let result = if credential.as_str().starts_with(join_invite::JOIN_V2_PREFIX) {
                            handlers::addressed_join::admit_v2(&db,&local_did,&credential).await
                        } else {handlers::addressed_join::admit(&db,&local_did,&credential).await};
                        send_result(&tx,DbResult::JoinInviteAdmitted {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::CheckJoinReadiness { credential, request_id }) => {
                        let result = if credential.as_str().starts_with(join_invite::JOIN_V2_PREFIX) {
                            handlers::addressed_join_v2::ready(&db,&local_did,&credential).await
                        } else {handlers::addressed_join::ready(&db,&local_did,&credential).await};
                        send_result(&tx,DbResult::JoinReadinessChecked {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::FinalizeJoinProof {credential,request_id,deadline}) => {
                        let result=handlers::addressed_join::finalize_before(&db,&local_did,&credential,deadline).await;
                        send_result(&tx,DbResult::JoinProofFinalized {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::EntityAccess { target, action, request_id }) => {
                        let result=handlers::entity_access::execute(&db,&blob_store,repl_tx.as_ref(),&local_did,&target,&action,keypair.as_ref(),|jti| send_result(&tx,DbResult::ApiTokenRevoked { jti:jti.to_owned() })).await;
                        if result.is_ok()
                            && matches!(action,fe_runtime::entity_access::EntityAccessAction::Assign {..} | fe_runtime::entity_access::EntityAccessAction::Revoke {..}) {
                                if let Some(verse)=role_grants::verse_of_scope(&target.scope) {
                                    handlers::scene_publication::echo_applied(&db,&blob_store,&local_did,&tx,admissions.retry_author_roles(&db,&verse,entity_change_tx.as_ref()).await).await;
                                }
                            }
                        send_result(&tx,DbResult::EntityAccessFinished { target,request_id,result:result.map_err(|error| error.to_string()) });
                    }
                    Ok(DbCommand::ResolveRolesForPeers { scope, peer_dids }) => {
                        let roles = handlers::rbac::resolve_roles_for_peers_handler(&db, &scope, &peer_dids).await;
                        send_result(&tx, DbResult::PeerRolesResolved { scope, roles });
                    }
                    Ok(DbCommand::AssignRole { peer_did, scope, role }) => {
                        match handlers::rbac::assign_role_handler(&db, &blob_store, repl_tx.as_ref(), &local_did, &peer_did, &scope, &role).await {
                            Ok(()) => {
                                let verse = role_grants::verse_of_scope(&scope);
                                send_result(&tx, DbResult::RoleAssigned { peer_did, scope, role });
                                // A local grant can unblock deferred inbound content rows.
                                if let Some(verse) = verse {
                                    handlers::scene_publication::echo_applied(&db,&blob_store,&local_did,&tx,admissions.retry_author_roles(&db, &verse, entity_change_tx.as_ref()).await).await;
                                }
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Assign role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::RevokeRole { peer_did, scope }) => {
                        match handlers::rbac::revoke_role_handler(&db, &blob_store, repl_tx.as_ref(), &local_did, &peer_did, &scope).await {
                            Ok(report) => {
                                if report.still_held() {
                                    // A Manager's revoke retracts only their own grant (plan
                                    // §2.3); surfacing this in the UI is D-32 FR-4.
                                    tracing::info!(
                                        peer_did, scope, role = %report.role_after,
                                        "RevokeRole: the target still holds its role through another grantor or inheritance"
                                    );
                                }
                                let verse = role_grants::verse_of_scope(&scope);
                                send_result(&tx, DbResult::RoleRevoked { peer_did, scope });
                                // An owner revoke of a pin can raise the peer back to an
                                // inherited role, so deferred rows are retried here too.
                                if let Some(verse) = verse {
                                    handlers::scene_publication::echo_applied(&db,&blob_store,&local_did,&tx,admissions.retry_author_roles(&db, &verse, entity_change_tx.as_ref()).await).await;
                                }
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Revoke role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GenerateScopedInvite { scope, role, expiry_hours }) => {
                        let Some(ref kp) = keypair else {
                            send_result(&tx, DbResult::Error("NodeKeypair not available for invite signing".to_string()));
                            continue;
                        };
                        match handlers::rbac::generate_scoped_invite_handler(kp, &local_did, &scope, &role, expiry_hours).await {
                            Ok(invite_link) => send_result(&tx, DbResult::ScopedInviteGenerated { invite_link }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Generate scoped invite failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveLocalRole { scope }) => {
                        match handlers::rbac::resolve_local_role_handler(&db, &local_did, &scope).await {
                            Ok(role) => send_result(&tx, DbResult::LocalRoleResolved { scope, role }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve local role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::MintApiToken { scope, max_role, ttl_hours, label, client_did, correlation_id }) => {
                        let Some(ref kp) = keypair else {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, "NodeKeypair not available for API token minting".to_string()));
                            continue;
                        };
                        match handlers::api_token::mint_api_token_handler(&db, kp, &local_did, &scope, &max_role, ttl_hours, label.as_deref(), client_did.as_deref()).await {
                            Ok(minted) => {
                                send_result(&tx, DbResult::ApiTokenMinted { correlation_id, token: minted.token, jti: minted.jti, scope, max_role, expires_at: minted.expires_at, label, client: minted.client });
                            }
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Mint API token failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::RevokeApiToken { jti, sub }) => {
                        match handlers::api_token::revoke_local_api_token_handler(&db, &local_did, &sub, &jti).await {
                            Ok(true) => send_result(&tx, DbResult::ApiTokenRevoked { jti }),
                            Ok(false) => send_result(&tx, DbResult::Error(format!("API token not found or not owned: {jti}"))),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Revoke API token failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ListApiTokens { offset, limit }) => {
                        match handlers::api_token::list_api_tokens_handler(&db, &local_did, offset, limit).await {
                            Ok((tokens, total)) => send_result(&tx, DbResult::ApiTokensListed { tokens, total }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("List API tokens failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ListApiTokensByScope { scope_prefix, offset, limit }) => {
                        match handlers::api_token::list_api_tokens_by_scope_handler(&db, &scope_prefix, offset, limit).await {
                            Ok((tokens, total)) => send_result(&tx, DbResult::ScopedApiTokensListed { tokens, total }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("List API tokens by scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolvePetalScope { petal_id }) => {
                        match handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve petal scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveNodeScope { node_id }) => {
                        match handlers::crud::resolve_node_scope_handler(&db, &node_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve node scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveFractalScope { fractal_id }) => {
                        match handlers::crud::resolve_fractal_scope_handler(&db, &fractal_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve fractal scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveVerseScope { verse_id }) => {
                        match handlers::crud::resolve_verse_scope_handler(&db, &verse_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve verse scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::LoadNodesByPetal { petal_id }) => {
                        match handlers::crud::load_nodes_by_petal_handler(&db, &blob_store, &petal_id).await {
                            Ok(nodes) => send_result(&tx, DbResult::NodesLoaded { petal_id, nodes }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Load nodes by petal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GetNodeTransform { node_id }) => {
                        match handlers::crud::get_node_transform_handler(&db, &node_id).await {
                            Ok(Some((position, rotation, scale))) => {
                                send_result(&tx, DbResult::NodeTransformLoaded { node_id, position, rotation, scale });
                            }
                            Ok(None) => send_result(&tx, DbResult::Error(format!("Node not found: {node_id}"))),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Get node transform failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::SetNodeProperty { node_id, key, value , correlation_id }) => {
                        match handlers::entity_property::set_entity_property_as(&db, &node_id, &key, &value, &local_did).await {
                            Ok(()) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    match handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        Ok(Some(petal_id)) => {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::PropertyChanged {
                                                node_id: node_id.clone(),
                                                petal_id,
                                                key: key.clone(),
                                                value: value.clone(),
                                            });
                                        }
                                        Ok(None) => tracing::warn!(node_id, "omitting property scene change without petal scope"),
                                        Err(e) => tracing::warn!(node_id, "omitting property scene change after petal lookup failure: {e}"),
                                    }
                                }

                                send_result(&tx, DbResult::NodePropertySet { node_id, key , correlation_id });
                            }
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Set node property failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GetNodeProperties { node_id , correlation_id }) => {
                        match handlers::entity_property::get_entity_properties_handler(&db, &node_id).await {
                            Ok(properties) => send_result(&tx, DbResult::NodePropertiesLoaded { node_id, properties , correlation_id }),
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Get node properties failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::DeleteNodeProperty { node_id, key , correlation_id }) => {
                        match handlers::entity_property::delete_entity_property_as(&db, &node_id, &key, &local_did).await {
                            Ok(()) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    match handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        Ok(Some(petal_id)) => {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::PropertyChanged {
                                                node_id: node_id.clone(),
                                                petal_id,
                                                key: key.clone(),
                                                value: serde_json::Value::Null,
                                            });
                                        }
                                        Ok(None) => tracing::warn!(node_id, "omitting property scene change without petal scope"),
                                        Err(e) => tracing::warn!(node_id, "omitting property scene change after petal lookup failure: {e}"),
                                    }
                                }

                                send_result(&tx, DbResult::NodePropertyDeleted { node_id, key , correlation_id });
                            }
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Delete node property failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::DeleteNode { node_id }) => {
                        let result=async {
                            let scope=handlers::crud::resolve_node_scope_handler(&db,&node_id).await?.ok_or_else(||anyhow::anyhow!("delete target unavailable"))?;
                            handlers::entity::prepare_scope_permission(&db,&scope,&local_did,fe_policy::RoleLevel::Editor).await?;
                            handlers::crud::tombstone_node_handler(&db,&node_id,&local_did).await
                        }.await;
                        match result {
                            Ok(outcome) => {
                                let petal_id=outcome.petal_id;
                                handlers::scene_publication::publish_tombstones_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeRemoved {
                                        node_id: node_id.clone(),
                                        petal_id: petal_id.clone(),
                                    });
                                }
                                if let Err(e) = handlers::node_log::append_node_log(
                                    &db, &node_id, "deleted", &local_did,
                                    &serde_json::json!({"petal_id": petal_id}),
                                ).await {
                                    tracing::warn!("node_log append failed for {node_id}: {e}");
                                }
                                send_result(&tx, DbResult::NodeDeleted { node_id, petal_id });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Delete node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::TombstoneNode { node_id, auth }) => {
                        // N-5: resolve the node's scope, then the caller's real role at it,
                        // then authorize (Editor+) BEFORE mutating. A `Local` caller asserts
                        // no role — the DB resolves the local user's here so the UI can't
                        // self-authorize.
                        let scope = handlers::crud::resolve_node_scope_handler(&db, &node_id).await.ok().flatten();
                        let Some(scope) = scope else {
                            send_result(&tx, DbResult::Error(format!("TombstoneNode: cannot resolve scope for {node_id}")));
                            continue;
                        };
                        let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                        let decision = fe_policy::authorize_node_delete(&ctx, &fe_policy::Scope::new(scope.clone()));
                        if !decision.is_allow() {
                            send_result(&tx, DbResult::Error(format!(
                                "TombstoneNode denied for {}: {}", ctx.subject_label(),
                                decision.reason().unwrap_or("unauthorized"),
                            )));
                            continue;
                        }
                        // The recorded actor is the resolved subject (external DID, or the
                        // DB-resolved local DID for a `Local` caller).
                        let actor = ctx.subject_label();
                        match handlers::crud::tombstone_node_handler(&db, &node_id, &actor).await {
                            Ok(outcome) => {
                                handlers::scene_publication::publish_tombstones_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                // FR-6 "exactly one event": side effects fire ONLY when this op
                                // actually tombstoned something. A repeat on an already-tombstoned
                                // node is an idempotent no-op (empty `tombstoned_ids`) — no scene
                                // change, no lifecycle event.
                                if !outcome.tombstoned_ids.is_empty() {
                                    // Split reconcile: mirror to the in-memory store / scene subscribers.
                                    if let Some(ref ect) = entity_change_tx {
                                        let _ = ect.send(fe_runtime::messages::SceneChange::NodeRemoved {
                                            node_id: node_id.clone(),
                                            petal_id: outcome.petal_id.clone(),
                                        });
                                    }
                                    // one NodeDeleted per delete op (+ PathReflow for a stamp).
                                    if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeDeleted {
                                            address: uri, node_id: node_id.clone(),
                                        });
                                    }
                                    if let Some(path) = outcome.reflow_path.clone() {
                                        // deleted_index tells T2's re-flow which stamp slot vanished.
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::PathReflow {
                                            path_id: path,
                                            deleted_index: outcome.deleted_instance_index,
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::NodeDeleted { node_id, petal_id: outcome.petal_id });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Tombstone node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CascadeTombstoneNode { node_id, auth }) => {
                        // N-5: resolve scope, then the caller's real role, then authorize
                        // before mutating; then tombstone the whole subtree through the log-first seam.
                        // A `Local` caller's role is DB-resolved (no UI-side assertion).
                        let scope = handlers::crud::resolve_node_scope_handler(&db, &node_id).await.ok().flatten();
                        let Some(scope) = scope else {
                            send_result(&tx, DbResult::Error(format!("CascadeTombstoneNode: cannot resolve scope for {node_id}")));
                            continue;
                        };
                        let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                        let decision = fe_policy::authorize_node_delete(&ctx, &fe_policy::Scope::new(scope.clone()));
                        if !decision.is_allow() {
                            send_result(&tx, DbResult::Error(format!(
                                "CascadeTombstoneNode denied for {}: {}", ctx.subject_label(),
                                decision.reason().unwrap_or("unauthorized"),
                            )));
                            continue;
                        }
                        let actor = ctx.subject_label();
                        match handlers::crud::cascade_tombstone_node_handler(&db, &node_id, &actor).await {
                            Ok(outcome) => {
                                handlers::scene_publication::publish_tombstones_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                // FR-6 "exactly one event": emit only when the cascade actually
                                // tombstoned something (idempotent repeat on an already-tombstoned
                                // subtree = no-op, empty `tombstoned_ids`).
                                if !outcome.tombstoned_ids.is_empty() {
                                    if let Some(ref ect) = entity_change_tx {
                                        for tombstoned_node_id in &outcome.tombstoned_ids {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::NodeRemoved {
                                                node_id: tombstoned_node_id.clone(),
                                                petal_id: outcome.petal_id.clone(),
                                            });
                                        }
                                    }
                                    // exactly one NodeDeleted for the cascade op (rooted at node_id).
                                    if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeDeleted {
                                            address: uri, node_id: node_id.clone(),
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::NodeDeleted { node_id, petal_id: outcome.petal_id });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Cascade tombstone failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::PromoteInstance { petal_id, path_id, instance_index, auth }) => {
                        // N-5: resolve the petal scope, then the caller's real role, then
                        // authorize (Editor+) BEFORE materializing the node. A `Local` caller's
                        // role is DB-resolved (no UI-side assertion).
                        let scope = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await.ok().flatten();
                        let Some(scope) = scope else {
                            send_result(&tx, DbResult::Error(format!("PromoteInstance: cannot resolve scope for petal {petal_id}")));
                            continue;
                        };
                        let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                        let decision = fe_policy::authorize_instance_promotion(&ctx, &fe_policy::Scope::new(scope.clone()));
                        if !decision.is_allow() {
                            send_result(&tx, DbResult::Error(format!(
                                "PromoteInstance denied for {}: {}", ctx.subject_label(),
                                decision.reason().unwrap_or("unauthorized"),
                            )));
                            continue;
                        }
                        let actor = match &auth {
                            fe_runtime::messages::CallerAuth::Local => local_did.as_str(),
                            fe_runtime::messages::CallerAuth::Identified { did, .. } => did.as_str(),
                            fe_runtime::messages::CallerAuth::Anonymous => {
                                send_result(&tx, DbResult::Error("PromoteInstance requires an identified actor".into()));
                                continue;
                            }
                        };
                        match handlers::crud::promote_instance_handler(&db, &petal_id, &path_id, instance_index, &local_did, actor, &scope).await {
                            Ok((node_id, newly_promoted)) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                // FR-6: emit NodePromoted only when a row was actually materialized
                                // (an idempotent no-op is not an op, so it emits nothing).
                                if newly_promoted {
                                    if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                        emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodePromoted {
                                            address: uri,
                                            node_id: node_id.clone(),
                                            path_id: path_id.clone(),
                                            instance_index,
                                        });
                                    }
                                }
                                send_result(&tx, DbResult::NodePromoted {
                                    node_id,
                                    petal_id,
                                    path_id,
                                    instance_index,
                                    newly_promoted,
                                });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Promote instance failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::RenameNode { node_id, new_name, auth }) => {
                        // N-5: resolve the node's scope, then the caller's real role at it,
                        // then authorize (Editor+) BEFORE mutating — see TombstoneNode.
                        let scope = handlers::crud::resolve_node_scope_handler(&db, &node_id).await.ok().flatten();
                        let Some(scope) = scope else {
                            send_result(&tx, DbResult::Error(format!("RenameNode: cannot resolve scope for {node_id}")));
                            continue;
                        };
                        let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                        let decision = fe_policy::authorize_node_edit(&ctx, &fe_policy::Scope::new(scope.clone()));
                        if !decision.is_allow() {
                            send_result(&tx, DbResult::Error(format!(
                                "RenameNode denied for {}: {}", ctx.subject_label(),
                                decision.reason().unwrap_or("unauthorized"),
                            )));
                            continue;
                        }
                        match handlers::crud::rename_node_handler(&db, &node_id, &new_name, &local_did, &ctx.subject_label()).await {
                            Ok(()) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    match handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        Ok(Some(petal_id)) => {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::NodeRenamed {
                                                node_id: node_id.clone(),
                                                petal_id,
                                                new_name: new_name.clone(),
                                            });
                                        }
                                        Ok(None) => tracing::warn!(node_id, "omitting rename scene change without petal scope"),
                                        Err(e) => tracing::warn!(node_id, "omitting rename scene change after petal lookup failure: {e}"),
                                    }
                                }
                                if let Some(uri) = lifecycle_uri(&scope, &node_id) {
                                    emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeRenamed {
                                        address: uri, node_id: node_id.clone(),
                                    });
                                }
                                send_result(&tx, DbResult::NodeRenamed { node_id, new_name });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Rename node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::DuplicateNode { node_id, auth }) => {
                        // N-5: authorize (Editor+) at the source node's scope BEFORE copying.
                        let scope = handlers::crud::resolve_node_scope_handler(&db, &node_id).await.ok().flatten();
                        let Some(scope) = scope else {
                            send_result(&tx, DbResult::Error(format!("DuplicateNode: cannot resolve scope for {node_id}")));
                            continue;
                        };
                        let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                        let decision = fe_policy::authorize_node_edit(&ctx, &fe_policy::Scope::new(scope.clone()));
                        if !decision.is_allow() {
                            send_result(&tx, DbResult::Error(format!(
                                "DuplicateNode denied for {}: {}", ctx.subject_label(),
                                decision.reason().unwrap_or("unauthorized"),
                            )));
                            continue;
                        }
                        match handlers::crud::duplicate_node_handler(&db, &node_id, &local_did, &ctx.subject_label()).await {
                            Ok(copy) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&copy.node_id).await;
                                // Mirror the CreateNode side effects so subscribers treat the
                                // copy as an ordinary new node (zero UI changes).
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeAdded {
                                        node: fe_runtime::messages::NodeDto {
                                            node_id: copy.node_id.clone(),
                                            petal_id: copy.petal_id.clone(),
                                            name: copy.name.clone(),
                                            position: copy.position,
                                            rotation: copy.rotation,
                                            scale: copy.scale,
                                            has_asset: copy.has_asset,
                                            asset_path: None,
                                        },
                                    });
                                }
                                if let Some(uri) = lifecycle_uri(&scope, &copy.node_id) {
                                    emit_lifecycle(&lifecycle_tx, fe_runtime::messages::LifecycleEvent::NodeCreated {
                                        address: uri, node_id: copy.node_id.clone(),
                                    });
                                }
                                send_result(&tx, DbResult::NodeCreated {
                                    id: copy.node_id,
                                    petal_id: copy.petal_id,
                                    name: copy.name,
                                    has_asset: copy.has_asset,
                                    correlation_id: None,
                                    position: copy.position,
                                });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Duplicate node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CountNodeDescendants { node_id }) => {
                        match handlers::crud::count_node_descendants_handler(&db, &node_id).await {
                            Ok(count) => send_result(&tx, DbResult::NodeDescendantCount { node_id, count }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Count node descendants failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::SetNodeView { node_id, state, groups, auth, correlation_id }) => {
                        // Lifecycle order (§lifecycle "Node view"): validate → tombstone-aware
                        // scope resolve → caller's real role → Editor+ → op-logged writes.
                        let reply = 'reply: {
                            if let Err(e) = handlers::node_view::validate_view_request(state.as_deref(), groups.as_deref()) {
                                break 'reply handlers::correlated_failure(&correlation_id, format!("SetNodeView rejected: {e}"));
                            }
                            let petal_id = match handlers::node_view::resolve_live_node_petal(&db, &node_id).await {
                                Ok(Some(petal_id)) => petal_id,
                                Ok(None) => break 'reply handlers::correlated_failure(&correlation_id, format!("SetNodeView: no live node {node_id}")),
                                Err(e) => break 'reply handlers::correlated_failure(&correlation_id, format!("SetNodeView failed: {e}")),
                            };
                            let Some(scope) = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await.ok().flatten() else {
                                break 'reply handlers::correlated_failure(&correlation_id, format!("SetNodeView: cannot resolve scope for {node_id}"));
                            };
                            let ctx = lifecycle_auth_context(&db, &local_did, &auth, &scope).await;
                            let decision = fe_policy::authorize_node_edit(&ctx, &fe_policy::Scope::new(scope.clone()));
                            if !decision.is_allow() {
                                break 'reply handlers::correlated_failure(&correlation_id, format!(
                                    "SetNodeView denied for {}: {}", ctx.subject_label(),
                                    decision.reason().unwrap_or("unauthorized"),
                                ));
                            }
                            let view_grant = if matches!(auth,fe_runtime::messages::CallerAuth::Local) {fe_runtime::node_history::NodeAccessGrant::Local} else {fe_runtime::node_history::NodeAccessGrant::Scoped(scope)};
                            let view_actor = ctx.subject_label();
                            match handlers::node_view::set_node_view_handler(&db, &node_id, state.as_deref(), groups.as_deref(),&view_actor,&view_grant).await {
                                Ok(view) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                    if let Some(ref ect) = entity_change_tx {
                                        let written = [
                                            (state.is_some(), handlers::node_view::VIEW_STATE_KEY, serde_json::json!(view.state)),
                                            (groups.is_some(), handlers::node_view::VIEW_GROUPS_KEY, serde_json::json!(view.groups)),
                                        ];
                                        for (_, key, value) in written.into_iter().filter(|(touched, _, _)| *touched) {
                                            let _ = ect.send(fe_runtime::messages::SceneChange::PropertyChanged {
                                                node_id: node_id.clone(),
                                                petal_id: petal_id.clone(),
                                                key: key.to_string(),
                                                value,
                                            });
                                        }
                                    }
                                    DbResult::NodeViewSet {
                                        node_id,
                                        petal_id,
                                        state: view.state,
                                        groups: view.groups,
                                        correlation_id,
                                    }
                                }
                                Err(e) => handlers::correlated_failure(&correlation_id, format!("SetNodeView failed: {e}")),
                            }
                        };
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::LoadPetalViewState { petal_id }) => {
                        match handlers::node_view::load_petal_view_state_handler(&db, &petal_id).await {
                            Ok(entries) => send_result(&tx, DbResult::PetalViewStateLoaded { petal_id, entries }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Load petal view state failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::SetPetalVisibilityGroups { petal_id, groups, auth, correlation_id }) => {
                        let reply = handlers::visibility_groups::set_petal_visibility_groups_reply(
                            &db, &local_did, petal_id, groups, auth, correlation_id,
                        ).await;
                        if let DbResult::PetalVisibilityGroupsLoaded { petal_id, .. } = &reply {
                            handlers::visibility_groups::publish_registry(&db, repl_tx.as_ref(), &blob_store, petal_id).await;
                        }
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::GetPetalVisibilityGroups { petal_id, correlation_id }) => {
                        let reply = handlers::visibility_groups::get_petal_visibility_groups_reply(&db, petal_id, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ComparePetalVisibilityGroups {petal_id,expected,groups,auth,correlation_id}) => {
                        let reply=handlers::visibility_groups::set_petal_visibility_groups_checked_reply(&db,&local_did,petal_id,groups,auth,correlation_id,Some(expected)).await;
                        if let DbResult::PetalVisibilityGroupsLoaded { petal_id, .. } = &reply {
                            handlers::visibility_groups::publish_registry(&db, repl_tx.as_ref(), &blob_store, petal_id).await;
                        }
                        send_result(&tx,reply);
                    }
                    Ok(DbCommand::SetScopeHidden { level, id, hidden, auth, correlation_id }) => {
                        let reply = handlers::visibility_groups::set_scope_hidden_reply(
                            &db, &local_did, level, id, hidden, auth, correlation_id,
                        ).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::GetPetalRequirements { petal_id, correlation_id }) => {
                        let result=match handlers::petal_requirements::get(&db,&local_did,&petal_id).await {
                            Ok(snapshot)=>DbResult::PetalRequirementsLoaded {snapshot,correlation_id},
                            Err(error)=>DbResult::RequestFailed {error:format!("Load requirements failed: {error}"),correlation_id},
                        };
                        handlers::send_result(&tx,result);
                    }
                    Ok(DbCommand::SetPetalRequirements { expected, requirements, correlation_id }) => {
                        let result=match handlers::petal_requirements::set(&db,&local_did,&expected,&requirements).await {
                            Ok(snapshot)=> {
                                if let Err(error)=handlers::local_catalog::publish_petal(&db,&blob_store,repl_tx.as_ref(),&local_did,&snapshot.petal_id).await {
                                    tracing::warn!(%error,"Requirements committed; catalog publication deferred until replica recovery");
                                }
                                DbResult::PetalRequirementsLoaded {snapshot,correlation_id}
                            }
                            Err(error)=>DbResult::RequestFailed {error:format!("Save requirements failed: {error}"),correlation_id},
                        };
                        handlers::send_result(&tx,result);
                    }
                    Ok(DbCommand::ReadTerrainApiCredential { request }) => {
                        let reply = api_auth::terrain_credential_reply(&db, request).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::SetPetalTerrain { petal_id, terrain, grant, correlation_id }) => {
                        let reply = handlers::petal_terrain::set_petal_terrain_reply(&db, &local_did, petal_id, terrain, grant, repl_tx.as_ref(), &blob_store, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::GetPetalTerrain { petal_id, correlation_id }) => {
                        let reply = handlers::petal_terrain::get_petal_terrain_reply(&db, petal_id, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::SetPetalRenderDistance { petal_id, render_distance, auth, correlation_id }) => {
                        let reply = handlers::petal_render_distance::set_petal_render_distance_reply(
                            &db, &local_did, petal_id, render_distance, auth, repl_tx.as_ref(), &blob_store, correlation_id,
                        ).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::GetPetalRenderDistance { petal_id, correlation_id }) => {
                        let reply = handlers::petal_render_distance::get_petal_render_distance_reply(&db, petal_id, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ClaimOrphanedVerse { verse_id, correlation_id }) => {
                        let reply = handlers::claim_orphaned_verse::claim_orphaned_verse_reply(&db, &local_did, verse_id, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::CreateFieldDef { scope, entity_type, key, value_type, default_val }) => {
                        match handlers::field_def::create_field_def_handler(&db, &scope, &entity_type, &key, &value_type, default_val.as_ref(), &local_did).await {
                            Ok(field_def_id) => send_result(&tx, DbResult::FieldDefCreated { field_def_id, scope, key }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create field def failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ListFieldDefs { scope }) => {
                        match handlers::field_def::list_field_defs_handler(&db, &scope).await {
                            Ok(rows) => {
                                let field_defs: Vec<fe_runtime::messages::FieldDefInfo> = rows.iter().filter_map(|r| {
                                    Some(fe_runtime::messages::FieldDefInfo {
                                        field_def_id: r.get("field_def_id")?.as_str()?.to_string(),
                                        scope: r.get("scope")?.as_str()?.to_string(),
                                        entity_type: r.get("entity_type")?.as_str()?.to_string(),
                                        key: r.get("key")?.as_str()?.to_string(),
                                        value_type: r.get("value_type")?.as_str()?.to_string(),
                                        default_val: r.get("default_val").cloned(),
                                        created_by: r.get("created_by").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                        created_at: r.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    })
                                }).collect();
                                send_result(&tx, DbResult::FieldDefsListed { scope, field_defs });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("List field defs failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::UpdateFieldDef { field_def_id, value_type, default_val }) => {
                        match handlers::field_def::update_field_def_handler(&db, &field_def_id, &value_type, default_val.as_ref()).await {
                            Ok(()) => send_result(&tx, DbResult::FieldDefUpdated { field_def_id }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Update field def failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GetFieldSchema {field_def_id,grant,correlation_id}) => {
                        let reply = match handlers::field_def::get_field_schema_handler(&db,&field_def_id,&grant,&local_did).await {
                            Ok(row) => DbResult::QueryResult {data:vec![row],correlation_id},
                            Err(error) => handlers::correlated_failure(&correlation_id,format!("Get field schema failed: {error}")),
                        };
                        send_result(&tx,reply);
                    }
                    Ok(DbCommand::SetFieldSchema { field_def_id, descriptor, grant, correlation_id }) => {
                        let reply = match handlers::field_def::set_field_schema_handler(&db, &field_def_id, &descriptor,&grant,&local_did).await {
                            Ok(row) => DbResult::QueryResult { data: vec![row], correlation_id },
                            Err(e) => handlers::correlated_failure(&correlation_id, format!("Set field schema failed: {e}")),
                        };
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ReadNodeMeasurements { node_id, petal_id, series, limit, grant, correlation_id }) => {
                        let reply = match handlers::node_measurements::read(&db, &node_id, &petal_id, series, limit, &grant, &local_did).await {
                            Ok(measurements) => DbResult::NodeMeasurementsLoaded { measurements, correlation_id },
                            Err(error) => DbResult::RequestFailed {error:format!("Read measurements failed: {error}"),correlation_id},
                        };
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ReadSceneMaterials { petal_id, expected_scope, node_ids, correlation_id }) => {
                        let reply = match handlers::scene_materials::read(&db, &petal_id, &expected_scope, &node_ids, &local_did).await {
                            Ok(materials) => DbResult::SceneMaterialsLoaded { materials, correlation_id },
                            Err(error) => DbResult::RequestFailed { error: format!("Read scene materials failed: {error}"), correlation_id },
                        };
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ReadNodeHistory { node_id, after_version, limit, at_version, grant, correlation_id }) => {
                        let reply = handlers::node_history::read_reply(&db,&node_id,after_version,limit,at_version,&grant,&local_did,correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::CreateObject { petal_id, input, actor_did, grant, correlation_id }) => {
                        let petal_scope = handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await.ok().flatten();
                        if let Err(error) = require_granted_node_role(&db, &grant, &local_did, petal_scope, fe_policy::RoleLevel::Editor, "Object creation").await {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Create object refused: {error}")));
                            continue;
                        }
                        let actor = object_command_actor(&grant,&local_did,&actor_did);
                        let outcome = handlers::node_history::create_object_granted(&db, &petal_id, &input.name, input.asset_id.as_deref(), input.position,
                            input.rotation.iter().map(|v|f64::from(*v)).collect(), input.scale.iter().map(|v|f64::from(*v)).collect(), input.properties, actor,&grant).await;
                        match outcome {
                            Ok((node_id, asset_path)) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    let quat = bevy::math::Quat::from_euler(bevy::math::EulerRot::XYZ, input.rotation[0], input.rotation[1], input.rotation[2]);
                                    if ect.send(fe_runtime::messages::SceneChange::NodeAdded { node: fe_runtime::messages::NodeDto {
                                        node_id:node_id.clone(),petal_id:petal_id.clone(),name:input.name.clone(),position:input.position,
                                        rotation:quat.to_array(),scale:input.scale,has_asset:input.asset_id.is_some(),asset_path:asset_path.clone(),
                                    }}).is_err() { tracing::warn!(node_id,"created object scene-change receiver closed"); }
                                }
                                let reply = match handlers::node_history::receipt(&db,&node_id).await {
                                    Ok(value) => {
                                        match handlers::node_history::materialization(&value,asset_path) {
                                            Ok(node) => send_result(&tx,DbResult::NodeMaterialized {node,correlation_id:ulid::Ulid::new().to_string()}),
                                            Err(error) => tracing::warn!(node_id,"created object GUI materialization could not decode: {error}"),
                                        }
                                        DbResult::NodeMutationCommitted {receipt:value,correlation_id}
                                    }
                                    Err(error) => handlers::correlated_failure(&correlation_id,format!("Object created; receipt read failed: {error}")),
                                };
                                send_result(&tx,reply);
                            }
                            Err(error) => send_result(&tx,handlers::correlated_failure(&correlation_id,format!("Create object refused: {error}"))),
                        }
                    }
                    Ok(DbCommand::PromoteCheckedStamp {capture,correlation_id}) => {
                        let outcome=async {
                            let prepared=handlers::stamp_promotion::prepare(&db,&capture,&correlation_id,&local_did).await?;
                            handlers::stamp_promotion::commit(&db,prepared).await
                        }.await;
                        match outcome {
                            Ok((node_id,properties,newly_promoted))=>{
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if newly_promoted {
                                    if let Some(address)=lifecycle_uri(&capture.scope,&node_id) {
                                        emit_lifecycle(&lifecycle_tx,fe_runtime::messages::LifecycleEvent::NodePromoted {
                                            address,node_id:node_id.clone(),path_id:capture.path_id.clone(),instance_index:capture.instance_index,
                                        });
                                    }
                                }
                                send_result(&tx,DbResult::CheckedStampPromoted {capture,node_id,properties,newly_promoted,correlation_id});
                            }
                            Err(error)=>send_result(&tx,DbResult::RequestFailed {error:format!("Stamp selection promotion refused: {error:#}"),correlation_id}),
                        }
                    }
                    Ok(DbCommand::CommitStampOverrides {edit,correlation_id}) => {
                        let outcome=async {
                            let prepared=handlers::stamp_overrides::prepare(&db,&edit,&correlation_id,&local_did).await?;
                            handlers::stamp_overrides::commit(&db,prepared).await
                        }.await;
                        match outcome {
                            Ok(receipt)=>{
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&receipt.node_id).await;
                                if let Some(ref ect)=entity_change_tx {
                                    for key in ["stamp.override.scale","stamp.override.rotation","stamp.override.arc_m"] {
                                        if ect.send(fe_runtime::messages::SceneChange::PropertyChanged {node_id:receipt.node_id.clone(),petal_id:receipt.state.petal_id.clone(),key:key.into(),value:receipt.state.properties[key].clone()}).is_err() {
                                            tracing::warn!(node_id=receipt.node_id,"committed stamp scene receiver closed");
                                        }
                                    }
                                }
                                send_result(&tx,DbResult::NodeMutationCommitted {receipt,correlation_id:Some(correlation_id)});
                            }
                            Err(error)=>send_result(&tx,DbResult::RequestFailed {error:format!("Stamp edit refused: {error:#}"),correlation_id}),
                        }
                    }
                    Ok(DbCommand::ReadSceneAssetPublication {request,correlation_id}) => {
                        let result=handlers::scene_asset_audit::read(&db,&request,&correlation_id,&local_did).await;
                        send_result(&tx,DbResult::SceneAssetPublicationRead {correlation_id,result});
                    }
                    Ok(DbCommand::ReadSceneAssetAdoption {node_id,request_id}) => {
                        let result=if request_id.is_empty() || request_id.len()>128 {Err(anyhow::anyhow!("invalid adoption request"))}
                            else {handlers::scene_asset_adoption::read(&db,&blob_store,&node_id,&local_did).await};
                        send_result(&tx,DbResult::SceneAssetAdoptionCandidate {node_id,request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::AdoptSceneAsset {candidate,request_id}) => {
                        let result=handlers::scene_asset_adoption::adopt(&db,&blob_store,&candidate,&local_did,&request_id).await;
                        if result.is_ok() {
                            handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&candidate.node_id).await;
                        }
                        send_result(&tx,DbResult::SceneAssetAdopted {request_id,result:result.map_err(|error|error.to_string())});
                    }
                    Ok(DbCommand::InspectSceneAssetTransfer {request,reply}) => {
                        let result=handlers::scene_asset_transfer::inspect(&db,&request,&local_did).await
                            .map_err(|error|{tracing::debug!("scene asset admission refused: {error:#}");"Scene asset is unavailable, changed or unauthorized".to_owned()});
                        if reply.try_send(result).is_err() { tracing::debug!("Scene asset admission owner expired"); }
                    }
                    Ok(DbCommand::ReadReplicatedNodeCurrent {node_id,request_id}) => {
                        let result=async {
                            anyhow::ensure!(!node_id.is_empty() && node_id.len()<=1024 && !request_id.is_empty() && request_id.len()<=128,"invalid replica read request");
                            let scope=handlers::crud::resolve_node_scope_handler(&db,&node_id).await?.ok_or_else(||anyhow::anyhow!("replica current target unavailable"))?;
                            let verse=crate::parse_scope(&scope)?.verse_id;
                            handlers::scene_publication::read_current_and_need(&db,&blob_store,&verse,&node_id,&local_did).await
                        }.await.map_err(|error|{tracing::warn!(node_id,"private replica current read deferred: {error:#}");"Current shared object is unavailable or changed; refresh before retrying.".to_owned()});
                        let (result,needed)=match result {Ok((current,needed))=>(Ok(current),needed),Err(error)=>(Err(error),None)};
                        send_result(&tx,DbResult::ReplicatedNodeCurrentRead {node_id,request_id,result});
                        if let Some(needed)=needed {send_result(&tx,DbResult::SceneAssetNeeded {needed});}
                    }
                    Ok(DbCommand::ReadEditorBatch {scope,petal_id,node_ids,target,correlation_id}) => {
                        let reply=match handlers::editor_batch::read_target(&db,&scope,&petal_id,&node_ids,&target,&local_did).await {
                            Ok(capture)=>DbResult::EditorBatchLoaded {capture,correlation_id},
                            Err(error)=>DbResult::RequestFailed {error:format!("Selection read refused: {error:#}"),correlation_id},
                        };
                        send_result(&tx,reply);
                    }
                    Ok(DbCommand::CommitCheckedEditorBatch {request,correlation_id}) => {
                        let result=async {
                            let prepared=handlers::editor_batch::prepare(&db,&request,&correlation_id,&local_did).await?;
                            let writes_registry=prepared.writes_registry();
                            handlers::editor_batch::commit(&db,prepared).await.map(|receipt|(receipt,writes_registry))
                        }.await;
                        let reply=match result {
                            Ok((receipt,writes_registry))=>{
                                for member in &receipt.members {
                                    handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&member.node_id).await;
                                }
                                // QA-0123: a group create/delete/undo also changed the petal's registry.
                                if writes_registry {
                                    handlers::visibility_groups::publish_registry(&db,repl_tx.as_ref(),&blob_store,&receipt.petal_id).await;
                                }
                                DbResult::EditorBatchCommitted {receipt,correlation_id}
                            },
                            Err(error)=>DbResult::RequestFailed {error:format!("Selection edit refused: {error:#}"),correlation_id},
                        };
                        send_result(&tx,reply);
                    }
                    Ok(DbCommand::ResolveCheckedEditorBatch {scope,petal_id,node_ids,target,request_id,correlation_id}) => {
                        let reply=match handlers::editor_batch::resolve_target(&db,&scope,&petal_id,&node_ids,&target,&request_id,&local_did).await {
                            Ok(resolution)=>DbResult::EditorBatchResolved {resolution,correlation_id},
                            Err(error)=>DbResult::RequestFailed {error:format!("Selection reconciliation refused: {error:#}"),correlation_id},
                        };
                        send_result(&tx,reply);
                    }
                    Ok(DbCommand::CommitCheckedNodeMutation {node_id,mutation,expected,correlation_id}) => {
                        match handlers::checked_mutation::commit(&db,&node_id,&mutation,&expected,&correlation_id,&local_did).await {
                            Ok(receipt)=>{
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect)=entity_change_tx {
                                    let change=match &mutation {
                                        fe_runtime::messages::NodeMutation::FullTransform {position,rotation,scale}=>fe_runtime::messages::SceneChange::NodeTransform {node_id:node_id.clone(),petal_id:receipt.state.petal_id.clone(),position:*position,rotation:*rotation,scale:*scale},
                                        fe_runtime::messages::NodeMutation::PropertySet {key,value}=>fe_runtime::messages::SceneChange::PropertyChanged {node_id:node_id.clone(),petal_id:receipt.state.petal_id.clone(),key:key.clone(),value:value.clone()},
                                        _=>unreachable!(),
                                    };
                                    if ect.send(change).is_err() {tracing::warn!(node_id,"checked mutation scene receiver closed");}
                                }
                                send_result(&tx,DbResult::NodeMutationCommitted {receipt,correlation_id:Some(correlation_id)});
                            }
                            Err(error)=>send_result(&tx,DbResult::RequestFailed {error:format!("Edit refused: {error:#}"),correlation_id}),
                        }
                    }
                    Ok(DbCommand::ResolveCheckedNodeMutation {node_id,petal_id,request_id,correlation_id})=>{
                        match handlers::checked_mutation::resolve(&db,&node_id,&petal_id,&request_id,&local_did).await {
                            Ok((receipt,current))=>send_result(&tx,DbResult::CheckedNodeMutationResolved {node_id,request_id,receipt,current,correlation_id}),
                            Err(error)=>send_result(&tx,DbResult::RequestFailed {error:format!("Edit reconciliation unavailable: {error:#}"),correlation_id}),
                        }
                    }
                    Ok(DbCommand::CommitNodeMutation { node_id, mutation, actor_did, grant, correlation_id }) => {
                        let node_scope = handlers::crud::resolve_node_scope_handler(&db, &node_id).await.ok().flatten();
                        if let Err(error) = require_granted_node_role(&db, &grant, &local_did, node_scope, fe_policy::RoleLevel::Editor, "Node mutation").await {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Node mutation refused: {error}")));
                            continue;
                        }
                        let actor = object_command_actor(&grant,&local_did,&actor_did);
                        use fe_runtime::messages::NodeMutation;
                        let outcome = match &mutation {
                            NodeMutation::PropertySet { key, value } => handlers::entity_property::set_entity_property_granted(&db, &node_id, key, value, actor,&grant).await,
                            NodeMutation::PropertyDeleted { key } => handlers::entity_property::delete_entity_property_granted(&db, &node_id, key, actor,&grant).await,
                            NodeMutation::FullTransform { position, rotation, scale } => handlers::transform::update_node_transform_granted(&db, &node_id, *position, *rotation, *scale, actor,&grant).await,
                        };
                        match outcome {
                            Ok(()) => {
                                handlers::scene_publication::publish_after_commit(&db,&blob_store,repl_tx.as_ref(),&local_did,&node_id).await;
                                if let Some(ref ect) = entity_change_tx {
                                    if let Ok(Some(petal_id)) = handlers::crud::resolve_node_petal_id_handler(&db, &node_id).await {
                                        let change = match &mutation {
                                            NodeMutation::PropertySet { key, value } => fe_runtime::messages::SceneChange::PropertyChanged { node_id: node_id.clone(), petal_id, key: key.clone(), value: value.clone() },
                                            NodeMutation::PropertyDeleted { key } => fe_runtime::messages::SceneChange::PropertyChanged { node_id: node_id.clone(), petal_id, key: key.clone(), value: serde_json::Value::Null },
                                            NodeMutation::FullTransform { position, rotation, scale } => fe_runtime::messages::SceneChange::NodeTransform { node_id: node_id.clone(), petal_id, position: *position, rotation: *rotation, scale: *scale },
                                        };
                                        if ect.send(change).is_err() { tracing::warn!(node_id, "committed node scene-change receiver closed"); }
                                    }
                                }
                                let reply = match handlers::node_history::receipt(&db, &node_id).await {
                                    Ok(receipt) => DbResult::NodeMutationCommitted { receipt, correlation_id },
                                    Err(error) => handlers::correlated_failure(&correlation_id, format!("Mutation committed; receipt read failed: {error}")),
                                };
                                send_result(&tx, reply);
                            }
                            Err(error) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("Node mutation refused: {error}"))),
                        }
                    }
                    Ok(DbCommand::DeleteFieldDef { field_def_id }) => {
                        match handlers::field_def::delete_field_def_handler(&db, &field_def_id).await {
                            Ok(()) => send_result(&tx, DbResult::FieldDefDeleted { field_def_id }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Delete field def failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ScopedQuery { sql, vars, petal_scope, correlation_id }) => {
                        let reply = handlers::scoped_query::scoped_query_reply(&db, &sql, &vars, &petal_scope, &local_did, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::RawQuery { sql, vars, correlation_id }) => {
                        let reply = handlers::raw_query::raw_query_reply(&db, &sql, &vars, correlation_id).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::ElevatedQuery { statements, vars, write_tables, actor_did, correlation_id }) => {
                        let reply = handlers::elevated_query::elevated_query_reply(
                            &db,
                            handlers::elevated_query::ElevatedSinks {
                                blob_store: &blob_store,
                                repl_tx: repl_tx.as_ref(),
                                entity_change_tx: entity_change_tx.as_ref(),
                            },
                            handlers::elevated_query::ElevatedRequest { statements, vars, write_tables, actor_did, correlation_id },
                        ).await;
                        send_result(&tx, reply);
                    }
                    // M2/F7 (A15): a distributed-query local partial. The
                    // spec is structured; the handler renders its own SQL
                    // via the fe-query builders (bound params only), so no
                    // SQL crosses the channel. The reply rides the embedded
                    // crossbeam sender, NOT the DbResult channel — this
                    // command has no reply family and must never be awaited
                    // through PendingApiRequests (see fe-runtime AGENTS.md
                    // §api-reply-correlation). The row cap is enforced in
                    // the handler against the caller's cap.
                    Ok(DbCommand::ExecuteTsPartial { verse_id, spec, shards, row_cap, reply }) => {
                        // RP-6 §2.6: the deferred-admission queue reports which
                        // requested shards still hold deferred readings here
                        // (OF-7b: scoped to the query's verse).
                        match handlers::ts_partial::execute_ts_partial(
                            &db, &verse_id, &spec, &shards,
                            row_cap,
                            &admissions,
                        ).await {
                            Ok(partial) => {
                                if let Err(e) = reply.send(partial) {
                                    tracing::warn!("ExecuteTsPartial reply send failed (requester gone): {e:?}");
                                }
                            }
                            Err(reason) => {
                                // F23 honesty rule: a FAILED local partial must
                                // never masquerade as an authoritative
                                // covered-empty answer — reply the failed flag
                                // so the merge reports the host in
                                // missing_hosts and its shards in
                                // missing_shards (A16 reads "host failed",
                                // never "covered").
                                tracing::warn!(reason, "ExecuteTsPartial failed");
                                if let Err(e) = reply.send(fe_runtime::distributed_query::TsPartialRows::failed()) {
                                    tracing::warn!("ExecuteTsPartial error-reply send failed: {e:?}");
                                }
                            }
                        }
                    }
                    // M2/F7: the DB-thread-originated IoT ingestion leg —
                    // the same emit-seam handler the fe-api db_reader path
                    // calls (durable-first, then one ReplicationEvent per
                    // accepted row; see §iot-readings). Harness peers and
                    // the sim lab ingest through this arm.
                    Ok(DbCommand::InsertIotReadings { petal_id, verse_id, source_did, readings, source_class, correlation_id }) => {
                        // AC-46: a replicated write needs the NODE's own role too.
                        let refused = match replicated_petal_scope(&db, verse_id.as_deref(), &petal_id).await {
                            Some(scope) => require_node_role(&db, &local_did, &scope, fe_policy::RoleLevel::Editor, "InsertIotReadings").await.err(),
                            None => None,
                        };
                        if let Some(refusal) = refused {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, refusal));
                        } else {
                            match handlers::iot_reading::insert_readings_with_replication(
                                &db,
                                &petal_id,
                                verse_id.as_deref(),
                                &source_did,
                                source_class,
                                &readings,
                                Some(&blob_store),
                                repl_tx.as_ref(),
                            ).await {
                                Ok(written) => send_result(&tx, DbResult::IotReadingsInserted { petal_id, written, correlation_id }),
                                // F24: a validation failure keeps its typed shape
                                // across the seam (`IotReadingsRejected`, the
                                // 422-class family) so the API fallback maps real
                                // HTTP statuses; only DB failures degrade to the
                                // generic `Error` reply.
                                Err(e) => match e.validation_rejection() {
                                    Some(reason) => send_result(&tx, DbResult::IotReadingsRejected { petal_id, reason, correlation_id }),
                                    None => send_result(&tx, DbResult::Error(format!("IoT readings ingest failed: {e}"))),
                                },
                            }
                        }
                    }
                    // --- R5 device formats / payloads (CDL-SPEC10 A3) ---
                    Ok(DbCommand::RegisterDeviceFormat { verse_id, descriptor, declared_by, correlation_id }) => {
                        // AC-46: a peer admits a device_format only from a verse Manager.
                        let verse_scope = build_scope(&verse_id, None, None);
                        if let Err(refusal) = require_node_role(&db, &local_did, &verse_scope, fe_policy::RoleLevel::Manager, "RegisterDeviceFormat").await {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, refusal));
                        } else {
                            match handlers::device_payload::register_format_with_replication(
                                &db, &verse_id, &descriptor, &declared_by, Some(&blob_store), repl_tx.as_ref(),
                            ).await {
                                Ok(()) => send_result(&tx, DbResult::DeviceFormatRegistered {
                                    verse_id, format_id: descriptor.format_id, correlation_id,
                                }),
                                Err(e) => match e.rejection() {
                                    Some(reason) => send_result(&tx, DbResult::DeviceFormatRejected { verse_id, reason, correlation_id }),
                                    None => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("RegisterDeviceFormat failed: {e}"))),
                                },
                            }
                        }
                    }
                    Ok(DbCommand::ListDeviceFormats { verse_id, correlation_id }) => {
                        match handlers::device_payload::list_device_formats(&db, &verse_id).await {
                            Ok(formats) => send_result(&tx, DbResult::DeviceFormatsListed { verse_id, formats, correlation_id }),
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("ListDeviceFormats failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::InsertDevicePayloads { petal_id, verse_id, source_did, source_class, payload, correlation_id }) => {
                        // AC-46: a replicated write needs the NODE's own role too.
                        let refused = match replicated_petal_scope(&db, verse_id.as_deref(), &petal_id).await {
                            Some(scope) => require_node_role(&db, &local_did, &scope, fe_policy::RoleLevel::Editor, "InsertDevicePayloads").await.err(),
                            None => None,
                        };
                        if let Some(refusal) = refused {
                            send_result(&tx, handlers::correlated_failure(&correlation_id, refusal));
                        } else {
                            match handlers::device_payload::insert_device_payloads(
                                &db, &petal_id, verse_id.as_deref(), &source_did, source_class, &payload,
                                Some(&blob_store), repl_tx.as_ref(),
                            ).await {
                                Ok((payload_id, asset_ids)) => send_result(&tx, DbResult::DevicePayloadsInserted { petal_id, payload_id, asset_ids, correlation_id }),
                                Err(e) => match e.rejection() {
                                    Some(reason) => send_result(&tx, DbResult::DevicePayloadsRejected { petal_id, reason, correlation_id }),
                                    None => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("InsertDevicePayloads failed: {e}"))),
                                },
                            }
                        }
                    }
                    Ok(DbCommand::ListDevicePayloads { petal_id, node_id, format_id, from_ms, to_ms, correlation_id }) => {
                        match handlers::device_payload::list_device_payloads(
                            &db, &petal_id, node_id.as_deref(), format_id.as_deref(), from_ms, to_ms,
                        ).await {
                            Ok(payloads) => send_result(&tx, DbResult::DevicePayloadsListed { petal_id, payloads, correlation_id }),
                            Err(e) => send_result(&tx, handlers::correlated_failure(&correlation_id, format!("ListDevicePayloads failed: {e}"))),
                        }
                    }
                    // --- Hexon crate registry (Phase 8) ---
                    Ok(DbCommand::InstallCrate { hexon_uri, manifest_hash, publisher_did, hexon_type, version, name, tags, petal_id, size_bytes }) => {
                        let tx = tx.clone();
                        match handlers::crate_registry::install_crate(
                            &db, &hexon_uri, &manifest_hash, &publisher_did, &hexon_type,
                            &version, &name, &tags, &petal_id, size_bytes,
                        ).await {
                            Ok(()) => send_result(&tx, DbResult::CrateInstalled { hexon_uri, petal_id }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Install crate failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::InstallCrateEntry { entry_id, hexon_uri, kind, asset_hash, format, label, metadata }) => {
                        let tx = tx.clone();
                        match handlers::crate_registry::install_crate_entry(
                            &db, &entry_id, &hexon_uri, &kind, &asset_hash, &format, &label, &metadata,
                        ).await {
                            Ok(()) => send_result(&tx, DbResult::CrateEntryInstalled { entry_id, hexon_uri }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Install crate entry failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::UninstallCrate { hexon_uri }) => {
                        let tx = tx.clone();
                        match handlers::crate_registry::uninstall_crate(&db, &hexon_uri).await {
                            Ok(()) => send_result(&tx, DbResult::CrateUninstalled { hexon_uri }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Uninstall crate failed: {e}"))),
                        }
                    }
                    // CDL-R2 (W4-8): self-signed device enrolment, published as an
                    // ordinary replicated row (canon_log/AGENTS.md §device-enrolment).
                    Ok(DbCommand::EnrolLocalDevice { verse_id, correlation_id }) => {
                        let reply = canon_log::device_enrolment::enrol_local_device_reply(
                            &db, &blob_store, repl_tx.as_ref(), keypair.as_ref(), secret_store.as_ref(), verse_id, correlation_id,
                        ).await;
                        send_result(&tx, reply);
                    }
                    Ok(DbCommand::RevokeLocalDevice { verse_id, device_key_hex, correlation_id }) => {
                        let reply = canon_log::device_enrolment::revoke_local_device_reply(
                            &db, &blob_store, repl_tx.as_ref(), keypair.as_ref(), secret_store.as_ref(), verse_id, device_key_hex, correlation_id,
                        ).await;
                        send_result(&tx, reply);
                    }
                    // A4: inbound replicated row — applied on the DB thread
                    // (the single SurrealDB writer). The handler takes no
                    // replication sender and this arm passes none, so an
                    // applied row can never re-enter the outbound bridge
                    // (loop-free by construction). A3: the handler gates
                    // admission on the author's role (fe-policy, Editor+);
                    // RP-3: `InboundAdmissions` queues a Deferred content row and
                    // retries every row the apply unblocked — one echo per row settled.
                    Ok(DbCommand::ApplyReplicatedRow { verse_id, table, record_id, row_bytes, author_did }) => {
                        handlers::scene_publication::echo_applied(&db,&blob_store,&local_did,&tx,admissions
                                .apply(
                                    &db,
                                    &verse_id,
                                    &table,
                                    &record_id,
                                    &row_bytes,
                                    &author_did,
                                    entity_change_tx.as_ref(),
                                )
                                .await).await;
                    }
                    Ok(DbCommand::RejudgeReplicatedNodes { verse_id, nodes, restore_only }) => {
                        use handlers::scene_publication::Rejudged;
                        for (record_id, candidates) in nodes {
                            match handlers::scene_publication::rejudge(&db, &verse_id, &record_id, &candidates, &local_did, entity_change_tx.as_ref(), restore_only).await {
                                Ok(Rejudged::Unchanged) => {}
                                Ok(Rejudged::Restored(outcome)) => {
                                    tracing::info!(verse_id, node_id = record_id, ?outcome, "Re-judged a scene row after a role change: applied the latest admissible version");
                                    handlers::scene_publication::echo_applied(&db, &blob_store, &local_did, &tx, vec![handlers::inbound_admissions::RowReport {
                                        verse_id: verse_id.clone(), table: "node".into(), record_id, outcome,
                                    }]).await;
                                }
                                Ok(Rejudged::Removed { petal_id }) => {
                                    tracing::info!(verse_id, node_id = record_id, "Re-judged a scene row after a role change: no admissible version, removed locally");
                                    send_result(&tx, DbResult::NodeDeleted { node_id: record_id, petal_id });
                                }
                                Err(error) => tracing::warn!(verse_id, node_id = record_id, "scene re-judge failed: {error:#}"),
                            }
                        }
                    }
                    Ok(DbCommand::ReemitLocalVerseCatalog { verse_id }) => {
                        match handlers::local_catalog::reemit(
                            &db, &blob_store, repl_tx.as_ref(), &local_did, &verse_id,
                        ).await {
                            Ok(emitted) => tracing::debug!(verse_id, emitted, "re-emitted stored local Verse catalog on replica open"),
                            Err(error) => tracing::warn!(verse_id, "local Verse catalog recovery refused or failed: {error:#}"),
                        }
                        match handlers::scene_publication::recover(&db,&blob_store,repl_tx.as_ref(),&local_did,&verse_id).await {
                            Ok(report) if report.deferred==0 && report.dependency_deferred==0 => tracing::info!(verse_id,examined=report.examined,enqueued=report.enqueued,"stored scene recovery enqueued; peer delivery is not certified"),
                            Ok(report) => tracing::warn!(verse_id,examined=report.examined,enqueued=report.enqueued,deferred=report.deferred,dependency_deferred=report.dependency_deferred,"stored scene recovery incomplete; dependencies or locally unproven rows remain deferred"),
                            Err(error) => tracing::warn!(verse_id,"stored scene recovery refused or incomplete: {error:#}"),
                        }
                    }
                    Ok(DbCommand::ReemitLocalRoleStatements { verse_id }) => {
                        match role_grants::reemit_local_statements(
                            &db,
                            repl_tx.as_ref(),
                            &blob_store,
                            &local_did,
                            Some(&verse_id),
                        )
                        .await
                        {
                            Ok(n) => tracing::debug!(verse_id, emitted = n, "re-emitted local role statements on replica open"),
                            Err(e) => tracing::warn!(verse_id, "role statement re-emit failed: {e:#}"),
                        }
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
                tracing::debug!(elapsed_ms = %cmd_start.elapsed().as_millis(), "DB command processed");
            }
            tracing::info!("Database thread shutting down");
            // Release the store while this runtime can still drive the
            // engine's shutdown, so a reopen of the same path succeeds. A live
            // shared clone keeps the router up until this runtime drops at
            // thread exit (src/AGENTS.md §shared-handle).
            drop(db);
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            send_result(&tx, DbResult::Stopped);
        });
    });
    let shared_db = startup_rx.recv().map_err(|_| {
        DbInitError::RuntimeBuild(std::io::Error::other(
            "DB thread startup channel closed (thread may have panicked)",
        ))
    })??;
    Ok(DbThread {
        join_handle: handle,
        shared_db,
    })
}

// ---------------------------------------------------------------------------
// Public utilities (used by handlers and external crates)
// ---------------------------------------------------------------------------

/// Derive the namespace ID bytes from a namespace secret — the **actual
/// iroh-docs namespace id**: the Ed25519 public key the secret's signing key
/// derives to (`iroh_docs::NamespaceSecret::from_bytes(secret).id()`), NOT a
/// private digest.
///
/// History (F20/M1 scrutiny finding 3): this used to be a keyed-BLAKE3
/// digest, which can never equal the Ed25519 id iroh-docs registers a
/// document under — so a verse imported with its secret could never be
/// reopened read-only by the stored id (the docs client's `open` missed). Verse rows
/// written before the fix still carry BLAKE3 hex in `namespace_id`;
/// fe-sync's secretless open falls back to scanning known docs for the
/// verse's manifest row on a stored-id miss (compatibility story). New
/// verses store this id, which `iroh_docs::api::DocsApi::open` (iroh-docs 0.101;
/// `Client::open` on 0.35) resolves directly.
///
/// Same seed → same Ed25519 keypair, so the derivation is deterministic and
/// identical across peers with the same secret.
pub fn derive_namespace_id(secret: &[u8; 32]) -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(secret)
        .verifying_key()
        .to_bytes()
}

/// Retrieve a namespace secret from the secret store. Returns `None` if not found.
///
/// Validates that the stored value is exactly 64 hex characters (a 32-byte secret).
pub fn get_namespace_secret(
    store: &dyn fe_identity::SecretStore,
    verse_id: &str,
) -> anyhow::Result<Option<String>> {
    let service = format!("fractalengine:verse:{}:ns_secret", verse_id);
    match store.get(&service, "fractalengine") {
        Ok(Some(secret_hex)) => {
            if secret_hex.len() != 64 {
                anyhow::bail!("Namespace secret has invalid length: {}", secret_hex.len());
            }
            hex::decode(&secret_hex)
                .map_err(|e| anyhow::anyhow!("Namespace secret is not valid hex: {e}"))?;
            Ok(Some(secret_hex))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}

#[cfg(test)]
mod namespace_id_tests {
    use super::*;

    /// F20/M1 finding 3: the derived id must BE the iroh-docs namespace id
    /// — the Ed25519 public key of the secret's signing key (what
    /// `NamespaceSecret::from_bytes(secret).id()` yields in iroh-docs), not a
    /// private digest that `DocsApi::open` can never resolve. Same-key
    /// equivalence is pinned cross-crate in fe-sync's
    /// `derive_namespace_id_matches_iroh_namespace_id`.
    #[test]
    fn derive_namespace_id_is_the_secret_ed25519_public_key() {
        let secret = [7u8; 32];
        let expected = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();
        assert_eq!(derive_namespace_id(&secret), expected);
    }

    #[test]
    fn derive_namespace_id_is_deterministic_and_secret_sensitive() {
        let a = derive_namespace_id(&[7u8; 32]);
        let b = derive_namespace_id(&[7u8; 32]);
        let c = derive_namespace_id(&[8u8; 32]);
        assert_eq!(a, b, "same secret → same namespace id");
        assert_ne!(a, c, "different secrets → different namespace ids");
    }

    /// The id must never equal the secret itself (it is the PUBLIC key half).
    #[test]
    fn derive_namespace_id_is_public_not_secret() {
        let secret = [9u8; 32];
        assert_ne!(derive_namespace_id(&secret), secret);
    }
}

#[cfg(test)]
mod join_verse_dispatch_tests {
    use super::*;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;
    use std::time::Duration;

    /// polish_lane_20261031 W1-S2 (M-03/OD-2): a garbage invite string must
    /// reply the typed `JoinVerseFailed`, not a bare `Error` — pins the
    /// contract end to end through the real DB-thread dispatch loop (an
    /// isolated temp-dir DB, so no cross-test file-lock contention with
    /// `fe-database/tests/db_test.rs`'s shared scene DB).
    #[test]
    fn join_with_invalid_invite_replies_join_verse_failed() {
        let tmp_dir = tempfile::tempdir().expect("temp dir for test DB");
        let db_path = tmp_dir
            .path()
            .join("join_fail.db")
            .to_string_lossy()
            .to_string();
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(8);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(8);

        let _handle = spawn_db_thread_with_sync(
            cmd_rx,
            res_tx,
            Arc::new(MockBlobStore::new()),
            None,
            None,
            None,
            None,
            Some(db_path),
        )
        .expect("DB thread init");

        let started = res_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("DbResult::Started");
        assert!(matches!(started, DbResult::Started));

        cmd_tx
            .send(DbCommand::JoinVerseByInvite {
                invite_string: "not-a-real-invite".to_string(),
            })
            .expect("send JoinVerseByInvite");

        let result = res_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("JoinVerseFailed reply");
        match result {
            DbResult::JoinVerseFailed { error } => {
                assert!(!error.is_empty(), "error text must be non-empty");
            }
            other => panic!("expected JoinVerseFailed, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase C: base64 → blob store migration (startup only)
// ---------------------------------------------------------------------------

/// Migrate legacy asset rows that store file content as base64 in the `data`
/// column to the content-addressed blob store.
async fn migrate_base64_assets_to_blob_store(
    db: &crate::repo::Db,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<(usize, usize)> {
    tracing::info!("Checking for base64 assets to migrate...");

    let mut res: surrealdb::IndexedResults = db
        .query("SELECT * FROM asset WHERE content_hash IS NONE AND data IS NOT NONE")
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;

    if rows.is_empty() {
        return Ok((0, 0));
    }

    tracing::info!("Found {} legacy base64 asset(s) to migrate", rows.len());

    let mut migrated_count: usize = 0;
    let mut total_bytes: usize = 0;

    for row in &rows {
        let asset_id = match row["asset_id"].as_str() {
            Some(id) => id,
            None => {
                tracing::warn!("Skipping asset row with missing asset_id");
                continue;
            }
        };
        let data_str = match row["data"].as_str() {
            Some(d) => d,
            None => {
                tracing::warn!("Skipping asset {asset_id}: data field is not a string");
                continue;
            }
        };

        let bytes =
            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_str) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("Skipping asset {asset_id}: base64 decode failed: {e}");
                    continue;
                }
            };

        let byte_len = bytes.len();

        let hash = match blob_store.add_blob(&bytes) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("Skipping asset {asset_id}: blob_store.add_blob failed: {e}");
                continue;
            }
        };
        let hex = hash_to_hex(&hash);

        if let Err(e) = db
            .query("UPDATE asset SET content_hash = $hash, data = NONE WHERE asset_id = $aid")
            .bind(("hash", hex.clone()))
            .bind(("aid", asset_id.to_string()))
            .await
        {
            tracing::error!("Skipping asset {asset_id}: DB UPDATE failed: {e}");
            continue;
        }

        tracing::info!("Migrated asset {asset_id} ({byte_len} bytes) → content_hash={hex}");
        migrated_count += 1;
        total_bytes += byte_len;
    }

    Ok((migrated_count, total_bytes))
}

#[cfg(test)]
mod subdir_tests {
    use super::*;

    /// The backfill must read where the pre-blob import path wrote; a moved
    /// path would make the startup backfill silently find nothing.
    #[test]
    fn legacy_imported_dir_is_where_old_imports_were_written() {
        let p = imported_assets_dir();
        assert!(
            p.ends_with("imported"),
            "expected path to end with 'imported', got {}",
            p.display()
        );
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.contains("fractalengine/assets/imported"), "got: {s}");
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::repo::{Db, Repo, Table};
    use crate::schema::Asset;
    use base64::Engine as _;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    /// Helper: create an in-memory SurrealDB instance with the asset schema applied.
    async fn setup_test_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        // Apply the asset table schema via the generated DDL
        db.query(schema::Asset::schema())
            .await
            .expect("define asset")
            .check()
            .expect("define asset check");
        db
    }

    /// Seed one legacy asset row with base64 data and no content_hash.
    async fn seed_legacy_asset(db: &Db, asset_id: &str, raw_bytes: &[u8]) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw_bytes);
        Repo::<Asset>::create_raw(
            db,
            serde_json::json!({
                "asset_id":     asset_id,
                "name":         "test-asset",
                "content_type": "model/gltf-binary",
                "size_bytes":   raw_bytes.len(),
                "data":         b64,
                "created_at":   "2025-01-01T00:00:00Z",
            }),
        )
        .await
        .expect("seed legacy asset");
    }

    #[tokio::test]
    async fn migrate_base64_asset_to_blob_store() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let raw = b"hello-glb-bytes";

        seed_legacy_asset(&db, "asset-1", raw).await;

        let (count, bytes) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("migration");
        assert_eq!(count, 1);
        assert_eq!(bytes, raw.len());

        // Verify content_hash is set in DB
        let mut res: surrealdb::IndexedResults = db
            .query("SELECT content_hash, data FROM asset WHERE asset_id = 'asset-1'")
            .await
            .expect("query");
        let rows: Vec<serde_json::Value> = res.take(0).expect("take");
        let row = &rows[0];
        assert!(
            row["content_hash"].is_string(),
            "content_hash should be set"
        );
        assert!(row["data"].is_null(), "data should be cleared (NONE)");

        // Verify blob exists in the store
        let ch = row["content_hash"].as_str().unwrap();
        let hash = hash_from_hex(ch).expect("valid hex");
        assert!(blob_store.has_blob(&hash), "blob should exist in store");
    }

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());

        seed_legacy_asset(&db, "asset-2", b"some-data").await;

        let (c1, _) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("first run");
        assert_eq!(c1, 1);

        // Second run should find nothing to migrate
        let (c2, b2) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("second run");
        assert_eq!(c2, 0);
        assert_eq!(b2, 0);
    }

    #[tokio::test]
    async fn migrate_skips_rows_with_content_hash() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());

        // Insert a row that already has content_hash (Phase B style)
        let _: Option<serde_json::Value> = db
            .create("asset")
            .content(serde_json::json!({
                "asset_id": "already-migrated",
                "name": "new-asset",
                "content_type": "model/gltf-binary",
                "size_bytes": 42,
                "content_hash": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                "created_at": "2025-01-01T00:00:00Z",
            }))
            .await
            .expect("seed new-style asset");

        let (count, bytes) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("migration");
        assert_eq!(count, 0);
        assert_eq!(bytes, 0);
    }
}
