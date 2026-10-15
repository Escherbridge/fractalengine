//! SPEC-10 §3.4 / §4 shadow sealer: closed windows of door-recorded members become sealed
//! observation hexons plus a signed kind-5 `observation_batch`; see `canon_log/AGENTS.md`
//! §observation.

use std::collections::{BTreeSet, HashSet};
use std::num::NonZeroUsize;

use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    verify_chain_bytes, AuthorizationRequest, AuthorizationSnapshot, Certificate, Chain,
    ChainError, ObjectClass, Principal, UnsignedCertificate, Verb, VerificationOptions,
};
use fe_canonical_log::compose::{CapabilityChainSource, Clock};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey};
use fe_canonical_log::crypto::artifact_aad::StoredArtifactAad;
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::ScopeKeyStore;
use fe_canonical_log::did_key::public_key_to_did;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::identity::ObservationSource;
use fe_canonical_log::observation::{
    node_anchor, observation_batch_schema_hash, HlcRange, ObservationBatchPayload,
    ObservationBatchPayloadParts, SensorTimeRange, OBSERVATION_BATCH_OPERATION_KIND,
};
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::segment::artifact::{
    pack_bulk_artifact, seal_artifact, BulkArtifactBody, EncryptionDescriptor, LaneClass,
    NonceLedger, SealRequest,
};
use fe_canonical_log::segment::hashseq::LaneKey;
use fe_canonical_log::segment::payload_shard::{PayloadTopicScope, SegmentSizeCaveat};
use fe_canonical_log::segment::relay_policy::{PeerIdentity, RelayAuthorizationView};
use fe_canonical_log::signing::sign_envelope;
use fe_query::columnar::observation::{
    read_observation_hexon, write_observation_hexon, ObservationHexonError,
};
use fe_sdk::legacy_identifier::LegacyTable;

use super::{
    legacy_id, load_members, materialize_observation_references, require_mode, resolve_members,
    seals_observations, ObservationPlaneError, ObservationTable, PayloadRefRow, ReadingRow,
    ResolvedObservation, WindowMember,
};
use crate::canon_log::admission::NoPrecondition;
use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::canonical_epoch::SurrealScopeEpochStore;
use crate::canon_log::verifier::{
    admit_and_append_durably, ensure_observation_batch_schema, DurableAdmissionError,
    DurableCandidateVerifier,
};
use crate::canon_log::{identifier_to_hex, op_id_from_hex, op_id_to_hex, row_exists, StorageError};
use crate::handlers::device_payload::OBSERVATION_WINDOW_BUCKET_MS;
use crate::migration::mode::MigrationMode;
use crate::repo::Db;

/// Default grace after a window's end before it counts as closed: an ingest that took its HLC
/// just before the boundary may record its membership a moment later.
pub const DEFAULT_CLOSE_GRACE_MS: i64 = 10_000;
/// Default bound of the sealer's `(key_id, nonce)` reuse guard (a D-CL24 caller number).
pub const DEFAULT_NONCE_LEDGER_CAPACITY: usize = 4_096;
/// How often one batch is re-sealed after observing a scope-epoch bump (SPEC-10 §4.2 rule 4).
const EPOCH_RETRY_LIMIT: usize = 3;

/// Sealer policy: window closing and the nonce guard. The window width is the door's
/// [`OBSERVATION_WINDOW_BUCKET_MS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SealPolicy {
    /// Milliseconds after a window's end before it is sealed.
    pub close_grace_ms: i64,
    /// Bound of the nonce-reuse ledger.
    pub nonce_ledger_capacity: NonZeroUsize,
}

impl Default for SealPolicy {
    fn default() -> Self {
        Self {
            close_grace_ms: DEFAULT_CLOSE_GRACE_MS,
            nonce_ledger_capacity: NonZeroUsize::new(DEFAULT_NONCE_LEDGER_CAPACITY)
                .expect("non-zero default"),
        }
    }
}

/// Who seals, for which verse and branch, under which capability chain.
pub struct ObservationSealerConfig<'a> {
    /// The legacy verse id whose petals' windows this sealer seals.
    pub verse_id: &'a str,
    /// The canonical branch every batch is appended to.
    pub branch_id: Identifier32,
    /// A locally held chain granting the author `append/op` (kinds ∋ 5) and `append/shard`.
    pub chain_id: Hash32,
    /// The node's Ed25519 key: the batch author.
    pub author: &'a SigningKey,
    /// Window and nonce policy.
    pub policy: SealPolicy,
}

