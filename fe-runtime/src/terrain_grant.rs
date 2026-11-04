//! Trusted terrain admission and bounded reads; see terrain-grant-design.md.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::{Duration, Instant};
use fe_identity::api_token::ApiClaims;
use fe_policy::RoleLevel;
use serde_json::Value;

pub const CAPTURE_LIMIT: usize = 8;
pub const CAPTURE_BUDGET: Duration = Duration::from_secs(5);
pub const CAPTURE_MAX_BYTES: usize = 8192;

/// Actual local publisher or a verified, ancestry-bound API credential.
#[derive(Debug, Clone)]
pub enum TerrainWriteGrant {
    Local,
    Api(TerrainApiGrant),
}

/// Trusted in-process admission, never reconstructed from request JSON.
#[derive(Clone)]
pub struct TerrainApiGrant {
    petal: String,
    admitted_scope: String,
    token_scope: String,
    role: RoleLevel,
    issuer: String,
    expiry: u64,
    jti: String,
    credential: TerrainCredentialSnapshot,
}

impl std::fmt::Debug for TerrainApiGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainApiGrant").finish_non_exhaustive()
    }
}

impl TerrainApiGrant {
    /// Bind claims already verified by this exact configured Ed25519 key.
    pub fn verified(
        claims: &ApiClaims, verifier: &ed25519_dalek::VerifyingKey,
        petal: String, admitted_scope: String, credential: TerrainCredentialSnapshot,
    ) -> Result<Self, &'static str> {
        let issuer = fe_identity::did_key_from_public_key(verifier);
        let role = RoleLevel::from(claims.max_role.as_str());
        if claims.sub != issuer || claims.token_type != "api"
            || !role.is_at_least(RoleLevel::Editor) || !bounded(&petal, 256)
            || !bounded(&admitted_scope, 4096) || !bounded(&claims.scope, 4096)
            || !bounded(&claims.jti, 128) || !bounded(credential.generation(), 64)
            || i64::try_from(claims.exp).is_err()
        { return Err("Terrain credential refused"); }
        Ok(Self { petal, admitted_scope, token_scope: claims.scope.clone(), role,
            issuer, expiry: claims.exp, jti: claims.jti.clone(), credential })
    }
    pub fn petal(&self) -> &str { &self.petal }
    pub fn admitted_scope(&self) -> &str { &self.admitted_scope }
    pub fn token_scope(&self) -> &str { &self.token_scope }
    pub fn role(&self) -> RoleLevel { self.role }
    pub fn issuer(&self) -> &str { &self.issuer }
    pub fn expiry(&self) -> u64 { self.expiry }
    pub fn jti(&self) -> &str { &self.jti }
    pub fn generation(&self) -> &str { self.credential.generation() }
    pub fn tokens(&self) -> &[Value] { self.credential.tokens() }
}

fn bounded(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

/// Host-wide permits remain owned by queued/in-flight engine reads after timeout.
#[derive(Clone)]
pub struct TerrainCaptureLimiter(Arc<tokio::sync::Semaphore>);

impl Default for TerrainCaptureLimiter {
    fn default() -> Self { Self(Arc::new(tokio::sync::Semaphore::new(CAPTURE_LIMIT))) }
}

impl TerrainCaptureLimiter {
    pub async fn acquire(&self, deadline: Instant) -> Result<tokio::sync::OwnedSemaphorePermit, &'static str> {
        tokio::time::timeout_at(deadline.into(), self.0.clone().acquire_owned())
            .await.map_err(|_| "Terrain credential unavailable")?
            .map_err(|_| "Terrain credential unavailable")
    }
}

struct ReadLease {
    active: AtomicBool,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// One fixed metadata read; its lease bounds API, queue and engine ownership.
#[derive(Clone)]
pub struct TerrainCredentialRead {
    id: String,
    issuer: String,
    jti: String,
    deadline: Instant,
    lease: Arc<ReadLease>,
}

impl std::fmt::Debug for TerrainCredentialRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainCredentialRead").finish_non_exhaustive()
    }
}

impl TerrainCredentialRead {
    pub fn new(id: String, issuer: String, jti: String, deadline: Instant,
        permit: tokio::sync::OwnedSemaphorePermit) -> Result<Self, &'static str> {
        if !bounded(&id, 64) || !bounded(&issuer, 256) || !bounded(&jti, 128) || Instant::now() >= deadline {
            return Err("Terrain credential unavailable");
        }
        Ok(Self { id, issuer, jti, deadline,
            lease: Arc::new(ReadLease { active: AtomicBool::new(true), _permit: permit }) })
    }
    pub fn id(&self) -> &str { &self.id }
    pub fn issuer(&self) -> &str { &self.issuer }
    pub fn jti(&self) -> &str { &self.jti }
    pub fn deadline(&self) -> Instant { self.deadline }
    pub fn active(&self) -> bool {
        self.lease.active.load(Ordering::Acquire) && Instant::now() < self.deadline
    }
    pub fn cancel(&self) { self.lease.active.store(false, Ordering::Release); }
    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.lease, &other.lease) && self.id == other.id
            && self.issuer == other.issuer && self.jti == other.jti && self.deadline == other.deadline
    }
}

/// Small fixed credential projection, not an API/token inventory.
#[derive(Clone)]
pub struct TerrainCredentialSnapshot {
    generation: String,
    tokens: Vec<Value>,
}

impl std::fmt::Debug for TerrainCredentialSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainCredentialSnapshot").finish_non_exhaustive()
    }
}

impl TerrainCredentialSnapshot {
    pub fn new(generation: String, tokens: Vec<Value>) -> Result<Self, &'static str> {
        if !bounded(&generation, 64) || tokens.len() > 1
            || serde_json::to_vec(&tokens).map_err(|_| "Terrain credential unavailable")?.len()
                + generation.len() > CAPTURE_MAX_BYTES
        { return Err("Terrain credential unavailable"); }
        Ok(Self { generation, tokens })
    }
    pub fn generation(&self) -> &str { &self.generation }
    pub fn tokens(&self) -> &[Value] { &self.tokens }
}

/// Every completion retains the same private read owner, including failures.
#[derive(Debug, Clone)]
pub struct TerrainCredentialReply {
    pub request: TerrainCredentialRead,
    pub snapshot: Option<TerrainCredentialSnapshot>,
}
