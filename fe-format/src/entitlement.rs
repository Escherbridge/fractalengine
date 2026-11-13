//! S-ENTITLEMENT primitives: EdDSA entitlement tokens, pinned issuer keys, X25519 content-key
//! wrap, XChaCha20-Poly1305 paid-entry sealing, `LocalIssuer` and the token-request proof
//! (design and wire formats: fe-format/src/AGENTS.md §entitlement).

use std::collections::BTreeMap;
use std::fmt;

use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, OsRng, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use jsonwebtoken::{decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

/// Domain string of the token-request proof (store-client contract v1 §5.2 rule 9).
pub const TOKEN_REQUEST_DOMAIN: &str = "fractalengine/entitlement-token-request/v1";
/// Largest accepted distance between a token request's `iat` and the verifier's clock.
pub const TOKEN_REQUEST_IAT_WINDOW_SECS: u64 = 300;
/// Largest token-request body `verify_token_request` parses.
pub const MAX_TOKEN_REQUEST_BYTES: usize = 4096;
/// Longest token lifetime (`exp - iat`) minted or accepted.
pub const MAX_ENTITLEMENT_TTL_SECS: u64 = 30 * 24 * 60 * 60;
/// Clock skew tolerated around `iat` and `exp` when verifying a token.
pub const TOKEN_CLOCK_LEEWAY_SECS: u64 = 60;
/// Length of a content key and of an X25519 key.
pub const CONTENT_KEY_LENGTH: usize = 32;

/// BLAKE3 derive-key context of the wrap key.
const WRAP_KDF_CONTEXT: &str = "fractalengine 2026-11 entitlement content-key wrap v2";
/// Associated-data prefix of a wrapped content key.
const WRAP_AAD_DOMAIN: &[u8] = b"fractalengine/entitlement-wrap/v2";
/// Associated-data prefix of a sealed paid entry.
const PAID_ENTRY_AAD_DOMAIN: &[u8] = b"fractalengine/entitlement-paid-entry/v2";
/// Format byte leading every wrapped key and sealed entry (2 = package-bound, R-STORE).
const FORMAT_VERSION: u8 = 2;
/// XChaCha20 nonce length.
const NONCE_LENGTH: usize = 24;
/// Poly1305 tag length.
const TAG_LENGTH: usize = 16;
/// Exact byte length of a wrapped content key: version, ephemeral public key, nonce, sealed key.
const WRAPPED_KEY_LENGTH: usize = 1 + 32 + NONCE_LENGTH + CONTENT_KEY_LENGTH + TAG_LENGTH;
/// Longest DID, package id or key id accepted anywhere in this module.
const MAX_IDENTIFIER_LENGTH: usize = 128;
/// Ed25519 did:key multicodec prefix.
const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];
/// X25519 did:key multicodec prefix.
const X25519_MULTICODEC: [u8; 2] = [0xec, 0x01];

// ---- errors -----------------------------------------------------------------

/// Why a token, wrap, entry or token request was refused.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum EntitlementError {
    /// A field fails its grammar or a document does not parse.
    #[error("malformed: {0}")]
    Malformed(String),
    /// The token names a key id that is not pinned.
    #[error("issuer key `{0}` is not pinned")]
    UnknownIssuerKey(String),
    /// The token signature does not verify against the pinned key.
    #[error("token signature does not verify")]
    BadSignature,
    /// `exp` has passed.
    #[error("token expired at {exp} (now {now})")]
    Expired { exp: u64, now: u64 },
    /// `iat` lies in the future beyond the leeway.
    #[error("token issued in the future")]
    NotYetValid,
    /// The token is for another package.
    #[error("token is for another package")]
    PackageMismatch,
    /// The token is for another build of the package.
    #[error("token is for another content hash")]
    ContentHashMismatch,
    /// The token was issued to another licensee.
    #[error("token was issued to another licensee")]
    LicenseeMismatch,
    /// The wrapped key does not open with this key and binding (wrong device, another package or
    /// build, or a tampered wrap).
    #[error("content key does not unwrap with this device key for this package")]
    WrongDevice,
    /// A paid entry does not open under this key and entry id.
    #[error("paid entry does not open with this content key")]
    EntryOpenFailed,
    /// The token-request proof does not verify.
    #[error("token-request proof does not verify")]
    ProofInvalid,
    /// The token request's `iat` is outside the accepted window.
    #[error("token request iat {iat} is more than {TOKEN_REQUEST_IAT_WINDOW_SECS}s from {now}")]
    StaleRequest { iat: u64, now: u64 },
    /// A cryptographic primitive or the randomness source failed.
    #[error("crypto failure: {0}")]
    Crypto(String),
}

impl EntitlementError {
    /// The token endpoint's error code for a refused token request (contract v1).
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::ProofInvalid | Self::StaleRequest { .. } => "proof_invalid",
            _ => "invalid_request",
        }
    }
}

/// Shorthand for a grammar failure.
fn malformed(detail: impl Into<String>) -> EntitlementError {
    EntitlementError::Malformed(detail.into())
}

// ---- small helpers ----------------------------------------------------------

/// Current Unix time in seconds (0 if the clock is before the epoch).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Overwrite `bytes` with zeros in a way the optimiser cannot elide.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference into `bytes`.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Fill `buffer` from the OS CSPRNG, failing closed.
fn fill_random(buffer: &mut [u8]) -> Result<(), EntitlementError> {
    OsRng
        .try_fill_bytes(buffer)
        .map_err(|_| EntitlementError::Crypto("randomness source unavailable".into()))
}