/// Everything one batch is sealed under: the petal payload topic at the chain's epoch and its
/// current scope key (SPEC-10 §4.2 rule 4: the payload, hexon and statistics share it).
pub struct SealingContext<'a> {
    /// The batch's verse identifier.
    pub verse_id: Identifier32,
    /// The batch's petal identifier.
    pub petal_id: Identifier32,
    /// The petal scope the operation and both artifacts carry.
    pub scope: Scope,
    /// The chain's epoch scope (where a bump would land).
    pub epoch_scope: Scope,
    /// The chain's (and the operation's) scope epoch.
    pub scope_epoch: u64,
    /// The petal payload topic both artifacts are sealed into.
    pub topic: PayloadTopicScope,
    /// The capability chain the operation references.
    pub chain_id: Hash32,
    chain_bytes: Vec<u8>,
    key: &'a SealingKey,
}

/// One sealed bulk artifact: its SPEC-6 id and exact stored bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedShard {
    /// `BLAKE3(stored)`.
    pub artifact_id: Hash32,
    /// The canonical sealed outer map.
    pub stored: Vec<u8>,
}

/// A signed kind-5 operation and its payload ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedObservationBatch {
    /// The complete canonical envelope bytes.
    pub bytes: Vec<u8>,
    /// The payload ciphertext the envelope commits to.
    pub ciphertext: Vec<u8>,
    /// `BLAKE3(bytes)`.
    pub op_id: Hash32,
}

/// One batch this sealer appended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedBatch {
    /// The kind-5 operation.
    pub op_id: Hash32,
    /// The legacy petal id.
    pub petal_id: String,
    /// The window it seals, when it came from door-recorded membership.
    pub window_bucket: Option<i64>,
    /// Rows across both Parquet members.
    pub count: u64,
    /// The sealed hexon.
    pub hexon_artifact_id: Hash32,
    /// The sealed statistics artifact.
    pub stats_artifact_id: Hash32,
    /// Whether its reference row materialized in the same call.
    pub reference_materialized: bool,
}

/// An accepted observation this node can never seal (SPEC-10 §8 rule 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncoveredObservation {
    /// The hot table.
    pub table: ObservationTable,
    /// The row id.
    pub row_id: String,
    /// Why.
    pub reason: String,
}

/// What one sealing pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SealReport {
    /// Batches appended, in sealing order.
    pub batches: Vec<SealedBatch>,
    /// Members recorded as uncovered in this pass.
    pub uncovered: Vec<UncoveredObservation>,
    /// Claims released because their operation never reached the verified log.
    pub released_orphan_claims: usize,
}

/// The shadow sealer (SPEC-10 §3.4, §4). Constructible only in `dual_emit_shadow` or
/// `shadow_rebuild`; it reads window membership ONLY from `observation_window_member` and never
/// scans the hot tables or filters on `source_did` (§3.4 rule 5).
pub struct ObservationSealer<'a> {
    mode: MigrationMode,
    db: &'a Db,
    verifier: &'a DurableCandidateVerifier,
    config: ObservationSealerConfig<'a>,
    author_did: String,
    ledger: NonceLedger,
}

enum Prepared {
    Ready(Box<PreparedBatch>),
    Oversized,
}

struct PreparedBatch {
    signed: SignedObservationBatch,
    hexon: SealedShard,
    stats: SealedShard,
    count: u64,
}

impl<'a> ObservationSealer<'a> {
    /// A sealer in `mode`, registering the kind-5 schema on `verifier` so its appends admit.
    pub fn new(
        mode: MigrationMode,
        db: &'a Db,
        verifier: &'a DurableCandidateVerifier,
        config: ObservationSealerConfig<'a>,
    ) -> Result<Self, ObservationPlaneError> {
        require_mode(mode, seals_observations)?;
        ensure_observation_batch_schema(&verifier.parts().schemas)
            .map_err(|error| ObservationPlaneError::Seal(error.to_string()))?;
        let author_did = public_key_to_did(&config.author.verifying_key().to_bytes());
        let ledger = NonceLedger::with_capacity(config.policy.nonce_ledger_capacity);
        Ok(Self {
            mode,
            db,
            verifier,
            config,
            author_did,
            ledger,
        })
    }

