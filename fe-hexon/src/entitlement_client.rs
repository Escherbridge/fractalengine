//! S-ENTITLEMENT token client: signed token requests to an endpoint advertising `entitlements`,
//! contract v1 refusal mapping, and a per-(licensee, package) token store renewed before `exp`
//! (see `fe-hexon/src/AGENTS.md` §entitlement).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use fe_format::entitlement::{
    ed25519_key_to_did, x25519_public_key, x25519_public_key_to_did, EntitlementError, EntitlementExpectation,
    EntitlementVerifier, TokenRequest,
};
use serde::{Deserialize, Serialize};

use crate::registry_source::{resolve_location, CatalogConfig, CATALOG_PREFIX, FEATURE_ENTITLEMENTS};

/// Token endpoint path under `{base}/api/v1/catalog/`.
pub const TOKEN_PATH: &str = "entitlements/token";
/// Largest token-endpoint response body read.
pub const TOKEN_RESPONSE_MAX_BYTES: u64 = 64 * 1024;
/// Largest stored token record read back from disk.
const STORED_RECORD_MAX_BYTES: u64 = 64 * 1024;
/// Backoff applied after a `429` that names no `Retry-After`.
pub const DEFAULT_RATE_LIMIT_BACKOFF_SECS: u64 = 60;
/// Longest `Retry-After` honoured (a hostile endpoint cannot park the client for days).
pub const MAX_RATE_LIMIT_BACKOFF_SECS: u64 = 60 * 60;
/// Shortest and longest renewal margin before `exp`.
const RENEW_MARGIN_BOUNDS_SECS: (u64, u64) = (60, 24 * 60 * 60);

// ---- identity ---------------------------------------------------------------

/// The local licensee: identity key (signs the proof, becomes `sub`) and device X25519 secret.
#[derive(Clone, Copy)]
pub struct Licensee<'a> {
    /// Ed25519 identity key.
    pub identity: &'a SigningKey,
    /// X25519 device secret (`fe_identity::x25519::DeviceKeypair::secret_bytes`).
    pub device_secret: &'a [u8; 32],
}

impl std::fmt::Debug for Licensee<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Licensee").field("did", &self.did()).finish_non_exhaustive()
    }
}

impl Licensee<'_> {
    /// The licensee identity DID (`sub` of every token issued to it).
    pub fn did(&self) -> String {
        ed25519_key_to_did(&self.identity.verifying_key())
    }

    /// The device X25519 DID tokens wrap the content key to.
    pub fn device_did(&self) -> String {
        x25519_public_key_to_did(&x25519_public_key(self.device_secret))
    }
}

// ---- transport --------------------------------------------------------------

/// One token-endpoint answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenHttpResponse {
    /// HTTP status.
    pub status: u16,
    /// Body, at most the requested cap.
    pub body: Vec<u8>,
    /// `Retry-After` in seconds, when the endpoint sent one.
    pub retry_after_secs: Option<u64>,
}

/// POSTs a token request; the binary supplies the http(s) implementation.
/// Implementations are anonymous (no credential or cookie headers), follow no redirects and use no
/// environment proxy (the shared registry client builder, extension_platform steer 2026-11-13).
pub trait TokenTransport: Send + Sync {
    /// POST `body` as `application/json` to `url`, reading at most `max_bytes` of the response.
    fn post_json(&self, url: &str, body: &[u8], max_bytes: u64) -> Result<TokenHttpResponse, String>;
}

// ---- errors -----------------------------------------------------------------