/// Install jsonwebtoken's pure-Rust crypto provider (idempotent).
fn ensure_crypto_provider() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

/// `content_hash` grammar: 64 lowercase hex characters.
fn check_content_hash(content_hash: &str) -> Result<(), EntitlementError> {
    let is_lower_hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    if content_hash.len() == 64 && content_hash.chars().all(is_lower_hex) {
        Ok(())
    } else {
        Err(malformed("content_hash must be 64 lowercase hex characters"))
    }
}

/// Non-empty, bounded identifier (package id, jti).
fn check_identifier(label: &str, value: &str) -> Result<(), EntitlementError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_LENGTH || value.chars().any(char::is_control) {
        return Err(malformed(format!("{label} must be 1..={MAX_IDENTIFIER_LENGTH} printable characters")));
    }
    Ok(())
}

/// Key-id grammar: 1..=64 of `[A-Za-z0-9._-]`.
fn check_kid(kid: &str) -> Result<(), EntitlementError> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if !kid.is_empty() && kid.len() <= 64 && kid.chars().all(allowed) {
        Ok(())
    } else {
        Err(malformed("kid must be 1..=64 of [A-Za-z0-9._-]"))
    }
}

/// Decode a `did:key:z…` body and strip an expected 2-byte multicodec prefix.
fn did_key_body(did: &str, multicodec: [u8; 2]) -> Result<[u8; 32], EntitlementError> {
    if did.len() > MAX_IDENTIFIER_LENGTH {
        return Err(malformed("DID too long"));
    }
    let encoded = did
        .strip_prefix("did:key:z")
        .ok_or_else(|| malformed("DID must start with did:key:z"))?;
    let decoded = bs58::decode(encoded).into_vec().map_err(|_| malformed("DID is not base58btc"))?;
    if decoded.len() != 34 || decoded[..2] != multicodec {
        return Err(malformed("DID has the wrong key type or length"));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded[2..]);
    Ok(key)
}

/// `did:key` of a 32-byte public key under a multicodec prefix.
fn did_key_from(multicodec: [u8; 2], key: &[u8; 32]) -> String {
    let mut body = Vec::with_capacity(34);
    body.extend_from_slice(&multicodec);
    body.extend_from_slice(key);
    format!("did:key:z{}", bs58::encode(body).into_string())
}

/// Ed25519 verifying key of a canonical `did:key` (non-canonical spellings refused).
pub fn ed25519_key_from_did(did: &str) -> Result<VerifyingKey, EntitlementError> {
    let bytes = did_key_body(did, ED25519_MULTICODEC)?;
    let key = VerifyingKey::from_bytes(&bytes).map_err(|_| malformed("not an ed25519 point"))?;
    if did_key_from(ED25519_MULTICODEC, &bytes) != did {
        return Err(malformed("DID is not in canonical did:key form"));
    }
    Ok(key)
}

/// Ed25519 `did:key` of a verifying key.
pub fn ed25519_key_to_did(key: &VerifyingKey) -> String {
    did_key_from(ED25519_MULTICODEC, &key.to_bytes())
}

/// X25519 public key of a canonical device `did:key` (multicodec `0xec01`).
pub fn x25519_public_key_from_did(did: &str) -> Result<[u8; 32], EntitlementError> {
    let bytes = did_key_body(did, X25519_MULTICODEC)?;
    if did_key_from(X25519_MULTICODEC, &bytes) != did {
        return Err(malformed("device DID is not in canonical did:key form"));
    }
    Ok(bytes)
}

/// X25519 device `did:key` of a public key (same form as `fe_identity::x25519`).
pub fn x25519_public_key_to_did(public_key: &[u8; 32]) -> String {
    did_key_from(X25519_MULTICODEC, public_key)
}

/// X25519 public key of a 32-byte static secret.
pub fn x25519_public_key(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

// ---- content key ------------------------------------------------------------

/// A 32-byte content key: zeroized on drop, redacted in `Debug`, compared in constant time.
pub struct ContentKey([u8; CONTENT_KEY_LENGTH]);

impl ContentKey {
    /// Take custody of raw key bytes.
    pub fn from_bytes(bytes: [u8; CONTENT_KEY_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Draw a fresh key from the OS CSPRNG.
    pub fn generate() -> Result<Self, EntitlementError> {
        let mut key = Self([0u8; CONTENT_KEY_LENGTH]);
        fill_random(&mut key.0)?;
        Ok(key)
    }

    /// Borrow the raw key bytes (the one disclosure point).
    pub fn expose_bytes(&self) -> &[u8; CONTENT_KEY_LENGTH] {
        &self.0
    }

    /// The AEAD keyed with this content key.
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.0))
    }
}

impl fmt::Debug for ContentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContentKey(redacted)")
    }
}

impl PartialEq for ContentKey {
    /// Constant-time comparison.
    fn eq(&self, other: &Self) -> bool {
        self.0.iter().zip(other.0.iter()).fold(0u8, |acc, (left, right)| acc | (left ^ right)) == 0
    }
}

impl Eq for ContentKey {}