    /// Seals every closed window of this verse's door-recorded, not-yet-sealed members.
    ///
    /// A window `[bucket, bucket + width)` is closed once `now_wall_ms - close_grace_ms` has
    /// passed its end. An oversized window is split by anchor, then by HLC (§3.4 rule 4); an
    /// observation that cannot be sealed is marked uncovered, never silently dropped.
    pub async fn seal_closed_windows(
        &mut self,
        now_wall_ms: i64,
    ) -> Result<SealReport, ObservationPlaneError> {
        let mut report = SealReport {
            released_orphan_claims: release_orphan_claims(self.db).await?,
            ..SealReport::default()
        };
        let petals = petals_of_verse(self.db, self.config.verse_id).await?;
        if petals.is_empty() {
            return Ok(report);
        }
        let latest_closed_bucket = now_wall_ms
            .saturating_sub(self.config.policy.close_grace_ms)
            .saturating_sub(OBSERVATION_WINDOW_BUCKET_MS);
        // One window's members in memory at a time: list the closed windows, then load each.
        for (petal_id, window_bucket) in
            closed_windows(self.db, &petals, latest_closed_bucket).await?
        {
            let members = load_members(
                self.db,
                "sealed_op_id = NONE AND uncovered = NONE AND petal_id = $petal \
                 AND window_bucket = $bucket",
                vec![
                    ("petal", serde_json::json!(petal_id)),
                    ("bucket", serde_json::json!(window_bucket)),
                ],
            )
            .await?;
            let mut coverable = Vec::with_capacity(members.len());
            for (member, resolved) in resolve_members(self.db, &members).await? {
                match resolved {
                    Ok(observation) => coverable.push((member, observation)),
                    Err(reason) => {
                        mark_uncovered(self.db, &member, &reason).await?;
                        report.uncovered.push(UncoveredObservation {
                            table: member.table,
                            row_id: member.row_id,
                            reason,
                        });
                    }
                }
            }

            let mut groups = vec![coverable];
            while let Some(group) = groups.pop() {
                if group.is_empty() {
                    continue;
                }
                let observations: Vec<&ResolvedObservation> =
                    group.iter().map(|(_, observation)| observation).collect();
                let (readings, payloads, sources) = rows_of(&observations);
                match self
                    .prepare(&petal_id, &readings, &payloads, &sources)
                    .await?
                {
                    Prepared::Ready(prepared) => {
                        let claimed: Vec<WindowMember> =
                            group.iter().map(|(member, _)| member.clone()).collect();
                        let mut batch = self.commit(&petal_id, *prepared, Some(&claimed)).await?;
                        batch.window_bucket = Some(window_bucket);
                        report.batches.push(batch);
                    }
                    Prepared::Oversized if group.len() == 1 => {
                        let (member, _) = &group[0];
                        let reason = "a single observation exceeds the sealed artifact bound";
                        mark_uncovered(self.db, member, reason).await?;
                        report.uncovered.push(UncoveredObservation {
                            table: member.table,
                            row_id: member.row_id.clone(),
                            reason: reason.to_string(),
                        });
                    }
                    Prepared::Oversized => {
                        let (first, second) = split_group(group);
                        groups.push(second);
                        groups.push(first);
                    }
                }
            }
        }
        Ok(report)
    }

    /// Seals explicit rows as one batch in `petal_id`, outside window membership: the building
    /// block the window path wraps, for a caller that already holds door-accepted rows.
    ///
    /// It signs whatever rows it is handed, so it is a fixture seam, never a live entry point:
    /// feeding it anything but this node's own door accepts breaks SPEC-10 §3.4 rule 5. The live
    /// path is [`Self::seal_closed_windows`].
    pub async fn seal_observations(
        &mut self,
        petal_id: &str,
        readings: Vec<ReadingRow>,
        payload_refs: Vec<PayloadRefRow>,
        sources: Vec<ObservationSource>,
    ) -> Result<SealedBatch, ObservationPlaneError> {
        match self
            .prepare(petal_id, &readings, &payload_refs, &sources)
            .await?
        {
            Prepared::Ready(prepared) => self.commit(petal_id, *prepared, None).await,
            Prepared::Oversized => Err(ObservationPlaneError::Unauthorized(
                "the batch exceeds the sealed artifact bound".to_string(),
            )),
        }
    }