/// Why no token was obtained (contract v1 token endpoint codes plus client-side refusals).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenRequestError {
    /// The endpoint does not advertise `entitlements` (never probed).
    #[error("this source offers no entitlements")]
    NotOffered,
    /// The base does not resolve to an https (or loopback http) token URL.
    #[error("token endpoint refused: {0}")]
    Location(String),
    /// `403 not_entitled`: no licence yet (also while an acquisition is completing).
    #[error("no licence for this package")]
    NotEntitled,
    /// `403 reissue_refused`: the licence was revoked.
    #[error("the issuer refuses to reissue this licence")]
    ReissueRefused,
    /// `403 device_limit_reached`: the licence admits no further devices.
    #[error("the licence admits no further devices")]
    DeviceLimitReached,
    /// `401 proof_invalid`: bad proof or clock outside the window.
    #[error("the endpoint rejected the request proof (check the device clock)")]
    ProofInvalid,
    /// `404 unknown_package`: no such `(package_id, content_hash)`.
    #[error("the endpoint does not know this package build")]
    UnknownPackage,
    /// `400 invalid_request`.
    #[error("the endpoint rejected the request as malformed")]
    InvalidRequest,
    /// `429 rate_limited`, or still inside the backoff it set.
    #[error("rate limited; retry in {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    /// Any other status or an unreadable body.
    #[error("unexpected token endpoint answer ({status}): {detail}")]
    Unexpected { status: u16, detail: String },
    /// The endpoint could not be reached.
    #[error("token endpoint unreachable: {0}")]
    Unreachable(String),
    /// The request could not be signed (grammar of a DID, package id or hash).
    #[error("token request not signed: {0}")]
    Request(EntitlementError),
    /// A `200` token that fails local verification; it is never stored.
    #[error("issued token does not verify: {0}")]
    TokenRejected(EntitlementError),
}

impl TokenRequestError {
    /// True for a refusal the issuer decided (no retry helps until something changes).
    pub fn is_definitive(&self) -> bool {
        matches!(self, Self::NotEntitled | Self::ReissueRefused | Self::DeviceLimitReached | Self::UnknownPackage)
    }
}

/// `{ ok: false, data: null, error: "<code>" }` (registry protocol v1 envelope).
#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<String>,
}

/// `200 { "token": "<jwt>" }`.
#[derive(Deserialize)]
struct TokenBody {
    token: String,
}

/// Map a non-200 answer to its refusal: the envelope code decides, the status breaks ties.
fn refusal(response: &TokenHttpResponse) -> TokenRequestError {
    let code = serde_json::from_slice::<ErrorEnvelope>(&response.body).ok().and_then(|envelope| envelope.error);
    match (response.status, code.as_deref()) {
        (_, Some("rate_limited")) | (429, _) => TokenRequestError::RateLimited {
            retry_after_secs: response
                .retry_after_secs
                .unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF_SECS)
                .clamp(1, MAX_RATE_LIMIT_BACKOFF_SECS),
        },
        (_, Some("not_entitled")) => TokenRequestError::NotEntitled,
        (_, Some("reissue_refused")) => TokenRequestError::ReissueRefused,
        (_, Some("device_limit_reached")) => TokenRequestError::DeviceLimitReached,
        (_, Some("proof_invalid")) | (401, _) => TokenRequestError::ProofInvalid,
        (_, Some("unknown_package")) | (404, _) => TokenRequestError::UnknownPackage,
        (_, Some("invalid_request")) | (400, _) => TokenRequestError::InvalidRequest,
        // An unknown 403 code is still a refusal by the issuer: fail closed as "no licence".
        (403, _) => TokenRequestError::NotEntitled,
        (status, code) => TokenRequestError::Unexpected { status, detail: code.unwrap_or("no error code").to_owned() },
    }
}

/// `{base}/api/v1/catalog/entitlements/token`, https only (http only on a loopback host).
pub fn token_url(base: &str) -> Result<String, TokenRequestError> {
    let url = resolve_location(base, CATALOG_PREFIX, TOKEN_PATH)
        .map_err(|error| TokenRequestError::Location(error.to_string()))?;
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(url);
    }
    let loopback = lower.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split('/').next().unwrap_or_default();
        let host = match authority.strip_prefix('[') {
            Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
            None => authority.rsplit_once(':').map_or(authority, |(host, _port)| host),
        };
        matches!(host, "127.0.0.1" | "localhost" | "::1")
    });
    if loopback {
        Ok(url)
    } else {
        Err(TokenRequestError::Location(format!("{url}: the token endpoint must be https")))
    }
}

// ---- token store --------------------------------------------------------------

/// A token held for one `(licensee, package_id)`; it covers exactly one `content_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldToken {
    /// Licensee identity DID (`sub`).
    pub licensee_did: String,
    /// The package's `hexon_id`.
    pub package_id: String,
    /// The build the token covers.
    pub content_hash: String,
    /// The EdDSA JWT.
    pub token: String,
    /// Verified `iat`.
    pub iat: u64,
    /// Verified `exp`.
    pub exp: u64,
}