impl Drop for ContentKey {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

// ---- X25519 wrap ------------------------------------------------------------

/// What a wrapped content key is bound to; a wrap opens only under the binding it was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapBinding<'a> {
    /// `license.json` `encrypted_key` (wrapped to the issuer): the package only, because the
    /// manifest's `content_hash` itself covers this wrap.
    Archive { package_id: &'a str },
    /// A token's `wrapped_content_key` (wrapped to a licensee device): package and build.
    Device { package_id: &'a str, content_hash: &'a str },
}

impl WrapBinding<'_> {
    /// Tagged, length-prefixed encoding fed to both the KDF and the AAD.
    fn encode(&self) -> Result<Vec<u8>, EntitlementError> {
        let (tag, package_id, content_hash) = match *self {
            Self::Archive { package_id } => (b'A', package_id, None),
            Self::Device { package_id, content_hash } => (b'D', package_id, Some(content_hash)),
        };
        check_identifier("package_id", package_id)?;
        let mut out = vec![tag];
        push_length_prefixed(&mut out, package_id.as_bytes());
        if let Some(content_hash) = content_hash {
            check_content_hash(content_hash)?;
            push_length_prefixed(&mut out, content_hash.as_bytes());
        }
        Ok(out)
    }
}

/// Append `bytes` behind a big-endian u16 length (fields are at most 128 bytes).
fn push_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Wrap key = BLAKE3-derive(shared ‖ ephemeral public ‖ recipient public ‖ binding).
fn derive_wrap_key(shared: &[u8; 32], ephemeral_public: &[u8; 32], recipient_public: &[u8; 32], binding: &[u8]) -> ContentKey {
    let mut material = Vec::with_capacity(96 + binding.len());
    material.extend_from_slice(shared);
    material.extend_from_slice(ephemeral_public);
    material.extend_from_slice(recipient_public);
    material.extend_from_slice(binding);
    let key = ContentKey(blake3::derive_key(WRAP_KDF_CONTEXT, &material));
    wipe(&mut material);
    key
}

/// Associated data binding a wrap to both public keys and to its package (and build).
fn wrap_aad(ephemeral_public: &[u8; 32], recipient_public: &[u8; 32], binding: &[u8]) -> Vec<u8> {
    [WRAP_AAD_DOMAIN, ephemeral_public, recipient_public, binding].concat()
}

/// Wrap `content_key` to a recipient X25519 public key (ephemeral-static ECDH) under `binding`; base64 out.
pub fn wrap_content_key(
    content_key: &ContentKey,
    recipient_public: &[u8; 32],
    binding: &WrapBinding<'_>,
) -> Result<String, EntitlementError> {
    let binding = binding.encode()?;
    let ephemeral = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral).to_bytes();
    let shared = ephemeral.diffie_hellman(&PublicKey::from(*recipient_public));
    if !shared.was_contributory() {
        return Err(malformed("recipient key is a low-order point"));
    }
    let wrap_key = derive_wrap_key(shared.as_bytes(), &ephemeral_public, recipient_public, &binding);
    let mut nonce = [0u8; NONCE_LENGTH];
    fill_random(&mut nonce)?;
    let aad = wrap_aad(&ephemeral_public, recipient_public, &binding);
    let sealed = wrap_key
        .cipher()
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: content_key.expose_bytes(), aad: &aad })
        .map_err(|_| EntitlementError::Crypto("wrap seal failed".into()))?;
    let mut out = Vec::with_capacity(WRAPPED_KEY_LENGTH);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&ephemeral_public);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(BASE64.encode(out))
}

/// Unwrap a base64 wrapped key with the recipient's X25519 static secret; the binding must match the wrap's.
pub fn unwrap_content_key(
    wrapped: &str,
    recipient_secret: &[u8; 32],
    binding: &WrapBinding<'_>,
) -> Result<ContentKey, EntitlementError> {
    let binding = binding.encode()?;
    let bytes = BASE64.decode(wrapped).map_err(|_| malformed("wrapped key is not base64"))?;
    if bytes.len() != WRAPPED_KEY_LENGTH || bytes[0] != FORMAT_VERSION {
        return Err(malformed("wrapped key has the wrong length or version"));
    }
    let mut ephemeral_public = [0u8; 32];
    ephemeral_public.copy_from_slice(&bytes[1..33]);
    let nonce = &bytes[33..33 + NONCE_LENGTH];
    let sealed = &bytes[33 + NONCE_LENGTH..];
    let secret = StaticSecret::from(*recipient_secret);
    let recipient_public = PublicKey::from(&secret).to_bytes();
    let shared = secret.diffie_hellman(&PublicKey::from(ephemeral_public));
    if !shared.was_contributory() {
        return Err(malformed("ephemeral key is a low-order point"));
    }
    let wrap_key = derive_wrap_key(shared.as_bytes(), &ephemeral_public, &recipient_public, &binding);
    let aad = wrap_aad(&ephemeral_public, &recipient_public, &binding);
    let mut opened = wrap_key
        .cipher()
        .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &aad })
        .map_err(|_| EntitlementError::WrongDevice)?;
    let mut key = ContentKey([0u8; CONTENT_KEY_LENGTH]);
    let fits = opened.len() == CONTENT_KEY_LENGTH;
    if fits {
        key.0.copy_from_slice(&opened);
    }
    wipe(&mut opened);
    if fits { Ok(key) } else { Err(malformed("unwrapped key has the wrong length")) }
}

// ---- paid entries -----------------------------------------------------------