    /// Builds, self-checks, seals and signs one batch; re-seals after an epoch bump.
    async fn prepare(
        &mut self,
        petal_id: &str,
        readings: &[ReadingRow],
        payloads: &[PayloadRefRow],
        sources: &[ObservationSource],
    ) -> Result<Prepared, ObservationPlaneError> {
        let verifier = self.verifier;
        for _ in 0..EPOCH_RETRY_LIMIT {
            let context = sealing_context(
                self.db,
                verifier,
                self.config.chain_id,
                self.config.verse_id,
                petal_id,
            )
            .await?;
            let written = match write_observation_hexon(
                &self.author_did,
                readings,
                payloads,
                sources.len(),
            ) {
                Ok(written) => written,
                Err(error) => return Err(ObservationPlaneError::Hexon(error.to_string())),
            };
            // Self-check: never sign a hexon hydration would refuse (decompression limits).
            match read_observation_hexon(&written.hexon_bytes) {
                Ok(_) => {}
                Err(ObservationHexonError::Io(_)) => return Ok(Prepared::Oversized),
                Err(error) => return Err(ObservationPlaneError::Hexon(error.to_string())),
            }
            let hexon = seal_shard(&context, &mut self.ledger, written.hexon_bytes)?;
            let stats = seal_shard(&context, &mut self.ledger, written.stats_bytes)?;
            // SPEC-10 §4.2 rule 2 (B1 §append-shard): the sealer itself proves `append/shard`
            // for both artifacts at their exact stored length before anything is signed.
            for shard in [&hexon, &stats] {
                match self.check_append_shard(&context, shard.stored.len() as u64) {
                    Ok(()) => {}
                    Err(ChainError::SegmentTooLarge { .. }) => return Ok(Prepared::Oversized),
                    Err(error) => {
                        return Err(ObservationPlaneError::Unauthorized(error.to_string()))
                    }
                }
            }

            let summary = &written.summary;
            let payload = ObservationBatchPayload::new(
                ObservationBatchPayloadParts {
                    hexon_artifact_id: hexon.artifact_id,
                    hexon_stored_length: hexon.stored.len() as u64,
                    hlc_range: HlcRange {
                        min: hlc_of(summary.hlc_range.0),
                        max: hlc_of(summary.hlc_range.1),
                    },
                    sensor_time_range: SensorTimeRange {
                        min_ms: summary.sensor_time_range_ms.0,
                        max_ms: summary.sensor_time_range_ms.1,
                    },
                    stats_hash: stats.artifact_id,
                    count: summary.count,
                    anchors: summary.anchors.iter().copied().map(Identifier32).collect(),
                    asset_refs: summary.asset_refs.iter().copied().map(Hash32).collect(),
                    sources: sources.to_vec(),
                    hexon_format_version: summary.hexon_format_version,
                    stats_stored_length: stats.stored.len() as u64,
                }
                .normalized(),
            )
            .map_err(|error| ObservationPlaneError::Payload(error.to_string()))?;

            let parents = branch_heads(self.db, self.config.branch_id).await?;
            if parents.is_empty() {
                return Err(ObservationPlaneError::NoBranchHeads);
            }
            let (packed, _) = crate::op_log::next_hlc_timestamp();
            let hlc = Hlc::new(packed >> 16, (packed & 0xFFFF) as u32);
            let signed = sign_observation_batch(
                &context,
                self.config.author,
                self.config.branch_id,
                parents,
                hlc,
                &payload,
            )?;

            // SPEC-10 §4.2 rule 4: a bump observed before the append discards this seal.
            if current_epoch(self.db, &context.epoch_scope).await? == context.scope_epoch {
                return Ok(Prepared::Ready(Box::new(PreparedBatch {
                    signed,
                    hexon,
                    stats,
                    count: summary.count,
                })));
            }
        }
        Err(ObservationPlaneError::EpochMovedDuringSeal)
    }

    /// Stores both artifacts, claims the members, appends through the durable admission door,
    /// and materializes the reference row. A refused append releases the claim.
    async fn commit(
        &self,
        petal_id: &str,
        prepared: PreparedBatch,
        members: Option<&[WindowMember]>,
    ) -> Result<SealedBatch, ObservationPlaneError> {
        let store = &self.verifier.parts().ciphertexts;
        for shard in [&prepared.hexon, &prepared.stats] {
            store
                .put_sealed_artifact(shard.artifact_id, &shard.stored)
                .await
                .map_err(|error| ObservationPlaneError::Seal(error.to_string()))?;
        }

        let op_id = prepared.signed.op_id;
        let op_hex = op_id_to_hex(op_id);
        if let Some(members) = members {
            claim_members(self.db, &op_hex, petal_id, members).await?;
        }

        let log = SurrealVerifiedLogStore::new(self.db.clone());
        let mut admitted = admit_and_append_durably(
            self.verifier,
            &log,
            &NoPrecondition,
            &prepared.signed.bytes,
            Some(&prepared.signed.ciphertext),
        )
        .await;
        if matches!(
            admitted,
            Err(DurableAdmissionError::CiphertextNotRetained(_)
                | DurableAdmissionError::LifecycleNotObserved(_))
        ) {
            // The operation IS appended; the same call finishes it (`AlreadyPresent`).
            admitted = admit_and_append_durably(
                self.verifier,
                &log,
                &NoPrecondition,
                &prepared.signed.bytes,
                Some(&prepared.signed.ciphertext),
            )
            .await;
        }
        match admitted {
            Ok(_) => {}
            Err(DurableAdmissionError::Admission(rejection)) => {
                if members.is_some() {
                    release_claim(self.db, &op_hex).await?;
                }
                return Err(ObservationPlaneError::Admission(rejection.to_string()));
            }
            Err(other) => return Err(ObservationPlaneError::Admission(other.to_string())),
        }

        let frontier = SortedFrontier::try_new([op_id])
            .map_err(|error| ObservationPlaneError::Replay(format!("{error:?}")))?;
        let replay = materialize_observation_references(
            self.mode,
            self.db,
            self.verifier,
            self.config.branch_id,
            &frontier,
        )
        .await?;
        Ok(SealedBatch {
            op_id,
            petal_id: petal_id.to_string(),
            window_bucket: None,
            count: prepared.count,
            hexon_artifact_id: prepared.hexon.artifact_id,
            stats_artifact_id: prepared.stats.artifact_id,
            reference_materialized: replay.applied.contains(&op_id)
                || replay.already_processed.contains(&op_id),
        })
    }