impl HeldToken {
    /// Unix time from which the client renews: a fifth of the lifetime before `exp`, within 1 min..1 day.
    pub fn renew_at(&self) -> u64 {
        let lifetime = self.exp.saturating_sub(self.iat);
        let margin = (lifetime / 5).clamp(RENEW_MARGIN_BOUNDS_SECS.0, RENEW_MARGIN_BOUNDS_SECS.1);
        self.exp.saturating_sub(margin)
    }
}

/// Persists tokens per `(licensee_did, package_id)`; a token is device-bound ciphertext, never content.
pub trait TokenStore: Send + Sync {
    /// The token held for the pair, if any.
    fn get(&self, licensee_did: &str, package_id: &str) -> Option<HeldToken>;
    /// Replace the token held for `token`'s pair.
    fn put(&self, token: &HeldToken) -> std::io::Result<()>;
    /// Drop the token held for the pair.
    fn remove(&self, licensee_did: &str, package_id: &str) -> std::io::Result<()>;
}

/// In-process [`TokenStore`].
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: Mutex<BTreeMap<(String, String), HeldToken>>,
}

impl TokenStore for MemoryTokenStore {
    fn get(&self, licensee_did: &str, package_id: &str) -> Option<HeldToken> {
        let tokens = self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens.get(&(licensee_did.to_owned(), package_id.to_owned())).cloned()
    }

    fn put(&self, token: &HeldToken) -> std::io::Result<()> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens.insert((token.licensee_did.clone(), token.package_id.clone()), token.clone());
        Ok(())
    }

    fn remove(&self, licensee_did: &str, package_id: &str) -> std::io::Result<()> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tokens.remove(&(licensee_did.to_owned(), package_id.to_owned()));
        Ok(())
    }
}

/// [`TokenStore`] over a directory: one JSON record per pair, named by a BLAKE3 of the pair.
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    root: PathBuf,
}

impl FileTokenStore {
    /// Store under `root` (created on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Record path of a pair (the name never carries the DID or id, so no path grammar applies).
    fn record_path(&self, licensee_did: &str, package_id: &str) -> PathBuf {
        let mut hasher = blake3::Hasher::new();
        hasher.update(licensee_did.as_bytes());
        hasher.update(&[0]);
        hasher.update(package_id.as_bytes());
        self.root.join(format!("{}.json", hasher.finalize().to_hex()))
    }
}

/// Read at most `max` bytes of `path`; `None` when absent, unreadable or oversized.
fn read_capped(path: &Path, max: u64) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > max {
        return None;
    }
    std::fs::read(path).ok().filter(|bytes| bytes.len() as u64 <= max)
}

impl TokenStore for FileTokenStore {
    fn get(&self, licensee_did: &str, package_id: &str) -> Option<HeldToken> {
        let bytes = read_capped(&self.record_path(licensee_did, package_id), STORED_RECORD_MAX_BYTES)?;
        let held: HeldToken = serde_json::from_slice(&bytes).ok()?;
        // A record filed under another pair (copied or colliding) is ignored.
        (held.licensee_did == licensee_did && held.package_id == package_id).then_some(held)
    }

    fn put(&self, token: &HeldToken) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.record_path(&token.licensee_did, &token.package_id);
        let staging = path.with_extension("json.partial");
        let bytes = serde_json::to_vec(token).map_err(std::io::Error::other)?;
        std::fs::write(&staging, bytes)?;
        std::fs::rename(&staging, &path)
    }

    fn remove(&self, licensee_did: &str, package_id: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.record_path(licensee_did, package_id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

// ---- client -----------------------------------------------------------------

/// Which build of which package at which endpoint a token is wanted for.
#[derive(Debug, Clone, Copy)]
pub struct TokenTarget<'a> {
    /// Endpoint base (the prefix under which `api/v1/catalog/` is served).
    pub base: &'a str,
    /// The endpoint's `config.json`; `None` when unreachable (stored tokens still serve).
    pub config: Option<&'a CatalogConfig>,
    /// The package's `hexon_id`.
    pub package_id: &'a str,
    /// The build's content hash, recomputed locally (never taken from a document).
    pub content_hash: &'a str,
}

/// Where a returned token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// Held from an earlier issue and still valid.
    Stored,
    /// Issued by this call.
    Issued,
}

/// A usable token; `renewal` carries why a due renewal failed while the held token still serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredToken {
    /// The verified token.
    pub held: HeldToken,
    /// Stored or freshly issued.
    pub source: TokenSource,
    /// A failed renewal attempt (the held token stays valid until `exp`).
    pub renewal: Option<TokenRequestError>,
}

