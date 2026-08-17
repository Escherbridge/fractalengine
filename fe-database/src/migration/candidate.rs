//! SPEC-8 candidate model (§2.3, §4.2, §5.1): a migration candidate is derived
//! ONLY from the original intent — never from an already-mutated row. See
//! `fe-database/src/migration/AGENTS.md` §candidate-derivation.

/// A caller's original intent, exposed only as what SPEC-8 permits a candidate to
/// be derived from: its mutation kind and a content digest. Implementors MUST NOT
/// expose database state, generated IDs, or old row values through this trait
/// (§2.3) — [`CandidateDerivation::derive`] takes only `&dyn IntentSnapshot`.
pub trait IntentSnapshot {
    /// The `DbCommand` mutation kind name (matches `mapping_inventory` keys).
    fn mutation_kind(&self) -> &str;
    /// BLAKE3 digest of the intent's own fields — never a DB-generated value.
    fn intent_digest(&self) -> [u8; 32];

    /// Hex-encoded [`Self::intent_digest`] convenience accessor.
    fn intent_digest_hex(&self) -> String {
        hex::encode(self.intent_digest())
    }
}

/// One scope-local member of a migration candidate set (§4.1.4, §4.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationCandidateMember {
    /// The scope this member belongs to (`VERSE#..-FRACTAL#..-PETAL#..` or narrower).
    pub scope: String,
    /// Name of the registered canonical intent schema this member conforms to.
    pub canonical_intent_schema_name: String,
    /// Opaque encoded envelope bytes. Never decoded or interpreted in this crate
    /// (§2.3, §4.2.3) — the canonical envelope encoding lives outside this slice.
    pub envelope_bytes: Vec<u8>,
    /// `op_id`s of required parent operations, in the deterministic order §4.1.4
    /// requires.
    pub required_parent_op_ids: Vec<String>,
}

/// The complete, deterministic set of candidate members derived from one intent
/// (§1.3, §4.1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationCandidateSet {
    /// Diagnostic-only correlation id — no canonical, branch, cursor, or
    /// content-address meaning (§5.1.3). The member `op_id`s are the canonical
    /// evidence, not this id.
    pub run_local_correlation_id: String,
    pub members: Vec<MigrationCandidateMember>,
}

impl MigrationCandidateSet {
    /// BLAKE3 digest over every member's opaque bytes, in member order. Used only
    /// to identify retained candidate bytes for the shadow ledger; never
    /// interpreted as content (§5.1.4).
    pub fn candidate_byte_hashes(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|member| hex::encode(blake3::hash(&member.envelope_bytes).as_bytes()))
            .collect()
    }
}

/// The seven-state correlation disposition (§5.1.5) — never collapsed into a
/// boolean success metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationDisposition {
    LegacySucceededWithoutCandidate,
    CandidateRejected,
    AppendPending,
    MaterializationPending,
    ComparedEqual,
    ComparedDifferent,
    OutcomeUnknown,
}

impl CorrelationDisposition {
    /// The exact lower_snake_case token this disposition serializes to (§5.1.5).
    pub fn as_token(self) -> &'static str {
        match self {
            Self::LegacySucceededWithoutCandidate => "legacy_succeeded_without_candidate",
            Self::CandidateRejected => "candidate_rejected",
            Self::AppendPending => "append_pending",
            Self::MaterializationPending => "materialization_pending",
            Self::ComparedEqual => "compared_equal",
            Self::ComparedDifferent => "compared_different",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

impl std::fmt::Display for CorrelationDisposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_token())
    }
}

/// Typed failure when deriving a candidate set from an intent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CandidateDerivationError {
    /// `mutation_kind` has no complete canonical mapping yet (§4.2.4) — the
    /// boundary must still run the legacy mutation and count this as uncovered.
    #[error("{mutation_kind} has no complete canonical mapping yet (SPEC-8 §4.2.4)")]
    Unmapped { mutation_kind: String },
    /// The intent mapped, but this particular candidate could not be derived.
    #[error("candidate derivation failed for {mutation_kind}: {reason}")]
    Invalid {
        mutation_kind: String,
        reason: String,
    },
}

/// Derives a [`MigrationCandidateSet`] from ONLY the original intent (§2.3,
/// §4.1.2). The signature itself is the enforcement mechanism: no DB handle and
/// no mutated-row access are reachable from this call, so a reverse-built
/// candidate is structurally impossible, not merely forbidden by convention.
pub trait CandidateDerivation {
    fn derive(
        &self,
        intent: &dyn IntentSnapshot,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError>;
}
