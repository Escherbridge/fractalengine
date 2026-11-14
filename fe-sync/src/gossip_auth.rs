//! Signed gossip frame v1, domain-separated signing and the replay guard; see `src/AGENTS.md` §gossip-signing.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use fe_canonical_log::cbor::{decode_canonical, encode_canonical_checked, CborValue};

/// Four-byte prefix that marks a signed frame; bytes that start with it are never parsed as legacy.
pub const FRAME_MAGIC: [u8; 4] = *b"FEG1";
/// The only frame version this build writes or accepts.
pub const FRAME_VERSION: u64 = 1;
/// Domain-separation prefix of every signed message.
pub const SIGNING_DOMAIN: &[u8] = b"fe-gossip-v1\0";
/// Frame-size ceiling, checked before any decoding (the gossip transport maximum).
pub const MAX_FRAME_BYTES: usize = crate::distributed_query::GOSSIP_MAX_MESSAGE_SIZE;
/// Longest accepted `verse_id` or `author_did`, in bytes.
pub const MAX_IDENTIFIER_BYTES: usize = 256;
/// Largest clock lead a frame may carry over the receiver's clock.
pub const MAX_FUTURE_SKEW_MS: u64 = 60_000;
/// Nonces remembered per (verse, author) for duplicate detection.
pub const REPLAY_WINDOW_CAPACITY: usize = 256;
/// Most (verse, author) windows one guard tracks before evicting the least recently touched.
pub const MAX_TRACKED_AUTHORS: usize = 4_096;
/// Environment variable selecting [`SigningMode`].
pub const GOSSIP_SIGNING_ENV_VAR: &str = "FE_SYNC_GOSSIP_SIGNING";
/// Nonce length in bytes.
pub const NONCE_BYTES: usize = 16;

const KEY_VERSION: &str = "v";
const KEY_KIND: &str = "kind";
const KEY_VERSE_ID: &str = "verse_id";
const KEY_AUTHOR_DID: &str = "author_did";
const KEY_HLC_MS: &str = "hlc_ms";
const KEY_NONCE: &str = "nonce";
const KEY_PAYLOAD: &str = "payload";
const KEY_SIGNATURE: &str = "sig";
const FRAME_FIELD_COUNT: usize = 8;

/// Named reasons a frame is refused (same style as `distributed_query::drop_reason`).
pub mod refusal {
    /// Frame exceeds [`super::MAX_FRAME_BYTES`]; checked before decoding.
    pub const FRAME_TOO_LARGE: &str = "gossip frame exceeds the size cap";
    /// Bytes do not start with the signed-frame magic.
    pub const NOT_A_SIGNED_FRAME: &str = "bytes are not a signed gossip frame";
    /// Body is not canonical CBOR or does not re-encode byte-exact.
    pub const NON_CANONICAL: &str = "signed frame body is not canonical CBOR";
    /// Body is canonical CBOR but not the v1 frame shape.
    pub const MALFORMED_FRAME: &str = "signed frame has the wrong shape";
    /// Frame version other than [`super::FRAME_VERSION`].
    pub const UNSUPPORTED_VERSION: &str = "signed frame version is not supported";
    /// `kind` names no known handler.
    pub const UNKNOWN_KIND: &str = "signed frame kind is unknown";
    /// A text field is not Unicode NFC, so it has no canonical encoding.
    pub const NON_NFC_TEXT: &str = "signed frame text is not Unicode NFC";
    /// Frame verse differs from the verse of the arrival topic.
    pub const VERSE_MISMATCH: &str = "signed frame verse does not match the arrival topic";
    /// `author_did` is not an Ed25519 did:key.
    pub const AUTHOR_NOT_DID_KEY: &str = "signed frame author is not an Ed25519 did:key";
    /// The verifying key does not render to the frame's exact `author_did` string.
    pub const AUTHOR_KEY_MISMATCH: &str = "verifying key does not match the frame author";
    /// Signature does not verify under the author's key.
    pub const BAD_SIGNATURE: &str = "signed frame signature does not verify";
    /// The signing key is not the frame author's key.
    pub const SIGNER_NOT_AUTHOR: &str = "signing key is not the frame author";
    /// Frame clock is further ahead than [`super::MAX_FUTURE_SKEW_MS`].
    pub const FUTURE_SKEW: &str = "signed frame clock is too far in the future";
    /// Frame is older than its kind's maximum age.
    pub const STALE: &str = "signed frame is older than its kind allows";
    /// Frame sits below the author's high-water mark minus the kind's maximum age.
    pub const BELOW_WINDOW_FLOOR: &str = "signed frame is below the author's replay window";
    /// Nonce already seen for this (verse, author).
    pub const REPLAYED: &str = "signed frame was already seen";
    /// Frame is at or below a clock whose nonces were evicted, so a replay cannot be ruled out.
    pub const REPLAY_WINDOW_EVICTED: &str = "signed frame predates the remembered replay window";
    /// Unsigned frame refused because signing is required.
    pub const LEGACY_REFUSED: &str = "unsigned gossip refused: signing is required";
    /// Unsigned frame claims an author already latched to signed frames.
    pub const FORGED_SENDER_LEGACY: &str = "unsigned gossip claims an author that signs its frames";
}

