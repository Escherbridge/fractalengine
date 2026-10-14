//! The one production `CandidateVerifier` instantiation over durable authority sources, and
//! the durable admission door. Rationale: `canon_log/AGENTS.md` §production-verifier.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use fe_canonical_log::author_key::admission::RegisteredSchemas;
use fe_canonical_log::author_key::continuity_grant::ContinuityGrantPayload;
use fe_canonical_log::author_key::disavow::DisavowPayload;
use fe_canonical_log::author_key::disavow_rescind::RescindPayload;
use fe_canonical_log::author_key::payloads::{RotationPayload, ROTATION_OPERATION_KIND};
use fe_canonical_log::compose::{
    ComposedCandidateVerifier, ComposedVerifierParts, LifecycleDispatch, PayloadSchemaRegistry,
    SchemaVerdict, SystemClock,
};
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::errors::AdmissionOutcome;
use fe_identity::SecretStore;

use crate::canon_log::admission::{
    admit_and_append, AdmissionRejection, AdmittedAppend, CheapPrecondition,
};
use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::artifact_store::SurrealArtifactStore;
use crate::canon_log::author_lifecycle::{
    DurableAuthorKeyLifecycle, LifecycleError, LifecycleReplayReport,
};
use crate::canon_log::canonical_epoch::DurableAuthorizationView;
use crate::canon_log::chain_store::ChainStore;
use crate::canon_log::scope_key_custody::SecretStoreScopeKeyCustody;
use crate::canon_log::StorageError;
use crate::repo::Db;

/// Provisional domain of the `author-key-rotation-v1` schema hash (CDL-PROV register).
pub const ROTATION_SCHEMA_DOMAIN: &[u8] = b"fe-schema-author-key-rotation-v1\0";
/// Provisional domain of the `manager-suspect-key-disavow-v1` schema hash.
pub const DISAVOW_SCHEMA_DOMAIN: &[u8] = b"fe-schema-manager-suspect-key-disavow-v1\0";
/// Provisional domain of the `manager-suspect-key-disavow-rescind-v1` schema hash.
pub const DISAVOW_RESCIND_SCHEMA_DOMAIN: &[u8] =
    b"fe-schema-manager-suspect-key-disavow-rescind-v1\0";
/// Provisional domain of the `owner-continuity-grant-v1` schema hash.
pub const CONTINUITY_GRANT_SCHEMA_DOMAIN: &[u8] = b"fe-schema-owner-continuity-grant-v1\0";

/// The four SPEC-2 lifecycle schema hashes this build registers: `BLAKE3(domain)`, the
/// SPEC-10 §4.1 rule 3 convention (`BLAKE3(ASCII(name) || 00)`). Provisional (CDL-PROV).
pub fn lifecycle_schemas() -> RegisteredSchemas {
    RegisteredSchemas {
        rotation: Hash32::of(ROTATION_SCHEMA_DOMAIN),
        disavow: Hash32::of(DISAVOW_SCHEMA_DOMAIN),
        disavow_rescind: Hash32::of(DISAVOW_RESCIND_SCHEMA_DOMAIN),
        continuity_grant: Hash32::of(CONTINUITY_GRANT_SCHEMA_DOMAIN),
    }
}

/// Decides whether a decrypted plaintext satisfies one registered schema.
pub type SchemaValidator = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// Why a schema registration was refused.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaRegistrationError {
    /// Kind 0 is not an operation kind (SPEC-1 §6).
    #[error("operation kind {0} is not a registrable kind")]
    InvalidKind(u16),
    /// This `(kind, schema)` pair already has a validator; a second one is never substituted.
    #[error("schema {schema_hash:?} is already registered for kind {operation_kind}")]
    AlreadyRegistered {
        /// The kind.
        operation_kind: u16,
        /// The schema.
        schema_hash: Hash32,
    },
}

/// Exactly the schemas this build interprets; everything else is `UnknownSchema` or, for an
/// application kind with no schema at all, `UnknownKind` (SPEC-1 §6 rule 7, SPEC-4 §5).
///
/// Registration is a call, not an edit of [`durable_candidate_verifier`]'s body (critic C3):
/// CDL-SPEC10 B3 adds kind 5 through [`Self::register`], including on a verifier already built.
pub struct RegisteredSchemaSet {
    entries: RwLock<HashMap<(u16, Hash32), SchemaValidator>>,
}

