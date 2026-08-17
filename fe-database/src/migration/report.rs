//! SPEC-8 §6.1 measurement report: every measurement carries its numerator AND
//! denominator (§6.1.3) — a bare percentage is never sufficient evidence. See
//! `fe-database/src/migration/AGENTS.md` §report and §always-blocked.

use crate::migration::mapping_inventory::MappingInventory;

/// One §6.1 measurement row: a fraction, never a bare percentage (§6.1.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub name: &'static str,
    pub numerator: u64,
    pub denominator: u64,
    /// Whether this measurement currently meets its §6.1 "required value for
    /// promotion".
    pub meets_promotion_requirement: bool,
}

impl Measurement {
    /// Convenience percentage view — `None` when the denominator is 0 (no
    /// events observed yet), since a percentage without its denominator is not
    /// evidence (§6.1.3).
    pub fn percentage(&self) -> Option<f64> {
        if self.denominator == 0 {
            None
        } else {
            Some(100.0 * self.numerator as f64 / self.denominator as f64)
        }
    }
}

/// Raw counters a pinned validation run accumulates (§6.1.1-§6.1.2). This
/// scaffolding provides the shape only — a real run charter supplies real
/// counts from an owner-approved run.
#[derive(Debug, Clone, Default)]
pub struct RunCounters {
    pub legacy_success: u64,
    pub legacy_success_with_candidate: u64,
    pub candidates_retained: u64,
    pub candidates_decoded_reencoded_ok: u64,
    pub candidates_expected: u64,
    pub candidates_admitted_or_dispositioned: u64,
    pub unapproved_field_differences: u64,
    pub replay_root_matches: u64,
    pub replay_attempts: u64,
    pub interruption_points_planned: u64,
    pub interruption_points_resolved_cleanly: u64,
    pub unauthorized_visibility_events: u64,
    pub unbounded_or_unaccounted_stores: u64,
    pub network_prohibited_events_observed: u64,
}

/// The full §6.1 measurement table for one pinned validation run.
#[derive(Debug, Clone)]
pub struct MeasurementReport {
    pub measurements: Vec<Measurement>,
}

impl MeasurementReport {
    /// Build the ten §6.1 rows from raw counters plus the current mapping
    /// inventory (§4.2.5, §6.1 "Ingress coverage").
    pub fn generate(
        counters: &RunCounters,
        inventory: &MappingInventory,
        unmapped_bypass_count: u64,
    ) -> Self {
        let measurements = vec![
            Measurement {
                name: "capture_completeness",
                numerator: counters.legacy_success_with_candidate,
                denominator: counters.legacy_success,
                meets_promotion_requirement: counters.legacy_success > 0
                    && counters.legacy_success_with_candidate == counters.legacy_success,
            },
            Measurement {
                name: "ingress_coverage",
                numerator: unmapped_bypass_count,
                denominator: 1,
                meets_promotion_requirement: unmapped_bypass_count == 0
                    && inventory.unmapped_count() == 0,
            },
            Measurement {
                name: "candidate_integrity",
                numerator: counters.candidates_decoded_reencoded_ok,
                denominator: counters.candidates_retained,
                meets_promotion_requirement: counters.candidates_retained > 0
                    && counters.candidates_decoded_reencoded_ok == counters.candidates_retained,
            },
            Measurement {
                name: "admission_and_replay_completeness",
                numerator: counters.candidates_admitted_or_dispositioned,
                denominator: counters.candidates_expected,
                meets_promotion_requirement: counters.candidates_expected > 0
                    && counters.candidates_admitted_or_dispositioned
                        == counters.candidates_expected,
            },
            Measurement {
                name: "projection_parity",
                numerator: counters.unapproved_field_differences,
                denominator: 1,
                meets_promotion_requirement: counters.unapproved_field_differences == 0,
            },
            Measurement {
                name: "determinism",
                numerator: counters.replay_root_matches,
                denominator: counters.replay_attempts,
                meets_promotion_requirement: counters.replay_attempts > 0
                    && counters.replay_root_matches == counters.replay_attempts,
            },
            Measurement {
                name: "crash_recovery",
                numerator: counters.interruption_points_resolved_cleanly,
                denominator: counters.interruption_points_planned,
                meets_promotion_requirement: counters.interruption_points_planned > 0
                    && counters.interruption_points_resolved_cleanly
                        == counters.interruption_points_planned,
            },
            Measurement {
                name: "authorization_and_privacy",
                numerator: counters.unauthorized_visibility_events,
                denominator: 1,
                meets_promotion_requirement: counters.unauthorized_visibility_events == 0,
            },
            Measurement {
                name: "local_editor_service",
                numerator: 0,
                denominator: 0,
                // No owner-approved local interaction latency/error-rate budget
                // has been declared yet for this build — §6.1's run charter
                // prerequisite is not satisfied, so this measurement can never
                // report a promotable value here (§6.1.1).
                meets_promotion_requirement: false,
            },
            Measurement {
                name: "storage_accounting",
                numerator: counters.unbounded_or_unaccounted_stores,
                denominator: 1,
                meets_promotion_requirement: counters.unbounded_or_unaccounted_stores == 0,
            },
            Measurement {
                name: "network_prohibition",
                numerator: counters.network_prohibited_events_observed,
                denominator: 1,
                meets_promotion_requirement: counters.network_prohibited_events_observed == 0,
            },
        ];

        Self { measurements }
    }

    /// True whenever any unmapped bypass exists or any measurement fails its
    /// promotion requirement. Correctly always true today: `ingress_coverage`
    /// can never pass while any D-CL20 kind is permanently unmapped (§4.2.5),
    /// and `local_editor_service` can never pass without an owner-approved
    /// budget this scaffolding does not define (§7 gate 4). Neither of these
    /// is a bug to work around — they are the SPEC-8 gates doing their job.
    pub fn blocks_promotion(&self, unmapped_bypass_count: u64) -> bool {
        unmapped_bypass_count > 0
            || self
                .measurements
                .iter()
                .any(|measurement| !measurement.meets_promotion_requirement)
    }
}