/// Associated data binding a sealed entry to its package and entry id (not the build: the
/// manifest's `content_hash` covers the sealed bytes).
fn paid_entry_aad(package_id: &str, entry_id: &str) -> Result<Vec<u8>, EntitlementError> {
    check_identifier("package_id", package_id)?;
    let mut aad = PAID_ENTRY_AAD_DOMAIN.to_vec();
    push_length_prefixed(&mut aad, package_id.as_bytes());
    aad.extend_from_slice(entry_id.as_bytes());
    Ok(aad)
}

/// Seal one non-free entry's bytes under the content key, bound to `package_id` and `entry_id`.
pub fn seal_paid_entry(
    content_key: &ContentKey,
    package_id: &str,
    entry_id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, EntitlementError> {
    let mut nonce = [0u8; NONCE_LENGTH];
    fill_random(&mut nonce)?;
    let aad = paid_entry_aad(package_id, entry_id)?;
    let sealed = content_key
        .cipher()
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &aad })
        .map_err(|_| EntitlementError::Crypto("entry seal failed".into()))?;
    let mut out = Vec::with_capacity(1 + NONCE_LENGTH + sealed.len());
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Open a sealed entry in memory; the caller never persists the plaintext.
pub fn open_paid_entry(
    content_key: &ContentKey,
    package_id: &str,
    entry_id: &str,
    sealed: &[u8],
) -> Result<Vec<u8>, EntitlementError> {
    if sealed.len() < 1 + NONCE_LENGTH + TAG_LENGTH || sealed[0] != FORMAT_VERSION {
        return Err(malformed("sealed entry has the wrong length or version"));
    }
    let nonce = &sealed[1..1 + NONCE_LENGTH];
    let aad = paid_entry_aad(package_id, entry_id)?;
    content_key
        .cipher()
        .decrypt(XNonce::from_slice(nonce), Payload { msg: &sealed[1 + NONCE_LENGTH..], aad: &aad })
        .map_err(|_| EntitlementError::EntryOpenFailed)
}

// ---- tokens -----------------------------------------------------------------

/// Claims of an EdDSA entitlement token (hexon_entitlements spec §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitlementClaims {
    /// Licensee ed25519 identity DID.
    pub sub: String,
    /// The package's `hexon_id`.
    pub package_id: String,
    /// Lowercase hex BLAKE3 of the canonical manifest with `signature` blanked.
    pub content_hash: String,
    /// Issued-at, Unix seconds.
    pub iat: u64,
    /// Expiry, Unix seconds.
    pub exp: u64,
    /// Token id; reserved, unused by v1.
    pub jti: String,
    /// Pinned issuer key id that signed this token.
    pub kid: String,
    /// Content key wrapped to the licensee's X25519 device key (base64).
    pub wrapped_content_key: String,
}

/// Trusted issuer keys, pinned locally as `(kid, ed25519 public key)` pairs.
#[derive(Debug, Clone, Default)]
pub struct IssuerKeyring {
    keys: BTreeMap<String, VerifyingKey>,
}

impl IssuerKeyring {
    /// An empty keyring (verifies nothing).
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin `key` under `kid`; re-pinning a kid to a different key is refused.
    pub fn pin(&mut self, kid: &str, key: VerifyingKey) -> Result<(), EntitlementError> {
        check_kid(kid)?;
        match self.keys.get(kid) {
            Some(existing) if *existing != key => Err(malformed(format!("kid `{kid}` is already pinned to another key"))),
            _ => {
                self.keys.insert(kid.to_string(), key);
                Ok(())
            }
        }
    }

    /// Pin the ed25519 key named by a `did:key` under `kid`.
    pub fn pin_did(&mut self, kid: &str, did: &str) -> Result<(), EntitlementError> {
        self.pin(kid, ed25519_key_from_did(did)?)
    }

    /// The key pinned under `kid`.
    pub fn get(&self, kid: &str) -> Option<&VerifyingKey> {
        self.keys.get(kid)
    }

    /// Number of pinned keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no key is pinned.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// What the opener expects a token to cover.
#[derive(Debug, Clone, Copy)]
pub struct EntitlementExpectation<'a> {
    /// The package being opened.
    pub package_id: &'a str,
    /// Its recomputed content hash (never taken from a document).
    pub content_hash: &'a str,
    /// The local licensee identity DID.
    pub licensee_did: &'a str,
}

/// A verified token and the content key it unwrapped.
#[derive(Debug)]
pub struct VerifiedEntitlement {
    /// The verified claims.
    pub claims: EntitlementClaims,
    /// The recovered content key.
    pub content_key: ContentKey,
}

/// Verifies entitlement tokens offline against pinned issuer keys; never phones home.
pub trait EntitlementVerifier: Send + Sync {
    /// Verify `token` for `expected` at Unix time `now` and unwrap its key with the device secret.
    fn verify(
        &self,
        token: &str,
        expected: &EntitlementExpectation<'_>,
        device_secret: &[u8; 32],
        now: u64,
    ) -> Result<VerifiedEntitlement, EntitlementError>;
}

/// Reference verifier over an [`IssuerKeyring`].
#[derive(Debug, Clone)]
pub struct KeyringVerifier {
    keyring: IssuerKeyring,
}

impl KeyringVerifier {
    /// A verifier trusting exactly `keyring`.
    pub fn new(keyring: IssuerKeyring) -> Self {
        Self { keyring }
    }

    /// The pinned keys.
    pub fn keyring(&self) -> &IssuerKeyring {
        &self.keyring
    }

