//! SPEC-8 §4.1, §4.3 dual-emit boundary: wraps — never modifies — the existing
//! `op_log::commit_operation` seam. See `fe-database/src/migration/AGENTS.md`
//! §boundary.

use std::future::Future;

use crate::migration::candidate::{
    CandidateDerivation, CorrelationDisposition, IntentSnapshot, MigrationCandidateSet,
};
use crate::migration::mapping_inventory::record_bypass;
use crate::migration::mode::MigrationMode;
use crate::migration::rebuild::ShadowMaterializer;
use crate::migration::shadow_store::{AppendOutcome, ShadowLedgerEntry, ShadowLedgerWriter};
use crate::op_log;
use crate::repo::Db;
use crate::types::OpLogEntry;

/// Why no shadow candidate covers this legacy attempt (§4.3.1, §5.1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncoveredReason {
    /// `derive()` returned an error before the legacy attempt was made.
    CandidateDerivationFailed,
    /// The shadow ledger's predeclared byte/entry/age bounds were already
    /// exhausted.
    LedgerBoundsExhausted,
}

/// Outcome of one [`commit_operation_dual_emit`] call. The `Err` path (when the
/// legacy mutation itself fails) is never wrapped in this type — it propagates
/// as the exact same `anyhow::Error` `op_log::commit_operation` produced, in
/// every mode, so legacy error text is byte-for-bit identical between
/// `LegacyOnly` and `DualEmitShadow` for the same failing input (§2.1).
#[derive(Debug)]
pub enum DualEmitOutcome<T> {
    /// `LegacyOnly` / `SeamReady`: no candidate was ever attempted or touched.
    LegacyOnlyResult(T),
    /// `DualEmitShadow`/`ShadowRebuild`, legacy succeeded, no candidate captured.
    Uncovered {
        legacy_result: T,
        reason: UncoveredReason,
    },
    /// `DualEmitShadow`/`ShadowRebuild`, legacy succeeded, candidate bytes are
    /// durable in the shadow ledger under `entry_id` with the recorded
    /// `disposition`.
    ShadowCaptured {
        legacy_result: T,
        entry_id: String,
        disposition: CorrelationDisposition,
    },
}

impl<T> DualEmitOutcome<T> {
    /// The plain legacy value every mode still returns to the caller (§3:
    /// user-visible authority is always the legacy SurrealDB projection before
    /// cutover).
    pub fn into_legacy_result(self) -> T {
        match self {
            Self::LegacyOnlyResult(v) => v,
            Self::Uncovered { legacy_result, .. } => legacy_result,
            Self::ShadowCaptured { legacy_result, .. } => legacy_result,
        }
    }
}

