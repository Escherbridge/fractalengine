//! Shared fixtures and injected seams for the W4-5 cross-spec integration tests
//! (`tests/cross_spec_integration.rs`).
//!
//! The crate's own `compose::fixtures` module shows the same shapes (a self-issued root chain,
//! a `ManagerAuthorityView` test double, in-memory seams) but is `pub(crate)` and so unreachable
//! from an integration-test crate; this module re-derives the small set this suite needs,
//! built entirely from `fe_canonical_log`'s public surface (`src/AGENTS.md` §crate-surface).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;

use fe_canonical_log::author_key::lineage::KeyState;
use fe_canonical_log::capability::ObjectClassSet;
use fe_canonical_log::capability::{
    AuthorityState, AuthorizationView, Caveats, Chain, ManagerAuthorityView, ObjectClass,
    Principal, UnsignedCertificate, Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::compose::{
    AuthorKeyLifecycleView, CapabilityChainSource, Clock, ComposedCandidateVerifier,
    ComposedVerifierParts, ContentByteStore, PayloadCiphertextSource, PayloadSchemaRegistry,
    SchemaVerdict,
};
use fe_canonical_log::crypto::aead::SealingKey;
use fe_canonical_log::crypto::scope_key::{InMemoryScopeKeyStore, ScopeKeyStore};
use fe_canonical_log::envelope::{
    Author, CapabilityRef, EquivocationKey, Hash32, Hlc, Identifier32, PayloadRef, Scope,
    UnsignedEnvelope, PROTOCOL_VERSION,
};
use fe_canonical_log::materialize::errors::{AppendError, AppendOutcome};
use fe_canonical_log::materialize::traits::{
    EquivocationIndex, ParentVerseLookup, VerifiedEnvelopeMeta, VerifiedLogStore,
};
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::segment::hashseq::LaneKey;
use fe_canonical_log::segment::relay_policy::{PeerIdentity, RelayAuthorizationView};
use fe_canonical_log::signing::sign_envelope;
use fe_canonical_log::wire::cursor::{BranchRegistry, CursorOrder, DurableCursor, ReplayOutcome};
use fe_canonical_log::wire::error::WireError;

/// The one verse every test in this suite shares.
pub const VERSE: Identifier32 = Identifier32([0x31; 32]);
/// A petal under [`VERSE`].
pub const PETAL_A: Identifier32 = Identifier32([0x32; 32]);
/// A SIBLING petal under the same verse, disjoint from [`PETAL_A`].
pub const PETAL_B: Identifier32 = Identifier32([0x33; 32]);
/// The scope epoch every minted chain and scope key certifies, unless a test bumps it.
pub const EPOCH: u64 = 1;
/// Start of every minted certificate's validity window.
pub const NOT_BEFORE_MS: u64 = 1_700_000_000_000;
/// Length of every minted certificate's validity window.
pub const ONE_HOUR_MS: u64 = 3_600_000;
/// A wall clock reading inside every certificate's window.
pub const NOW_MS: u64 = NOT_BEFORE_MS + 1_000;
/// A schema hash fixture operations carry when payload schema validation is not the point.
pub const SCHEMA: Hash32 = Hash32([0x90; 32]);
/// The authority record every self-issued chain in this suite certifies under.
const AUTHORITY_ID: Hash32 = Hash32([0xa9; 32]);

/// A `Hash32` of one repeated filler byte, for readable fixture identifiers.
pub fn hash(filler: u8) -> Hash32 {
    Hash32([filler; 32])
}

/// The verse-wide scope every test starts from.
pub fn verse_scope() -> Scope {
    Scope::verse_wide(VERSE)
}

/// A petal-scoped scope under [`VERSE`].
pub fn petal_scope(petal: Identifier32) -> Scope {
    Scope::new(VERSE, Some(petal), None).expect("petal scope")
}

/// An Ed25519 signing key from a raw 32-byte seed.
pub fn signing_key(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes(&seed)
}

/// The raw Ed25519 public key for `key`.
pub fn public_key(key: &SigningKey) -> [u8; 32] {
    key.verifying_key().to_bytes()
}

/// The SPEC-3 principal (DID + public key) for `key`.
pub fn principal_of(key: &SigningKey) -> Principal {
    Principal::from_public_key(public_key(key))
}

/// A self-issued, one-certificate capability chain: `author` grants itself `verbs` x `classes`
/// over `grant_scope` at `epoch_scope`/`scope_epoch`.
///
/// `tests/fixtures/capability-chain-v1.json`'s own root certificate is issued by a Manager
/// principal straight to an audience; here the author plays both roles. That models a genesis
/// author with no recorded lineage root (`KeyState::Unknown`, "authority rests on the chain",
/// SPEC-2 §4 rule 2) rather than reusing the fixture's frozen chain bytes directly: that chain's
/// own leaf audience key has no seed recorded in the fixture (only its public key), so nothing
/// in this crate could sign as it, and its `chain_id` is independent of any operation this suite
/// can construct. Minting fresh keeps every admission in this file end-to-end real (a real
/// Ed25519-signed certificate, a real SPEC-3 chain walk) rather than asserting against a
/// chain this process cannot actually exercise.
pub fn self_issued_chain(
    author: &SigningKey,
    verbs: &[Verb],
    classes: &[ObjectClass],
    grant_scope: Scope,
    epoch_scope: Scope,
    scope_epoch: u64,
) -> (Vec<u8>, Hash32) {
    let principal = principal_of(author);
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal.clone(),
        audience: principal,
        parent_certificate_id: None,
        issuer_authority_id: AUTHORITY_ID,
        grant_scope,
        epoch_scope,
        scope_epoch,
        verbs: VerbSet::from_verbs(verbs.iter().copied()).expect("verbs"),
        object_classes: ObjectClassSet::from_classes(classes.iter().copied()).expect("classes"),
        not_before_ms: NOT_BEFORE_MS,
        not_after_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
        delegation_depth: 0,
        caveats: Caveats::default(),
    }
    .sign(author)
    .expect("sign root certificate");
    let bytes = Chain::from_certificates(&[certificate])
        .expect("chain")
        .encode_canonical()
        .expect("encode chain");
    let chain_id = Hash32::of(&bytes);
    (bytes, chain_id)
}

