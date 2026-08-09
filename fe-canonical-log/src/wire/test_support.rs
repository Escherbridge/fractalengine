//! In-memory fakes for SPEC-7 conformance testing.
//!
//! Not gated behind `#[cfg(test)]`: `tests/spec7_conformance.rs` is a separate crate that
//! links only this library's public surface, so these fakes must be real, always-compiled
//! exports. They are test doubles, not reference implementations — the real branch registry,
//! commit pipeline, snapshot source, and capability verifier are wave-3/database concerns.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::envelope::{Author, Hash32, Identifier32, Scope};

use super::commit::{CanonicalCommitPipeline, CommitBinding, PipelineResult};
use super::cursor::{
    BranchRegistry, CommittedDelta, CursorOrder, CursorTuple, DurableCursor, ProjectionIdentity,
    ReplayOutcome, SnapshotReason,
};
use super::error::WireError;
use super::session::{CapabilityVerifier, ObjectClassBits, VerbBits, VerifiedAuthorization};
use super::snapshot::ScopeSnapshotSource;

fn encode_position(index: usize) -> Vec<u8> {
    (index as u64).to_be_bytes().to_vec()
}

fn decode_position(bytes: &[u8]) -> Result<usize, WireError> {
    let raw: [u8; 8] = bytes.try_into().map_err(|_| WireError::CursorInvalid)?;
    Ok(u64::from_be_bytes(raw) as usize)
}

struct RegistryState {
    committed: HashMap<CursorTuple, Vec<CommittedDelta>>,
}

/// A [`BranchRegistry`] backed by an in-memory append log, keyed by [`CursorTuple`].
///
/// `delivery_position` is the big-endian count of deltas committed to that tuple at issuance
/// time; `frontier_commitment` is the real [`crate::frontier::SortedFrontier::commitment`] over
/// every op_id committed to the tuple so far, so a tampered claim is genuinely detectable.
pub struct InMemoryBranchRegistry {
    state: Mutex<RegistryState>,
}

impl Default for InMemoryBranchRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBranchRegistry {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                committed: HashMap::new(),
            }),
        }
    }

    /// Test seam: commits one operation to `tuple`, returning the durable cursor it advances to.
    pub fn commit(
        &self,
        tuple: CursorTuple,
        op_id: Hash32,
        change_summary: Vec<u8>,
    ) -> DurableCursor {
        let mut state = self.state.lock().expect("registry lock");
        let deltas = state.committed.entry(tuple.clone()).or_default();
        let mut op_ids: Vec<Hash32> = deltas.iter().map(|delta| delta.op_id).collect();
        op_ids.push(op_id);
        let commitment = crate::frontier::SortedFrontier::try_new(op_ids)
            .expect("at least one op_id")
            .commitment();
        let cursor =
            DurableCursor::new(tuple.clone(), commitment, encode_position(deltas.len() + 1));
        deltas.push(CommittedDelta {
            op_id,
            branch_id: tuple.branch_id,
            scope: tuple.subscription_scope,
            projection_identity: tuple.projection_identity.clone(),
            cursor: cursor.clone(),
            change_summary,
        });
        cursor
    }
}

#[async_trait]
impl BranchRegistry for InMemoryBranchRegistry {
    async fn validate(&self, cursor: &DurableCursor) -> Result<(), WireError> {
        let state = self.state.lock().expect("registry lock");
        let deltas = state
            .committed
            .get(cursor.tuple())
            .ok_or(WireError::CursorInvalid)?;
        let index = decode_position(cursor.delivery_position())?;
        if index > deltas.len() {
            return Err(WireError::CursorInvalid);
        }
        let expected_commitment = if index == 0 {
            Hash32::of_empty()
        } else {
            deltas[index - 1].cursor.frontier_commitment()
        };
        if expected_commitment != cursor.frontier_commitment() {
            return Err(WireError::InvalidFrontierCommitment);
        }
        Ok(())
    }

