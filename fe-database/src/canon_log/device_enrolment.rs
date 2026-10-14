//! Durable SPEC-3 §10.2.1 device-enrolment registry (owner decision R2): the
//! `canon_device_enrolment` table, the local enrol/revoke doors, the replicated-row admission and
//! apply, and the §10.2.1 delivery view. Rationale: `canon_log/AGENTS.md` §device-enrolment.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    verify_chain_bytes, AuthorizationRequest, AuthorizationSnapshot, ChainError, ObjectClass,
    Principal, Verb, VerificationOptions,
};
use fe_canonical_log::compose::{Clock, SystemClock};
use fe_canonical_log::crypto::{
    AuthorizationRecord, CryptoError, DeliveryAuthorizationView, DeviceEnrolmentRevocationV1,
    DeviceEnrolmentV1, DeviceEnrolmentView, DevicePublicKey, EnrolmentRegistry,
    IssuerAuthorityView, VerifiedEnrolmentArtifact, DEVICE_PUBLIC_KEY_LENGTH,
};
use fe_canonical_log::envelope::{CapabilityRef, Hash32, Scope};
use fe_runtime::messages::{DbResult, ReplicatedRowOutcome};

use crate::canon_log::canonical_epoch::DurableAuthorizationView;
use crate::canon_log::chain_store::ChainStore;
use crate::canon_log::{op_id_to_hex, row_exists, StorageError};
use crate::handlers::correlated_failure;
use crate::repo::Db;
use crate::{replicate_row, BlobStoreHandle, ReplicationSender};

/// The replicated table: one row per `enrolment_record_id(principal, device)`.
pub const DEVICE_ENROLMENT_TABLE: &str = "canon_device_enrolment";

/// `SecretStore` account (service `fractalengine`) holding this node's X25519 device secret.
pub const LOCAL_DEVICE_KEY_ACCOUNT: &str = "device_keypair";

/// One stored and replicated row. `artifact_cbor` is the only authority; every other column is
/// an index [`EnrolmentRow::verify`] re-derives from it and refuses on any disagreement.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrolmentRow {
    /// Lowercase hex of `enrolment_record_id(principal, device_key)`; also the wire key.
    pub record_id: String,
    /// The principal that signed the artifact.
    pub principal_did: String,
    /// Lowercase hex of the X25519 device key.
    pub device_key_hex: String,
    /// Standard base64 of the exact canonical artifact bytes.
    pub artifact_cbor: String,
    /// Whether the artifact is the signed tombstone.
    pub revoked: bool,
}

impl EnrolmentRow {
    /// The row for a verified artifact; the only way a row is ever built for storage.
    pub fn of(artifact: &VerifiedEnrolmentArtifact) -> Self {
        Self {
            record_id: op_id_to_hex(artifact.record_id()),
            principal_did: artifact.principal().did.clone(),
            device_key_hex: hex::encode(artifact.device_key().0),
            artifact_cbor: BASE64.encode(artifact.canonical_bytes()),
            revoked: artifact.is_revocation(),
        }
    }

    /// Verifies `artifact_cbor` through the W4-6 door, then refuses a row whose index columns
    /// are not exactly the ones that artifact derives (the `revoked` flag included).
    pub fn verify(&self) -> Result<VerifiedEnrolmentArtifact, String> {
        let bytes = BASE64
            .decode(&self.artifact_cbor)
            .map_err(|error| format!("artifact_cbor is not base64: {error}"))?;
        let artifact = VerifiedEnrolmentArtifact::decode_and_verify(&bytes)
            .map_err(|error| format!("artifact does not verify: {error}"))?;
        if Self::of(&artifact) != *self {
            return Err("index columns disagree with the verified artifact".to_string());
        }
        Ok(artifact)
    }
}

/// What one write did to its record slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrolmentWrite {
    /// The slot now holds this artifact.
    Stored,
    /// The slot already held it, or a same-class artifact that wins convergence.
    Unchanged,
    /// An enrolment met a held revocation, which dominates it forever.
    DominatedByRevocation,
}