/// Requests, verifies, stores and renews entitlement tokens.
pub struct EntitlementClient {
    transport: Arc<dyn TokenTransport>,
    verifier: Arc<dyn EntitlementVerifier>,
    store: Arc<dyn TokenStore>,
    /// Token URL → Unix time before which no request is sent (set by `429`).
    backoff: Mutex<BTreeMap<String, u64>>,
}

impl EntitlementClient {
    /// A client verifying with `verifier` (pinned issuer keys) and holding tokens in `store`.
    pub fn new(transport: Arc<dyn TokenTransport>, verifier: Arc<dyn EntitlementVerifier>, store: Arc<dyn TokenStore>) -> Self {
        Self { transport, verifier, store, backoff: Mutex::new(BTreeMap::new()) }
    }

    /// Verify `token` for the build and licensee; the unwrapped content key is dropped (zeroized) here.
    fn verify(&self, token: &str, package_id: &str, content_hash: &str, licensee: &Licensee<'_>, now: u64) -> Result<HeldToken, EntitlementError> {
        let licensee_did = licensee.did();
        let expected = EntitlementExpectation { package_id, content_hash, licensee_did: &licensee_did };
        let verified = self.verifier.verify(token, &expected, licensee.device_secret, now)?;
        Ok(HeldToken {
            licensee_did,
            package_id: package_id.to_owned(),
            content_hash: content_hash.to_owned(),
            token: token.to_owned(),
            iat: verified.claims.iat,
            exp: verified.claims.exp,
        })
    }

    /// The held token for this build, re-verified now; `None` when absent, for another build, or invalid.
    pub fn stored_token(&self, package_id: &str, content_hash: &str, licensee: &Licensee<'_>, now: u64) -> Option<HeldToken> {
        let held = self.store.get(&licensee.did(), package_id)?;
        if held.content_hash != content_hash {
            return None;
        }
        self.verify(&held.token, package_id, content_hash, licensee, now).ok()
    }

    /// Drop the held token for a package (uninstall).
    pub fn forget(&self, package_id: &str, licensee: &Licensee<'_>) -> std::io::Result<()> {
        self.store.remove(&licensee.did(), package_id)
    }

    /// A valid token for `target`: the held one until it is due for renewal, else a new one from
    /// the endpoint (only when it advertises `entitlements`); a failed renewal keeps the held token.
    pub fn token_for(&self, target: &TokenTarget<'_>, licensee: &Licensee<'_>, now: u64) -> Result<AcquiredToken, TokenRequestError> {
        let held = self.stored_token(target.package_id, target.content_hash, licensee, now);
        if let Some(held) = &held {
            if now < held.renew_at() {
                return Ok(AcquiredToken { held: held.clone(), source: TokenSource::Stored, renewal: None });
            }
        }
        match self.request(target, licensee, now) {
            Ok(issued) => Ok(AcquiredToken { held: issued, source: TokenSource::Issued, renewal: None }),
            Err(error) => match held {
                Some(held) => Ok(AcquiredToken { held, source: TokenSource::Stored, renewal: Some(error) }),
                None => Err(error),
            },
        }
    }

    /// POST one signed request, verify the answer and store it.
    fn request(&self, target: &TokenTarget<'_>, licensee: &Licensee<'_>, now: u64) -> Result<HeldToken, TokenRequestError> {
        if !target.config.is_some_and(|config| config.has(FEATURE_ENTITLEMENTS)) {
            return Err(TokenRequestError::NotOffered);
        }
        let url = token_url(target.base)?;
        {
            let backoff = self.backoff.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(&until) = backoff.get(&url) {
                if now < until {
                    return Err(TokenRequestError::RateLimited { retry_after_secs: until - now });
                }
            }
        }
        let request = TokenRequest::sign(licensee.identity, target.package_id, target.content_hash, &licensee.device_did(), now)
            .map_err(TokenRequestError::Request)?;
        let body = serde_json::to_vec(&request.to_json())
            .map_err(|error| TokenRequestError::Request(EntitlementError::Malformed(error.to_string())))?;
        let response = self
            .transport
            .post_json(&url, &body, TOKEN_RESPONSE_MAX_BYTES)
            .map_err(TokenRequestError::Unreachable)?;
        if response.status != 200 {
            let refused = refusal(&response);
            if let TokenRequestError::RateLimited { retry_after_secs } = refused {
                let mut backoff = self.backoff.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                backoff.insert(url, now.saturating_add(retry_after_secs));
            }
            return Err(refused);
        }
        if response.body.len() as u64 > TOKEN_RESPONSE_MAX_BYTES {
            return Err(TokenRequestError::Unexpected { status: 200, detail: "response too large".into() });
        }
        let issued: TokenBody = serde_json::from_slice(&response.body)
            .map_err(|error| TokenRequestError::Unexpected { status: 200, detail: format!("token body: {error}") })?;
        let held = self
            .verify(&issued.token, target.package_id, target.content_hash, licensee, now)
            .map_err(TokenRequestError::TokenRejected)?;
        if let Err(error) = self.store.put(&held) {
            // The token still serves this session; it is simply requested again next start.
            tracing::warn!(package_id = target.package_id, %error, "entitlement token not persisted");
        }
        Ok(held)
    }
}

