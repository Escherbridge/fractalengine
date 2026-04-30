//! TestPeer: an isolated fractalengine peer for functional testing.
//!
//! Each TestPeer owns its own in-memory SurrealDB instance, blob store,
//! sync thread, and ed25519 identity. The DB thread is a simplified
//! reproduction of `fe-database::spawn_db_thread_with_sync` using the
//! `Mem` engine so tests avoid file-lock conflicts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam::channel::{Receiver, Sender};
use fe_database::BlobStoreHandle;
use fe_identity::NodeKeypair;
use fe_runtime::messages::{ApiTokenInfo, DbCommand, DbResult};
use fe_sync::messages::{SyncCommand, SyncEvent};
use fe_sync::{FsBlobStore, SyncCommandSender, SyncEventReceiver};

/// Default timeout for waiting on DB results.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// An isolated fractalengine peer instance for testing.
#[allow(dead_code)]
pub struct TestPeer {
    pub name: String,
    pub data_dir: PathBuf,
    pub blob_store: BlobStoreHandle,
    pub db_cmd_tx: Sender<DbCommand>,
    pub db_result_rx: Receiver<DbResult>,
    pub sync_cmd_tx: SyncCommandSender,
    pub sync_evt_rx: SyncEventReceiver,
    pub keypair: NodeKeypair,
    _db_thread: Option<JoinHandle<()>>,
    _sync_thread: Option<JoinHandle<()>>,
}

