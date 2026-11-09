//! Copy-for-BI targets as connector descriptors; the string builders stay in `gis::egress_strings`.

use crate::gis::egress_strings::{self as es, ExportFormat};

/// Everything a snippet builder may read: the API base, the active petal and the scoped SQL.
#[derive(Debug, Clone, Copy)]
pub struct SnippetInputs<'a> {
    pub base: &'a str,
    pub petal_id: &'a str,
    pub sql: &'a str,
}

/// One built-in Copy-for-BI target: a stable id, the row label and a pure snippet builder.
#[derive(Debug, Clone, Copy)]
pub struct ConnectorDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub build: fn(&SnippetInputs) -> String,
}

/// A rendered row for the egress card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetRow {
    pub label: String,
    pub text: String,
    /// False for a package-provided row.
    pub builtin: bool,
}

/// A snippet an `egress.snippet` binding returned (bindings.md §6): text only, shown for copying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSnippet {
    pub label: String,
    pub text: String,
}

/// Longest package-provided snippet shown (bindings.md §6 `egress.snippet`).
const MAX_BOUND_SNIPPET_BYTES: usize = 16 * 1024;

fn sql_text(inputs: &SnippetInputs) -> String {
    inputs.sql.to_string()
}

fn query_endpoint(inputs: &SnippetInputs) -> String {
    es::query_post_url(inputs.base)
}

fn export_parquet(inputs: &SnippetInputs) -> String {
    es::export_url(inputs.base, inputs.petal_id, inputs.sql, ExportFormat::Parquet)
}

fn export_csv(inputs: &SnippetInputs) -> String {
    es::export_url(inputs.base, inputs.petal_id, inputs.sql, ExportFormat::Csv)
}

fn duckdb(inputs: &SnippetInputs) -> String {
    es::duckdb_snippet(&export_parquet(inputs))
}

/// The compiled-in targets in card order; no package list can remove or reorder them (R33).
pub const COPY_FOR_BI_TARGETS: [ConnectorDescriptor; 5] = [
    ConnectorDescriptor { id: "sql", label: "SQL (POST /api/v1/query)", build: sql_text },
    ConnectorDescriptor { id: "query-endpoint", label: "Query endpoint", build: query_endpoint },
    ConnectorDescriptor { id: "export-parquet", label: "Export URL (parquet)", build: export_parquet },
    ConnectorDescriptor { id: "export-csv", label: "Export URL (csv)", build: export_csv },
    ConnectorDescriptor { id: "duckdb", label: "DuckDB snippet", build: duckdb },
];

/// Built-in rows first, then package-provided ones; an empty or oversized package snippet is skipped.
pub fn snippet_rows(inputs: &SnippetInputs, bound: &[BoundSnippet]) -> Vec<SnippetRow> {
    let mut rows: Vec<SnippetRow> = COPY_FOR_BI_TARGETS
        .iter()
        .map(|target| SnippetRow { label: target.label.to_string(), text: (target.build)(inputs), builtin: true })
        .collect();
    rows.extend(
        bound
            .iter()
            .filter(|snippet| !snippet.text.is_empty() && snippet.text.len() <= MAX_BOUND_SNIPPET_BYTES)
            .map(|snippet| SnippetRow { label: snippet.label.clone(), text: snippet.text.clone(), builtin: false }),
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_render_the_card_strings_byte_for_byte_and_built_ins_lead() {
        let sql = es::petal_scope_sql("petal-1");
        let inputs = SnippetInputs { base: "http://localhost:8765/", petal_id: "petal-1", sql: &sql };
        let bound = [
            BoundSnippet { label: "Tableau".into(), text: "tableau://x".into() },
            BoundSnippet { label: "Oversized".into(), text: "x".repeat(MAX_BOUND_SNIPPET_BYTES + 1) },
        ];
        let rows = snippet_rows(&inputs, &bound);

        let parquet = es::export_url(inputs.base, "petal-1", &sql, ExportFormat::Parquet);
        let expected = [
            sql.clone(),
            es::query_post_url(inputs.base),
            parquet.clone(),
            es::export_url(inputs.base, "petal-1", &sql, ExportFormat::Csv),
            es::duckdb_snippet(&parquet),
        ];
        let built_in: Vec<&str> = rows.iter().filter(|row| row.builtin).map(|row| row.text.as_str()).collect();
        assert_eq!(built_in, expected.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(rows[..5].iter().all(|row| row.builtin), "built-ins render first");
        assert_eq!(rows.len(), 6, "the oversized package snippet is dropped");
        assert_eq!(rows[5], SnippetRow { label: "Tableau".into(), text: "tableau://x".into(), builtin: false });
    }
}