    /// Steps 1-2 (signature by a pinned kid, time, package, hash, licensee) without unwrapping.
    pub fn verify_claims(
        &self,
        token: &str,
        expected: &EntitlementExpectation<'_>,
        now: u64,
    ) -> Result<EntitlementClaims, EntitlementError> {
        ensure_crypto_provider();
        let header = decode_header(token).map_err(|_| malformed("token header does not decode"))?;
        if header.alg != Algorithm::EdDSA {
            return Err(malformed("token algorithm must be EdDSA"));
        }
        let kid = header.kid.ok_or_else(|| malformed("token header has no kid"))?;
        let key = self.keyring.get(&kid).ok_or_else(|| EntitlementError::UnknownIssuerKey(kid.clone()))?;
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims = std::collections::HashSet::new();
        let data = decode::<EntitlementClaims>(token, &DecodingKey::from_ed_der(&key.to_bytes()), &validation)
            .map_err(|error| match error.kind() {
                jsonwebtoken::errors::ErrorKind::InvalidSignature => EntitlementError::BadSignature,
                _ => malformed("token does not decode"),
            })?;
        let claims = data.claims;
        if claims.kid != kid {
            return Err(malformed("claim kid differs from header kid"));
        }
        match claims.exp.checked_sub(claims.iat) {
            Some(span) if span > 0 && span <= MAX_ENTITLEMENT_TTL_SECS => {}
            _ => return Err(malformed("token lifetime is empty, negative or over the cap")),
        }
        if now > claims.exp.saturating_add(TOKEN_CLOCK_LEEWAY_SECS) {
            return Err(EntitlementError::Expired { exp: claims.exp, now });
        }
        if claims.iat > now.saturating_add(TOKEN_CLOCK_LEEWAY_SECS) {
            return Err(EntitlementError::NotYetValid);
        }
        if claims.package_id != expected.package_id {
            return Err(EntitlementError::PackageMismatch);
        }
        if claims.content_hash != expected.content_hash {
            return Err(EntitlementError::ContentHashMismatch);
        }
        if claims.sub != expected.licensee_did {
            return Err(EntitlementError::LicenseeMismatch);
        }
        Ok(claims)
    }
}

impl EntitlementVerifier for KeyringVerifier {
    fn verify(
        &self,
        token: &str,
        expected: &EntitlementExpectation<'_>,
        device_secret: &[u8; 32],
        now: u64,
    ) -> Result<VerifiedEntitlement, EntitlementError> {
        let claims = self.verify_claims(token, expected, now)?;
        let binding = WrapBinding::Device { package_id: &claims.package_id, content_hash: &claims.content_hash };
        let content_key = unwrap_content_key(&claims.wrapped_content_key, device_secret, &binding)?;
        Ok(VerifiedEntitlement { claims, content_key })
    }
}

// ---- issuer -----------------------------------------------------------------

/// One mint: who, which device, which build, the archive's issuer-wrapped key and the lifetime.
#[derive(Debug, Clone, Copy)]
pub struct MintRequest<'a> {
    /// Licensee ed25519 identity DID (becomes `sub`).
    pub licensee_did: &'a str,
    /// Licensee X25519 device DID the content key is re-wrapped to.
    pub device_x25519_did: &'a str,
    /// The package's `hexon_id`.
    pub package_id: &'a str,
    /// The build's content hash.
    pub content_hash: &'a str,
    /// `license.json` `encrypted_key`: the content key wrapped to this issuer for `package_id`.
    /// Issuers take it from their own catalogue record for the requested package, never from a request.
    pub encrypted_key: &'a str,
    /// Token lifetime in seconds (1..=[`MAX_ENTITLEMENT_TTL_SECS`]).
    pub ttl_secs: u64,
    /// Token id; a random one when `None`.
    pub jti: Option<&'a str>,
}

impl<'a> MintRequest<'a> {
    /// A mint for a verified token request.
    pub fn for_request(request: &'a TokenRequest, encrypted_key: &'a str, ttl_secs: u64) -> Self {
        Self {
            licensee_did: &request.licensee_did,
            device_x25519_did: &request.device_x25519_did,
            package_id: &request.package_id,
            content_hash: &request.content_hash,
            encrypted_key,
            ttl_secs,
            jti: None,
        }
    }
}

/// A self-hosted issuer: an ed25519 signing key under a kid plus the X25519 key archives wrap to.
pub struct LocalIssuer {
    kid: String,
    signing_key: SigningKey,
    wrap_secret: [u8; 32],
}

impl fmt::Debug for LocalIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LocalIssuer").field("kid", &self.kid).finish_non_exhaustive()
    }
}

impl Drop for LocalIssuer {
    fn drop(&mut self) {
        wipe(&mut self.wrap_secret);
    }
}

impl LocalIssuer {
    /// An issuer from existing key material.
    pub fn new(kid: &str, signing_key: SigningKey, wrap_secret: [u8; 32]) -> Result<Self, EntitlementError> {
        check_kid(kid)?;
        Ok(Self { kid: kid.to_string(), signing_key, wrap_secret })
    }

    /// A fresh issuer with newly generated keys.
    pub fn generate(kid: &str) -> Result<Self, EntitlementError> {
        let mut seed = [0u8; 32];
        fill_random(&mut seed)?;
        let signing_key = SigningKey::from_bytes(&seed);
        wipe(&mut seed);
        let mut wrap_secret = [0u8; 32];
        fill_random(&mut wrap_secret)?;
        Self::new(kid, signing_key, wrap_secret)
    }