    async fn compare(
        &self,
        left: &DurableCursor,
        right: &DurableCursor,
    ) -> Result<CursorOrder, WireError> {
        if left.tuple() != right.tuple() {
            return Ok(CursorOrder::Incomparable);
        }
        let left_index = decode_position(left.delivery_position())?;
        let right_index = decode_position(right.delivery_position())?;
        Ok(match left_index.cmp(&right_index) {
            std::cmp::Ordering::Equal => CursorOrder::Equal,
            std::cmp::Ordering::Less => CursorOrder::LeftBeforeRight,
            std::cmp::Ordering::Greater => CursorOrder::LeftAfterRight,
        })
    }

    async fn replay_after(&self, cursor: &DurableCursor) -> Result<ReplayOutcome, WireError> {
        let state = self.state.lock().expect("registry lock");
        let deltas = match state.committed.get(cursor.tuple()) {
            Some(deltas) => deltas,
            None => {
                return Ok(ReplayOutcome::SnapshotRequired(
                    SnapshotReason::CursorUnavailable,
                ))
            }
        };
        let index = match decode_position(cursor.delivery_position()) {
            Ok(index) => index,
            Err(_) => {
                return Ok(ReplayOutcome::SnapshotRequired(
                    SnapshotReason::CursorInvalid,
                ))
            }
        };
        if index > deltas.len() {
            return Ok(ReplayOutcome::SnapshotRequired(
                SnapshotReason::CursorUnavailable,
            ));
        }
        Ok(ReplayOutcome::Deltas(deltas[index..].to_vec()))
    }

    async fn snapshot_cursor(&self, scope: &Scope) -> Result<DurableCursor, WireError> {
        let state = self.state.lock().expect("registry lock");
        for (tuple, deltas) in state.committed.iter() {
            if &tuple.subscription_scope == scope {
                let commitment = deltas
                    .last()
                    .map(|delta| delta.cursor.frontier_commitment())
                    .unwrap_or_else(Hash32::of_empty);
                return Ok(DurableCursor::new(
                    tuple.clone(),
                    commitment,
                    encode_position(deltas.len()),
                ));
            }
        }
        Err(WireError::CursorInvalid)
    }
}

/// A single-response [`CanonicalCommitPipeline`] for tests that need exact control over one
/// `commit_submit` outcome.
pub struct ScriptedCommitPipeline {
    /// The `op_id` [`CanonicalCommitPipeline::derive_op_id`] always returns.
    pub derived_op_id: Hash32,
    /// The result [`CanonicalCommitPipeline::submit_candidate`] always returns.
    pub result: PipelineResult,
}

#[async_trait]
impl CanonicalCommitPipeline for ScriptedCommitPipeline {
    fn derive_op_id(&self, _complete_envelope_bytes: &[u8]) -> Hash32 {
        self.derived_op_id
    }

    async fn submit_candidate(
        &self,
        _complete_envelope_bytes: &[u8],
        _payload_ciphertext: Option<&[u8]>,
    ) -> PipelineResult {
        self.result.clone()
    }
}

struct MockPipelineState {
    by_op_id: HashMap<Hash32, (Vec<u8>, PipelineResult)>,
}

/// A registry-backed [`CanonicalCommitPipeline`]: derives `op_id` as `BLAKE3(bytes)`,
/// deduplicates identical resubmissions, and rejects a differing byte string for an existing
/// `op_id` as [`PipelineResult::IntegrityConflict`] — exercising the real dedup contract rather
/// than a single scripted answer.
pub struct MockCommitPipeline {
    registry: Arc<InMemoryBranchRegistry>,
    tuple: CursorTuple,
    state: Mutex<MockPipelineState>,
}

impl MockCommitPipeline {
    /// Builds a pipeline that commits accepted candidates to `registry` under `tuple`.
    pub fn new(registry: Arc<InMemoryBranchRegistry>, tuple: CursorTuple) -> Self {
        Self {
            registry,
            tuple,
            state: Mutex::new(MockPipelineState {
                by_op_id: HashMap::new(),
            }),
        }
    }
}

#[async_trait]
impl CanonicalCommitPipeline for MockCommitPipeline {
    fn derive_op_id(&self, complete_envelope_bytes: &[u8]) -> Hash32 {
        Hash32::of(complete_envelope_bytes)
    }

