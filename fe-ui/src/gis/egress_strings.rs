//! Pure egress string builders for the GIS panel's "Copy for BI" card.
//! No egui, no I/O — see `fe-ui/src/panels/AGENTS.md` §egress-card.

/// Default fe-api base URL (matches the inspector's curl precedent).
pub(crate) const DEFAULT_API_BASE: &str = "http://localhost:8765";
/// Default shareable-link TTL in seconds (plan D2: default 1h, max 24h).
pub(crate) const DEFAULT_SHARE_TTL_SECS: u64 = 3600;

/// Validates the exact API share lifetime contract without coercion.
pub(crate) fn parse_share_ttl(value: &str) -> Result<u64, &'static str> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|ttl| (1..=86_400).contains(ttl))
        .ok_or("Enter a whole number of seconds from 1 to 86400.")
}

/// Which spatial context the egress SQL is scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EgressSource {
    #[default]
    Petal,
    Node,
    Bbox,
    /// iot_reading rows of the petal, pinned to a snapshot time.
    Readings,
}

/// Export download format (plan Phase 2 routes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportFormat {
    Parquet,
    Csv,
}

impl ExportFormat {
    /// Route filename segment: `export.parquet` / `export.csv`.
    pub(crate) fn route_name(self) -> &'static str {
        match self {
            ExportFormat::Parquet => "export.parquet",
            ExportFormat::Csv => "export.csv",
        }
    }
    /// Share-body `format` field value.
    pub(crate) fn body_name(self) -> &'static str {
        match self {
            ExportFormat::Parquet => "parquet",
            ExportFormat::Csv => "csv",
        }
    }
}

/// Panel-local state for the egress card (never reads `NodeManager.selected`
/// implicitly — see project memory: track-selection-two-concepts).
pub(crate) struct EgressCardState {
    pub source: EgressSource,
    pub base_url_buf: String,
    /// Node id for `EgressSource::Node`, filled by an explicit user action.
    pub node_id_buf: String,
    pub ttl_buf: String,
    /// Snapshot cutoff (epoch ms) for `EgressSource::Readings`; set when Readings is chosen or refreshed.
    pub readings_as_of_ms: Option<i64>,
}

impl Default for EgressCardState {
    fn default() -> Self {
        Self {
            source: EgressSource::default(),
            base_url_buf: DEFAULT_API_BASE.to_string(),
            node_id_buf: String::new(),
            ttl_buf: DEFAULT_SHARE_TTL_SECS.to_string(),
            readings_as_of_ms: None,
        }
    }
}

/// Escapes a SurrealQL single-quoted string literal (backslash escapes, as `query_tab::sql_literal`).
fn escape_sql_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Default petal-scope node SELECT; `tombstone = NONE` hides soft-deleted rows like REST.
pub(crate) fn petal_scope_sql(petal_id: &str) -> String {
    format!(
        "SELECT * FROM node WHERE petal_id = '{}' AND tombstone = NONE",
        escape_sql_str(petal_id)
    )
}

/// Single-node SELECT within a petal.
pub(crate) fn node_selection_sql(petal_id: &str, node_id: &str) -> String {
    format!(
        "SELECT * FROM node WHERE petal_id = '{}' AND node_id = '{}' AND tombstone = NONE",
        escape_sql_str(petal_id),
        escape_sql_str(node_id),
    )
}

/// Node-only SELECT (no petal context) — the read a copy-API/report string
/// round-trips to (FR-5). `tombstone = NONE` hides soft-deleted rows.
pub(crate) fn node_by_id_sql(node_id: &str) -> String {
    format!(
        "SELECT * FROM node WHERE node_id = '{}' AND tombstone = NONE",
        escape_sql_str(node_id),
    )
}

/// `GET /api/v1/nodes/:node_id` read-endpoint URL (T5 FR-2 per-object read).
pub(crate) fn node_read_url(base: &str, node_id: &str) -> String {
    format!(
        "{}/api/v1/nodes/{}",
        trim_base(base),
        percent_encode(node_id)
    )
}

