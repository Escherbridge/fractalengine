//! Opt-in, loopback-only API-token emission for headless agent QA
//! (`FE_API_DEV_TOKEN_OUT`) — see fe-api/AGENTS.md §qa-dev-token.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use fe_database::repo::Db;
use fe_database::RoleLevel;
use fe_identity::NodeKeypair;

/// Env var naming the file the GUI writes its dev tokens to; unset = off.
pub const DEV_TOKEN_OUT_ENV: &str = "FE_API_DEV_TOKEN_OUT";

/// Lifetime of every emitted token (8 h — one QA session, not a credential).
pub const DEV_TOKEN_TTL_SECS: u64 = 8 * 60 * 60;

/// The output path from [`DEV_TOKEN_OUT_ENV`]; unset or blank = feature off.
pub fn dev_token_out_from_env() -> Option<PathBuf> {
    let raw = std::env::var_os(DEV_TOKEN_OUT_ENV)?;
    if raw.to_string_lossy().trim().is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    tracing::warn!(
        path = %path.display(),
        "{DEV_TOKEN_OUT_ENV} is set: API tokens for every local verse (Owner where this \
         node owns the verse) will be written to this file while the API is loopback-bound"
    );
    Some(path)
}

/// `http://<addr>` for a loopback listener; any other bind is refused.
pub fn loopback_api_base(bound: SocketAddr) -> anyhow::Result<String> {
    if !bound.ip().is_loopback() {
        anyhow::bail!(
            "refusing to emit dev API tokens: the API is bound to {bound}, not a loopback address"
        );
    }
    Ok(format!("http://{bound}"))
}

/// What one write put in the file — no token material, so safe to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevTokenEmission {
    pub api_base: String,
    /// verse id → the role its token was minted at.
    pub roles: BTreeMap<String, String>,
    /// RFC 3339; every token in the file is valid at least until then.
    pub expires_at: String,
}