/// Fake token endpoint over a [`LocalIssuer`](fe_format::entitlement::LocalIssuer) (client and gate journeys).
#[cfg(test)]
pub(crate) mod test_endpoint {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use fe_format::entitlement::{verify_token_request, ContentKey, IssuerKeyring, KeyringVerifier, LocalIssuer, MintRequest};

    use super::{TokenHttpResponse, TokenTransport};

    /// One scripted answer in place of a mint.
    pub(crate) enum Scripted {
        /// A contract v1 error envelope with its status and optional `Retry-After`.
        Refuse(u16, &'static str, Option<u64>),
        /// A `200` carrying this token verbatim.
        Token(String),
    }

    /// Verifies each request like a hosted registry and mints with the issuer, unless a script is queued.
    pub(crate) struct FakeEndpoint {
        pub(crate) issuer: LocalIssuer,
        encrypted_key: String,
        pub(crate) now: AtomicU64,
        pub(crate) posts: AtomicUsize,
        pub(crate) urls: Mutex<Vec<String>>,
        script: Mutex<VecDeque<Scripted>>,
        pub(crate) ttl_secs: u64,
    }

    impl FakeEndpoint {
        /// An endpoint whose catalogue record wraps `content_key` to its issuer.
        pub(crate) fn new(kid: &str, content_key: &ContentKey, now: u64) -> Arc<Self> {
            let issuer = LocalIssuer::generate(kid).unwrap();
            let encrypted_key = issuer.encrypted_key_for(content_key).unwrap();
            Arc::new(Self {
                issuer,
                encrypted_key,
                now: AtomicU64::new(now),
                posts: AtomicUsize::new(0),
                urls: Mutex::new(Vec::new()),
                script: Mutex::new(VecDeque::new()),
                ttl_secs: 3600,
            })
        }

        /// A verifier pinning exactly this endpoint's issuer key.
        pub(crate) fn verifier(&self) -> Arc<KeyringVerifier> {
            let mut keyring = IssuerKeyring::new();
            keyring.pin(self.issuer.kid(), self.issuer.verifying_key()).unwrap();
            Arc::new(KeyringVerifier::new(keyring))
        }

        /// Queue an answer for the next request.
        pub(crate) fn script(&self, answer: Scripted) {
            self.script.lock().unwrap().push_back(answer);
        }

        /// Requests received so far.
        pub(crate) fn post_count(&self) -> usize {
            self.posts.load(Ordering::SeqCst)
        }

        fn envelope(status: u16, code: &str, retry_after_secs: Option<u64>) -> TokenHttpResponse {
            let body = serde_json::json!({ "ok": false, "data": null, "error": code });
            TokenHttpResponse { status, body: serde_json::to_vec(&body).unwrap(), retry_after_secs }
        }
    }