    /// The key id tokens carry.
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The ed25519 key verifiers pin under [`kid`](Self::kid).
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// The X25519 public key publishers wrap a content key to (`license.json` `encrypted_key`).
    pub fn wrap_public_key(&self) -> [u8; 32] {
        x25519_public_key(&self.wrap_secret)
    }

    /// The `encrypted_key` value for `package_id`'s archive sealed under `content_key`.
    pub fn encrypted_key_for(&self, content_key: &ContentKey, package_id: &str) -> Result<String, EntitlementError> {
        wrap_content_key(content_key, &self.wrap_public_key(), &WrapBinding::Archive { package_id })
    }

    /// Mint a token: unwrap the package-bound archive key with the issuer secret (a wrap made for
    /// another package does not open), re-wrap it to the device for this build, sign.
    pub fn mint(&self, request: &MintRequest<'_>, now: u64) -> Result<String, EntitlementError> {
        ensure_crypto_provider();
        ed25519_key_from_did(request.licensee_did)?;
        let device_public = x25519_public_key_from_did(request.device_x25519_did)?;
        check_identifier("package_id", request.package_id)?;
        check_content_hash(request.content_hash)?;
        if request.ttl_secs == 0 || request.ttl_secs > MAX_ENTITLEMENT_TTL_SECS {
            return Err(malformed(format!("ttl must be 1..={MAX_ENTITLEMENT_TTL_SECS} seconds")));
        }
        let jti = match request.jti {
            Some(jti) => {
                check_identifier("jti", jti)?;
                jti.to_string()
            }
            None => {
                let mut raw = [0u8; 16];
                fill_random(&mut raw)?;
                URL_SAFE_NO_PAD.encode(raw)
            }
        };
        let archive_binding = WrapBinding::Archive { package_id: request.package_id };
        let content_key = unwrap_content_key(request.encrypted_key, &self.wrap_secret, &archive_binding)?;
        let device_binding = WrapBinding::Device { package_id: request.package_id, content_hash: request.content_hash };
        let claims = EntitlementClaims {
            sub: request.licensee_did.to_string(),
            package_id: request.package_id.to_string(),
            content_hash: request.content_hash.to_string(),
            iat: now,
            exp: now.saturating_add(request.ttl_secs),
            jti,
            kid: self.kid.clone(),
            wrapped_content_key: wrap_content_key(&content_key, &device_public, &device_binding)?,
        };
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        let der = {
            use ed25519_dalek::pkcs8::EncodePrivateKey;
            self.signing_key
                .to_pkcs8_der()
                .map_err(|_| EntitlementError::Crypto("PKCS#8 encoding failed".into()))?
        };
        encode(&header, &claims, &EncodingKey::from_ed_der(der.as_bytes()))
            .map_err(|error| EntitlementError::Crypto(format!("token signing failed: {error}")))
    }
}

// ---- token request ----------------------------------------------------------

/// Body of `POST {base}/api/v1/catalog/entitlements/token` (store-client contract v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRequest {
    /// The package's `hexon_id`.
    pub package_id: String,
    /// The build's content hash.
    pub content_hash: String,
    /// Licensee ed25519 identity DID; its key signs `proof`.
    pub licensee_did: String,
    /// The device X25519 DID the token's key is wrapped to.
    pub device_x25519_did: String,
    /// Unix seconds.
    pub iat: u64,
    /// Base64 ed25519 signature over the domain, a line feed and the canonical body minus `proof`.
    pub proof: String,
}