    /// `append/shard` for an artifact of `stored_length` bytes in the batch scope.
    fn check_append_shard(
        &self,
        context: &SealingContext<'_>,
        stored_length: u64,
    ) -> Result<(), ChainError> {
        let author = Principal::from_public_key(self.config.author.verifying_key().to_bytes());
        verify_chain_bytes(
            &context.chain_bytes,
            &AuthorizationRequest {
                verb: Verb::Append,
                object_class: ObjectClass::Shard,
                target_scope: context.scope,
                resource_id: None,
                requester: author.clone(),
                operation_author: author,
                operation_scope_epoch: context.scope_epoch,
                operation_kind: Some(OBSERVATION_BATCH_OPERATION_KIND),
                is_structural_operation: false,
                payload_bytes: None,
                segment_bytes: Some(stored_length),
                schema_hash: Some(observation_batch_schema_hash()),
                preview_hz: None,
                now_ms: self.verifier.parts().clock.now_ms(),
            },
            &self.verifier.parts().authority,
            &AuthorizationSnapshot {
                epoch_causally_valid_for_operation: false,
            },
            &VerificationOptions::live(),
        )
        .map(|_| ())
    }
}

/// The chain's leaf grant.
pub(crate) fn chain_leaf(chain_bytes: &[u8]) -> Result<UnsignedCertificate, ObservationPlaneError> {
    let chain = Chain::decode_canonical(chain_bytes)
        .map_err(|error| ObservationPlaneError::ChainMalformed(error.to_string()))?;
    let leaf = chain
        .certificates
        .last()
        .ok_or_else(|| ObservationPlaneError::ChainMalformed("empty chain".to_string()))?;
    Certificate::decode_canonical(leaf)
        .map(|certificate| certificate.unsigned)
        .map_err(|error| ObservationPlaneError::ChainMalformed(error.to_string()))
}

async fn current_epoch(db: &Db, epoch_scope: &Scope) -> Result<u64, ObservationPlaneError> {
    SurrealScopeEpochStore::new(db.clone())
        .current_epoch(epoch_scope)
        .await
        .map_err(|error| {
            ObservationPlaneError::Storage(StorageError::query("scope epoch read", error))
        })?
        .ok_or(ObservationPlaneError::EpochUnknown)
}

/// Resolves the sealing context for `petal_id` of `verse_id` under `chain_id`: the chain's epoch
/// must still be current, and custody must hold the petal key at that epoch.
pub async fn sealing_context<'v>(
    db: &Db,
    verifier: &'v DurableCandidateVerifier,
    chain_id: Hash32,
    verse_id: &str,
    petal_id: &str,
) -> Result<SealingContext<'v>, ObservationPlaneError> {
    let chain_bytes = verifier
        .parts()
        .chains
        .chain_bytes(&chain_id)
        .ok_or(ObservationPlaneError::ChainUnavailable(chain_id))?;
    let leaf = chain_leaf(&chain_bytes)?;
    let current = current_epoch(db, &leaf.epoch_scope).await?;
    if current != leaf.scope_epoch {
        return Err(ObservationPlaneError::ChainEpochStale {
            chain_epoch: leaf.scope_epoch,
            current_epoch: current,
        });
    }
    let verse = legacy_id(LegacyTable::Verse, verse_id)?;
    let petal = legacy_id(LegacyTable::Petal, petal_id)?;
    let scope = Scope::new(verse, Some(petal), None)
        .map_err(|error| ObservationPlaneError::Identifier(error.to_string()))?;
    let epoch = leaf.scope_epoch;
    let key = verifier
        .parts()
        .scope_keys
        .scope_key(&ScopeKeyId { scope, epoch })
        .ok_or(ObservationPlaneError::ScopeKeyUnavailable { epoch })?;
    let key_id = derive_key_id(&scope, epoch, key)
        .map_err(|error| ObservationPlaneError::Seal(error.to_string()))?;
    Ok(SealingContext {
        verse_id: verse,
        petal_id: petal,
        scope,
        epoch_scope: leaf.epoch_scope,
        scope_epoch: epoch,
        topic: PayloadTopicScope {
            verse_id: verse,
            petal_id: petal,
            scope_epoch: epoch,
            key_id,
        },
        chain_id,
        chain_bytes,
        key,
    })
}

/// The sealer's own view of its one lane: the epoch and key it just resolved, nothing else.
struct SealingLaneView {
    epoch: u64,
    key_id: Identifier32,
}