/// The single write door: converges `artifact` into its slot under `EnrolmentRegistry`'s total
/// order and returns what the slot holds afterwards. Only verified artifacts have a spelling.
///
/// A held row that no longer verifies is a storage fault, never overwritten: replacing an
/// unreadable revocation with a fresh enrolment would un-revoke a device.
pub async fn store_verified_artifact(
    db: &Db,
    artifact: &VerifiedEnrolmentArtifact,
) -> Result<(EnrolmentWrite, VerifiedEnrolmentArtifact), StorageError> {
    let record_id = op_id_to_hex(artifact.record_id());
    let held = held_artifact(db, &record_id).await?;
    let mut slot = EnrolmentRegistry::new();
    if let Some(held) = &held {
        slot.record(held.clone());
    }
    if !slot.record(artifact.clone()) {
        let Some(held) = held else {
            unreachable!("an empty slot always records");
        };
        let write = if held.is_revocation() && !artifact.is_revocation() {
            EnrolmentWrite::DominatedByRevocation
        } else {
            EnrolmentWrite::Unchanged
        };
        return Ok((write, held));
    }

    let row = serde_json::to_value(EnrolmentRow::of(artifact))
        .map_err(|error| StorageError::malformed("device enrolment write", error))?;
    let statement = if held.is_some() {
        "UPDATE canon_device_enrolment MERGE $row WHERE record_id = $rid"
    } else {
        "CREATE canon_device_enrolment CONTENT $row"
    };
    db.query(statement)
        .bind(("row", row))
        .bind(("rid", record_id))
        .await
        .map_err(|error| StorageError::query("device enrolment write", error))?
        .check()
        .map_err(|error| StorageError::query("device enrolment write", error))?;
    Ok((EnrolmentWrite::Stored, artifact.clone()))
}

/// The verified artifact held under `record_id`, if any.
async fn held_artifact(
    db: &Db,
    record_id: &str,
) -> Result<Option<VerifiedEnrolmentArtifact>, StorageError> {
    let mut response = db
        .query(
            "SELECT record_id, principal_did, device_key_hex, artifact_cbor, revoked \
             FROM canon_device_enrolment WHERE record_id = $rid LIMIT 1",
        )
        .bind(("rid", record_id.to_string()))
        .await
        .map_err(|error| StorageError::query("device enrolment read", error))?
        .check()
        .map_err(|error| StorageError::query("device enrolment read", error))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|error| StorageError::query("device enrolment read", error))?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let artifact = serde_json::from_value::<EnrolmentRow>(row)
        .map_err(|error| error.to_string())
        .and_then(|row| row.verify())
        .map_err(|detail| StorageError::malformed("device enrolment read", detail))?;
    Ok(Some(artifact))
}

// ---------------------------------------------------------------------------
// The durable DeviceEnrolmentView and the §10.2.1 delivery view
// ---------------------------------------------------------------------------

/// The production `DeviceEnrolmentView`: the table re-verified into W4-6's `EnrolmentRegistry`.
///
/// A startup snapshot (the seam is synchronous): reload after an enrolment write. A row that
/// does not verify is not held, so its slot answers `Unknown`, which refuses.
#[derive(Clone, Debug, Default)]
pub struct DurableDeviceEnrolmentView {
    registry: EnrolmentRegistry,
    load_faults: Vec<String>,
}

