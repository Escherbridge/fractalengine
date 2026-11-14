//! Fixed transaction diagnostics shared by scene reads and publication; see AGENTS.md.

use std::collections::HashMap;
use surrealdb::types::{ErrorDetails, QueryError};

#[derive(Clone, Copy)]
pub(super) enum Transaction {
    CurrentRead,
    Publication,
}

impl Transaction {
    fn label(self) -> &'static str {
        match self {
            Self::CurrentRead => "scene current read transaction",
            Self::Publication => "scene publication transaction",
        }
    }

    pub(super) fn request_failed(self, error: surrealdb::Error) -> anyhow::Error {
        anyhow::anyhow!("{} request_failed kind {}", self.label(), error.kind_str())
    }

    pub(super) fn failed(
        self,
        errors: HashMap<usize, surrealdb::Error>,
        guard_labels: &[&'static str],
    ) -> anyhow::Error {
        let mut ordered: Vec<_> = errors.into_iter().collect();
        ordered.sort_by_key(|(index, _)| *index);
        let selected = ordered
            .iter()
            .find(|(_, error)| {
                !matches!(
                    error.details(),
                    ErrorDetails::Query(Some(QueryError::NotExecuted))
                )
            })
            .or_else(|| ordered.first());
        let Some((index, error)) = selected else {
            return anyhow::anyhow!("{} error_unavailable", self.label());
        };
        let stage = if *index == 0 {
            "begin"
        } else if let Some(label) = guard_labels.get(*index - 1) {
            *label
        } else if *index == guard_labels.len() + 1 {
            "commit"
        } else {
            "unexpected_statement"
        };
        let message = error
            .message()
            .strip_prefix("An error occurred: ")
            .unwrap_or(error.message());
        let reason = if matches!(error.details(), ErrorDetails::Thrown) {
            match message {
                "hierarchy authority changed" => "authority_changed",
                "ambiguous hierarchy identity" => "ancestry_ambiguous",
                "hierarchy ancestry changed" => "ancestry_changed",
                "replica current row changed" | "committed scene row changed" => "node_changed",
                "replica current point or ancestry changed"
                | "committed scene ancestry or point changed" => "point_or_ancestry_changed",
                "local scene journal changed" => "journal_changed",
                "local tombstone proof changed" => "tombstone_proof_changed",
                "scene version changed" => "scene_clock_changed",
                "field clock changed" => "field_clock_changed",
                "own field versions changed" => "own_versions_changed",
                "replica current asset metadata changed" | "scene asset changed" => {
                    "asset_metadata_changed"
                }
                _ => "unrecognized_database_error",
            }
        } else {
            match error.details() {
                ErrorDetails::Query(Some(QueryError::NotExecuted)) => "transaction_aborted",
                ErrorDetails::Query(Some(QueryError::TimedOut { .. })) => "query_timed_out",
                ErrorDetails::Query(Some(QueryError::Cancelled)) => "query_cancelled",
                ErrorDetails::Query(Some(QueryError::TransactionConflict)) => "transaction_conflict",
                _ => "unrecognized_database_error",
            }
        };
        anyhow::anyhow!("{} statement {index} stage {stage} kind {} reason {reason} errors {}", self.label(), error.kind_str(), ordered.len())
    }
}