impl RelayAuthorizationView for SealingLaneView {
    fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
        Some(self.epoch)
    }

    fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
        Some(self.key_id)
    }

    fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
        false
    }

    fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
        false
    }

    fn may_wrap_scope_key_for_peer(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
        false
    }
}

/// Seals `content` as a `BulkArtifact` of class `shard` in the context's petal topic, through
/// the one SPEC-6 door (`seal_artifact`) with a fresh nonce under the current key.
pub fn seal_shard(
    context: &SealingContext<'_>,
    ledger: &mut NonceLedger,
    content: Vec<u8>,
) -> Result<SealedShard, ObservationPlaneError> {
    let seal_error = |error: &dyn std::fmt::Display| ObservationPlaneError::Seal(error.to_string());
    let body = BulkArtifactBody::new(context.topic, ObjectClass::Shard, content)
        .map_err(|error| seal_error(&error))?;
    let view = SealingLaneView {
        epoch: context.scope_epoch,
        key_id: context.topic.key_id,
    };
    let nonce = FreshNonce::draw(context.topic.key_id).map_err(|error| seal_error(&error))?;
    let params = nonce.params();
    let descriptor = EncryptionDescriptor::new(params.suite_id, params.key_id, params.nonce);
    // The size bound is the chain's `max_segment_bytes`, checked on the exact stored length.
    let packing = pack_bulk_artifact(
        &view,
        &body,
        &SegmentSizeCaveat::new(u64::MAX, 0),
        &descriptor,
    )
    .map_err(|error| seal_error(&error))?;
    let aad = StoredArtifactAad::bulk_artifact(packing.plaintext.len(), params)
        .preimage()
        .map_err(|error| seal_error(&error))?;
    let sealed_payload =
        seal(context.key, nonce, &aad, &packing.plaintext).map_err(|error| seal_error(&error))?;
    let sealed = seal_artifact(
        &view,
        ledger,
        SealRequest {
            lane: &body.lane_key(),
            scope_epoch: context.scope_epoch,
            lane_class: LaneClass::BulkArtifact,
            descriptor,
            ciphertext: sealed_payload.ciphertext,
        },
    )
    .map_err(|error| seal_error(&error))?;
    let stored = sealed
        .encode_canonical()
        .map_err(|error| seal_error(&error))?;
    Ok(SealedShard {
        artifact_id: Hash32::of(&stored),
        stored,
    })
}

/// Signs a kind-5 `observation_batch` by `author` in the context's petal scope, its payload
/// sealed under the context's petal key with the SPEC-1 §5.2 AAD.
pub fn sign_observation_batch(
    context: &SealingContext<'_>,
    author: &SigningKey,
    branch_id: Identifier32,
    parents: Vec<Hash32>,
    hlc: Hlc,
    payload: &ObservationBatchPayload,
) -> Result<SignedObservationBatch, ObservationPlaneError> {
    let seal_error = |error: &dyn std::fmt::Display| ObservationPlaneError::Seal(error.to_string());
    let nonce = FreshNonce::draw(context.topic.key_id).map_err(|error| seal_error(&error))?;
    let mut unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: OBSERVATION_BATCH_OPERATION_KIND,
        scope: context.scope,
        author: Author::from_public_key(author.verifying_key().to_bytes()),
        capability: CapabilityRef {
            chain_id: context.chain_id,
            scope_epoch: context.scope_epoch,
        },
        schema_hash: observation_batch_schema_hash(),
        branch_id,
        parents,
        hlc,
        payload: PayloadRef {
            encryption: Some(nonce.params()),
            ..PayloadRef::empty()
        },
    };
    let aad = payload_aad_preimage(&unsigned).map_err(|error| seal_error(&error))?;
    let plaintext = payload
        .encode_canonical()
        .map_err(|error| ObservationPlaneError::Payload(error.to_string()))?;
    let sealed = seal(context.key, nonce, &aad, &plaintext).map_err(|error| seal_error(&error))?;
    unsigned.payload = sealed.payload_ref();
    let bytes = sign_envelope(author, &unsigned)
        .map_err(|error| seal_error(&error))?
        .encode_canonical()
        .map_err(|error| seal_error(&error))?;
    Ok(SignedObservationBatch {
        op_id: Hash32::of(&bytes),
        bytes,
        ciphertext: sealed.ciphertext,
    })
}