/// The persistent SPEC-3 authorization view: `issuer` is Manager+ at `epoch_scope`, nobody else
/// is, and the view's current epoch is whatever the case sets (or unrecorded).
#[derive(Clone, Debug)]
pub struct ManagerView {
    issuer: Principal,
    epoch_scope: Scope,
    current_epoch: Option<u64>,
}

impl ManagerView {
    /// A view recognizing `issuer` as Manager+ at `epoch_scope`, currently at `epoch`.
    pub fn at_epoch(issuer: Principal, epoch_scope: Scope, epoch: u64) -> Self {
        Self {
            issuer,
            epoch_scope,
            current_epoch: Some(epoch),
        }
    }

    /// A view that records no epoch for `epoch_scope` at all (SPEC-2 rule 5 evidence gap).
    pub fn without_epoch_state(issuer: Principal, epoch_scope: Scope) -> Self {
        Self {
            issuer,
            epoch_scope,
            current_epoch: None,
        }
    }
}

impl AuthorizationView for ManagerView {
    fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
        if *epoch_scope == self.epoch_scope {
            self.current_epoch
        } else {
            None
        }
    }

    fn version(&self) -> u64 {
        1
    }
}

impl ManagerAuthorityView for ManagerView {
    fn authority_is_manager_plus(
        &self,
        authority_id: &Hash32,
        issuer: &Principal,
        epoch_scope: &Scope,
        _scope_epoch: u64,
    ) -> AuthorityState {
        if *epoch_scope != self.epoch_scope {
            return AuthorityState::Unknown;
        }
        if *authority_id == AUTHORITY_ID && *issuer == self.issuer {
            AuthorityState::ManagerPlus
        } else {
            AuthorityState::NotManagerPlus
        }
    }

    fn principal_is_manager_plus(
        &self,
        principal_asked: &Principal,
        _epoch_scope: &Scope,
        _scope_epoch: u64,
    ) -> AuthorityState {
        if *principal_asked == self.issuer {
            AuthorityState::ManagerPlus
        } else {
            AuthorityState::NotManagerPlus
        }
    }
}

/// A clock stopped at one instant.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

/// No lineage evidence reaches any key: every author key is [`KeyState::Unknown`] (SPEC-2 §4
/// rule 2, "authority rests on the chain").
#[derive(Clone, Copy, Debug)]
pub struct UnknownLifecycle;

impl AuthorKeyLifecycleView for UnknownLifecycle {
    fn key_state(&self, _author: &[u8; 32], _scope: &Scope, _parents: &[Hash32]) -> KeyState {
        KeyState::Unknown
    }
}

/// No ciphertext is stored locally; only a caller-supplied one can be opened.
#[derive(Clone, Copy, Debug)]
pub struct NoCiphertexts;

impl PayloadCiphertextSource for NoCiphertexts {
    fn ciphertext(&self, _ciphertext_hash: &Hash32) -> Option<Vec<u8>> {
        None
    }
}