/// What a signed frame carries; the signature covers it, so a frame cannot change handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GossipFrameKind {
    /// Distributed-query request or response.
    Compute,
    /// Batched tileset advertisements.
    TilesetAd,
    /// Reachability dial-back probe.
    ReachProbe,
    /// Reachability dial-back result.
    ReachResult,
}

impl GossipFrameKind {
    /// Wire name of the kind.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::TilesetAd => "tileset_ad",
            Self::ReachProbe => "reach_probe",
            Self::ReachResult => "reach_result",
        }
    }

    /// Parses a wire name.
    pub fn from_wire(raw: &str) -> Option<Self> {
        match raw {
            "compute" => Some(Self::Compute),
            "tileset_ad" => Some(Self::TilesetAd),
            "reach_probe" => Some(Self::ReachProbe),
            "reach_result" => Some(Self::ReachResult),
            _ => None,
        }
    }

    /// Maximum frame age for this kind, in milliseconds.
    pub fn max_age_ms(self) -> u64 {
        match self {
            Self::Compute => crate::distributed_query::MAX_QUERY_TIMEOUT_MS + 30_000,
            Self::TilesetAd => 10 * 60_000,
            Self::ReachProbe | Self::ReachResult => 60_000,
        }
    }
}

/// Whether unsigned (legacy) gossip is still admitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SigningMode {
    /// Legacy frames pass to the F23 rules while their author is not latched.
    #[default]
    Compat,
    /// Every unsigned frame is refused.
    Require,
}

impl SigningMode {
    /// Parses a setting: unset or empty is `Compat`; an unrecognised value fails closed to `Require`.
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self::Compat;
        };
        match value.to_ascii_lowercase().as_str() {
            "compat" => Self::Compat,
            "require" | "required" => Self::Require,
            other => {
                tracing::warn!(
                    value = %other,
                    "{GOSSIP_SIGNING_ENV_VAR} is not compat or require; refusing unsigned gossip"
                );
                Self::Require
            }
        }
    }

    /// Reads [`GOSSIP_SIGNING_ENV_VAR`].
    pub fn from_env() -> Self {
        Self::parse(std::env::var(GOSSIP_SIGNING_ENV_VAR).ok().as_deref())
    }
}

/// One signed gossip frame, v1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedGossipFrame {
    /// Frame version; always [`FRAME_VERSION`] for frames this build writes.
    pub v: u64,
    /// Handler the payload is meant for.
    pub kind: GossipFrameKind,
    /// Verse whose topic the frame belongs to.
    pub verse_id: String,
    /// did:key of the signer.
    pub author_did: String,
    /// Author clock in milliseconds.
    pub hlc_ms: u64,
    /// Per-frame unique value used for duplicate detection.
    pub nonce: [u8; NONCE_BYTES],
    /// Opaque handler payload.
    pub payload: Vec<u8>,
    /// Ed25519 signature over [`SignedGossipFrame::signing_message`].
    pub sig: [u8; 64],
}

