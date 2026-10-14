//! Test fixtures shared by the composed-verifier flow tests: a real Manager-rooted capability
//! chain, a real scope key, and real signed operations sealed under it.

use std::cell::Cell;
use std::collections::HashMap;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;

use crate::author_key::lineage::KeyState;
use crate::author_key::test_support::schemas as lifecycle_schemas;
use crate::capability::{
    AuthorityState, AuthorizationView, Caveats, Chain, ManagerAuthorityView, ObjectClass,
    ObjectClassSet, Principal, UnsignedCertificate, Verb, VerbSet, CAPABILITY_VERSION,
};
use crate::cbor::{decode_canonical, encode_canonical, CborValue};
use crate::crypto::aead::{seal, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
use crate::crypto::key_id::derive_key_id;
use crate::crypto::scope_key::{InMemoryScopeKeyStore, ScopeKeyStore};
use crate::envelope::{
    Author, CapabilityRef, EquivocationKey, Hash32, Hlc, Identifier32, PayloadRef, Scope,
    UnsignedEnvelope, PROTOCOL_VERSION,
};
use crate::kind::OperationKind;
use crate::materialize::traits::{EquivocationIndex, ParentVerseLookup};
use crate::payload_aad::payload_aad_preimage;
use crate::retention::crypto_shred::ScopeKeyId;
use crate::signing::sign_envelope;

use super::verifier::{
    AuthorKeyLifecycleView, CapabilityChainSource, Clock, ComposedCandidateVerifier,
    ComposedVerifierParts, PayloadCiphertextSource, PayloadSchemaRegistry, SchemaVerdict,
};

/// The one key the fixture view treats as a current Manager+ root issuer.
pub(crate) const MANAGER_SEED: u8 = 0x01;
/// The operation author every chain is issued to unless a case says otherwise.
pub(crate) const AUTHOR_SEED: u8 = 0x03;
/// The key a rotation by [`AUTHOR_SEED`] activates.
pub(crate) const SUCCESSOR_SEED: u8 = 0x04;
/// An authenticated principal no chain is issued to.
pub(crate) const STRANGER_SEED: u8 = 0x05;
/// The scope epoch every chain certifies.
pub(crate) const EPOCH: u64 = 4;
/// Start of every certificate's validity window.
pub(crate) const NOT_BEFORE_MS: u64 = 1_700_000_000_000;
/// Length of every certificate's validity window.
pub(crate) const ONE_HOUR_MS: u64 = 3_600_000;
/// The fixed wall clock, inside every window.
pub(crate) const NOW_MS: u64 = NOT_BEFORE_MS + 1_000;
/// The schema ordinary kind-1 fixture operations carry.
pub(crate) const SCHEMA: Hash32 = Hash32([0x50; 32]);
/// The branch every fixture operation belongs to.
pub(crate) const BRANCH: Identifier32 = Identifier32([0x55; 32]);
/// The authority record the fixture view resolves the Manager root issuer under.
const AUTHORITY_ID: Hash32 = Hash32([0xa0; 32]);

pub(crate) fn hash(filler: u8) -> Hash32 {
    Hash32([filler; 32])
}

pub(crate) fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub(crate) fn public_key(seed: u8) -> [u8; 32] {
    signing_key(seed).verifying_key().to_bytes()
}

pub(crate) fn principal(seed: u8) -> Principal {
    Principal::from_public_key(public_key(seed))
}

pub(crate) fn verse_scope() -> Scope {
    Scope::verse_wide(Identifier32([0x11; 32]))
}

pub(crate) fn petal_scope() -> Scope {
    Scope::new(
        Identifier32([0x11; 32]),
        Some(Identifier32([0x12; 32])),
        None,
    )
    .expect("petal scope")
}

/// A one-certificate chain: the Manager issues `verbs` x `classes` over the whole verse straight
/// to `audience_seed`, at [`EPOCH`], with no caveats. Returns its exact canonical bytes.
pub(crate) fn root_chain(audience_seed: u8, verbs: &[Verb], classes: &[ObjectClass]) -> Vec<u8> {
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER_SEED),
        audience: principal(audience_seed),
        parent_certificate_id: None,
        issuer_authority_id: AUTHORITY_ID,
        grant_scope: verse_scope(),
        epoch_scope: verse_scope(),
        scope_epoch: EPOCH,
        verbs: VerbSet::from_verbs(verbs.iter().copied()).expect("verbs"),
        object_classes: ObjectClassSet::from_classes(classes.iter().copied()).expect("classes"),
        not_before_ms: NOT_BEFORE_MS,
        not_after_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
        delegation_depth: 0,
        caveats: Caveats::default(),
    }
    .sign(&signing_key(MANAGER_SEED))
    .expect("sign root");
    Chain::from_certificates(&[certificate])
        .expect("chain")
        .encode_canonical()
        .expect("encode chain")
}

/// The persistent authorization view: the Manager is Manager+ at verse scope, nobody else is,
/// and the verse's current epoch is whatever the case sets (or unrecorded).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ManagerView {
    current_epoch: Option<u64>,
}