impl DurableDeviceEnrolmentView {
    /// Reads and re-verifies every `canon_device_enrolment` row.
    pub async fn load(db: &Db) -> Result<Self, StorageError> {
        let mut response = db
            .query(
                "SELECT record_id, principal_did, device_key_hex, artifact_cbor, revoked \
                 FROM canon_device_enrolment",
            )
            .await
            .map_err(|error| StorageError::query("device enrolment load", error))?
            .check()
            .map_err(|error| StorageError::query("device enrolment load", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("device enrolment load", error))?;

        let mut view = Self::default();
        for row in rows {
            match serde_json::from_value::<EnrolmentRow>(row)
                .map_err(|error| error.to_string())
                .and_then(|row| row.verify())
            {
                Ok(artifact) => {
                    view.registry.record(artifact);
                }
                Err(fault) => {
                    tracing::warn!("canon_device_enrolment row not held: {fault}");
                    view.load_faults.push(fault);
                }
            }
        }
        Ok(view)
    }

    /// The registry the table loaded into.
    pub fn registry(&self) -> &EnrolmentRegistry {
        &self.registry
    }

    /// Rows [`Self::load`] refused to hold.
    pub fn load_faults(&self) -> &[String] {
        &self.load_faults
    }
}

impl DeviceEnrolmentView for DurableDeviceEnrolmentView {
    fn device_enrolment(
        &self,
        recipient: &Principal,
        device_key: DevicePublicKey,
    ) -> AuthorizationRecord {
        self.registry.device_enrolment(recipient, device_key)
    }
}

/// The production `DeliveryAuthorizationView`: issuer Manager+ from `DurableAuthorizationView`,
/// device binding from [`DurableDeviceEnrolmentView`], recipient `decrypt` from held chains.
pub struct DurableDeliveryView {
    authority: DurableAuthorizationView,
    enrolments: DurableDeviceEnrolmentView,
    chains: ChainStore,
}

impl DurableDeliveryView {
    /// Composes three already-loaded sources.
    pub fn new(
        authority: DurableAuthorizationView,
        enrolments: DurableDeviceEnrolmentView,
        chains: ChainStore,
    ) -> Self {
        Self {
            authority,
            enrolments,
            chains,
        }
    }

    /// Loads all three sources from durable state; same anchor rule as
    /// `DurableAuthorizationView::load`.
    pub async fn load(
        db: &Db,
        interim_authority_anchor: Option<Hash32>,
    ) -> Result<Self, StorageError> {
        let chains = ChainStore::load(db).await?;
        let authority = DurableAuthorizationView::load_with_chain_store(
            db,
            interim_authority_anchor,
            chains.clone(),
        )
        .await?;
        let enrolments = DurableDeviceEnrolmentView::load(db).await?;
        Ok(Self::new(authority, enrolments, chains))
    }