/// Bbox-scoped SELECT over the local XZ plane; tolerates reversed min/max.
/// Coordinate index contract: `position.coordinates = [x, z]` (see
/// `gis::query::extract_xz`).
pub(crate) fn bbox_sql(petal_id: &str, min: [f32; 2], max: [f32; 2]) -> String {
    let (min_x, max_x) = (min[0].min(max[0]), min[0].max(max[0]));
    let (min_z, max_z) = (min[1].min(max[1]), min[1].max(max[1]));
    format!(
        "SELECT * FROM node WHERE petal_id = '{}' \
         AND position.coordinates[0] >= {min_x} AND position.coordinates[0] <= {max_x} \
         AND position.coordinates[1] >= {min_z} AND position.coordinates[1] <= {max_z} \
         AND tombstone = NONE",
        escape_sql_str(petal_id),
    )
}

/// Snapshot-pinned, deterministically ordered reading SELECT (the export sorts the same way).
pub(crate) fn readings_sql(petal_id: &str, as_of_ms: i64) -> String {
    format!(
        "SELECT * FROM iot_reading WHERE petal_id = '{}' AND recorded_at_ms <= {as_of_ms} \
         ORDER BY recorded_at_ms, reading_id",
        escape_sql_str(petal_id),
    )
}

/// Readings export URL: the plain export URL plus `&as_of_ms=<n>` so the body (and ETag) stays fixed.
pub(crate) fn readings_export_url(
    base: &str,
    petal_id: &str,
    as_of_ms: i64,
    format: ExportFormat,
) -> String {
    format!(
        "{}&as_of_ms={as_of_ms}",
        export_url(base, petal_id, &readings_sql(petal_id, as_of_ms), format)
    )
}