/// Capability chain artifacts held locally, by `chain_id = BLAKE3(chain bytes)`.
#[derive(Clone, Debug, Default)]
pub struct ChainMap(BTreeMap<Hash32, Vec<u8>>);

impl ChainMap {
    /// A shelf holding exactly `chains`, keyed by each one's own BLAKE3.
    pub fn holding(chains: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self(
            chains
                .into_iter()
                .map(|bytes| (Hash32::of(&bytes), bytes))
                .collect(),
        )
    }
}

impl CapabilityChainSource for ChainMap {
    fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>> {
        self.0.get(chain_id).cloned()
    }
}

/// Accepts any plaintext that decodes as canonical CBOR, regardless of `schema_hash` or kind.
/// Schema-identity conformance is covered elsewhere (`fe-canonical-log`'s own unit tests); this
/// suite only needs "the payload opened and validated", not "against which exact schema".
#[derive(Clone, Copy, Debug)]
pub struct AnyCanonicalSchema;

impl PayloadSchemaRegistry for AnyCanonicalSchema {
    fn interprets_kind(&self, _operation_kind: u16) -> bool {
        true
    }

    fn validate(
        &self,
        _schema_hash: &Hash32,
        _operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict {
        match fe_canonical_log::cbor::decode_canonical(plaintext) {
            Ok(_) => SchemaVerdict::Valid,
            Err(_) => SchemaVerdict::Invalid,
        }
    }
}

/// No operation has been admitted at any equivocation key.
#[derive(Clone, Copy, Debug)]
pub struct NoEquivocation;

#[async_trait]
impl EquivocationIndex for NoEquivocation {
    async fn op_id_at(&self, _key: EquivocationKey) -> Option<Hash32> {
        None
    }
}

/// Every parent is locally admitted in [`VERSE`].
#[derive(Clone, Copy, Debug)]
pub struct SameVerseParents;

#[async_trait]
impl ParentVerseLookup for SameVerseParents {
    async fn verse_id_of(&self, _parent_op_id: Hash32) -> Option<Identifier32> {
        Some(VERSE)
    }
}

/// An in-memory [`VerifiedLogStore`]: exactly-once append, keyed by claimed `op_id`.
#[derive(Default)]
pub struct MemoryLogStore(Mutex<BTreeMap<Hash32, Vec<u8>>>);

#[async_trait]
impl VerifiedLogStore for MemoryLogStore {
    async fn append(
        &self,
        claimed_op_id: Hash32,
        bytes: Vec<u8>,
    ) -> Result<AppendOutcome, AppendError> {
        let mut rows = self.0.lock().expect("lock");
        if let Some(existing) = rows.get(&claimed_op_id) {
            if *existing == bytes {
                return Ok(AppendOutcome::AlreadyPresent);
            }
            return Err(AppendError::IntegrityConflict {
                op_id: claimed_op_id,
            });
        }
        rows.insert(claimed_op_id, bytes);
        Ok(AppendOutcome::Appended)
    }

    async fn get_bytes(&self, op_id: Hash32) -> Option<Vec<u8>> {
        self.0.lock().expect("lock").get(&op_id).cloned()
    }

    async fn get_meta(&self, _op_id: Hash32) -> Option<VerifiedEnvelopeMeta> {
        None
    }

    async fn parents_of(&self, _op_id: Hash32) -> Option<Vec<Hash32>> {
        None
    }
}

/// Raw content-addressed byte storage for [`fe_canonical_log::compose::ByteStoreSealedArtifactStore`].
#[derive(Default)]
pub struct MemoryByteStore(BTreeMap<Hash32, Vec<u8>>);

impl ContentByteStore for MemoryByteStore {
    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.0.get(&artifact_id).cloned()
    }

    fn has(&self, artifact_id: Hash32) -> bool {
        self.0.contains_key(&artifact_id)
    }

    fn put(&mut self, artifact_id: Hash32, bytes: Vec<u8>) {
        self.0.insert(artifact_id, bytes);
    }
}

/// The scope-key custody a composed verifier or sealed-proof environment reads from, holding
/// exactly one key at `(scope, epoch)`.
pub fn scope_key_store(scope: Scope, epoch: u64, key_bytes: [u8; 32]) -> InMemoryScopeKeyStore {
    let mut store = InMemoryScopeKeyStore::new();
    store
        .insert_scope_key(
            ScopeKeyId { scope, epoch },
            SealingKey::from_bytes(key_bytes),
        )
        .expect("insert scope key");
    store
}

/// The concrete [`ComposedCandidateVerifier`] every test in this suite composes.
pub type TestVerifier = ComposedCandidateVerifier<
    ChainMap,
    ManagerView,
    UnknownLifecycle,
    InMemoryScopeKeyStore,
    NoCiphertexts,
    AnyCanonicalSchema,
    FixedClock,