    /// The device-binding half.
    pub fn enrolments(&self) -> &DurableDeviceEnrolmentView {
        &self.enrolments
    }
}

impl IssuerAuthorityView for DurableDeliveryView {
    fn issuer_manager_authority(
        &self,
        issuer: &Principal,
        capability: &CapabilityRef,
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationRecord {
        self.authority
            .issuer_manager_authority(issuer, capability, scope, scope_epoch)
    }
}

impl DeviceEnrolmentView for DurableDeliveryView {
    fn device_enrolment(
        &self,
        recipient: &Principal,
        device_key: DevicePublicKey,
    ) -> AuthorizationRecord {
        self.enrolments.device_enrolment(recipient, device_key)
    }
}

impl DeliveryAuthorizationView for DurableDeliveryView {
    /// Granted when a held chain whose leaf audience is `recipient` verifies SPEC-3 §2.4 for
    /// `decrypt/op` at exactly `scope` and `scope_epoch`, the recipient as requester, under the
    /// live policy. No held chain, or only chains whose epoch scope the view cannot resolve, is
    /// `Unknown`; held chains that all fail are `Refused`.
    fn recipient_decrypt_authority(
        &self,
        recipient: &Principal,
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationRecord {
        let candidates = self.chains.chains_for_leaf_audience(&recipient.did);
        let request = AuthorizationRequest {
            verb: Verb::Decrypt,
            object_class: ObjectClass::Operation,
            target_scope: *scope,
            resource_id: scope.resource_id(),
            requester: recipient.clone(),
            operation_author: recipient.clone(),
            operation_scope_epoch: scope_epoch,
            operation_kind: None,
            is_structural_operation: false,
            payload_bytes: None,
            segment_bytes: None,
            schema_hash: None,
            preview_hz: None,
            now_ms: SystemClock.now_ms(),
        };
        let mut answer = AuthorizationRecord::Unknown;
        for (chain_id, chain_bytes) in candidates {
            match verify_chain_bytes(
                &chain_bytes,
                &request,
                &self.authority,
                &AuthorizationSnapshot {
                    epoch_causally_valid_for_operation: false,
                },
                &VerificationOptions::live(),
            ) {
                Ok(verified) if verified.chain_id == chain_id => {
                    return AuthorizationRecord::Granted
                }
                Err(ChainError::EpochScopeUnknown) => {}
                Ok(_) | Err(_) => answer = AuthorizationRecord::Refused,
            }
        }
        answer
    }
}

// ---------------------------------------------------------------------------
// Inbound replicated rows (called from handlers::replicated_row)
// ---------------------------------------------------------------------------

/// Verifies one inbound row; `None` for the empty `Doc::del` marker, which carries no signature
/// and so can never revoke anything.
fn verify_replicated_row(
    record_id: &str,
    row_bytes: &[u8],
) -> Result<Option<VerifiedEnrolmentArtifact>, String> {
    if row_bytes.is_empty() {
        return Ok(None);
    }
    let row: EnrolmentRow = serde_json::from_slice(row_bytes)
        .map_err(|error| format!("not a device enrolment row: {error}"))?;
    let artifact = row.verify()?;
    if row.record_id != record_id {
        return Err("the wire key is not the artifact's record id".to_string());
    }
    Ok(Some(artifact))
}

/// The inbound admission branch: the artifact verifies and its principal IS the entry author.
/// Role-independent by decision (a Viewer must be able to enrol to receive keys).
pub(crate) fn admit_replicated_enrolment(
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> Result<(), String> {
    match verify_replicated_row(record_id, row_bytes)? {
        None => Ok(()),
        Some(artifact) if artifact.principal().did == author_did => Ok(()),
        Some(_) => Err("device enrolment row not authored by its own principal".to_string()),
    }
}

/// The inbound apply arm. Re-verifies (a row is never stored from unverified bytes, whatever
/// called this), then converges through [`store_verified_artifact`].
pub(crate) async fn apply_replicated_enrolment(
    db: &Db,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<ReplicatedRowOutcome> {
    let artifact = match verify_replicated_row(record_id, row_bytes) {
        Ok(Some(artifact)) => artifact,
        Ok(None) => return Ok(ReplicatedRowOutcome::NotApplicable),
        Err(reason) => {
            tracing::warn!(record_id, %reason, "device enrolment row refused at apply");
            return Ok(ReplicatedRowOutcome::Denied);
        }
    };
    let (write, _) = store_verified_artifact(db, &artifact).await?;
    Ok(match write {
        EnrolmentWrite::Stored if artifact.is_revocation() => {
            ReplicatedRowOutcome::AppliedTombstone
        }
        EnrolmentWrite::Stored | EnrolmentWrite::Unchanged => ReplicatedRowOutcome::Applied,
        EnrolmentWrite::DominatedByRevocation => ReplicatedRowOutcome::SkippedTombstoned,
    })
}

// ---------------------------------------------------------------------------
// Local enrol / revoke (DbCommand::EnrolLocalDevice / RevokeLocalDevice)
// ---------------------------------------------------------------------------

/// Every reason a local enrol or revoke is refused.
#[derive(Debug, thiserror::Error)]
pub enum LocalEnrolmentError {
    /// No node keypair: the principal cannot sign, and a placeholder signature is never made.
    #[error("no node keypair: a device enrolment must be signed by its principal")]
    MissingNodeKeypair,
    /// No secret store: the local device key has nowhere to live.
    #[error("no secret store for the local device key")]
    MissingSecretStore,
    /// The verse to publish into is not known locally.
    #[error("verse {0} is not known locally")]
    UnknownVerse(String),
    /// The device secret could not be loaded or generated.
    #[error("local device key: {0}")]
    DeviceKey(String),
    /// `device_key_hex` is not 32 bytes of hex.
    #[error("device_key_hex is not a 32-byte hex X25519 key: {0}")]
    InvalidDeviceKey(String),
    /// The local device key carries a revocation, which dominates forever.
    #[error(
        "device key {device_key_hex} is revoked and revocation dominates forever; \
         RevokeLocalDevice it to rotate to a fresh key"
    )]
    DeviceKeyRevoked {
        /// The revoked key.
        device_key_hex: String,
    },
    /// The artifact could not be signed or did not verify.
    #[error(transparent)]
    Artifact(#[from] CryptoError),
    /// The substrate failed; the slot is unchanged.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// What a local enrol or revoke left in its slot.
#[derive(Clone, Debug)]
pub struct LocalEnrolment {
    /// What the write did.
    pub write: EnrolmentWrite,
    /// The row the slot holds afterwards; what gets published.
    pub held: EnrolmentRow,
}

/// The node identity as the principal and its signing key; the seed copy is wiped best-effort.
fn principal_signer(keypair: &fe_identity::NodeKeypair) -> (Principal, SigningKey) {
    let mut seed = keypair.seed_bytes();
    let signing_key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    std::hint::black_box(&seed);
    (
        Principal::from_public_key(keypair.verifying_key().to_bytes()),
        signing_key,
    )
}

/// Signs this node's device key into an enrolment and stores it through the verification door.
pub async fn enrol_local_device(
    db: &Db,
    keypair: Option<&fe_identity::NodeKeypair>,
    secret_store: Option<&Arc<dyn fe_identity::SecretStore>>,
    enrolled_at_ms: u64,
) -> Result<LocalEnrolment, LocalEnrolmentError> {
    let keypair = keypair.ok_or(LocalEnrolmentError::MissingNodeKeypair)?;
    let secret_store = secret_store.ok_or(LocalEnrolmentError::MissingSecretStore)?;
    let device =
        fe_identity::load_or_generate_device_keypair(secret_store, LOCAL_DEVICE_KEY_ACCOUNT)
            .map_err(|error| LocalEnrolmentError::DeviceKey(error.to_string()))?;
    let (principal, signing_key) = principal_signer(keypair);
    let bytes = DeviceEnrolmentV1 {
        principal,
        device_key: DevicePublicKey(device.public_key()),
        enrolled_at_ms,
    }
    .sign(&signing_key)?
    .encode_canonical()?;
    let artifact = VerifiedEnrolmentArtifact::decode_and_verify(&bytes)?;
    let (write, held) = store_verified_artifact(db, &artifact).await?;
    if write == EnrolmentWrite::DominatedByRevocation {
        return Err(LocalEnrolmentError::DeviceKeyRevoked {
            device_key_hex: hex::encode(device.public_key()),
        });
    }
    Ok(LocalEnrolment {
        write,
        held: EnrolmentRow::of(&held),
    })
}

/// Signs a revocation of one of this principal's devices and stores it. When it revokes this
/// node's own current device key, that secret is deleted afterwards so the next enrolment mints
/// a fresh key (a revoked key can never be re-enrolled).
pub async fn revoke_local_device(
    db: &Db,
    keypair: Option<&fe_identity::NodeKeypair>,
    secret_store: Option<&Arc<dyn fe_identity::SecretStore>>,
    device_key_hex: &str,
    revoked_at_ms: u64,
) -> Result<LocalEnrolment, LocalEnrolmentError> {
    let keypair = keypair.ok_or(LocalEnrolmentError::MissingNodeKeypair)?;
    let device_key = parse_device_key(device_key_hex)?;
    let (principal, signing_key) = principal_signer(keypair);
    let bytes = DeviceEnrolmentRevocationV1 {
        principal,
        device_key,
        revoked_at_ms,
    }
    .sign(&signing_key)?
    .encode_canonical()?;
    let artifact = VerifiedEnrolmentArtifact::decode_and_verify(&bytes)?;
    let (write, held) = store_verified_artifact(db, &artifact).await?;

    // Revocation first, deletion second: a crash in between leaves a revoked secret that the
    // next enrolment refuses (fail closed), never a deleted key with no revocation.
    if let Some(store) = secret_store {
        if let Ok(local) = fe_identity::load_device_keypair(store, LOCAL_DEVICE_KEY_ACCOUNT) {
            if local.public_key() == device_key.0 {
                if let Err(error) =
                    fe_identity::delete_device_keypair(store, LOCAL_DEVICE_KEY_ACCOUNT)
                {
                    tracing::warn!(
                        "revoked local device key could not be deleted ({error}); \
                         the next enrolment will refuse until it is"
                    );
                }
            }
        }
    }
    Ok(LocalEnrolment {
        write,
        held: EnrolmentRow::of(&held),
    })
}

fn parse_device_key(device_key_hex: &str) -> Result<DevicePublicKey, LocalEnrolmentError> {
    let bytes = hex::decode(device_key_hex)
        .map_err(|error| LocalEnrolmentError::InvalidDeviceKey(error.to_string()))?;
    let key: [u8; DEVICE_PUBLIC_KEY_LENGTH] = bytes
        .try_into()
        .map_err(|_| LocalEnrolmentError::InvalidDeviceKey("wrong length".to_string()))?;
    Ok(DevicePublicKey(key))
}

/// Publishes the held row into `verse_id`'s replica; no new transport (owner R2).
fn publish(
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    held: &EnrolmentRow,
) -> Result<(), String> {
    let row_json = serde_json::to_vec(held).map_err(|error| error.to_string())?;
    replicate_row(
        repl_tx,
        blob_store,
        verse_id,
        DEVICE_ENROLMENT_TABLE,
        &held.record_id,
        &row_json,
    );
    Ok(())
}

async fn require_known_verse(db: &Db, verse_id: &str) -> Result<(), LocalEnrolmentError> {
    if row_exists(db, "verse", "verse_id", verse_id).await? {
        Ok(())
    } else {
        Err(LocalEnrolmentError::UnknownVerse(verse_id.to_string()))
    }
}

/// `DbCommand::EnrolLocalDevice`: enrol, publish the held row into the verse, reply.
#[allow(clippy::too_many_arguments)]
pub async fn enrol_local_device_reply(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    keypair: Option<&fe_identity::NodeKeypair>,
    secret_store: Option<&Arc<dyn fe_identity::SecretStore>>,
    verse_id: String,
    correlation_id: Option<String>,
) -> DbResult {
    let enrolled = async {
        require_known_verse(db, &verse_id).await?;
        enrol_local_device(db, keypair, secret_store, SystemClock.now_ms()).await
    }
    .await;
    match enrolled {
        Ok(LocalEnrolment { held, .. }) => {
            if let Err(error) = publish(repl_tx, blob_store, &verse_id, &held) {
                return correlated_failure(
                    &correlation_id,
                    format!("Enrolment stored but not published: {error}"),
                );
            }
            DbResult::LocalDeviceEnrolled {
                verse_id,
                record_id_hex: held.record_id,
                principal_did: held.principal_did,
                device_key_hex: held.device_key_hex,
                correlation_id,
            }
        }
        Err(error) => correlated_failure(
            &correlation_id,
            format!("Enrol local device failed: {error}"),
        ),
    }
}

/// `DbCommand::RevokeLocalDevice`: revoke, publish the held tombstone into the verse, reply.
#[allow(clippy::too_many_arguments)]
pub async fn revoke_local_device_reply(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    keypair: Option<&fe_identity::NodeKeypair>,
    secret_store: Option<&Arc<dyn fe_identity::SecretStore>>,
    verse_id: String,
    device_key_hex: String,
    correlation_id: Option<String>,
) -> DbResult {
    let revoked = async {
        require_known_verse(db, &verse_id).await?;
        revoke_local_device(
            db,
            keypair,
            secret_store,
            &device_key_hex,
            SystemClock.now_ms(),
        )
        .await
    }
    .await;
    match revoked {
        Ok(LocalEnrolment { held, .. }) => {
            if let Err(error) = publish(repl_tx, blob_store, &verse_id, &held) {
                return correlated_failure(
                    &correlation_id,
                    format!("Revocation stored but not published: {error}"),
                );
            }
            DbResult::LocalDeviceRevoked {
                verse_id,
                record_id_hex: held.record_id,
                device_key_hex: held.device_key_hex,
                correlation_id,
            }
        }
        Err(error) => correlated_failure(
            &correlation_id,
            format!("Revoke local device failed: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_canonical_log::crypto::DeviceSecretKey;

    fn principal_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn artifact(seed: u8, device_seed: u8, revoked: bool) -> VerifiedEnrolmentArtifact {
        let principal = Principal::from_public_key(principal_key(seed).verifying_key().to_bytes());
        let device_key = DeviceSecretKey::from_bytes([device_seed; 32]).public_key();
        let bytes = if revoked {
            DeviceEnrolmentRevocationV1 {
                principal,
                device_key,
                revoked_at_ms: 1_000,
            }
            .sign(&principal_key(seed))
            .and_then(|signed| signed.encode_canonical())
        } else {
            DeviceEnrolmentV1 {
                principal,
                device_key,
                enrolled_at_ms: 1_000,
            }
            .sign(&principal_key(seed))
            .and_then(|signed| signed.encode_canonical())
        }
        .expect("artifact bytes");
        VerifiedEnrolmentArtifact::decode_and_verify(&bytes).expect("verifies")
    }

    #[test]
    fn a_row_whose_index_columns_lie_about_its_artifact_does_not_verify() {
        let enrolment = artifact(0x02, 0x0d, false);
        let honest = EnrolmentRow::of(&enrolment);
        assert_eq!(honest.verify(), Ok(enrolment));

        let other = EnrolmentRow::of(&artifact(0x03, 0x0d, false));
        let lies: [(&str, EnrolmentRow); 4] = [
            (
                "a revocation flag the signature domain does not carry",
                EnrolmentRow {
                    revoked: true,
                    ..honest.clone()
                },
            ),
            (
                "another principal's DID",
                EnrolmentRow {
                    principal_did: other.principal_did.clone(),
                    ..honest.clone()
                },
            ),
            (
                "another slot's record id",
                EnrolmentRow {
                    record_id: other.record_id.clone(),
                    ..honest.clone()
                },
            ),
            (
                "bytes that are not base64",
                EnrolmentRow {
                    artifact_cbor: "!".to_string(),
                    ..honest.clone()
                },
            ),
        ];
        for (case, row) in lies {
            assert!(row.verify().is_err(), "{case} must not verify");
        }
    }

    #[test]
    fn admission_binds_the_author_to_the_principal_and_the_wire_key_to_the_record() {
        let enrolment = artifact(0x02, 0x0d, false);
        let row = EnrolmentRow::of(&enrolment);
        let bytes = serde_json::to_vec(&row).expect("row json");
        let principal_did = enrolment.principal().did.clone();
        let stranger_did = artifact(0x03, 0x0d, false).principal().did.clone();

        assert_eq!(
            admit_replicated_enrolment(&row.record_id, &bytes, &principal_did),
            Ok(())
        );
        assert!(admit_replicated_enrolment(&row.record_id, &bytes, &stranger_did).is_err());
        assert!(admit_replicated_enrolment(&"00".repeat(32), &bytes, &principal_did).is_err());
        assert_eq!(
            admit_replicated_enrolment(&row.record_id, &[], &stranger_did),
            Ok(()),
            "the unsigned del marker is admitted only to be reported NotApplicable"
        );
    }
}