impl SignedGossipFrame {
    /// Builds a frame authored and signed by `secret`, with a fresh nonce.
    pub fn signed(
        secret: &iroh::SecretKey,
        kind: GossipFrameKind,
        verse_id: &str,
        hlc_ms: u64,
        payload: Vec<u8>,
    ) -> Result<Self, &'static str> {
        let author_did = did_of(&secret.public()).ok_or(refusal::SIGNER_NOT_AUTHOR)?;
        let mut frame = Self {
            v: FRAME_VERSION,
            kind,
            verse_id: verse_id.to_owned(),
            author_did,
            hlc_ms,
            nonce: fresh_nonce(hlc_ms),
            payload,
            sig: [0u8; 64],
        };
        sign_domain(secret, &mut frame)?;
        Ok(frame)
    }

    /// The exact bytes the signature covers: domain prefix then the canonical body without `sig`.
    pub fn signing_message(&self) -> Result<Vec<u8>, &'static str> {
        let body = encode_value(&self.body_value(false))?;
        let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + body.len());
        message.extend_from_slice(SIGNING_DOMAIN);
        message.extend_from_slice(&body);
        Ok(message)
    }

    /// Encodes the frame as magic followed by canonical CBOR.
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        let body = encode_value(&self.body_value(true))?;
        let mut bytes = Vec::with_capacity(FRAME_MAGIC.len() + body.len());
        bytes.extend_from_slice(&FRAME_MAGIC);
        bytes.extend_from_slice(&body);
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(refusal::FRAME_TOO_LARGE);
        }
        Ok(bytes)
    }

    /// Strictly decodes a frame: size cap first, then canonical decode, shape, and a byte-exact re-encode.
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(refusal::FRAME_TOO_LARGE);
        }
        let Some(body) = bytes.strip_prefix(FRAME_MAGIC.as_slice()) else {
            return Err(refusal::NOT_A_SIGNED_FRAME);
        };
        let value = decode_canonical(body).map_err(|_| refusal::NON_CANONICAL)?;
        let frame = Self::from_value(value)?;
        if encode_value(&frame.body_value(true))? != body {
            return Err(refusal::NON_CANONICAL);
        }
        Ok(frame)
    }

    fn body_value(&self, include_signature: bool) -> CborValue {
        let mut entries = Vec::with_capacity(FRAME_FIELD_COUNT);
        entries.push((text_key(KEY_VERSION), CborValue::Uint(self.v)));
        entries.push((text_key(KEY_KIND), text_key(self.kind.as_wire())));
        entries.push((text_key(KEY_VERSE_ID), CborValue::Text(self.verse_id.clone())));
        entries.push((text_key(KEY_AUTHOR_DID), CborValue::Text(self.author_did.clone())));
        entries.push((text_key(KEY_HLC_MS), CborValue::Uint(self.hlc_ms)));
        entries.push((text_key(KEY_NONCE), CborValue::Bytes(self.nonce.to_vec())));
        entries.push((text_key(KEY_PAYLOAD), CborValue::Bytes(self.payload.clone())));
        if include_signature {
            entries.push((text_key(KEY_SIGNATURE), CborValue::Bytes(self.sig.to_vec())));
        }
        CborValue::Map(entries)
    }

    fn from_value(value: CborValue) -> Result<Self, &'static str> {
        let CborValue::Map(entries) = value else {
            return Err(refusal::MALFORMED_FRAME);
        };
        if entries.len() != FRAME_FIELD_COUNT {
            return Err(refusal::MALFORMED_FRAME);
        }
        let mut version = None;
        let mut kind = None;
        let mut verse_id = None;
        let mut author_did = None;
        let mut hlc_ms = None;
        let mut nonce = None;
        let mut payload = None;
        let mut sig = None;
        for (key, field) in entries {
            let CborValue::Text(name) = key else {
                return Err(refusal::MALFORMED_FRAME);
            };
            match (name.as_str(), field) {
                (KEY_VERSION, CborValue::Uint(raw)) => version = Some(raw),
                (KEY_KIND, CborValue::Text(raw)) => kind = Some(raw),
                (KEY_VERSE_ID, CborValue::Text(raw)) => verse_id = Some(raw),
                (KEY_AUTHOR_DID, CborValue::Text(raw)) => author_did = Some(raw),
                (KEY_HLC_MS, CborValue::Uint(raw)) => hlc_ms = Some(raw),
                (KEY_NONCE, CborValue::Bytes(raw)) => nonce = Some(raw),
                (KEY_PAYLOAD, CborValue::Bytes(raw)) => payload = Some(raw),
                (KEY_SIGNATURE, CborValue::Bytes(raw)) => sig = Some(raw),
                _ => return Err(refusal::MALFORMED_FRAME),
            }
        }
        let (
            Some(version),
            Some(kind),
            Some(verse_id),
            Some(author_did),
            Some(hlc_ms),
            Some(nonce),
            Some(payload),
            Some(sig),
        ) = (version, kind, verse_id, author_did, hlc_ms, nonce, payload, sig)
        else {
            return Err(refusal::MALFORMED_FRAME);
        };
        if version != FRAME_VERSION {
            return Err(refusal::UNSUPPORTED_VERSION);
        }
        let kind = GossipFrameKind::from_wire(&kind).ok_or(refusal::UNKNOWN_KIND)?;
        if !identifier_is_bounded(&verse_id) || !identifier_is_bounded(&author_did) {
            return Err(refusal::MALFORMED_FRAME);
        }
        let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| refusal::MALFORMED_FRAME)?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| refusal::MALFORMED_FRAME)?;
        Ok(Self {
            v: version,
            kind,
            verse_id,
            author_did,
            hlc_ms,
            nonce,
            payload,
            sig,
        })
    }
}