/// The heads of `branch_id` in the verified log: every operation no other operation names as a
/// parent, ascending. A kind-5 batch descends from all of them.
pub async fn branch_heads(db: &Db, branch_id: Identifier32) -> Result<Vec<Hash32>, StorageError> {
    #[derive(serde::Deserialize)]
    struct Row {
        op_id_hex: String,
        #[serde(default)]
        parent_op_ids: Vec<String>,
    }
    let rows: Vec<serde_json::Value> = db
        .query("SELECT op_id_hex, parent_op_ids FROM verified_op_log WHERE branch_id = $branch")
        .bind(("branch", identifier_to_hex(branch_id)))
        .await
        .map_err(|error| StorageError::query("branch head read", error))?
        .check()
        .map_err(|error| StorageError::query("branch head read", error))?
        .take(0)
        .map_err(|error| StorageError::query("branch head read", error))?;
    let rows: Vec<Row> = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row).map_err(|error| StorageError::malformed("op row", error))
        })
        .collect::<Result<_, _>>()?;
    let named_as_parent: HashSet<&str> = rows
        .iter()
        .flat_map(|row| row.parent_op_ids.iter().map(String::as_str))
        .collect();
    let mut heads: Vec<Hash32> = rows
        .iter()
        .filter(|row| !named_as_parent.contains(row.op_id_hex.as_str()))
        .filter_map(|row| op_id_from_hex(&row.op_id_hex))
        .collect();
    heads.sort();
    heads.dedup();
    Ok(heads)
}

fn hlc_of((wall_ms, counter): (i64, i32)) -> Hlc {
    Hlc::new(wall_ms.max(0) as u64, counter.max(0) as u32)
}

/// The hexon rows of `observations`, each pointing at its source in the sorted, deduplicated
/// `sources` list it returns (§3.2 rule 9: every listed source is used).
pub(crate) fn rows_of(
    observations: &[&ResolvedObservation],
) -> (Vec<ReadingRow>, Vec<PayloadRefRow>, Vec<ObservationSource>) {
    let sources: Vec<ObservationSource> = observations
        .iter()
        .map(|observation| observation.source().clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let index_of = |source: &ObservationSource| {
        sources
            .binary_search(source)
            .expect("every source was collected above") as u32
    };
    let mut readings = Vec::new();
    let mut payloads = Vec::new();
    for observation in observations {
        match observation {
            ResolvedObservation::Reading { hot, source } => readings.push(ReadingRow {
                reading_id: hot.reading_id.clone(),
                node_id: hot.node_id.clone(),
                metric: hot.metric.clone(),
                value: hot.value,
                units: hot.units.clone(),
                recorded_at: hot.recorded_at.clone(),
                recorded_at_ms: hot.recorded_at_ms,
                hlc_timestamp: hot.hlc_timestamp as u64,
                payload_id: hot.payload_id.clone(),
                source_index: index_of(source),
            }),
            ResolvedObservation::Payload {
                hot,
                asset_content_hashes,
                source,
            } => payloads.push(PayloadRefRow {
                payload_id: hot.payload_id.clone(),
                node_id: hot.node_id.clone(),
                format_id: hot.format_id.clone(),
                recorded_at: hot.recorded_at.clone(),
                recorded_at_ms: hot.recorded_at_ms,
                hlc_timestamp: hot.hlc_timestamp as u64,
                asset_ids: hot.asset_ids.clone(),
                asset_content_hashes: asset_content_hashes.clone(),
                source_index: index_of(source),
            }),
        }
    }
    (readings, payloads, sources)
}

type Group = Vec<(WindowMember, ResolvedObservation)>;

/// Splits an oversized group by anchor; a one-anchor group splits by `(hlc, id)` halves.
fn split_group(mut group: Group) -> (Group, Group) {
    let anchor_of = |observation: &ResolvedObservation| {
        node_anchor(observation.node_id()).map_or([0u8; 32], |anchor| anchor.0)
    };
    let anchors: BTreeSet<[u8; 32]> = group
        .iter()
        .map(|(_, observation)| anchor_of(observation))
        .collect();
    if anchors.len() > 1 {
        let lower: BTreeSet<[u8; 32]> = anchors.iter().take(anchors.len() / 2).copied().collect();
        let (first, second) = group
            .into_iter()
            .partition(|(_, observation)| lower.contains(&anchor_of(observation)));
        return (first, second);
    }
    group.sort_by(|(_, a), (_, b)| {
        let (a_hlc, a_id) = a.hlc_and_id();
        let (b_hlc, b_id) = b.hlc_and_id();
        (a_hlc, a_id).cmp(&(b_hlc, b_id))
    });
    let second = group.split_off(group.len() / 2);
    (group, second)
}

/// The closed windows of `petals` that still hold a pending member, in `(petal, bucket)` order.
async fn closed_windows(
    db: &Db,
    petals: &[String],
    latest_closed_bucket: i64,
) -> Result<Vec<(String, i64)>, StorageError> {
    #[derive(serde::Deserialize)]
    struct Window {
        petal_id: String,
        window_bucket: i64,
    }
    let rows: Vec<serde_json::Value> = db
        .query(
            "SELECT petal_id, window_bucket FROM observation_window_member \
             WHERE sealed_op_id = NONE AND uncovered = NONE AND petal_id IN $petals \
             AND window_bucket <= $latest GROUP BY petal_id, window_bucket",
        )
        .bind(("petals", petals.to_vec()))
        .bind(("latest", latest_closed_bucket))
        .await
        .map_err(|error| StorageError::query("closed window read", error))?
        .check()
        .map_err(|error| StorageError::query("closed window read", error))?
        .take(0)
        .map_err(|error| StorageError::query("closed window read", error))?;
    let mut windows: BTreeSet<(String, i64)> = BTreeSet::new();
    for row in rows {
        let window: Window = serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("closed window", error))?;
        windows.insert((window.petal_id, window.window_bucket));
    }
    Ok(windows.into_iter().collect())
}