/// Mint one token per verse at the local node's own role there (Owner for a
/// verse it owns, never above its role, none where it has no role) and
/// atomically replace `path` with the owner-only JSON document. The loopback
/// check runs first: a non-loopback `bound` mints and writes nothing.
pub async fn emit_dev_tokens(
    path: &Path,
    bound: SocketAddr,
    db: &Db,
    keypair: &NodeKeypair,
    client_did: &str,
    verse_ids: &[String],
) -> anyhow::Result<DevTokenEmission> {
    let api_base = loopback_api_base(bound)?;
    let local_did = keypair.to_did_key();
    let generation = fe_database::api_auth::current_generation(db).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let expires_at = chrono::DateTime::from_timestamp((now + DEV_TOKEN_TTL_SECS) as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    let mut tokens = BTreeMap::new();
    let mut roles = BTreeMap::new();
    for verse_id in verse_ids {
        if roles.contains_key(verse_id) {
            continue;
        }
        let scope = format!("VERSE#{verse_id}");
        let role = match fe_database::role_manager::resolve_role(db, &local_did, &scope).await {
            Ok(role) => role,
            Err(error) => {
                tracing::warn!(%verse_id, %error, "dev token: could not resolve the local role; skipping verse");
                continue;
            }
        };
        if role == RoleLevel::None {
            continue;
        }
        let role = role.to_string();
        let token = fe_identity::mint_client_api_token(
            keypair,
            &scope,
            &role,
            DEV_TOKEN_TTL_SECS,
            &fe_database::api_auth::new_jti(&generation)?,
            client_did,
        )?;
        tokens.insert(verse_id.clone(), token);
        roles.insert(verse_id.clone(), role);
    }

    // Isolated QA roots also get the local-operator debug credential (fractalengine src/AGENTS.md §qa-operator).
    let operator_token = if fe_runtime::qa_debug::qa_operator_enabled() {
        Some(fe_identity::mint_client_api_token(
            keypair,
            fe_runtime::qa_debug::QA_OPERATOR_SCOPE,
            &RoleLevel::Owner.to_string(),
            DEV_TOKEN_TTL_SECS,
            &fe_database::api_auth::new_jti(&generation)?,
            client_did,
        )?)
    } else {
        None
    };
    anyhow::ensure!(fe_database::api_auth::current_generation(db).await? == generation, "Credential generation changed during dev-token publication");
    let mut document = serde_json::json!({
        "api_base": api_base,
        "tokens": tokens,
        "roles": roles,
        "client": client_did,
        "expires_at": expires_at,
    });
    if let Some(operator_token) = operator_token {
        document["operator_token"] = serde_json::Value::String(operator_token);
    }
    fe_identity::file_fallback::replace_owner_only_file(
        path,
        &serde_json::to_vec_pretty(&document)?,
    )?;
    Ok(DevTokenEmission {
        api_base,
        roles,
        expires_at,
    })
}

/// Inputs for the dev-token writer thread.
pub struct DevTokenWriter {
    pub path: PathBuf,
    /// The DB thread's shared handle (role resolution only — never written).
    pub db: Db,
    /// The node keypair (the API's verifying key must match it).
    pub keypair: NodeKeypair,
    /// Fires once with the bound address ([`crate::ApiConfig::listening_tx`]).
    pub listening_rx: crossbeam::channel::Receiver<SocketAddr>,
    /// The current local verse ids, re-sent whenever the set changes.
    pub verse_ids_rx: crossbeam::channel::Receiver<Vec<String>>,
}

/// Spawn the writer: wait for the listener, refuse a non-loopback bind for
/// good, then rewrite the file for every (coalesced) verse-set change.
pub fn spawn_dev_token_writer(writer: DevTokenWriter) {
    if let Err(error) = spawn_dev_token_service(writer) {
        tracing::error!(%error, "could not start the dev-token writer; no token file will be written");
    }
}

/// Spawn a stoppable owner of dev-token publication under a fresh client DID.
pub fn spawn_dev_token_service(writer: DevTokenWriter) -> std::io::Result<fe_runtime::service_lifecycle::OwnedService> {
    spawn_dev_token_service_as(writer, fe_identity::new_client_did())
}

/// [`spawn_dev_token_service`] under the caller's client DID, so the portal host and the dev tokens
/// share one API client (one upload bucket, one `client` claim).
pub fn spawn_dev_token_service_as(
    writer: DevTokenWriter,
    client_did: String,
) -> std::io::Result<fe_runtime::service_lifecycle::OwnedService> {
    use fe_runtime::service_lifecycle::{OwnedService, StopSignal};
    let stop = StopSignal::default();
    let worker_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("fe-dev-token".to_string())
        .spawn(move || writer.run(worker_stop, client_did))?;
    Ok(OwnedService::new("dev-token", stop, spawned))
}

impl DevTokenWriter {
    fn run(self, stop: fe_runtime::service_lifecycle::StopSignal, client_did: String) {
        let bound = loop {
            if stop.is_requested() { return; }
            match self.listening_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(bound) => break bound,
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => return,
            }
        };
        if let Err(error) = loopback_api_base(bound) {
            tracing::error!(%error, "{DEV_TOKEN_OUT_ENV} ignored; nothing written");
            return;
        }
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "dev-token writer runtime failed; nothing written");
                return;
            }
        };
        // One API client per process: the agent's uploads share one owner
        // bucket across rewrites (fe-identity/src/AGENTS.md §api-token).
        while !stop.is_requested() {
            let mut verse_ids = match self.verse_ids_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(ids) => ids,
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            };
            while let Ok(newer) = self.verse_ids_rx.try_recv() {
                verse_ids = newer;
            }
            match runtime.block_on(emit_dev_tokens(
                &self.path,
                bound,
                &self.db,
                &self.keypair,
                &client_did,
                &verse_ids,
            )) {
                Ok(emission) => tracing::info!(
                    path = %self.path.display(),
                    api_base = %emission.api_base,
                    roles = ?emission.roles,
                    expires_at = %emission.expires_at,
                    "wrote dev API tokens"
                ),
                Err(error) => tracing::error!(
                    path = %self.path.display(),
                    %error,
                    "could not write dev API tokens"
                ),
            }
        }
    }
}