impl std::fmt::Debug for RegisteredSchemaSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<(u16, Hash32)> = self.read_entries().keys().copied().collect();
        keys.sort();
        formatter
            .debug_struct("RegisteredSchemaSet")
            .field("registered", &keys)
            .finish()
    }
}

impl RegisteredSchemaSet {
    /// Interprets nothing.
    pub fn empty() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// The four SPEC-2 lifecycle schemas (kind 1), each validated by its own canonical decoder.
    pub fn with_lifecycle_schemas() -> Self {
        let set = Self::empty();
        let schemas = lifecycle_schemas();
        let lifecycle: [(Hash32, SchemaValidator); 4] = [
            (
                schemas.rotation,
                Arc::new(|bytes| RotationPayload::decode_canonical(bytes).is_ok()),
            ),
            (
                schemas.disavow,
                Arc::new(|bytes| DisavowPayload::decode_canonical(bytes).is_ok()),
            ),
            (
                schemas.disavow_rescind,
                Arc::new(|bytes| RescindPayload::decode_canonical(bytes).is_ok()),
            ),
            (
                schemas.continuity_grant,
                Arc::new(|bytes| ContinuityGrantPayload::decode_canonical(bytes).is_ok()),
            ),
        ];
        for (schema_hash, validator) in lifecycle {
            set.register_validator(ROTATION_OPERATION_KIND, schema_hash, validator)
                .expect("the four lifecycle schema hashes are distinct");
        }
        set
    }

    /// Registers `validator` for `(operation_kind, schema_hash)`; refuses kind 0 and a repeat.
    pub fn register(
        &self,
        operation_kind: u16,
        schema_hash: Hash32,
        validator: impl Fn(&[u8]) -> bool + Send + Sync + 'static,
    ) -> Result<(), SchemaRegistrationError> {
        self.register_validator(operation_kind, schema_hash, Arc::new(validator))
    }

    fn register_validator(
        &self,
        operation_kind: u16,
        schema_hash: Hash32,
        validator: SchemaValidator,
    ) -> Result<(), SchemaRegistrationError> {
        if OperationKind::from_u16(operation_kind).is_err() {
            return Err(SchemaRegistrationError::InvalidKind(operation_kind));
        }
        let mut entries = self.entries.write().expect("schema registry lock");
        if entries.contains_key(&(operation_kind, schema_hash)) {
            return Err(SchemaRegistrationError::AlreadyRegistered {
                operation_kind,
                schema_hash,
            });
        }
        entries.insert((operation_kind, schema_hash), validator);
        Ok(())
    }

    fn read_entries(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, HashMap<(u16, Hash32), SchemaValidator>> {
        self.entries.read().expect("schema registry lock")
    }
}

impl PayloadSchemaRegistry for RegisteredSchemaSet {
    fn interprets_kind(&self, operation_kind: u16) -> bool {
        self.read_entries()
            .keys()
            .any(|(kind, _)| *kind == operation_kind)
    }

    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict {
        let validator = {
            let entries = self.read_entries();
            let application = OperationKind::from_u16(operation_kind)
                .is_ok_and(OperationKind::requires_registered_interpretation);
            if application && !entries.keys().any(|(kind, _)| *kind == operation_kind) {
                return SchemaVerdict::UnknownKind;
            }
            match entries.get(&(operation_kind, *schema_hash)) {
                Some(validator) => Arc::clone(validator),
                None => return SchemaVerdict::UnknownSchema,
            }
        };
        if validator(plaintext) {
            SchemaVerdict::Valid
        } else {
            SchemaVerdict::Invalid
        }
    }
}

/// The production composed verifier: every seam a durable fe-database source.
pub type DurableCandidateVerifier = ComposedCandidateVerifier<
    ChainStore,
    DurableAuthorizationView,
    DurableAuthorKeyLifecycle,
    SecretStoreScopeKeyCustody,
    SurrealArtifactStore,
    RegisteredSchemaSet,
    SystemClock,
>;

/// Every reason the durable verifier could not be built.
#[derive(Debug, thiserror::Error)]
pub enum DurableVerifierError {
    /// A durable source could not be read.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The lifecycle could not be rebuilt from the verified log.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
}

/// The durable verifier and what its startup lifecycle rebuild found.
pub struct LoadedVerifier {
    /// The verifier `admit_and_append` callers use.
    pub verifier: DurableCandidateVerifier,
    /// What rebuilding the SPEC-2 lifecycle from `verified_op_log` did.
    pub lifecycle_replay: LifecycleReplayReport,
}

impl std::fmt::Debug for LoadedVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedVerifier")
            .field("lifecycle_replay", &self.lifecycle_replay)
            .finish_non_exhaustive()
    }
}