/// The legacy petal ids of `verse_id`.
async fn petals_of_verse(db: &Db, verse_id: &str) -> Result<Vec<String>, StorageError> {
    db.query(
        "SELECT VALUE petal_id FROM petal WHERE fractal_id IN \
         (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $verse)",
    )
    .bind(("verse", verse_id.to_string()))
    .await
    .map_err(|error| StorageError::query("verse petal read", error))?
    .check()
    .map_err(|error| StorageError::query("verse petal read", error))?
    .take(0)
    .map_err(|error| StorageError::query("verse petal read", error))
}

/// Claims every member for `op_hex` before the append; any member already taken releases the
/// whole claim (single-sealer assumption, `canon_log/AGENTS.md` §observation).
async fn claim_members(
    db: &Db,
    op_hex: &str,
    petal_id: &str,
    members: &[WindowMember],
) -> Result<(), ObservationPlaneError> {
    let mut claimed = 0usize;
    for table in [ObservationTable::Reading, ObservationTable::Payload] {
        let ids: Vec<String> = members
            .iter()
            .filter(|member| member.table == table)
            .map(|member| member.row_id.clone())
            .collect();
        if ids.is_empty() {
            continue;
        }
        let rows: Vec<serde_json::Value> = db
            .query(
                "UPDATE observation_window_member SET sealed_op_id = $op \
                 WHERE petal_id = $petal AND `table` = $table AND row_id IN $ids \
                 AND sealed_op_id = NONE AND uncovered = NONE",
            )
            .bind(("op", op_hex.to_string()))
            .bind(("petal", petal_id.to_string()))
            .bind(("table", table.as_str().to_string()))
            .bind(("ids", ids))
            .await
            .map_err(|error| StorageError::query("window member claim", error))?
            .check()
            .map_err(|error| StorageError::query("window member claim", error))?
            .take(0)
            .map_err(|error| StorageError::query("window member claim", error))?;
        claimed += rows.len();
    }
    if claimed != members.len() {
        release_claim(db, op_hex).await?;
        return Err(ObservationPlaneError::ConcurrentClaim);
    }
    Ok(())
}

async fn release_claim(db: &Db, op_hex: &str) -> Result<(), StorageError> {
    db.query("UPDATE observation_window_member SET sealed_op_id = NONE WHERE sealed_op_id = $op")
        .bind(("op", op_hex.to_string()))
        .await
        .map_err(|error| StorageError::query("window member release", error))?
        .check()
        .map_err(|error| StorageError::query("window member release", error))?;
    Ok(())
}

/// Releases every claim whose operation never reached `verified_op_log` (a crash between the
/// claim and the append); returns how many operations' claims it released.
async fn release_orphan_claims(db: &Db) -> Result<usize, StorageError> {
    let claimed: Vec<String> = db
        .query(
            "SELECT VALUE sealed_op_id FROM observation_window_member WHERE sealed_op_id != NONE",
        )
        .await
        .map_err(|error| StorageError::query("window member claim read", error))?
        .check()
        .map_err(|error| StorageError::query("window member claim read", error))?
        .take(0)
        .map_err(|error| StorageError::query("window member claim read", error))?;
    let claimed: BTreeSet<String> = claimed.into_iter().collect();
    let mut released = 0;
    for op_hex in claimed {
        if !row_exists(db, "verified_op_log", "op_id_hex", &op_hex).await? {
            release_claim(db, &op_hex).await?;
            released += 1;
        }
    }
    Ok(released)
}

async fn mark_uncovered(db: &Db, member: &WindowMember, reason: &str) -> Result<(), StorageError> {
    db.query(
        "UPDATE observation_window_member SET uncovered = $reason \
         WHERE `table` = $table AND row_id = $row AND petal_id = $petal AND sealed_op_id = NONE",
    )
    .bind(("reason", reason.to_string()))
    .bind(("table", member.table.as_str().to_string()))
    .bind(("row", member.row_id.clone()))
    .bind(("petal", member.petal_id.clone()))
    .await
    .map_err(|error| StorageError::query("window member uncovered", error))?
    .check()
    .map_err(|error| StorageError::query("window member uncovered", error))?;
    Ok(())
}