>;

/// Composes [`TestVerifier`] from one held chain, an authorization view, and scope-key custody.
pub fn build_verifier(
    chain_bytes: Vec<u8>,
    authority: ManagerView,
    scope_keys: InMemoryScopeKeyStore,
) -> TestVerifier {
    ComposedCandidateVerifier::new(ComposedVerifierParts {
        chains: ChainMap::holding([chain_bytes]),
        authority,
        lifecycle: UnknownLifecycle,
        scope_keys,
        ciphertexts: NoCiphertexts,
        schemas: AnyCanonicalSchema,
        clock: FixedClock(NOW_MS),
    })
}

/// Builds one unsigned SPEC-1 envelope from its semantic fields; every field a test does not
/// care about is still named explicitly so nothing hides behind a default.
#[allow(clippy::too_many_arguments)]
pub fn build_unsigned(
    author: &SigningKey,
    operation_kind: u16,
    scope: Scope,
    chain_id: Hash32,
    scope_epoch: u64,
    schema_hash: Hash32,
    branch_id: Identifier32,
    parents: Vec<Hash32>,
    hlc: Hlc,
    payload: PayloadRef,
) -> UnsignedEnvelope {
    UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind,
        scope,
        author: Author::from_public_key(public_key(author)),
        capability: CapabilityRef {
            chain_id,
            scope_epoch,
        },
        schema_hash,
        branch_id,
        parents,
        hlc,
        payload,
    }
}

/// Signs `unsigned` with `author`, returning the complete SPEC-1 envelope.
pub fn sign(
    author: &SigningKey,
    unsigned: &UnsignedEnvelope,
) -> fe_canonical_log::envelope::CompleteEnvelope {
    sign_envelope(author, unsigned).expect("sign envelope")
}

/// The persistent view [`fe_canonical_log::segment::artifact::seal_artifact`] consults: reports
/// exactly the `(epoch, key_id)` a test declares as current for one lane, so sealing proceeds
/// under what the test claims is the authorized current key. Seed/fetch/wrap capability are
/// irrelevant to sealing and always answer false; nothing in this suite calls them.
#[derive(Clone, Debug, Default)]
pub struct SealingView(BTreeMap<LaneKey, (u64, Identifier32)>);

impl SealingView {
    /// A view declaring `(epoch, key_id)` current for exactly `lane`.
    pub fn declaring(lane: LaneKey, epoch: u64, key_id: Identifier32) -> Self {
        let mut view = Self::default();
        view.0.insert(lane, (epoch, key_id));
        view
    }
}

impl RelayAuthorizationView for SealingView {
    fn current_scope_epoch(&self, lane: &LaneKey) -> Option<u64> {
        self.0.get(lane).map(|(epoch, _)| *epoch)
    }

    fn current_key_id(&self, lane: &LaneKey) -> Option<Identifier32> {
        self.0.get(lane).map(|(_, key_id)| *key_id)
    }

    fn has_seed_capability(
        &self,
        _peer: &PeerIdentity,
        _lane: &LaneKey,
        _scope_epoch: u64,
    ) -> bool {
        false
    }

    fn has_fetch_capability(
        &self,
        _peer: &PeerIdentity,
        _lane: &LaneKey,
        _scope_epoch: u64,
    ) -> bool {
        false
    }

    fn may_wrap_scope_key_for_peer(
        &self,
        _peer: &PeerIdentity,
        _lane: &LaneKey,
        _scope_epoch: u64,
    ) -> bool {
        false
    }
}

/// The only route to a [`DurableCursor`] outside the crate: [`BranchRegistry::issue_cursor`] is
/// a provided method whose body is the sole caller of the private constructor. Every other
/// method is unreachable from this suite, which only ever issues a cursor to build a
/// `CommittedDelta` fixture -- it never drives replay.
pub struct StubBranchRegistry;

#[async_trait]
impl BranchRegistry for StubBranchRegistry {
    async fn validate(&self, _cursor: &DurableCursor) -> Result<(), WireError> {
        unimplemented!("not exercised by this suite")
    }

    async fn compare(
        &self,
        _left: &DurableCursor,
        _right: &DurableCursor,
    ) -> Result<CursorOrder, WireError> {
        unimplemented!("not exercised by this suite")
    }

    async fn replay_after(&self, _cursor: &DurableCursor) -> Result<ReplayOutcome, WireError> {
        unimplemented!("not exercised by this suite")
    }

    async fn snapshot_cursor(&self, _scope: &Scope) -> Result<DurableCursor, WireError> {
        unimplemented!("not exercised by this suite")
    }
}