    async fn submit_candidate(
        &self,
        complete_envelope_bytes: &[u8],
        _payload_ciphertext: Option<&[u8]>,
    ) -> PipelineResult {
        let op_id = self.derive_op_id(complete_envelope_bytes);
        let mut state = self.state.lock().expect("pipeline lock");

        if let Some((existing_bytes, existing_result)) = state.by_op_id.get(&op_id) {
            if existing_bytes.as_slice() != complete_envelope_bytes {
                return PipelineResult::IntegrityConflict;
            }
            return match existing_result {
                PipelineResult::Committed(binding) | PipelineResult::AlreadyCommitted(binding) => {
                    PipelineResult::AlreadyCommitted(binding.clone())
                }
                other => other.clone(),
            };
        }

        let cursor = self.registry.commit(self.tuple.clone(), op_id, Vec::new());
        let binding = CommitBinding {
            branch_id: self.tuple.branch_id,
            scope: self.tuple.subscription_scope,
            projection_identity: self.tuple.projection_identity.clone(),
            cursor,
        };
        let result = PipelineResult::Committed(binding);
        state
            .by_op_id
            .insert(op_id, (complete_envelope_bytes.to_vec(), result.clone()));
        result
    }
}

/// A [`ScopeSnapshotSource`] that reads live state from a shared [`InMemoryBranchRegistry`], so
/// a test can commit new operations between taking a snapshot and resuming from it to prove
/// lag-recovery convergence.
pub struct MockScopeSnapshotSource {
    registry: Arc<InMemoryBranchRegistry>,
    view_bytes: Vec<u8>,
}

impl MockScopeSnapshotSource {
    /// Builds a source over `registry`; every snapshot's view bytes are `view_bytes`.
    pub fn new(registry: Arc<InMemoryBranchRegistry>, view_bytes: Vec<u8>) -> Self {
        Self {
            registry,
            view_bytes,
        }
    }
}

#[async_trait]
impl ScopeSnapshotSource for MockScopeSnapshotSource {
    async fn snapshot(
        &self,
        _branch_id: Identifier32,
        scope: &Scope,
        _projection_identity: &ProjectionIdentity,
    ) -> Result<(DurableCursor, Vec<u8>), WireError> {
        let cursor = self.registry.snapshot_cursor(scope).await?;
        Ok((cursor, self.view_bytes.clone()))
    }
}

struct ChainRecord {
    authorization: VerifiedAuthorization,
    expired: bool,
}

/// A [`CapabilityVerifier`] over a registered table of chain bytes, each independently
/// expirable to exercise revalidation-on-expiry behavior.
pub struct MockCapabilityVerifier {
    chains: Mutex<HashMap<Vec<u8>, ChainRecord>>,
    calls: Mutex<VecDeque<Vec<u8>>>,
}

impl Default for MockCapabilityVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl MockCapabilityVerifier {
    /// Builds an empty verifier.
    pub fn new() -> Self {
        Self {
            chains: Mutex::new(HashMap::new()),
            calls: Mutex::new(VecDeque::new()),
        }
    }

    /// Registers `chain_bytes` as verifying to `authorization`.
    pub fn register(&self, chain_bytes: Vec<u8>, authorization: VerifiedAuthorization) {
        self.chains.lock().expect("verifier lock").insert(
            chain_bytes,
            ChainRecord {
                authorization,
                expired: false,
            },
        );
    }

    /// Marks a previously registered chain as expired; subsequent verification fails.
    pub fn expire(&self, chain_bytes: &[u8]) {
        if let Some(record) = self
            .chains
            .lock()
            .expect("verifier lock")
            .get_mut(chain_bytes)
        {
            record.expired = true;
        }
    }