/// THE production instantiation: loads every durable source, composes them, then rebuilds the
/// SPEC-2 lifecycle from the verified log through the verifier's own payload half.
///
/// A snapshot, like `DurableAuthorizationView`: rebuild after an admitted epoch bump or a role
/// change. Interprets the four lifecycle schemas; register more on
/// `verifier.parts().schemas`.
pub async fn durable_candidate_verifier(
    db: &Db,
    secret_store: Arc<dyn SecretStore>,
    interim_authority_anchor: Option<Hash32>,
) -> Result<LoadedVerifier, DurableVerifierError> {
    let chains = ChainStore::load(db).await?;
    let authority = DurableAuthorizationView::load_with_chain_store(
        db,
        interim_authority_anchor,
        chains.clone(),
    )
    .await?;
    let lifecycle_roles = DurableAuthorizationView::load(db, interim_authority_anchor).await?;
    let verifier = ComposedCandidateVerifier::new(ComposedVerifierParts {
        chains,
        authority,
        lifecycle: DurableAuthorKeyLifecycle::new(lifecycle_schemas(), lifecycle_roles),
        scope_keys: SecretStoreScopeKeyCustody::load(db, secret_store).await?,
        ciphertexts: SurrealArtifactStore::load(db).await?,
        schemas: RegisteredSchemaSet::with_lifecycle_schemas(),
        clock: SystemClock,
    });
    let store = SurrealVerifiedLogStore::new(db.clone());
    let lifecycle_replay = verifier
        .parts()
        .lifecycle
        .replay_verified_log(&verifier, &store)
        .await?;
    Ok(LoadedVerifier {
        verifier,
        lifecycle_replay,
    })
}

/// One durable admission: the append, and what the lifecycle did with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableAdmission {
    /// The admitted, appended operation.
    pub append: AdmittedAppend,
    /// The lifecycle's answer: `Ok(NotLifecycle)` for an ordinary operation, the SPEC-2
    /// entrypoint's answer for a lifecycle one, `Err` when it could not be re-opened.
    pub lifecycle: Result<LifecycleDispatch, AdmissionOutcome>,
}

/// Every reason [`admit_and_append_durably`] did not finish.
#[derive(Debug, thiserror::Error)]
pub enum DurableAdmissionError {
    /// Admission or the append refused; nothing was appended.
    #[error(transparent)]
    Admission(#[from] AdmissionRejection),
    /// The operation IS appended but its ciphertext was not retained, so a later replay could
    /// not re-open it. Retry the same call: the append answers `AlreadyPresent`.
    #[error("operation appended but its payload ciphertext was not retained: {0}")]
    CiphertextNotRetained(StorageError),
    /// The operation IS appended but the lifecycle could not observe it; retry the same call.
    #[error("operation appended but the lifecycle did not observe it: {0}")]
    LifecycleNotObserved(LifecycleError),
}

/// The production admission door over [`DurableCandidateVerifier`].
///
/// `admit_and_append` (SPEC-4 §2 order, precondition before append), then the payload
/// ciphertext retained in the local artifact store, then the lifecycle told — so the next
/// admission already sees a rotation this one recorded. Calling `admit_and_append` directly
/// with this verifier skips the last two steps and leaves the lifecycle stale until restart.
pub async fn admit_and_append_durably<C>(
    verifier: &DurableCandidateVerifier,
    store: &SurrealVerifiedLogStore,
    precondition: &C,
    candidate_bytes: &[u8],
    payload_ciphertext: Option<&[u8]>,
) -> Result<DurableAdmission, DurableAdmissionError>
where
    C: CheapPrecondition + ?Sized,
{
    let append = admit_and_append(
        verifier,
        store,
        store,
        precondition,
        store,
        candidate_bytes,
        payload_ciphertext,
    )
    .await?;
    if let Some(ciphertext) = payload_ciphertext {
        verifier
            .parts()
            .ciphertexts
            .put_payload_ciphertext(ciphertext)
            .await
            .map_err(DurableAdmissionError::CiphertextNotRetained)?;
    }
    let lifecycle = verifier
        .parts()
        .lifecycle
        .observe_appended(verifier, store, append.meta.op_id, payload_ciphertext)
        .await
        .map_err(DurableAdmissionError::LifecycleNotObserved)?;
    Ok(DurableAdmission { append, lifecycle })
}