    impl TokenTransport for FakeEndpoint {
        fn post_json(&self, url: &str, body: &[u8], _max_bytes: u64) -> Result<TokenHttpResponse, String> {
            self.posts.fetch_add(1, Ordering::SeqCst);
            self.urls.lock().unwrap().push(url.to_owned());
            if let Some(answer) = self.script.lock().unwrap().pop_front() {
                return Ok(match answer {
                    Scripted::Refuse(status, code, retry) => Self::envelope(status, code, retry),
                    Scripted::Token(token) => TokenHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&serde_json::json!({ "token": token })).unwrap(),
                        retry_after_secs: None,
                    },
                });
            }
            let now = self.now.load(Ordering::SeqCst);
            let request = match verify_token_request(body, now) {
                Ok(request) => request,
                Err(error) => {
                    let status = if error.wire_code() == "proof_invalid" { 401 } else { 400 };
                    return Ok(Self::envelope(status, error.wire_code(), None));
                }
            };
            // The issuer takes `encrypted_key` from its own catalogue record, never from the request.
            let token = self
                .issuer
                .mint(&MintRequest::for_request(&request, &self.encrypted_key, self.ttl_secs), now)
                .map_err(|error| error.to_string())?;
            Ok(TokenHttpResponse {
                status: 200,
                body: serde_json::to_vec(&serde_json::json!({ "token": token })).unwrap(),
                retry_after_secs: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_endpoint::{FakeEndpoint, Scripted};
    use super::*;
    use fe_format::entitlement::{ContentKey, MintRequest};

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const NEXT_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    const NOW: u64 = 1_762_473_600;
    const BASE: &str = "https://registry.example.test";

    fn random_bytes() -> [u8; 32] {
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
        bytes
    }

    fn config(features: &[&str]) -> CatalogConfig {
        CatalogConfig { features: features.iter().map(|feature| feature.to_string()).collect(), ..CatalogConfig::default() }
    }

    /// Journey: no `entitlements` feature → no request; first ask issues and persists; the held
    /// token serves until its renewal point, then renews; a revoked licence keeps the held token
    /// until exp; an update asks again; a token the pins reject is never stored; a 429 backs off.
    #[test]
    fn token_is_requested_only_when_offered_held_per_package_and_renewed_before_exp() {
        let content_key = ContentKey::generate().unwrap();
        let endpoint = FakeEndpoint::new("issuer-1", &content_key, NOW);
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(directory.path()));
        let client = EntitlementClient::new(endpoint.clone(), endpoint.verifier(), store.clone());
        let identity = SigningKey::from_bytes(&random_bytes());
        let device = random_bytes();
        let licensee = Licensee { identity: &identity, device_secret: &device };
        let offered = config(&["index", "detail", "attestations", FEATURE_ENTITLEMENTS]);
        let target = TokenTarget { base: BASE, config: Some(&offered), package_id: "org.example.analytics-pack", content_hash: HASH };

        // A static tree (no `entitlements`) is never probed.
        let static_tree = config(&["index", "detail", "attestations"]);
        let unoffered = TokenTarget { config: Some(&static_tree), ..target };
        assert_eq!(client.token_for(&unoffered, &licensee, NOW), Err(TokenRequestError::NotOffered));
        assert_eq!(client.token_for(&TokenTarget { config: None, ..target }, &licensee, NOW), Err(TokenRequestError::NotOffered));
        assert_eq!(endpoint.post_count(), 0);

        // First ask: issued, verified, persisted per (licensee, package); the URL is the contract path.
        let first = client.token_for(&target, &licensee, NOW).unwrap();
        assert_eq!(first.source, TokenSource::Issued);
        assert_eq!(endpoint.urls.lock().unwrap()[0], "https://registry.example.test/api/v1/catalog/entitlements/token");
        assert_eq!(store.get(&licensee.did(), target.package_id).unwrap(), first.held);

        // A fresh client over the same directory serves the held token without a request.
        let restarted = EntitlementClient::new(endpoint.clone(), endpoint.verifier(), store.clone());
        let held = restarted.token_for(&target, &licensee, NOW + 60).unwrap();
        assert_eq!((held.source, endpoint.post_count()), (TokenSource::Stored, 1));

        // Past the renewal point (a fifth of the lifetime before exp) it renews.
        let due = first.held.renew_at();
        endpoint.now.store(due, std::sync::atomic::Ordering::SeqCst);
        let renewed = client.token_for(&target, &licensee, due).unwrap();
        assert_eq!((renewed.source, endpoint.post_count()), (TokenSource::Issued, 2));
        assert!(renewed.held.exp > first.held.exp);

        // Licence revoked: a due renewal is refused but the held token serves until exp.
        let due = renewed.held.renew_at();
        endpoint.script(Scripted::Refuse(403, "reissue_refused", None));
        let kept = client.token_for(&target, &licensee, due).unwrap();
        assert_eq!((kept.source, kept.renewal), (TokenSource::Stored, Some(TokenRequestError::ReissueRefused)));
        assert_eq!(kept.held, renewed.held);

        // After an update (new content_hash) the held token no longer covers the build: ask again.
        endpoint.now.store(NOW, std::sync::atomic::Ordering::SeqCst);
        let update = TokenTarget { content_hash: NEXT_HASH, ..target };
        assert_eq!(client.token_for(&update, &licensee, NOW).unwrap().held.content_hash, NEXT_HASH);

        // A 200 whose token is for another package is rejected and never replaces the held one.
        let other_package = endpoint
            .issuer
            .mint(
                &MintRequest {
                    licensee_did: &licensee.did(),
                    device_x25519_did: &licensee.device_did(),
                    package_id: "org.example.other",
                    content_hash: HASH,
                    encrypted_key: &endpoint.issuer.encrypted_key_for(&content_key).unwrap(),
                    ttl_secs: 3600,
                    jti: None,
                },
                NOW,
            )
            .unwrap();
        endpoint.script(Scripted::Token(other_package));
        let fresh_package = TokenTarget { package_id: "org.example.fleet-sim", ..target };
        assert_eq!(
            client.token_for(&fresh_package, &licensee, NOW),
            Err(TokenRequestError::TokenRejected(EntitlementError::PackageMismatch))
        );
        assert!(store.get(&licensee.did(), "org.example.fleet-sim").is_none());

        // 429 with Retry-After: the next ask inside the window sends nothing.
        endpoint.script(Scripted::Refuse(429, "rate_limited", Some(30)));
        let posts = endpoint.post_count();
        assert_eq!(client.token_for(&fresh_package, &licensee, NOW), Err(TokenRequestError::RateLimited { retry_after_secs: 30 }));
        assert_eq!(client.token_for(&fresh_package, &licensee, NOW + 10), Err(TokenRequestError::RateLimited { retry_after_secs: 20 }));
        assert_eq!(endpoint.post_count(), posts + 1, "no request inside the backoff");

        // Uninstall forgets the held token.
        client.forget(target.package_id, &licensee).unwrap();
        assert!(client.stored_token(target.package_id, NEXT_HASH, &licensee, NOW).is_none());
    }

    /// Every contract v1 refusal maps to its own state; the envelope code wins over the status.
    #[test]
    fn token_endpoint_refusals_map_to_contract_codes() {
        let response = |status: u16, code: Option<&str>, retry: Option<u64>| TokenHttpResponse {
            status,
            body: code.map_or_else(Vec::new, |code| serde_json::to_vec(&serde_json::json!({ "ok": false, "data": null, "error": code })).unwrap()),
            retry_after_secs: retry,
        };
        let cases = [
            (response(400, Some("invalid_request"), None), TokenRequestError::InvalidRequest),
            (response(401, Some("proof_invalid"), None), TokenRequestError::ProofInvalid),
            (response(403, Some("not_entitled"), None), TokenRequestError::NotEntitled),
            (response(403, Some("reissue_refused"), None), TokenRequestError::ReissueRefused),
            (response(403, Some("device_limit_reached"), None), TokenRequestError::DeviceLimitReached),
            (response(404, Some("unknown_package"), None), TokenRequestError::UnknownPackage),
            (response(429, Some("rate_limited"), Some(30)), TokenRequestError::RateLimited { retry_after_secs: 30 }),
            (response(429, None, None), TokenRequestError::RateLimited { retry_after_secs: DEFAULT_RATE_LIMIT_BACKOFF_SECS }),
            (response(429, None, Some(864_000)), TokenRequestError::RateLimited { retry_after_secs: MAX_RATE_LIMIT_BACKOFF_SECS }),
            (response(403, Some("future_code"), None), TokenRequestError::NotEntitled),
            (response(502, None, None), TokenRequestError::Unexpected { status: 502, detail: "no error code".into() }),
        ];
        for (answer, expected) in cases {
            assert_eq!(refusal(&answer), expected, "{answer:?}");
        }
        assert!(token_url("http://registry.example.test").is_err(), "plain http to a remote host is refused");
        assert!(token_url("http://127.0.0.1:8080").is_ok());
        assert!(token_url("C:/catalog").is_err(), "a folder endpoint has no token endpoint");
    }
}