    /// Every chain byte string presented to [`CapabilityVerifier::verify`], in call order.
    pub fn calls(&self) -> Vec<Vec<u8>> {
        self.calls
            .lock()
            .expect("verifier lock")
            .iter()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl CapabilityVerifier for MockCapabilityVerifier {
    async fn verify(
        &self,
        capability_chain_bytes: &[u8],
        _requested_verb: VerbBits,
        _requested_object_class: ObjectClassBits,
        _requested_scope: &Scope,
    ) -> Result<VerifiedAuthorization, WireError> {
        self.calls
            .lock()
            .expect("verifier lock")
            .push_back(capability_chain_bytes.to_vec());
        let chains = self.chains.lock().expect("verifier lock");
        match chains.get(capability_chain_bytes) {
            Some(record) if !record.expired => Ok(record.authorization.clone()),
            _ => Err(WireError::CapabilityNotVerified),
        }
    }
}

/// A principal for tests: an [`Author`] with a deterministic key, for readable fixtures.
pub fn test_principal(filler: u8) -> Author {
    Author::from_public_key([filler; 32])
}

/// Test seam: a cursor at position zero (nothing yet observed) for `tuple`. Legal outside this
/// crate only because `test_support` is the sanctioned test-fixture surface — production code
/// still cannot construct a [`DurableCursor`] except through [`BranchRegistry::issue_cursor`].
pub fn cursor_at_position_zero(tuple: CursorTuple) -> DurableCursor {
    DurableCursor::new(tuple, Hash32::of_empty(), encode_position(0))
}

/// Test seam: builds an arbitrary (possibly tampered) cursor, for negative-path fixtures such
/// as a claimed `frontier_commitment` that does not match any real committed frontier.
pub fn cursor_with_claim(
    tuple: CursorTuple,
    frontier_commitment: Hash32,
    delivery_position: Vec<u8>,
) -> DurableCursor {
    DurableCursor::new(tuple, frontier_commitment, delivery_position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Scope as EnvelopeScope;

    fn tuple() -> CursorTuple {
        CursorTuple {
            verse_id: Identifier32([0x11; 32]),
            branch_id: Identifier32([0x22; 32]),
            subscription_scope: EnvelopeScope::verse_wide(Identifier32([0x11; 32])),
            projection_identity: ProjectionIdentity {
                materializer_id: "fe-scene".to_owned(),
                version: 1,
            },
        }
    }

    #[tokio::test]
    async fn the_in_memory_registry_validates_only_cursors_it_issued() {
        let registry = InMemoryBranchRegistry::new();
        let cursor = registry.commit(tuple(), Hash32([0x01; 32]), Vec::new());
        assert!(registry.validate(&cursor).await.is_ok());

        let tampered = DurableCursor::new(
            tuple(),
            Hash32([0xff; 32]),
            cursor.delivery_position().to_vec(),
        );
        assert_eq!(
            registry.validate(&tampered).await,
            Err(WireError::InvalidFrontierCommitment)
        );
    }

    #[tokio::test]
    async fn the_mock_commit_pipeline_dedups_identical_bytes_and_rejects_conflicts() {
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let pipeline = MockCommitPipeline::new(registry, tuple());
        let bytes = vec![1, 2, 3];

        let first = pipeline.submit_candidate(&bytes, None).await;
        assert!(matches!(first, PipelineResult::Committed(_)));

        let second = pipeline.submit_candidate(&bytes, None).await;
        assert!(matches!(second, PipelineResult::AlreadyCommitted(_)));

        // Different bytes hash to a different real BLAKE3 op_id, so this is a second, distinct
        // commit rather than a conflict; `IntegrityConflict` (same op_id, different bytes) is
        // covered directly against `ScriptedCommitPipeline` in `commit.rs`'s own tests.
        let distinct = pipeline.submit_candidate(&[9, 9, 9], None).await;
        assert!(matches!(distinct, PipelineResult::Committed(_)));
    }

    #[tokio::test]
    async fn the_mock_capability_verifier_rejects_an_expired_chain() {
        let verifier = MockCapabilityVerifier::new();
        let chain_bytes = vec![7, 7, 7];
        verifier.register(
            chain_bytes.clone(),
            VerifiedAuthorization {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0x02; 32]),
                epoch_scope: EnvelopeScope::verse_wide(Identifier32([0x11; 32])),
                scope_epoch: 1,
                expires_at_ms: 1,
            },
        );
        assert!(verifier
            .verify(
                &chain_bytes,
                0,
                0,
                &EnvelopeScope::verse_wide(Identifier32([0x11; 32]))
            )
            .await
            .is_ok());
        verifier.expire(&chain_bytes);
        assert_eq!(
            verifier
                .verify(
                    &chain_bytes,
                    0,
                    0,
                    &EnvelopeScope::verse_wide(Identifier32([0x11; 32]))
                )
                .await,
            Err(WireError::CapabilityNotVerified)
        );
        assert_eq!(verifier.calls().len(), 2);
    }
}