/// Signs `frame` in place; refuses when `secret` is not the key behind `frame.author_did`.
pub fn sign_domain(secret: &iroh::SecretKey, frame: &mut SignedGossipFrame) -> Result<(), &'static str> {
    if did_of(&secret.public()).as_deref() != Some(frame.author_did.as_str()) {
        return Err(refusal::SIGNER_NOT_AUTHOR);
    }
    let message = frame.signing_message()?;
    frame.sig = secret.sign(&message).to_bytes();
    Ok(())
}

/// Verifies `frame` under `author_public` (strict Ed25519), which must render to the exact `author_did`.
pub fn verify_domain(frame: &SignedGossipFrame, author_public: &iroh::PublicKey) -> Result<(), &'static str> {
    // Exact-string binding: an alternate spelling of the same key would get its own replay window.
    if did_of(author_public).as_deref() != Some(frame.author_did.as_str()) {
        return Err(refusal::AUTHOR_KEY_MISMATCH);
    }
    let message = frame.signing_message()?;
    let signature = iroh::Signature::from_bytes(&frame.sig);
    author_public
        .verify(&message, &signature)
        .map_err(|_| refusal::BAD_SIGNATURE)
}

/// Resolves an Ed25519 did:key to the iroh key that verifies its frames.
pub fn author_public_key(author_did: &str) -> Result<iroh::PublicKey, &'static str> {
    let verifying_key = fe_identity::did_key::public_key_from_did_key(author_did)
        .map_err(|_| refusal::AUTHOR_NOT_DID_KEY)?;
    iroh::PublicKey::from_bytes(&verifying_key.to_bytes()).map_err(|_| refusal::AUTHOR_NOT_DID_KEY)
}

/// Verifies `frame` under the key its own `author_did` names.
pub fn verify_author(frame: &SignedGossipFrame) -> Result<(), &'static str> {
    verify_domain(frame, &author_public_key(&frame.author_did)?)
}

/// True when the bytes claim to be a signed frame; such bytes are never re-parsed as legacy.
pub fn is_signed_frame_attempt(bytes: &[u8]) -> bool {
    bytes.starts_with(&FRAME_MAGIC)
}