impl TestPeer {
    /// Spawn a new isolated peer.
    ///
    /// Each peer gets:
    /// - A unique temp subdirectory under `temp_dir/{name}/`
    /// - Its own FsBlobStore at `{temp_dir}/{name}/blobs/`
    /// - Its own in-memory SurrealDB instance
    /// - Its own sync thread with a unique iroh identity
    /// - Its own ed25519 keypair for invite signing
    pub fn spawn(name: &str, temp_dir: &Path) -> Result<Self> {
        let peer_dir = temp_dir.join(name);
        std::fs::create_dir_all(&peer_dir)
            .with_context(|| format!("create peer dir: {}", peer_dir.display()))?;

        let blob_dir = peer_dir.join("blobs");
        let blob_store: BlobStoreHandle = Arc::new(
            FsBlobStore::new(blob_dir).context("create blob store")?,
        );

        let keypair = NodeKeypair::generate();
        let local_did = keypair.to_did_key();

        // DB channels
        let (db_cmd_tx, db_cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (db_result_tx, db_result_rx) = crossbeam::channel::bounded::<DbResult>(64);

        // Replication channel (bridges DB writes into sync commands)
        let (_repl_tx, _repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(64);

        // Sync channels
        let (sync_cmd_tx, sync_cmd_rx) = crossbeam::channel::bounded::<SyncCommand>(64);
        let (sync_evt_tx, sync_evt_rx) = crossbeam::channel::bounded::<SyncEvent>(64);

        // Spawn sync thread
        let iroh_secret = keypair.to_iroh_secret();
        let sync_blob_store = blob_store.clone();
        let sync_handle = fe_sync::spawn_sync_thread(
            iroh_secret,
            sync_blob_store,
            sync_cmd_rx,
            sync_evt_tx,
            local_did.clone(),
        );

        // Spawn DB thread with in-memory SurrealDB
        let db_blob_store = blob_store.clone();
        let _db_keypair = NodeKeypair::generate(); // separate keypair for DB invite ops
        // We actually want to use the SAME keypair for invite generation,
        // so clone the seed bytes.
        let kp_seed = keypair.seed_bytes();
        let db_local_did = local_did.clone();
        let db_handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build DB Tokio runtime");

            rt.block_on(async move {
                // Use in-memory SurrealDB to avoid file lock conflicts
                let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
                    surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
                        .await
                        .expect("SurrealDB Mem init");
                db.use_ns("fractalengine")
                    .use_db("fractalengine")
                    .await
                    .expect("SurrealDB ns/db");

                // Apply schema
                fe_database::rbac::apply_schema(&db)
                    .await
                    .expect("Schema application failed");

                fe_database::api_token_store::apply_api_token_schema(&db)
                    .await
                    .expect("API token schema");

                tracing::info!("Peer DB ready (in-memory)");
                db_result_tx.send(DbResult::Started).ok();

                // Reconstruct keypair from seed for invite operations
                let invite_keypair = NodeKeypair::from_bytes(&kp_seed)
                    .expect("reconstruct keypair from seed");

                // Command loop (simplified version of fe-database::spawn_db_thread_with_sync)
                loop {
                    match db_cmd_rx.recv() {
                        Ok(DbCommand::Ping) => {
                            db_result_tx.send(DbResult::Pong).ok();
                        }
                        Ok(DbCommand::Seed) => {
                            match seed_test_data(&db, &db_local_did).await {
                                Ok((petal_name, rooms)) => {
                                    db_result_tx
                                        .send(DbResult::Seeded { petal_name, rooms })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!("Seed failed: {e}")))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::CreateVerse { name }) => {
                            match create_verse(&db, &name, &db_local_did).await {
                                Ok(id) => {
                                    db_result_tx
                                        .send(DbResult::VerseCreated { id, name })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Create verse failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::CreateFractal { verse_id, name }) => {
                            match create_fractal(&db, &verse_id, &name, &db_local_did).await {
                                Ok(id) => {
                                    db_result_tx
                                        .send(DbResult::FractalCreated { id, verse_id, name })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Create fractal failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::CreatePetal { fractal_id, name }) => {
                            match create_petal(&db, &fractal_id, &name, &db_local_did).await {
                                Ok(id) => {
                                    db_result_tx
                                        .send(DbResult::PetalCreated {
                                            id,
                                            fractal_id,
                                            name,
                                        })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Create petal failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::CreateNode {
                            petal_id,
                            name,
                            position,
                        }) => {
                            match create_node(&db, &petal_id, &name, position).await {
                                Ok(id) => {
                                    db_result_tx
                                        .send(DbResult::NodeCreated {
                                            id,
                                            petal_id,
                                            name,
                                            has_asset: false,
                                        })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Create node failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::ImportGltf {
                            petal_id,
                            name,
                            file_path,
                            position,
                        }) => {
                            match import_gltf(
                                &db,
                                &db_blob_store,
                                &petal_id,
                                &name,
                                &file_path,
                                position,
                            )
                            .await
                            {
                                Ok((node_id, asset_id, asset_path)) => {
                                    db_result_tx
                                        .send(DbResult::GltfImported {
                                            node_id,
                                            asset_id,
                                            petal_id,
                                            name,
                                            asset_path,
                                            position,
                                        })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "GLTF import failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::LoadHierarchy) => {
                            match load_hierarchy(&db).await {
                                Ok(verses) => {
                                    db_result_tx
                                        .send(DbResult::HierarchyLoaded { verses })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Load hierarchy failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::GenerateVerseInvite {
                            verse_id,
                            include_write_cap,
                            expiry_hours,
                        }) => {
                            match generate_invite(
                                &db,
                                &verse_id,
                                include_write_cap,
                                expiry_hours,
                                &invite_keypair,
                            )
                            .await
                            {
                                Ok(invite_string) => {
                                    db_result_tx
                                        .send(DbResult::VerseInviteGenerated {
                                            verse_id,
                                            invite_string,
                                        })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Generate invite failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::JoinVerseByInvite { invite_string }) => {
                            match join_verse(&db, &invite_string, &db_local_did).await {
                                Ok((verse_id, verse_name)) => {
                                    db_result_tx
                                        .send(DbResult::VerseJoined {
                                            verse_id,
                                            verse_name,
                                        })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Join verse failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::ResetDatabase) => {
                            match async {
                                fe_database::admin::clear_all_tables(&db).await?;
                                seed_test_data(&db, &db_local_did).await
                            }
                            .await
                            {
                                Ok((petal_name, rooms)) => {
                                    db_result_tx
                                        .send(DbResult::DatabaseReset { petal_name, rooms })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Database reset failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::UpdateNodeTransform { .. }) => {
                            // Not implemented in test harness.
                        }
                        Ok(DbCommand::UpdateNodeUrl { .. }) => {
                            // Not implemented in test harness.
                        }
                        Ok(DbCommand::RenameEntity { .. })
                        | Ok(DbCommand::SetVerseDefaultAccess { .. })
                        | Ok(DbCommand::UpdateFractalDescription { .. })
                        | Ok(DbCommand::DeleteEntity { .. })
                        | Ok(DbCommand::ResolveRolesForPeers { .. })
                        | Ok(DbCommand::AssignRole { .. })
                        | Ok(DbCommand::RevokeRole { .. })
                        | Ok(DbCommand::GenerateScopedInvite { .. })
                        | Ok(DbCommand::ResolveLocalRole { .. }) => {
                            // Entity settings commands — not implemented in test harness.
                        }
                        Ok(DbCommand::MintApiToken {
                            scope,
                            max_role,
                            ttl_hours,
                            label,
                        }) => {
                            let jti = ulid::Ulid::new().to_string();
                            let ttl_secs = (ttl_hours as u64) * 3600;
                            match fe_identity::api_token::mint_api_token(
                                &invite_keypair,
                                &scope,
                                &max_role,
                                ttl_secs,
                                &jti,
                            ) {
                                Ok(token) => {
                                    let now_ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs();
                                    let created_at = chrono::DateTime::from_timestamp(
                                        now_ts as i64,
                                        0,
                                    )
                                    .unwrap_or_default()
                                    .to_rfc3339();
                                    let expires_at = chrono::DateTime::from_timestamp(
                                        (now_ts + ttl_secs) as i64,
                                        0,
                                    )
                                    .unwrap_or_default()
                                    .to_rfc3339();

                                    let record =
                                        fe_database::api_token_store::ApiTokenRecord {
                                            jti: jti.clone(),
                                            scope: scope.clone(),
                                            max_role: max_role.clone(),
                                            label: label.clone(),
                                            sub: db_local_did.clone(),
                                            created_at: created_at.clone(),
                                            expires_at: expires_at.clone(),
                                            revoked: false,
                                        };
                                    match fe_database::api_token_store::store_api_token(
                                        &db, &record,
                                    )
                                    .await
                                    {
                                        Ok(()) => {
                                            db_result_tx
                                                .send(DbResult::ApiTokenMinted {
                                                    token,
                                                    jti,
                                                    scope,
                                                    max_role,
                                                    expires_at,
                                                    label,
                                                })
                                                .ok();
                                        }
                                        Err(e) => {
                                            db_result_tx
                                                .send(DbResult::Error(format!(
                                                    "Store API token failed: {e}"
                                                )))
                                                .ok();
                                        }
                                    }
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Mint API token failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::RevokeApiToken { jti }) => {
                            match fe_database::api_token_store::revoke_api_token(&db, &jti)
                                .await
                            {
                                Ok(true) => {
                                    db_result_tx
                                        .send(DbResult::ApiTokenRevoked { jti })
                                        .ok();
                                }
                                Ok(false) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "API token not found: {jti}"
                                        )))
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "Revoke API token failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::ListApiTokens) => {
                            match fe_database::api_token_store::list_active_tokens(
                                &db,
                                &db_local_did,
                            )
                            .await
                            {
                                Ok(records) => {
                                    let tokens: Vec<ApiTokenInfo> = records
                                        .into_iter()
                                        .map(|r| ApiTokenInfo {
                                            jti: r.jti,
                                            scope: r.scope,
                                            max_role: r.max_role,
                                            label: r.label,
                                            created_at: r.created_at,
                                            expires_at: r.expires_at,
                                            revoked: r.revoked,
                                        })
                                        .collect();
                                    db_result_tx
                                        .send(DbResult::ApiTokensListed { tokens })
                                        .ok();
                                }
                                Err(e) => {
                                    db_result_tx
                                        .send(DbResult::Error(format!(
                                            "List API tokens failed: {e}"
                                        )))
                                        .ok();
                                }
                            }
                        }
                        Ok(DbCommand::ListApiTokensByScope { .. }) => {
                            // Not implemented in test harness
                            db_result_tx.send(DbResult::ScopedApiTokensListed { tokens: vec![] }).ok();
                        }
                        Ok(DbCommand::ResolvePetalScope { .. })
                        | Ok(DbCommand::ResolveNodeScope { .. }) => {
                            // Scope resolution — not implemented in test harness.
                        }
                        Ok(DbCommand::Shutdown) | Err(_) => break,
                    }
                }
                tracing::info!("Peer DB thread shutting down");
                db_result_tx.send(DbResult::Stopped).ok();
            });
        });

        // Wait for DB Started
        let started = db_result_rx
            .recv_timeout(DEFAULT_TIMEOUT)
            .context("timeout waiting for DB Started")?;
        match &started {
            DbResult::Started => {}
            DbResult::Error(e) => anyhow::bail!("DB start error: {e}"),
            other => anyhow::bail!("unexpected DB result on start: {other:?}"),
        }

        // Wait for Sync Started
        let sync_started = sync_evt_rx
            .recv_timeout(DEFAULT_TIMEOUT)
            .context("timeout waiting for Sync Started")?;
        match &sync_started {
            SyncEvent::Started { online } => {
                tracing::info!(
                    "Peer '{name}' sync started (online={online})"
                );
            }
            other => {
                tracing::warn!("Unexpected sync event on start: {other:?}");
            }
        }

        Ok(Self {
            name: name.to_string(),
            data_dir: peer_dir,
            blob_store,
            db_cmd_tx,
            db_result_rx,
            sync_cmd_tx,
            sync_evt_rx,
            keypair,
            _db_thread: Some(db_handle),
            _sync_thread: Some(sync_handle),
        })
    }

    /// Send a DB command.
    pub fn send(&self, cmd: DbCommand) {
        self.db_cmd_tx.send(cmd).expect("DB command channel closed");
    }

    /// Send a command and wait for a specific result variant (with default timeout).
    #[allow(dead_code)]
    pub fn send_and_wait<F: Fn(&DbResult) -> bool>(
        &self,
        cmd: DbCommand,
        predicate: F,
    ) -> Result<DbResult> {
        self.send(cmd);
        self.wait_for(predicate, DEFAULT_TIMEOUT)
    }

    /// Drain results until we find one matching the predicate or timeout.
    pub fn wait_for<F: Fn(&DbResult) -> bool>(
        &self,
        predicate: F,
        timeout: Duration,
    ) -> Result<DbResult> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                anyhow::bail!(
                    "Timeout waiting for DB result on peer '{}'",
                    self.name
                );
            }
            match self.db_result_rx.recv_timeout(remaining) {
                Ok(result) => {
                    // Check for errors first
                    if let DbResult::Error(ref e) = result {
                        anyhow::bail!(
                            "DB error on peer '{}': {e}",
                            self.name
                        );
                    }
                    if predicate(&result) {
                        return Ok(result);
                    }
                    // Not our result, keep draining
                    tracing::debug!(
                        "Peer '{}' skipping result: {:?}",
                        self.name,
                        result
                    );
                }
                Err(_) => {
                    anyhow::bail!(
                        "Timeout waiting for DB result on peer '{}'",
                        self.name
                    );
                }
            }
        }
    }

    /// Shut down the peer (DB + sync threads).
    #[allow(dead_code)]
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        // Shut down DB
        let _ = self.db_cmd_tx.send(DbCommand::Shutdown);
        if let Some(h) = self._db_thread.take() {
            let _ = h.join();
        }
        // Shut down sync
        let _ = self.sync_cmd_tx.send(SyncCommand::Shutdown);
        if let Some(h) = self._sync_thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for TestPeer {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

// ---------------------------------------------------------------------------
// Simplified DB handlers (replicated from fe-database for in-memory usage)
// ---------------------------------------------------------------------------

use fe_runtime::messages::{
    FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
};
use surrealdb::engine::local::Db;

async fn seed_test_data(
    db: &surrealdb::Surreal<Db>,
    local_did: &str,
) -> anyhow::Result<(String, Vec<String>)> {
    // Check idempotency
    let mut check: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse WHERE name = 'Genesis Verse' LIMIT 1")
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;
    if !existing.is_empty() {
        return Ok((
            "Genesis Petal".to_string(),
            vec![
                "Lobby".to_string(),
                "Workshop".to_string(),
                "Gallery".to_string(),
            ],
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();

    // Create genesis verse
    let verse_id = ulid::Ulid::new().to_string();
    let ns_secret = rand_bytes_32();
    let ns_id = *blake3::keyed_hash(b"fractalengine:verse:namespace_id", &ns_secret).as_bytes();
    let ns_id_hex = hex::encode(ns_id);

    let _: Option<serde_json::Value> = db
        .create("verse")
        .content(serde_json::json!({
            "verse_id": verse_id,
            "name": "Genesis Verse",
            "created_by": local_did,
            "created_at": now,
            "namespace_id": ns_id_hex,
        }))
        .await?;

    // Create fractal
    let fractal_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("fractal")
        .content(serde_json::json!({
            "fractal_id": fractal_id,
            "verse_id": verse_id,
            "owner_did": local_did,
            "name": "Genesis Fractal",
            "created_at": now,
        }))
        .await?;

    // Create petal
    let petal_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("petal")
        .content(serde_json::json!({
            "petal_id": petal_id,
            "name": "Genesis Petal",
            "node_id": local_did,
            "created_at": now,
            "fractal_id": fractal_id,
        }))
        .await?;

    // Create rooms
    let rooms = vec!["Lobby", "Workshop", "Gallery"];
    for room_name in &rooms {
        let _: Option<serde_json::Value> = db
            .create("room")
            .content(serde_json::json!({
                "petal_id": petal_id,
                "name": room_name,
            }))
            .await?;
    }

    Ok((
        "Genesis Petal".to_string(),
        rooms.iter().map(|s| s.to_string()).collect(),
    ))
}

async fn create_verse(
    db: &surrealdb::Surreal<Db>,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let verse_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let ns_secret = rand_bytes_32();
    let ns_id = *blake3::keyed_hash(b"fractalengine:verse:namespace_id", &ns_secret).as_bytes();
    let ns_id_hex = hex::encode(ns_id);
    let ns_secret_hex = hex::encode(ns_secret);

    let _: Option<serde_json::Value> = db
        .create("verse")
        .content(serde_json::json!({
            "verse_id": verse_id,
            "name": name,
            "created_by": local_did,
            "created_at": now,
            "namespace_id": ns_id_hex,
        }))
        .await?;

    // Store namespace secret in a thread-local map instead of OS keyring
    // (tests should not touch the real keyring).
    NS_SECRETS.with(|m| {
        m.borrow_mut()
            .insert(verse_id.clone(), ns_secret_hex);
    });

    Ok(verse_id)
}

async fn create_fractal(
    db: &surrealdb::Surreal<Db>,
    verse_id: &str,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let fractal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let _: Option<serde_json::Value> = db
        .create("fractal")
        .content(serde_json::json!({
            "fractal_id": fractal_id,
            "verse_id": verse_id,
            "owner_did": local_did,
            "name": name,
            "created_at": now,
        }))
        .await?;
    Ok(fractal_id)
}

async fn create_petal(
    db: &surrealdb::Surreal<Db>,
    fractal_id: &str,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let petal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let _: Option<serde_json::Value> = db
        .create("petal")
        .content(serde_json::json!({
            "petal_id": petal_id,
            "name": name,
            "node_id": local_did,
            "created_at": now,
            "fractal_id": fractal_id,
        }))
        .await?;
    Ok(petal_id)
}

async fn create_node(
    db: &surrealdb::Surreal<Db>,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
) -> anyhow::Result<String> {
    let node_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    db.query(
        "CREATE node CONTENT {
            node_id: $node_id,
            petal_id: $petal_id,
            display_name: $name,
            position: <geometry<point>> [$x, $z],
            elevation: $y,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: false,
            created_at: $now,
        }",
    )
    .bind(("node_id", node_id.clone()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("name", name.to_string()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("now", now))
    .await?;
    Ok(node_id)
}

async fn import_gltf(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    petal_id: &str,
    name: &str,
    file_path: &str,
    position: [f32; 3],
) -> anyhow::Result<(String, String, String)> {
    let path = std::path::Path::new(file_path);
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Could not read file {}: {e}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("glb");
    let content_type = if ext == "glb" {
        "model/gltf-binary"
    } else {
        "model/gltf+json"
    };
    let size_bytes = bytes.len();

    let hash = blob_store
        .add_blob(&bytes)
        .map_err(|e| anyhow::anyhow!("blob_store.add_blob failed: {e}"))?;
    let content_hash = fe_database::hash_to_hex(&hash);

    let asset_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let _: Option<serde_json::Value> = db
        .create("asset")
        .content(serde_json::json!({
            "asset_id": asset_id,
            "name": name,
            "content_type": content_type,
            "size_bytes": size_bytes,
            "content_hash": content_hash,
            "created_at": now,
        }))
        .await?;

    let asset_path = format!("blob://{}.{}", content_hash, ext);

    let node_id = ulid::Ulid::new().to_string();
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    db.query(
        "CREATE node CONTENT {
            node_id: $node_id,
            petal_id: $petal_id,
            display_name: $name,
            asset_id: $asset_id,
            position: <geometry<point>> [$x, $z],
            elevation: $y,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: false,
            created_at: $now,
        }",
    )
    .bind(("node_id", node_id.clone()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("name", name.to_string()))
    .bind(("asset_id", asset_id.clone()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("now", now))
    .await?;

    Ok((node_id, asset_id, asset_path))
}

async fn load_hierarchy(
    db: &surrealdb::Surreal<Db>,
) -> anyhow::Result<Vec<VerseHierarchyData>> {
    // Load all verses
    let mut vr: surrealdb::IndexedResults = db
        .query("SELECT verse_id, name, namespace_id FROM verse")
        .await?;
    let verse_rows: Vec<serde_json::Value> = vr.take(0)?;

    let mut verses = Vec::new();
    for vrow in &verse_rows {
        let verse_id = vrow["verse_id"].as_str().unwrap_or("").to_string();
        let verse_name = vrow["name"].as_str().unwrap_or("").to_string();
        let namespace_id = vrow["namespace_id"].as_str().map(|s| s.to_string());

        // Load fractals for this verse
        let mut fr: surrealdb::IndexedResults = db
            .query("SELECT fractal_id, name FROM fractal WHERE verse_id = $vid")
            .bind(("vid", verse_id.clone()))
            .await?;
        let fractal_rows: Vec<serde_json::Value> = fr.take(0)?;

        let mut fractals = Vec::new();
        for frow in &fractal_rows {
            let fractal_id = frow["fractal_id"].as_str().unwrap_or("").to_string();
            let fractal_name = frow["name"].as_str().unwrap_or("").to_string();

            // Load petals for this fractal
            let mut pr: surrealdb::IndexedResults = db
                .query("SELECT petal_id, name FROM petal WHERE fractal_id = $fid")
                .bind(("fid", fractal_id.clone()))
                .await?;
            let petal_rows: Vec<serde_json::Value> = pr.take(0)?;

            let mut petals = Vec::new();
            for prow in &petal_rows {
                let petal_id = prow["petal_id"].as_str().unwrap_or("").to_string();
                let petal_name = prow["name"].as_str().unwrap_or("").to_string();

                // Load nodes for this petal
                let mut nr: surrealdb::IndexedResults = db
                    .query("SELECT node_id, display_name, asset_id, elevation FROM node WHERE petal_id = $pid")
                    .bind(("pid", petal_id.clone()))
                    .await?;
                let node_rows: Vec<serde_json::Value> = nr.take(0)?;

                let mut nodes = Vec::new();
                for nrow in &node_rows {
                    let nid = nrow["node_id"].as_str().unwrap_or("").to_string();
                    let nname = nrow["display_name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let has_asset = nrow["asset_id"].as_str().is_some();

                    nodes.push(NodeHierarchyData {
                        id: nid,
                        name: nname,
                        has_asset,
                        position: [0.0, 0.0, 0.0],
                        asset_path: None,
                        petal_id: petal_id.clone(),
                        webpage_url: None,
                    });
                }

                petals.push(PetalHierarchyData {
                    id: petal_id,
                    name: petal_name,
                    nodes,
                });
            }

            fractals.push(FractalHierarchyData {
                id: fractal_id,
                name: fractal_name,
                petals,
            });
        }

        verses.push(VerseHierarchyData {
            id: verse_id,
            name: verse_name,
            namespace_id,
            fractals,
        });
    }

    Ok(verses)
}

async fn generate_invite(
    db: &surrealdb::Surreal<Db>,
    verse_id: &str,
    include_write_cap: bool,
    expiry_hours: u32,
    keypair: &NodeKeypair,
) -> anyhow::Result<String> {
    // Look up verse row for namespace_id and name.
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT namespace_id, name FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("verse not found: {verse_id}"))?;

    let namespace_id = row["namespace_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("verse has no namespace_id"))?
        .to_string();
    let verse_name = row["name"].as_str().unwrap_or("Unknown").to_string();

    // Retrieve namespace secret from thread-local map (not OS keyring in tests).
    let namespace_secret = if include_write_cap {
        NS_SECRETS.with(|m| m.borrow().get(verse_id).cloned())
    } else {
        None
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry = now + (expiry_hours as u64) * 3600;

    let creator_node_addr = hex::encode(keypair.verifying_key().to_bytes());

    let mut invite = fe_database::invite::VerseInvite {
        namespace_id,
        namespace_secret,
        creator_node_addr,
        verse_name,
        verse_id: verse_id.to_string(),
        expiry_timestamp: expiry,
        signature: String::new(),
    };
    invite.sign(keypair);

    Ok(invite.to_invite_string())
}

async fn join_verse(
    db: &surrealdb::Surreal<Db>,
    invite_string: &str,
    local_did: &str,
) -> anyhow::Result<(String, String)> {
    let invite = fe_database::invite::VerseInvite::from_invite_string(invite_string)?;

    // Verify signature
    let creator_pk_bytes = hex::decode(&invite.creator_node_addr)
        .map_err(|e| anyhow::anyhow!("Invalid creator_node_addr hex: {e}"))?;
    let creator_pk_array: [u8; 32] = creator_pk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("creator_node_addr is not 32 bytes"))?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&creator_pk_array)
        .map_err(|e| anyhow::anyhow!("Invalid creator public key: {e}"))?;
    if !invite.verify(&verifying_key) {
        anyhow::bail!("Invalid invite signature");
    }

    if invite.is_expired() {
        anyhow::bail!("Invite has expired");
    }

    let verse_id = &invite.verse_id;
    let verse_name = &invite.verse_name;
    let namespace_id = &invite.namespace_id;

    // Check if verse already exists
    let mut check: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;

    if existing.is_empty() {
        let now = chrono::Utc::now().to_rfc3339();
        let _: Option<serde_json::Value> = db
            .create("verse")
            .content(serde_json::json!({
                "verse_id": verse_id,
                "name": verse_name,
                "created_by": invite.creator_node_addr,
                "created_at": now,
                "namespace_id": namespace_id,
            }))
            .await?;
    }

    // Create verse_member row
    let now = chrono::Utc::now().to_rfc3339();
    let member_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("verse_member")
        .content(serde_json::json!({
            "member_id": member_id,
            "verse_id": verse_id,
            "peer_did": local_did,
            "status": "active",
            "invited_by": invite.creator_node_addr,
            "invite_sig": invite.signature,
            "invite_timestamp": now,
        }))
        .await?;

    Ok((verse_id.clone(), verse_name.clone()))
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn rand_bytes_32() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

// Thread-local namespace secret store (avoids touching the real OS keyring in tests).
thread_local! {
    static NS_SECRETS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}