/// The future `commit_operation` ingress (§4.1): derives at most one migration
/// candidate set from `intent` alone, strictly before the legacy attempt, then
/// calls the existing UNMODIFIED [`op_log::commit_operation`] for the legacy
/// half. Shadow capture never feeds back into the legacy result (§4.1.6) and
/// never retries the legacy mutation (§4.1.5) — it is attempted exactly once
/// regardless of mode.
#[allow(clippy::too_many_arguments)]
pub async fn commit_operation_dual_emit<T, F, Fut>(
    db: &Db,
    mode: MigrationMode,
    run_id: &str,
    entry: OpLogEntry,
    materialize: F,
    intent: &dyn IntentSnapshot,
    derivation: &dyn CandidateDerivation,
    materializer: &dyn ShadowMaterializer,
    ledger: &dyn ShadowLedgerWriter,
) -> anyhow::Result<DualEmitOutcome<T>>
where
    F: FnOnce(u64) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    match mode {
        MigrationMode::LegacyOnly | MigrationMode::SeamReady => {
            let result = op_log::commit_operation(db, entry, materialize).await?;
            Ok(DualEmitOutcome::LegacyOnlyResult(result))
        }
        MigrationMode::DualEmitShadow | MigrationMode::ShadowRebuild => {
            commit_dual_emit_shadow(
                run_id,
                db,
                entry,
                materialize,
                intent,
                derivation,
                materializer,
                ledger,
            )
            .await
        }
        MigrationMode::CanonicalAuthoritativeLocal => {
            // Owner-gated and unreachable via `MigrationFlags::from_env` in this
            // build (§3 item 4). If ever reached directly, stop before any write
            // rather than silently falling back to divergent legacy authoring
            // (§4.3.6).
            anyhow::bail!(
                "canonical_authoritative_local is owner-gated and unreachable in this build (SPEC-8 §3 item 4)"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn commit_dual_emit_shadow<T, F, Fut>(
    run_id: &str,
    db: &Db,
    entry: OpLogEntry,
    materialize: F,
    intent: &dyn IntentSnapshot,
    derivation: &dyn CandidateDerivation,
    materializer: &dyn ShadowMaterializer,
    ledger: &dyn ShadowLedgerWriter,
) -> anyhow::Result<DualEmitOutcome<T>>
where
    F: FnOnce(u64) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    // §4.1.2, §4.3.1: derive strictly before the legacy attempt, from the
    // intent alone — no DB handle is reachable from `derive()`.
    let candidate = match derivation.derive(intent) {
        Ok(candidate) => candidate,
        Err(derivation_error) => {
            tracing::warn!(
                "migration candidate derivation failed for {}: {derivation_error}",
                intent.mutation_kind()
            );
            record_bypass();
            let legacy_result = op_log::commit_operation(db, entry, materialize).await?;
            return Ok(DualEmitOutcome::Uncovered {
                legacy_result,
                reason: UncoveredReason::CandidateDerivationFailed,
            });
        }
    };

    // §5.1.4: make the candidate bytes durable before the legacy attempt, so a
    // subsequent legacy failure can be recognized as tainting already-durable
    // evidence rather than as a routine miss.
    let append_entry = build_ledger_entry(
        run_id,
        intent,
        &candidate,
        CorrelationDisposition::AppendPending,
    );
    let entry_id = match ledger.append(append_entry).await? {
        AppendOutcome::Exhausted { .. } => {
            let legacy_result = op_log::commit_operation(db, entry, materialize).await?;
            return Ok(DualEmitOutcome::Uncovered {
                legacy_result,
                reason: UncoveredReason::LedgerBoundsExhausted,
            });
        }
        AppendOutcome::Appended { entry_id } => entry_id,
    };

    // §4.1.5: exactly one legacy mutation attempt, regardless of shadow outcome.
    let legacy_result = match op_log::commit_operation(db, entry, materialize).await {
        Ok(value) => value,
        Err(error) => {
            // §4.1.5, §4.3.2: candidate bytes were already durable — taint the
            // run, never retry, never describe the candidate as equivalent to a
            // successful legacy mutation. `error` propagates unmodified so its
            // text is byte-identical to what `LegacyOnly` would have produced.
            let taint_entry = build_ledger_entry(
                run_id,
                intent,
                &candidate,
                CorrelationDisposition::CandidateRejected,
            );
            let _ = ledger.append(taint_entry).await;
            return Err(error);
        }
    };

    // §4.3.3: validate the candidate can be reduced at all. A materializer
    // failure here never undoes the legacy result and never claims a canonical
    // commit to the caller — the returned value is always just the plain
    // legacy result (§3: user-visible authority stays the legacy projection).
    let all_reduced = candidate
        .members
        .iter()
        .all(|member| materializer.reduce(member).is_ok());

    if !all_reduced {
        let pending_entry = build_ledger_entry(
            run_id,
            intent,
            &candidate,
            CorrelationDisposition::MaterializationPending,
        );
        let _ = ledger.append(pending_entry).await;
        return Ok(DualEmitOutcome::ShadowCaptured {
            legacy_result,
            entry_id,
            disposition: CorrelationDisposition::MaterializationPending,
        });
    }

    Ok(DualEmitOutcome::ShadowCaptured {
        legacy_result,
        entry_id,
        disposition: CorrelationDisposition::AppendPending,
    })
}

fn build_ledger_entry(
    run_id: &str,
    intent: &dyn IntentSnapshot,
    candidate: &MigrationCandidateSet,
    disposition: CorrelationDisposition,
) -> ShadowLedgerEntry {
    ShadowLedgerEntry {
        run_id: run_id.to_string(),
        intent_digest_hex: intent.intent_digest_hex(),
        mutation_kind: intent.mutation_kind().to_string(),
        run_local_correlation_id: candidate.run_local_correlation_id.clone(),
        // Member `op_id` assignment belongs to the canonical envelope/append
        // layer, which this scaffolding does not implement — see AGENTS.md
        // §member-op-ids.
        member_op_ids_json: "[]".to_string(),
        candidate_byte_hashes_json: serde_json::to_string(&candidate.candidate_byte_hashes())
            .unwrap_or_else(|_| "[]".to_string()),
        disposition,
    }
}