/// Encodes and signs a frame for broadcast.
pub fn seal(
    secret: &iroh::SecretKey,
    kind: GossipFrameKind,
    verse_id: &str,
    hlc_ms: u64,
    payload: Vec<u8>,
) -> Result<Vec<u8>, &'static str> {
    SignedGossipFrame::signed(secret, kind, verse_id, hlc_ms, payload)?.encode()
}

/// Outcome of [`open_ingress`] for one received gossip message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GossipIngress {
    /// A verified, in-window signed frame; call [`ReplayGuard::record`] once it is admitted.
    Signed(SignedGossipFrame),
    /// An unsigned message admitted to the legacy (F23) path under [`SigningMode::Compat`].
    Legacy,
}

/// Ingress pipeline: size cap, magic tripwire, strict decode, topic verse, signature, replay window.
pub fn open_ingress(
    bytes: &[u8],
    topic_verse_id: &str,
    now_ms: u64,
    mode: SigningMode,
    guard: &ReplayGuard,
) -> Result<GossipIngress, &'static str> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(refusal::FRAME_TOO_LARGE);
    }
    if !is_signed_frame_attempt(bytes) {
        return match mode {
            SigningMode::Compat => Ok(GossipIngress::Legacy),
            SigningMode::Require => Err(refusal::LEGACY_REFUSED),
        };
    }
    // Tripwire: from here every failure is final, never a fall-through to the legacy codec.
    let frame = SignedGossipFrame::decode(bytes)?;
    if frame.verse_id != topic_verse_id {
        return Err(refusal::VERSE_MISMATCH);
    }
    verify_author(&frame)?;
    guard.check(&frame, now_ms)?;
    Ok(GossipIngress::Signed(frame))
}

/// Per-(verse, author) replay window state.
#[derive(Debug, Default)]
struct AuthorWindow {
    high_water_ms: u64,
    evicted_floor_ms: Option<u64>,
    recent: VecDeque<([u8; NONCE_BYTES], u64)>,
    last_touched: u64,
}

/// In-memory replay guard: per-(verse, author) high-water floor, nonce LRU, max age and skew bound.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    verses: HashMap<String, HashMap<String, AuthorWindow>>,
    tracked_authors: usize,
    touch_counter: u64,
}