/// Bytes a token-request proof signs: domain, `\n`, canonical JSON of `document` minus `proof`.
pub fn token_request_signing_bytes(document: &Value) -> Result<Vec<u8>, EntitlementError> {
    let mut body = document.as_object().cloned().ok_or_else(|| malformed("token request must be a JSON object"))?;
    body.remove("proof");
    let canonical = crate::signature::canonical_json_bytes(&Value::Object(body))
        .map_err(|error| malformed(format!("token request does not canonicalise: {error}")))?;
    let mut bytes = Vec::with_capacity(TOKEN_REQUEST_DOMAIN.len() + 1 + canonical.len());
    bytes.extend_from_slice(TOKEN_REQUEST_DOMAIN.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(&canonical);
    Ok(bytes)
}

/// The base64 `proof` for `document` (every member but `proof` is covered) under `signing_key`.
pub fn token_request_proof(signing_key: &SigningKey, document: &Value) -> Result<String, EntitlementError> {
    let bytes = token_request_signing_bytes(document)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

impl TokenRequest {
    /// A signed request from the licensee identity key for one device and build.
    pub fn sign(
        signing_key: &SigningKey,
        package_id: &str,
        content_hash: &str,
        device_x25519_did: &str,
        iat: u64,
    ) -> Result<Self, EntitlementError> {
        check_identifier("package_id", package_id)?;
        check_content_hash(content_hash)?;
        x25519_public_key_from_did(device_x25519_did)?;
        let mut request = Self {
            package_id: package_id.to_string(),
            content_hash: content_hash.to_string(),
            licensee_did: ed25519_key_to_did(&signing_key.verifying_key()),
            device_x25519_did: device_x25519_did.to_string(),
            iat,
            proof: String::new(),
        };
        request.proof = token_request_proof(signing_key, &request.to_json())?;
        Ok(request)
    }

    /// The request as a JSON document (the wire body).
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Verify a received token-request body: grammar, proof over the body as received, `iat` window.
pub fn verify_token_request(body: &[u8], now: u64) -> Result<TokenRequest, EntitlementError> {
    if body.len() > MAX_TOKEN_REQUEST_BYTES {
        return Err(malformed("token request too large"));
    }
    let document: Value = serde_json::from_slice(body).map_err(|_| malformed("token request is not JSON"))?;
    let request: TokenRequest =
        serde_json::from_value(document.clone()).map_err(|error| malformed(format!("token request: {error}")))?;
    check_identifier("package_id", &request.package_id)?;
    check_content_hash(&request.content_hash)?;
    x25519_public_key_from_did(&request.device_x25519_did)?;
    let licensee_key = ed25519_key_from_did(&request.licensee_did)?;
    let signature_bytes = BASE64.decode(&request.proof).map_err(|_| EntitlementError::ProofInvalid)?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| EntitlementError::ProofInvalid)?;
    let signed = token_request_signing_bytes(&document)?;
    licensee_key.verify_strict(&signed, &signature).map_err(|_| EntitlementError::ProofInvalid)?;
    if now.abs_diff(request.iat) > TOKEN_REQUEST_IAT_WINDOW_SECS {
        return Err(EntitlementError::StaleRequest { iat: request.iat, now });
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    const NOW: u64 = 1_762_473_600;

    fn identity() -> SigningKey {
        let mut seed = [0u8; 32];
        fill_random(&mut seed).unwrap();
        SigningKey::from_bytes(&seed)
    }

    fn device_secret() -> [u8; 32] {
        let mut secret = [0u8; 32];
        fill_random(&mut secret).unwrap();
        secret
    }

    /// AC-3/AC-4 journey: publisher seals under a content key wrapped to the issuer, the issuer
    /// mints for one licensee device, the verifier recovers the key and opens the paid entry;
    /// every forged or mismatched token is refused with its own reason, never plaintext.
    #[test]
    fn licensed_entry_round_trips_and_every_forged_token_is_refused() {
        let issuer = LocalIssuer::generate("issuer-1").unwrap();
        let attacker = LocalIssuer::generate("rogue").unwrap();
        let mut keyring = IssuerKeyring::new();
        keyring.pin("issuer-1", issuer.verifying_key()).unwrap();
        assert!(keyring.pin("issuer-1", attacker.verifying_key()).is_err(), "a pinned kid is never re-pinned");
        let verifier = KeyringVerifier::new(keyring);

        // Publisher side: seal one paid entry, wrap the key to the issuer for license.json.
        let content_key = ContentKey::generate().unwrap();
        let sealed = seal_paid_entry(&content_key, "pkg.terrain", "terrain_full", b"full-resolution tiles").unwrap();
        let encrypted_key = issuer.encrypted_key_for(&content_key, "pkg.terrain").unwrap();

        let licensee = identity();
        let licensee_did = ed25519_key_to_did(&licensee.verifying_key());
        let device = device_secret();
        let device_did = x25519_public_key_to_did(&x25519_public_key(&device));
        let mint = |minter: &LocalIssuer, hash: &str, at: u64| {
            minter
                .mint(
                    &MintRequest {
                        licensee_did: &licensee_did,
                        device_x25519_did: &device_did,
                        package_id: "pkg.terrain",
                        content_hash: hash,
                        encrypted_key: &encrypted_key,
                        ttl_secs: 3600,
                        jti: None,
                    },
                    at,
                )
                .unwrap()
        };
        let expected = EntitlementExpectation { package_id: "pkg.terrain", content_hash: HASH, licensee_did: &licensee_did };

        // Valid: the exact content key comes back and the entry opens to its plaintext.
        let good = mint(&issuer, HASH, NOW);
        let verified = verifier.verify(&good, &expected, &device, NOW + 10).unwrap();
        assert_eq!(verified.content_key, content_key);
        assert_eq!(open_paid_entry(&verified.content_key, "pkg.terrain", "terrain_full", &sealed).unwrap(), b"full-resolution tiles");
        for (why, package, entry) in [("entry id", "pkg.terrain", "preview_1"), ("package", "pkg.other", "terrain_full")] {
            assert_eq!(
                open_paid_entry(&verified.content_key, package, entry, &sealed),
                Err(EntitlementError::EntryOpenFailed),
                "a sealed entry is bound to its {why}"
            );
        }

        // Cross-package wrap (key-unwrap oracle): package A's archive wrap presented in a mint for
        // package B never opens, so the issuer cannot be made to re-wrap A's key to the caller.
        let lifted = MintRequest {
            licensee_did: &licensee_did,
            device_x25519_did: &device_did,
            package_id: "pkg.other",
            content_hash: HASH,
            encrypted_key: &encrypted_key,
            ttl_secs: 3600,
            jti: None,
        };
        assert_eq!(issuer.mint(&lifted, NOW), Err(EntitlementError::WrongDevice));
        // A device wrap is bound to its build too: re-labelling the build cannot open it.
        let other_build = WrapBinding::Device { package_id: "pkg.terrain", content_hash: OTHER_HASH };
        assert_eq!(
            unwrap_content_key(&verified.claims.wrapped_content_key, &device, &other_build).map(|_| ()),
            Err(EntitlementError::WrongDevice)
        );

        // The attacker cannot mint against the issuer's archive key (it is wrapped to the issuer).
        assert_eq!(
            attacker.mint(
                &MintRequest {
                    licensee_did: &licensee_did,
                    device_x25519_did: &device_did,
                    package_id: "pkg.terrain",
                    content_hash: HASH,
                    encrypted_key: &encrypted_key,
                    ttl_secs: 3600,
                    jti: None,
                },
                NOW,
            ),
            Err(EntitlementError::WrongDevice)
        );
        // Attacker-signed token naming the pinned kid: same claims, wrong signer.
        let spoofed_kid = {
            let impostor = LocalIssuer::new("issuer-1", identity(), issuer.wrap_secret).unwrap();
            mint(&impostor, HASH, NOW)
        };
        let mut flipped = good.clone();
        let index = flipped.len() - 10;
        let replacement = if flipped.as_bytes()[index] == b'A' { "B" } else { "A" };
        flipped.replace_range(index..index + 1, replacement);
        let other_licensee = ed25519_key_to_did(&identity().verifying_key());

        let cases: Vec<(&str, String, EntitlementExpectation<'_>, [u8; 32], u64, EntitlementError)> = vec![
            ("forged kid", mint_rogue(&attacker, &licensee_did, &device_did, &content_key), expected, device, NOW, EntitlementError::UnknownIssuerKey("rogue".into())),
            ("spoofed pinned kid", spoofed_kid, expected, device, NOW, EntitlementError::BadSignature),
            ("flipped signature", flipped, expected, device, NOW, EntitlementError::BadSignature),
            ("wrong hash", mint(&issuer, OTHER_HASH, NOW), expected, device, NOW, EntitlementError::ContentHashMismatch),
            ("expired", good.clone(), expected, device, NOW + 3600 + TOKEN_CLOCK_LEEWAY_SECS + 1, EntitlementError::Expired { exp: NOW + 3600, now: NOW + 3600 + TOKEN_CLOCK_LEEWAY_SECS + 1 }),
            ("wrong package", good.clone(), EntitlementExpectation { package_id: "pkg.other", ..expected }, device, NOW, EntitlementError::PackageMismatch),
            ("wrong licensee", good.clone(), EntitlementExpectation { licensee_did: &other_licensee, ..expected }, device, NOW, EntitlementError::LicenseeMismatch),
            ("wrong device", good.clone(), expected, device_secret(), NOW, EntitlementError::WrongDevice),
        ];
        for (why, token, expectation, secret, at, refusal) in cases {
            assert_eq!(verifier.verify(&token, &expectation, &secret, at).map(|_| ()), Err(refusal), "{why}");
        }
    }

    /// A token from an unpinned issuer that wraps its own key (the attacker controls everything but the pin).
    fn mint_rogue(attacker: &LocalIssuer, licensee_did: &str, device_did: &str, content_key: &ContentKey) -> String {
        let encrypted_key = attacker.encrypted_key_for(content_key, "pkg.terrain").unwrap();
        attacker
            .mint(
                &MintRequest {
                    licensee_did,
                    device_x25519_did: device_did,
                    package_id: "pkg.terrain",
                    content_hash: HASH,
                    encrypted_key: &encrypted_key,
                    ttl_secs: 3600,
                    jti: Some("rogue-1"),
                },
                NOW,
            )
            .unwrap()
    }

    /// Token-request proof: valid once inside the iat window; covers unknown members as received;
    /// refuses tampering, a stale iat, another signer and a domain-less signature.
    #[test]
    fn token_request_proof_verifies_in_window_and_refuses_tampering() {
        let licensee = identity();
        let device_did = x25519_public_key_to_did(&x25519_public_key(&device_secret()));
        let request = TokenRequest::sign(&licensee, "pkg.terrain", HASH, &device_did, NOW).unwrap();
        let body = |value: &Value| serde_json::to_vec(value).unwrap();

        assert_eq!(verify_token_request(&body(&request.to_json()), NOW + 299).unwrap(), request);

        let mut extended = request.to_json();
        extended["client"] = Value::from("fe/1");
        extended["proof"] = Value::from(token_request_proof(&licensee, &extended).unwrap());
        assert!(verify_token_request(&body(&extended), NOW).is_ok(), "members unknown to v1 are signed as received");

        let mut tampered = request.to_json();
        tampered["package_id"] = Value::from("pkg.other");
        let mut appended = request.to_json();
        appended["client"] = Value::from("fe/1");
        let mut other_signer = request.to_json();
        other_signer["proof"] = Value::from(token_request_proof(&identity(), &request.to_json()).unwrap());
        let mut no_domain = request.to_json();
        let bare = crate::signature::canonical_json_bytes(&{
            let mut doc = request.to_json();
            doc.as_object_mut().unwrap().remove("proof");
            doc
        })
        .unwrap();
        no_domain["proof"] = Value::from(BASE64.encode(licensee.sign(&bare).to_bytes()));

        let cases = [
            ("tampered field", tampered, NOW, EntitlementError::ProofInvalid),
            ("member added after signing", appended, NOW, EntitlementError::ProofInvalid),
            ("another signer", other_signer, NOW, EntitlementError::ProofInvalid),
            ("no domain separation", no_domain, NOW, EntitlementError::ProofInvalid),
            ("stale iat", request.to_json(), NOW + 301, EntitlementError::StaleRequest { iat: NOW, now: NOW + 301 }),
        ];
        for (why, document, at, refusal) in cases {
            assert_eq!(verify_token_request(&body(&document), at).map(|_| ()), Err(refusal.clone()), "{why}");
            assert_eq!(refusal.wire_code(), "proof_invalid", "{why}");
        }
        assert_eq!(verify_token_request(b"{}", NOW).unwrap_err().wire_code(), "invalid_request");
    }
}