impl ManagerView {
    pub(crate) fn at_epoch(epoch: u64) -> Self {
        Self {
            current_epoch: Some(epoch),
        }
    }

    pub(crate) fn without_epoch_state() -> Self {
        Self {
            current_epoch: None,
        }
    }
}

impl AuthorizationView for ManagerView {
    fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
        if *epoch_scope == verse_scope() {
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
        if *epoch_scope != verse_scope() {
            return AuthorityState::Unknown;
        }
        if *authority_id == AUTHORITY_ID && *issuer == principal(MANAGER_SEED) {
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
        if *principal_asked == principal(MANAGER_SEED) {
            AuthorityState::ManagerPlus
        } else {
            AuthorityState::NotManagerPlus
        }
    }
}

/// A clock stopped at one instant.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FixedClock(pub(crate) u64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

/// Chain artifacts held locally, by `chain_id = BLAKE3(chain bytes)`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ChainShelf(HashMap<Hash32, Vec<u8>>);

impl ChainShelf {
    pub(crate) fn holding(chains: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self(
            chains
                .into_iter()
                .map(|bytes| (Hash32::of(&bytes), bytes))
                .collect(),
        )
    }
}

impl CapabilityChainSource for ChainShelf {
    fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>> {
        self.0.get(chain_id).cloned()
    }
}

/// No ciphertext is stored locally; only a caller-supplied one can be opened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoCiphertexts;

impl PayloadCiphertextSource for NoCiphertexts {
    fn ciphertext(&self, _ciphertext_hash: &Hash32) -> Option<Vec<u8>> {
        None
    }
}

/// No lineage evidence reaches any key.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoLifecycle;

impl AuthorKeyLifecycleView for NoLifecycle {
    fn key_state(&self, _author: &[u8; 32], _scope: &Scope, _parents: &[Hash32]) -> KeyState {
        KeyState::Unknown
    }
}

/// Registered `(kind, schema)` pairs; a registered plaintext is valid when it is canonical CBOR.
#[derive(Clone, Debug)]
pub(crate) struct SchemaTable {
    application_kinds: Vec<u16>,
    schemas: Vec<(u16, Hash32)>,
}

impl SchemaTable {
    /// Kind 1 under [`SCHEMA`] and the SPEC-2 rotation schema; no application kind.
    pub(crate) fn kind_one_only() -> Self {
        Self {
            application_kinds: Vec::new(),
            schemas: vec![(1, SCHEMA), (1, lifecycle_schemas().rotation)],
        }
    }
}

impl PayloadSchemaRegistry for SchemaTable {
    fn interprets_kind(&self, operation_kind: u16) -> bool {
        self.application_kinds.contains(&operation_kind)
    }

    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict {
        if matches!(
            OperationKind::from_u16(operation_kind),
            Ok(OperationKind::Application(_))
        ) && !self.interprets_kind(operation_kind)
        {
            return SchemaVerdict::UnknownKind;
        }
        if !self.schemas.contains(&(operation_kind, *schema_hash)) {
            return SchemaVerdict::UnknownSchema;
        }
        match decode_canonical(plaintext) {
            Ok(_) => SchemaVerdict::Valid,
            Err(_) => SchemaVerdict::Invalid,
        }
    }
}

/// No operation has been admitted at any equivocation key.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoEquivocation;

#[async_trait]
impl EquivocationIndex for NoEquivocation {
    async fn op_id_at(&self, _key: EquivocationKey) -> Option<Hash32> {
        None
    }
}

/// Every parent is locally admitted in the fixture verse.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SameVerseParents;

#[async_trait]
impl ParentVerseLookup for SameVerseParents {
    async fn verse_id_of(&self, _parent_op_id: Hash32) -> Option<Identifier32> {
        Some(verse_scope().verse_id())
    }
}

/// One signed operation, the ciphertext it commits to, and what it was built from.
#[derive(Clone, Debug)]
pub(crate) struct SealedOperation {
    pub(crate) bytes: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) op_id: Hash32,
    pub(crate) plaintext_fields: CborValue,
    pub(crate) scope: Scope,
}

/// The verifier every fixture builds, generic over its lifecycle view.
pub(crate) type FixtureVerifier<L> = ComposedCandidateVerifier<
    ChainShelf,
    ManagerView,
    L,
    InMemoryScopeKeyStore,
    NoCiphertexts,
    SchemaTable,
    FixedClock,
>;

/// A verse with one scope key at `(verse, epoch)` and one author chain, sealing operations under
/// both.
#[derive(Debug)]
pub(crate) struct ComposedFixture {
    scope_epoch: u64,
    key_bytes: [u8; SEALING_KEY_LENGTH],
    author_chain: Vec<u8>,
    next_counter: Cell<u32>,
}

impl ComposedFixture {
    pub(crate) fn new(scope_epoch: u64) -> Self {
        Self {
            scope_epoch,
            key_bytes: [0x5a; SEALING_KEY_LENGTH],
            author_chain: root_chain(AUTHOR_SEED, &[Verb::Append], &[ObjectClass::Operation]),
            next_counter: Cell::new(0),
        }
    }