impl ReplayGuard {
    /// An empty guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only window check for a verified frame at the receiver's `now_ms`.
    pub fn check(&self, frame: &SignedGossipFrame, now_ms: u64) -> Result<(), &'static str> {
        let max_age_ms = frame.kind.max_age_ms();
        if frame.hlc_ms > now_ms.saturating_add(MAX_FUTURE_SKEW_MS) {
            return Err(refusal::FUTURE_SKEW);
        }
        if now_ms.saturating_sub(frame.hlc_ms) > max_age_ms {
            return Err(refusal::STALE);
        }
        let Some(window) = self.window(&frame.verse_id, &frame.author_did) else {
            return Ok(());
        };
        // Window floor, not strict monotone: gossip hops reorder frames.
        if frame.hlc_ms.saturating_add(max_age_ms) < window.high_water_ms {
            return Err(refusal::BELOW_WINDOW_FLOOR);
        }
        if window.recent.iter().any(|(nonce, _)| *nonce == frame.nonce) {
            return Err(refusal::REPLAYED);
        }
        if window.evicted_floor_ms.is_some_and(|floor| frame.hlc_ms <= floor) {
            return Err(refusal::REPLAY_WINDOW_EVICTED);
        }
        Ok(())
    }

    /// Records an admitted frame; call only after signature, membership and role checks pass.
    pub fn record(&mut self, frame: &SignedGossipFrame) {
        self.touch_counter += 1;
        let touched = self.touch_counter;
        if self.window(&frame.verse_id, &frame.author_did).is_none() {
            if self.tracked_authors >= MAX_TRACKED_AUTHORS {
                self.evict_least_recently_touched();
            }
            self.tracked_authors += 1;
        }
        let window = self
            .verses
            .entry(frame.verse_id.clone())
            .or_default()
            .entry(frame.author_did.clone())
            .or_default();
        window.last_touched = touched;
        window.high_water_ms = window.high_water_ms.max(frame.hlc_ms);
        if window.recent.iter().any(|(nonce, _)| *nonce == frame.nonce) {
            return;
        }
        window.recent.push_back((frame.nonce, frame.hlc_ms));
        if window.recent.len() > REPLAY_WINDOW_CAPACITY {
            if let Some((_, evicted_hlc_ms)) = window.recent.pop_front() {
                window.evicted_floor_ms =
                    Some(window.evicted_floor_ms.map_or(evicted_hlc_ms, |floor| floor.max(evicted_hlc_ms)));
            }
        }
    }

    /// [`Self::check`] then [`Self::record`] for callers with no admission step in between.
    pub fn check_and_record(&mut self, frame: &SignedGossipFrame, now_ms: u64) -> Result<(), &'static str> {
        self.check(frame, now_ms)?;
        self.record(frame);
        Ok(())
    }

    /// True once a signed frame from this author was admitted in this verse (the session latch).
    pub fn has_verified_author(&self, verse_id: &str, author_did: &str) -> bool {
        self.window(verse_id, author_did).is_some()
    }

    /// Drops every window of a verse (replica closed).
    pub fn forget_verse(&mut self, verse_id: &str) {
        if let Some(authors) = self.verses.remove(verse_id) {
            self.tracked_authors -= authors.len();
        }
    }

    fn window(&self, verse_id: &str, author_did: &str) -> Option<&AuthorWindow> {
        self.verses.get(verse_id)?.get(author_did)
    }

    fn evict_least_recently_touched(&mut self) {
        let oldest = self
            .verses
            .iter()
            .flat_map(|(verse_id, authors)| {
                authors
                    .iter()
                    .map(move |(author_did, window)| (window.last_touched, verse_id, author_did))
            })
            .min_by_key(|(last_touched, _, _)| *last_touched)
            .map(|(_, verse_id, author_did)| (verse_id.clone(), author_did.clone()));
        let Some((verse_id, author_did)) = oldest else {
            return;
        };
        if let Some(authors) = self.verses.get_mut(&verse_id) {
            if authors.remove(&author_did).is_some() {
                self.tracked_authors -= 1;
            }
            if authors.is_empty() {
                self.verses.remove(&verse_id);
            }
        }
    }
}

fn text_key(name: &str) -> CborValue {
    CborValue::Text(name.to_owned())
}

fn encode_value(value: &CborValue) -> Result<Vec<u8>, &'static str> {
    encode_canonical_checked(value).map_err(|_| refusal::NON_NFC_TEXT)
}

fn identifier_is_bounded(identifier: &str) -> bool {
    !identifier.is_empty() && identifier.len() <= MAX_IDENTIFIER_BYTES
}

fn did_of(public: &iroh::PublicKey) -> Option<String> {
    fe_identity::did_key::did_key_from_public_key_bytes(public.as_bytes())
}