/// RFC-3986 percent-encoding: everything but unreserved chars is `%XX`.
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Base URL with any trailing `/` removed.
fn trim_base(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// `POST /api/v1/query` endpoint URL (JSON body carries the SQL).
pub(crate) fn query_post_url(base: &str) -> String {
    format!("{}/api/v1/query", trim_base(base))
}

/// Export GET URL per plan Task 2.3/2.4:
/// `GET /api/v1/petals/:petal_id/<export.ext>?query=<urlencoded sql>`.
pub(crate) fn export_url(base: &str, petal_id: &str, sql: &str, format: ExportFormat) -> String {
    format!(
        "{}/api/v1/petals/{}/{}?query={}",
        trim_base(base),
        percent_encode(petal_id),
        format.route_name(),
        percent_encode(sql),
    )
}

/// DuckDB httpfs connection snippet over the parquet export URL (plan D1).
pub(crate) fn duckdb_snippet(parquet_export_url: &str) -> String {
    format!("SELECT * FROM read_parquet('{parquet_export_url}');")
}

/// Minimal JSON string escape (`\`, `"`, and common control chars).
fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// curl command minting a shareable signed URL via `POST /api/v1/query/share`
/// (plan Task 3.2 contract: body = sql + format + ttl). Displayed for manual
/// use until fe-ui grows an HTTP-client seam — see track open_decisions.
pub(crate) fn mint_share_curl(
    base: &str,
    sql: &str,
    format: ExportFormat,
    ttl_secs: u64,
) -> String {
    let body = format!(
        r#"{{"sql":"{}","format":"{}","ttl_secs":{}}}"#,
        escape_json_str(sql),
        format.body_name(),
        ttl_secs,
    );
    format!(
        "curl -s -X POST {}/api/v1/query/share -H \"Authorization: Bearer <YOUR_API_TOKEN>\" -H \"Content-Type: application/json\" -d \"{}\"",
        trim_base(base),
        body.replace('\\', "\\\\").replace('"', "\\\""),
    )
}

// ---------------------------------------------------------------------------
// FR-5 seam: per-object copy-API-string / report (called by T4's context menu)
// ---------------------------------------------------------------------------

/// A node id is "addressable" for egress purposes if it is non-empty once
/// trimmed. (The full `fe://verse/fractal/petal/node` URI additionally needs
/// the scope, which is resolved server-side by T5's `/nodes/:id/address`; this
/// pure seam has only the node id, so it emits the node-scoped SQL + REST URL.)
fn addressable_id(node_id: &str) -> Option<&str> {
    let t = node_id.trim();
    (!t.is_empty()).then_some(t)
}

/// Copy-paste API/SQL string for a single object (FR-5). Pure formatting — no
/// I/O, no `block_on` (N-5). Returns the node-scoped SurrealQL SELECT the
/// analyst/agent pastes into an external tool; `None` if `node_id` is empty
/// (not addressable). The string round-trips: pasting the SQL reads the same
/// node. Called by T4's copy-API-string context-menu verb.
pub(crate) fn api_string_for(node_id: &str) -> Option<String> {
    let id = addressable_id(node_id)?;
    Some(node_by_id_sql(id))
}

/// Human-readable report string for a single object (FR-5). Pure formatting.
/// Returns a concise multi-line report with the node's read endpoint and the
/// query that returns its live data; `None` if `node_id` is not reportable
/// (empty). Called by T4's report context-menu verb.
pub(crate) fn report_for(node_id: &str) -> Option<String> {
    let id = addressable_id(node_id)?;
    Some(format!(
        "Node {id}\n  read:  GET {}\n  query: {}",
        node_read_url(DEFAULT_API_BASE, id),
        node_by_id_sql(id),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn petal_scope_sql_exact() {
        assert_eq!(
            petal_scope_sql("petal-1"),
            "SELECT * FROM node WHERE petal_id = 'petal-1' AND tombstone = NONE"
        );
    }

    #[test]
    fn petal_scope_sql_escapes_single_quote() {
        assert_eq!(
            petal_scope_sql("pe'tal"),
            "SELECT * FROM node WHERE petal_id = 'pe\\'tal' AND tombstone = NONE"
        );
    }

    #[test]
    fn node_selection_sql_exact() {
        assert_eq!(
            node_selection_sql("petal-1", "node-9"),
            "SELECT * FROM node WHERE petal_id = 'petal-1' AND node_id = 'node-9' AND tombstone = NONE",
        );
    }

    #[test]
    fn bbox_sql_exact_and_normalizes_reversed_bounds() {
        let expect = "SELECT * FROM node WHERE petal_id = 'petal-1' \
                      AND position.coordinates[0] >= -50 AND position.coordinates[0] <= 50 \
                      AND position.coordinates[1] >= -50 AND position.coordinates[1] <= 50 \
                      AND tombstone = NONE";
        assert_eq!(bbox_sql("petal-1", [-50.0, -50.0], [50.0, 50.0]), expect);
        assert_eq!(
            bbox_sql("petal-1", [50.0, 50.0], [-50.0, -50.0]),
            expect,
            "reversed bounds normalize"
        );
    }

    #[test]
    fn percent_encode_reserved_and_unreserved() {
        assert_eq!(percent_encode("a-Z0._~"), "a-Z0._~");
        assert_eq!(percent_encode("* ='/"), "%2A%20%3D%27%2F");
    }

    #[test]
    fn query_post_url_trims_trailing_slash() {
        assert_eq!(
            query_post_url("http://localhost:8765/"),
            "http://localhost:8765/api/v1/query"
        );
    }

    #[test]
    fn export_url_parquet_encodes_query_param() {
        let sql = petal_scope_sql("petal-1");
        assert_eq!(
            export_url(DEFAULT_API_BASE, "petal-1", &sql, ExportFormat::Parquet),
            "http://localhost:8765/api/v1/petals/petal-1/export.parquet?query=\
             SELECT%20%2A%20FROM%20node%20WHERE%20petal_id%20%3D%20%27petal-1%27%20AND%20tombstone%20%3D%20NONE",
        );
    }

    #[test]
    fn export_url_csv_variant_and_encoded_petal_segment() {
        let url = export_url(
            DEFAULT_API_BASE,
            "petal 1",
            "SELECT * FROM node",
            ExportFormat::Csv,
        );
        assert!(
            url.contains("/api/v1/petals/petal%201/export.csv?query="),
            "got {url}"
        );
    }

    #[test]
    fn duckdb_snippet_wraps_url() {
        assert_eq!(
            duckdb_snippet("http://h/api/v1/petals/p/export.parquet?query=x"),
            "SELECT * FROM read_parquet('http://h/api/v1/petals/p/export.parquet?query=x');",
        );
        // Readings leg: SQL and export URL pin the snapshot.
        assert_eq!(
            readings_sql("p'1", 1700),
            "SELECT * FROM iot_reading WHERE petal_id = 'p\\'1' AND recorded_at_ms <= 1700 \
             ORDER BY recorded_at_ms, reading_id",
        );
        let url = readings_export_url(DEFAULT_API_BASE, "p1", 1700, ExportFormat::Parquet);
        assert!(url.contains("/petals/p1/export.parquet?query=SELECT%20%2A%20FROM%20iot_reading"));
        assert!(url.ends_with("&as_of_ms=1700"), "got {url}");
        assert_eq!(
            duckdb_snippet(&url),
            format!("SELECT * FROM read_parquet('{url}');"),
        );
    }

    #[test]
    fn mint_share_curl_exact() {
        for (input, expected) in [
            ("1", Some(1)),
            (" 3600 ", Some(3600)),
            ("86400", Some(86400)),
            ("", None),
            ("0", None),
            ("-1", None),
            ("1.5", None),
            ("abc", None),
            ("86401", None),
            ("18446744073709551616", None),
        ] {
            assert_eq!(parse_share_ttl(input).ok(), expected, "{input}");
        }
        let cmd = mint_share_curl(
            DEFAULT_API_BASE,
            "SELECT * FROM node WHERE petal_id = 'p'",
            ExportFormat::Parquet,
            3600,
        );
        assert_eq!(
            cmd,
            "curl -s -X POST http://localhost:8765/api/v1/query/share \
             -H \"Authorization: Bearer <YOUR_API_TOKEN>\" -H \"Content-Type: application/json\" \
             -d \"{\\\"sql\\\":\\\"SELECT * FROM node WHERE petal_id = 'p'\\\",\\\"format\\\":\\\"parquet\\\",\\\"ttl_secs\\\":3600}\"",
        );
    }

    #[test]
    fn default_state_uses_petal_source_and_default_base() {
        let s = EgressCardState::default();
        assert_eq!(s.source, EgressSource::Petal);
        assert_eq!(s.base_url_buf, DEFAULT_API_BASE);
        assert_eq!(s.ttl_buf, "3600");
        assert!(s.node_id_buf.is_empty());
    }

    #[test]
    fn api_string_for_returns_node_scoped_sql() {
        assert_eq!(
            api_string_for("node-9").as_deref(),
            Some("SELECT * FROM node WHERE node_id = 'node-9' AND tombstone = NONE"),
        );
        pasted_string_reads_same_node_then_nothing_after_tombstone();
    }

    /// FR-5 + QA-0127: the pasted string reads the node through the real DB thread, nothing once tombstoned.
    fn pasted_string_reads_same_node_then_nothing_after_tombstone() {
        use fe_runtime::messages::{CallerAuth, DbCommand, DbResult};
        use std::time::Duration;
        const TIMEOUT: Duration = Duration::from_secs(20);
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("paste_rt.db").to_string_lossy().to_string();
        let blob_store: fe_database::BlobStoreHandle =
            std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let thread = fe_database::spawn_db_thread_shared(
            cmd_rx, res_tx, blob_store, None, None, None, None, Some(db_path), None,
        )
        .expect("spawn db thread");
        let recv = || res_rx.recv_timeout(TIMEOUT).expect("db result within timeout");
        assert!(matches!(recv(), DbResult::Started));

        cmd_tx.send(DbCommand::CreateVerse { name: "V".into() }).unwrap();
        let DbResult::VerseCreated { id: verse_id, .. } = recv() else { panic!("VerseCreated") };
        cmd_tx.send(DbCommand::CreateFractal { verse_id, name: "F".into() }).unwrap();
        let DbResult::FractalCreated { id: fractal_id, .. } = recv() else { panic!("FractalCreated") };
        cmd_tx.send(DbCommand::CreatePetal { fractal_id, name: "P".into() }).unwrap();
        let DbResult::PetalCreated { id: petal_id, .. } = recv() else { panic!("PetalCreated") };
        cmd_tx
            .send(DbCommand::CreateNode {
                petal_id: petal_id.clone(),
                name: "Pasted".into(),
                position: [0.0, 0.0, 0.0],
                correlation_id: None,
            })
            .unwrap();
        let DbResult::NodeCreated { id: node_id, .. } = recv() else { panic!("NodeCreated") };
        cmd_tx
            .send(DbCommand::SetNodeProperty {
                node_id: node_id.clone(),
                key: "label".into(),
                value: serde_json::json!("pasted"),
                correlation_id: None,
            })
            .unwrap();
        assert!(matches!(recv(), DbResult::NodePropertySet { .. }));

        let pasted = api_string_for(&node_id).expect("api string");
        let run_pasted = || {
            cmd_tx
                .send(DbCommand::ScopedQuery {
                    sql: pasted.clone(),
                    vars: Default::default(),
                    petal_scope: vec![petal_id.clone()],
                    correlation_id: "paste-read".into(),
                })
                .unwrap();
            match recv() {
                DbResult::QueryResult { data, .. } => data,
                other => panic!("expected QueryResult, got {other:?}"),
            }
        };
        let rows = run_pasted();
        assert_eq!(rows.len(), 1, "pasted string reads the node: {pasted}");
        assert_eq!(rows[0]["node_id"], node_id);
        cmd_tx
            .send(DbCommand::GetNodeProperties { node_id: node_id.clone(), correlation_id: None })
            .unwrap();
        let DbResult::NodePropertiesLoaded { properties, .. } = recv() else { panic!("NodePropertiesLoaded") };
        assert_eq!(rows[0]["properties"], properties, "pasted read matches GetNodeProperties");

        cmd_tx
            .send(DbCommand::TombstoneNode { node_id: node_id.clone(), auth: CallerAuth::Local })
            .unwrap();
        assert!(matches!(recv(), DbResult::NodeDeleted { .. }));
        assert!(run_pasted().is_empty(), "tombstoned node hidden from the pasted read");

        drop(cmd_tx); // close the channel so the DB loop exits before the temp dir drops
        let _ = thread.join_handle.join();
    }

    #[test]
    fn api_string_for_escapes_and_rejects_empty() {
        assert_eq!(
            api_string_for("n'x").as_deref(),
            Some("SELECT * FROM node WHERE node_id = 'n\\'x' AND tombstone = NONE"),
        );
        assert_eq!(api_string_for("").as_deref(), None);
        assert_eq!(
            api_string_for("   ").as_deref(),
            None,
            "whitespace-only not addressable"
        );
    }

    #[test]
    fn report_for_contains_endpoint_and_query_and_rejects_empty() {
        let r = report_for("node-9").expect("reportable");
        assert!(r.contains("Node node-9"), "{r}");
        assert!(r.contains("/api/v1/nodes/node-9"), "{r}");
        assert!(r.contains("node_id = 'node-9'"), "{r}");
        assert_eq!(report_for(""), None);
    }
}