    fn scope_key(&self) -> SealingKey {
        SealingKey::from_bytes(self.key_bytes)
    }

    /// Custody holding exactly the fixture's verse key.
    pub(crate) fn scope_keys(&self) -> InMemoryScopeKeyStore {
        let mut store = InMemoryScopeKeyStore::new();
        store
            .insert_scope_key(
                ScopeKeyId {
                    scope: verse_scope(),
                    epoch: self.scope_epoch,
                },
                self.scope_key(),
            )
            .expect("custody");
        store
    }

    pub(crate) fn no_equivocation(&self) -> NoEquivocation {
        NoEquivocation
    }

    pub(crate) fn same_verse_parents(&self) -> SameVerseParents {
        SameVerseParents
    }

    /// A verse-scoped operation of `kind` under [`SCHEMA`].
    pub(crate) fn operation(
        &self,
        author_seed: u8,
        parents: Vec<Hash32>,
        kind: u16,
    ) -> SealedOperation {
        self.operation_in(author_seed, verse_scope(), parents, kind)
    }

    /// An operation of `kind` under [`SCHEMA`] in `scope`.
    pub(crate) fn operation_in(
        &self,
        author_seed: u8,
        scope: Scope,
        parents: Vec<Hash32>,
        kind: u16,
    ) -> SealedOperation {
        let fields = CborValue::Map(vec![(CborValue::Uint(0), CborValue::Uint(7))]);
        self.sealed(
            author_seed,
            scope,
            parents,
            kind,
            SCHEMA,
            &encode_canonical(&fields),
        )
    }

    /// A signed operation whose payload is `plaintext` sealed under the verse key with the §5.2
    /// AAD, naming the author chain and a fresh HLC counter.
    pub(crate) fn sealed(
        &self,
        author_seed: u8,
        scope: Scope,
        parents: Vec<Hash32>,
        kind: u16,
        schema_hash: Hash32,
        plaintext: &[u8],
    ) -> SealedOperation {
        let key = self.scope_key();
        let key_id = derive_key_id(&verse_scope(), self.scope_epoch, &key).expect("key id");
        let nonce = FreshNonce::draw(key_id).expect("nonce");
        let counter = self.next_counter.get();
        self.next_counter.set(counter + 1);
        let mut unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: kind,
            scope,
            author: Author::from_public_key(public_key(author_seed)),
            capability: CapabilityRef {
                chain_id: Hash32::of(&self.author_chain),
                scope_epoch: self.scope_epoch,
            },
            schema_hash,
            branch_id: BRANCH,
            parents,
            hlc: Hlc::new(NOW_MS, counter),
            payload: PayloadRef {
                encryption: Some(nonce.params()),
                ..PayloadRef::empty()
            },
        };
        let aad = payload_aad_preimage(&unsigned).expect("aad");
        let sealed = seal(&key, nonce, &aad, plaintext).expect("seal");
        unsigned.payload = sealed.payload_ref();
        let bytes = sign_envelope(&signing_key(author_seed), &unsigned)
            .expect("sign")
            .encode_canonical()
            .expect("encode");
        SealedOperation {
            op_id: Hash32::of(&bytes),
            bytes,
            ciphertext: sealed.ciphertext,
            plaintext_fields: decode_canonical(plaintext).expect("canonical plaintext"),
            scope,
        }
    }

    /// The verifier with the author chain held, the view at `view_epoch`, and the verse key.
    pub(crate) fn verifier<L: AuthorKeyLifecycleView>(
        &self,
        view_epoch: u64,
        lifecycle: L,
    ) -> FixtureVerifier<L> {
        self.verifier_with(
            ManagerView::at_epoch(view_epoch),
            lifecycle,
            self.scope_keys(),
        )
    }

    /// [`Self::verifier`] over a caller-chosen custody store.
    pub(crate) fn verifier_with_keys<L: AuthorKeyLifecycleView>(
        &self,
        view_epoch: u64,
        lifecycle: L,
        scope_keys: InMemoryScopeKeyStore,
    ) -> FixtureVerifier<L> {
        self.verifier_with(ManagerView::at_epoch(view_epoch), lifecycle, scope_keys)
    }

    /// [`Self::verifier`] over a caller-chosen authorization view.
    pub(crate) fn verifier_with_view<L: AuthorKeyLifecycleView>(
        &self,
        view: ManagerView,
        lifecycle: L,
    ) -> FixtureVerifier<L> {
        self.verifier_with(view, lifecycle, self.scope_keys())
    }

    fn verifier_with<L: AuthorKeyLifecycleView>(
        &self,
        view: ManagerView,
        lifecycle: L,
        scope_keys: InMemoryScopeKeyStore,
    ) -> FixtureVerifier<L> {
        ComposedCandidateVerifier::new(ComposedVerifierParts {
            chains: ChainShelf::holding([self.author_chain.clone()]),
            authority: view,
            lifecycle,
            scope_keys,
            ciphertexts: NoCiphertexts,
            schemas: SchemaTable::kind_one_only(),
            clock: FixedClock(NOW_MS),
        })
    }
}