/// Unique per-frame nonce: keyed BLAKE3 over a process counter, keyed by OS randomness drawn once.
fn fresh_nonce(hlc_ms: u64) -> [u8; NONCE_BYTES] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static PROCESS_KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = PROCESS_KEY.get_or_init(|| iroh::SecretKey::generate().to_bytes());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut input = [0u8; 16];
    input[..8].copy_from_slice(&count.to_le_bytes());
    input[8..].copy_from_slice(&hlc_ms.to_le_bytes());
    let digest = blake3::keyed_hash(key, &input);
    let mut nonce = [0u8; NONCE_BYTES];
    nonce.copy_from_slice(&digest.as_bytes()[..NONCE_BYTES]);
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: u64 = 1_800_000_000_000;
    const VERSE: &str = "verse-alpha";

    fn author_key() -> iroh::SecretKey {
        iroh::SecretKey::from_bytes(&[7u8; 32])
    }

    fn other_key() -> iroh::SecretKey {
        iroh::SecretKey::from_bytes(&[9u8; 32])
    }

    fn admit(bytes: &[u8], guard: &mut ReplayGuard, now_ms: u64) -> Result<SignedGossipFrame, &'static str> {
        match open_ingress(bytes, VERSE, now_ms, SigningMode::Compat, guard)? {
            GossipIngress::Signed(frame) => {
                guard.record(&frame);
                Ok(frame)
            }
            GossipIngress::Legacy => panic!("a signed frame fell through to the legacy path"),
        }
    }

    #[test]
    fn signed_frame_round_trips_byte_exact() {
        let secret = author_key();
        let bytes = seal(&secret, GossipFrameKind::Compute, VERSE, NOW_MS, b"{\"request\":1}".to_vec())
            .expect("seal");
        assert!(is_signed_frame_attempt(&bytes));

        let decoded = SignedGossipFrame::decode(&bytes).expect("decode");
        assert_eq!(decoded.kind, GossipFrameKind::Compute);
        assert_eq!(decoded.payload, b"{\"request\":1}");
        assert_eq!(
            decoded.author_did,
            fe_identity::did_key::did_key_from_public_key_bytes(secret.public().as_bytes()).unwrap()
        );
        assert_eq!(decoded.encode().expect("re-encode"), bytes, "decode then encode is byte-exact");
        verify_author(&decoded).expect("signature verifies under the author's did:key");

        let mut trailing = bytes.clone();
        trailing.push(0x00);
        assert_eq!(SignedGossipFrame::decode(&trailing), Err(refusal::NON_CANONICAL));

        // Legacy traffic passes only in compat; magic-prefixed garbage never becomes legacy.
        let guard = ReplayGuard::new();
        let legacy = b"{\"Request\":{}}";
        assert_eq!(open_ingress(legacy, VERSE, NOW_MS, SigningMode::Compat, &guard), Ok(GossipIngress::Legacy));
        assert_eq!(
            open_ingress(legacy, VERSE, NOW_MS, SigningMode::Require, &guard),
            Err(refusal::LEGACY_REFUSED)
        );
        let mut garbage = FRAME_MAGIC.to_vec();
        garbage.extend_from_slice(legacy);
        assert_eq!(
            open_ingress(&garbage, VERSE, NOW_MS, SigningMode::Compat, &guard),
            Err(refusal::NON_CANONICAL)
        );
        assert_eq!(SigningMode::parse(None), SigningMode::Compat);
        assert_eq!(SigningMode::parse(Some("required")), SigningMode::Require);
        assert_eq!(SigningMode::parse(Some("requird")), SigningMode::Require, "typos fail closed");
    }

    #[test]
    fn tampered_payload_or_wrong_verse_is_rejected_with_named_reason() {
        let secret = author_key();
        let signed = SignedGossipFrame::signed(&secret, GossipFrameKind::TilesetAd, VERSE, NOW_MS, b"ads".to_vec())
            .expect("sign");
        let encoded = |frame: &SignedGossipFrame| frame.encode().expect("encode");

        let mut tampered_payload = signed.clone();
        tampered_payload.payload = b"adz".to_vec();
        let mut swapped_kind = signed.clone();
        swapped_kind.kind = GossipFrameKind::Compute;
        let mut wrong_key = signed.clone();
        wrong_key.sig = other_key().sign(&wrong_key.signing_message().unwrap()).to_bytes();
        let mut not_a_did = signed.clone();
        not_a_did.author_did = "did:web:example.org".into();
        let mut future_version = signed.clone();
        future_version.v = 2;
        let mut oversized = FRAME_MAGIC.to_vec();
        oversized.resize(MAX_FRAME_BYTES + 1, 0);

        let cases: Vec<(&str, Vec<u8>, &str, &str)> = vec![
            ("tampered payload", encoded(&tampered_payload), VERSE, refusal::BAD_SIGNATURE),
            ("kind swapped after signing", encoded(&swapped_kind), VERSE, refusal::BAD_SIGNATURE),
            ("signed with another key", encoded(&wrong_key), VERSE, refusal::BAD_SIGNATURE),
            ("arrived on another verse topic", encoded(&signed), "verse-beta", refusal::VERSE_MISMATCH),
            ("author is not a did:key", encoded(&not_a_did), VERSE, refusal::AUTHOR_NOT_DID_KEY),
            ("unknown frame version", encoded(&future_version), VERSE, refusal::UNSUPPORTED_VERSION),
            ("oversized frame", oversized, VERSE, refusal::FRAME_TOO_LARGE),
        ];
        let guard = ReplayGuard::new();
        for (case, bytes, topic, expected) in cases {
            assert_eq!(
                open_ingress(&bytes, topic, NOW_MS, SigningMode::Compat, &guard),
                Err(expected),
                "{case}"
            );
        }
        let mut stolen = signed.clone();
        assert_eq!(sign_domain(&other_key(), &mut stolen), Err(refusal::SIGNER_NOT_AUTHOR));
        assert!(matches!(
            open_ingress(&encoded(&signed), VERSE, NOW_MS, SigningMode::Require, &guard),
            Ok(GossipIngress::Signed(_))
        ));
    }

    #[test]
    fn replayed_or_stale_frame_is_rejected_and_fresh_one_admitted() {
        let secret = author_key();
        let mut guard = ReplayGuard::new();
        let seal_at = |kind, hlc_ms| seal(&secret, kind, VERSE, hlc_ms, b"p".to_vec()).expect("seal");

        let first = seal_at(GossipFrameKind::TilesetAd, NOW_MS);
        let first_frame = admit(&first, &mut guard, NOW_MS).expect("fresh frame admitted");
        assert!(guard.has_verified_author(VERSE, &first_frame.author_did), "author latches");
        assert_eq!(admit(&first, &mut guard, NOW_MS + 1), Err(refusal::REPLAYED));

        let stale = seal_at(GossipFrameKind::TilesetAd, NOW_MS - 11 * 60_000);
        assert_eq!(admit(&stale, &mut guard, NOW_MS), Err(refusal::STALE));
        let future = seal_at(GossipFrameKind::Compute, NOW_MS + 2 * 60_000);
        assert_eq!(admit(&future, &mut guard, NOW_MS), Err(refusal::FUTURE_SKEW));

        // A frame within skew raises the high-water mark; an old compute frame now falls below the floor.
        admit(&seal_at(GossipFrameKind::Compute, NOW_MS + 50_000), &mut guard, NOW_MS).expect("skewed admitted");
        let below_floor = seal_at(GossipFrameKind::Compute, NOW_MS - 5_000);
        assert_eq!(admit(&below_floor, &mut guard, NOW_MS), Err(refusal::BELOW_WINDOW_FLOOR));
        admit(&seal_at(GossipFrameKind::TilesetAd, NOW_MS + 1), &mut guard, NOW_MS).expect("fresh admitted");

        // Another author's flood cannot evict this author's nonces.
        let flooder = other_key();
        for offset in 0..(REPLAY_WINDOW_CAPACITY as u64 + 10) {
            let bytes = seal(&flooder, GossipFrameKind::TilesetAd, VERSE, NOW_MS + offset, b"f".to_vec()).unwrap();
            admit(&bytes, &mut guard, NOW_MS).expect("flood frame admitted");
        }
        assert_eq!(admit(&first, &mut guard, NOW_MS), Err(refusal::REPLAYED));

        // Overflowing this author's own window refuses the evicted frame rather than reopening it.
        for offset in 0..(REPLAY_WINDOW_CAPACITY as u64) {
            let hlc_ms = NOW_MS + 50_001 + offset;
            admit(&seal_at(GossipFrameKind::TilesetAd, hlc_ms), &mut guard, NOW_MS).unwrap();
        }
        assert_eq!(admit(&first, &mut guard, NOW_MS), Err(refusal::REPLAY_WINDOW_EVICTED));

        guard.forget_verse(VERSE);
        assert!(!guard.has_verified_author(VERSE, &first_frame.author_did));
    }
}
