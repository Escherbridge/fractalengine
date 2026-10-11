//! Full-router integration tests for /query, GIS, and export endpoints via the
//! API harness (fe-test-harness/src/AGENTS.md §api-harness).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fractalengine_test_harness::api::{body_bytes, ApiHarness};
use serde_json::json;

/// GET `path` with a Bearer token, returning (status, content-type, raw body).
async fn get_raw(h: &ApiHarness, path: &str, token: &str) -> (StatusCode, String, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("build request");
    let resp = h.request(req).await;
    let status = resp.status();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, ctype, body_bytes(resp).await)
}

/// POST a SurrealQL string to /api/v1/query.
async fn query(
    h: &ApiHarness,
    token: &str,
    sql: &str,
    vars: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    h.post_json(
        "/api/v1/query",
        Some(token),
        &json!({ "sql": sql, "vars": vars }),
    )
    .await
}

// ---------------------------------------------------------------------------
// (a) /query happy path + single-SELECT enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_happy_path_and_single_select_enforced() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let n1 = h
        .seed_node(&hier.petal_id, "Node One", [1.0, 2.0, 3.0], None)
        .await
        .expect("seed n1");
    let n2 = h
        .seed_node(&hier.petal_id, "Node Two", [4.0, 5.0, 6.0], None)
        .await
        .expect("seed n2");
    let token = h.mint_token(&hier.verse_scope(), "viewer");

    // Seeded nodes come back through the ApiResponse envelope (+ FR-5 CRS stamp).
    let (status, body) = query(
        &h,
        &token,
        "SELECT * FROM node WHERE petal_id = $pid",
        json!({ "pid": hier.petal_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true, "{body}");
    let rows = body["data"]["data"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["node_id"].as_str()).collect();
    assert!(
        ids.contains(&n1.as_str()) && ids.contains(&n2.as_str()),
        "{ids:?}"
    );
    assert!(body["data"]["crs"].is_string(), "CRS stamp missing: {body}");

    // Second statement (semicolon chaining) rejected, not partially executed.
    let (status, body) = query(
        &h,
        &token,
        "SELECT * FROM node; SELECT * FROM verse",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // guard errors ride the JSON envelope
    assert_eq!(body["ok"], false);
    assert_eq!(
        body["error"],
        "semicolons are not allowed (single statement only)"
    );

    // Non-SELECT rejected.
    let (_, body) = query(&h, &token, "UPDATE node SET display_name = 'x'", json!({})).await;
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "only SELECT statements are allowed");
}

// ---------------------------------------------------------------------------
// (b) injection-guard rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_injection_guard_rejections() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&hier.verse_scope(), "viewer");

    // (expected-substring, injection attempt)
    for (needle, sql) in [
        (
            "DELETE keyword is not allowed",
            "SELECT * FROM node WHERE x = (DELETE node)",
        ),
        ("not allowed", "SELECT * FROM secrets"),
        // RBAC tables are excluded from the BI egress whitelist.
        ("not allowed", "SELECT * FROM verse_member"),
        // Subquery FROM targets are whitelist-checked too.
        (
            "not allowed",
            "SELECT * FROM node WHERE id IN (SELECT id FROM session_cache)",
        ),
        // Non-identifier FROM targets cannot be whitelist-checked.
        ("unsupported FROM target", "SELECT * FROM $tbl"),
    ] {
        let (status, body) = query(&h, &token, sql, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], false, "should reject: {sql}");
        let err = body["error"].as_str().unwrap_or_default();
        assert!(err.contains(needle), "sql={sql} err={err}");
    }
}

// ---------------------------------------------------------------------------
// (c) RBAC negatives
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rbac_insufficient_role_denied() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");

    // Role "none" on /query: viewer floor enforced (error envelope, no rows).
    let none_token = h.mint_token(&hier.verse_scope(), "none");
    let (status, body) = query(&h, &none_token, "SELECT * FROM node", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "insufficient permissions");

    // Write route: viewer cannot create a verse (manager floor, checked
    // before the crossbeam send so it errors instead of hanging).
    let viewer = h.mint_token(&hier.verse_scope(), "viewer");
    let (status, body) = h
        .post_json("/api/v1/verses", Some(&viewer), &json!({ "name": "Nope" }))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "insufficient permissions");

    // Scoped read route: insufficient role is 403; a foreign scope is the
    // unknown-petal 404 (§existence-oracle).
    let csv_path = format!("/api/v1/petals/{}/export.csv", hier.petal_id);
    let (status, _, _) = get_raw(&h, &csv_path, &none_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let foreign = h.mint_token("VERSE#someone-else", "viewer");
    let (status, _, _) = get_raw(&h, &csv_path, &foreign).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// (c2) DEC-C21: REST writes anchor on the DB-resolved target (direct reader)
// ---------------------------------------------------------------------------

/// The REST decoy wart over the real router + `db_reader`: an own-verse URL
/// prefix in front of a victim petal/fractal is denied 404 with the unified
/// text, never written (the harness has no DB thread, so a write that got
/// past authz would answer "request timed out" instead).
#[tokio::test]
async fn rest_creates_deny_a_foreign_target_behind_an_own_prefix() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed own chain");
    let victim = h.seed_hierarchy().await.expect("seed victim chain");
    let token = h.mint_token(&own.verse_scope(), "editor");

    let node_path = format!(
        "/api/v1/verses/{}/fractals/{}/petals/{}/nodes",
        own.verse_id, own.fractal_id, victim.petal_id
    );
    let petal_path = format!(
        "/api/v1/verses/{}/fractals/{}/petals",
        own.verse_id, victim.fractal_id
    );
    for path in [node_path, petal_path] {
        let (status, body) = h
            .post_json(&path, Some(&token), &json!({ "name": "planted" }))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{path}");
    }

    // Role floor still answers first, with its own (non-oracle) text.
    let viewer = h.mint_token(&own.verse_scope(), "viewer");
    let path = format!(
        "/api/v1/verses/{}/fractals/{}/petals/{}/nodes",
        own.verse_id, own.fractal_id, own.petal_id
    );
    let (status, body) = h
        .post_json(&path, Some(&viewer), &json!({ "name": "nope" }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "insufficient permissions");
}

/// DEC-C21 sweep finding: field-def update/delete took only an id and had no
/// scope check — a Manager of any verse could rewrite another verse's schema.
/// Now the field def's STORED scope must be covered (and list checks the
/// requested scope); every denial is the unified text, before any DB send.
#[tokio::test]
async fn field_def_mutations_require_the_stored_scope() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed own chain");
    let victim = h.seed_hierarchy().await.expect("seed victim chain");
    let field_def_id = ulid::Ulid::new().to_string();
    h.db.query(
        "CREATE field_def CONTENT { field_def_id: $id, scope: $scope, entity_type: 'node', \
         key: 'k', value_type: 'string', created_by: 'did:key:z6MkVictim', created_at: 'now' }",
    )
    .bind(("id", field_def_id.clone()))
    .bind(("scope", victim.verse_scope()))
    .await
    .expect("seed field_def")
    .check()
    .expect("field_def row");
    let manager = h.mint_token(&own.verse_scope(), "manager");
    let by_id = format!("/api/v1/field-defs/by-id/{field_def_id}");

    for method in ["PATCH", "DELETE"] {
        let req = Request::builder()
            .method(method)
            .uri(&by_id)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {manager}"))
            .body(Body::from(
                json!({ "value_type": "number", "default_val": null }).to_string(),
            ))
            .expect("build request");
        let resp = h.request(req).await;
        let body: serde_json::Value =
            serde_json::from_slice(&body_bytes(resp).await).expect("json body");
        assert_eq!(body["ok"], false, "{method}: {body}");
        assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{method}");
    }

    let list = format!(
        "/api/v1/field-defs/{}",
        victim.verse_scope().replace('#', "%23")
    );
    let (_, body) = h.get(&list, Some(&manager)).await;
    assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{body}");
}

// ---------------------------------------------------------------------------
// (d) GIS endpoint round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gis_nodes_round_trip() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let n1 = h
        .seed_node(&hier.petal_id, "Gis A", [10.0, 2.5, -20.0], None)
        .await
        .expect("seed n1");
    let n2 = h
        .seed_node(&hier.petal_id, "Gis B", [100.0, 0.0, 100.0], None)
        .await
        .expect("seed n2");
    let token = h.mint_token(&hier.verse_scope(), "viewer");

    // Actual wire shape is { petal_id, nodes: [GisNodeDto] } (position = local
    // [x, z] meters + elevation) — not a GeoJSON FeatureCollection.
    let path = format!("/api/v1/petals/{}/gis/nodes", hier.petal_id);
    let (status, body) = h.get(&path, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["petal_id"], hier.petal_id);
    let nodes = body["data"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 2);
    let a = nodes
        .iter()
        .find(|n| n["node_id"] == *n1)
        .expect("n1 present");
    assert_eq!(a["display_name"], "Gis A");
    assert_eq!(a["position"][0], 10.0);
    assert_eq!(a["position"][1], -20.0);
    assert_eq!(a["elevation"], 2.5);

    // Local-meter bbox filter narrows the result set.
    let (status, body) = h
        .get(&format!("{path}?bbox=0,-30,20,0"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let nodes = body["data"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1, "bbox should exclude n2: {body}");
    assert_eq!(nodes[0]["node_id"], *n1);
    let _ = n2;
}

// ---------------------------------------------------------------------------
// (e) export.csv rows + headers (parquet is ungated — fe-api has no features)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_csv_returns_rows_and_headers() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let nid = h
        .seed_node(&hier.petal_id, "Csv Node", [1.5, 2.0, 3.0], None)
        .await
        .expect("seed node");
    let token = h.mint_token(&hier.verse_scope(), "viewer");

    let (status, ctype, bytes) = get_raw(
        &h,
        &format!("/api/v1/petals/{}/export.csv", hier.petal_id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype, "text/csv; charset=utf-8");
    let csv = String::from_utf8(bytes).expect("utf8 csv");
    let mut lines = csv.lines();
    // Line 0: CRS comment (unconfigured petal stays PETAL-LOCAL, never 4326).
    let crs_line = lines.next().expect("crs line");
    assert!(
        crs_line.starts_with("# crs=PETAL-LOCAL:meters"),
        "{crs_line}"
    );
    assert!(!crs_line.contains("4326"), "{crs_line}");
    // Line 1: column headers.
    let header = lines.next().expect("header line");
    assert!(
        header.starts_with("node_id,petal_id,x_m,y_m,z_m,rotation_x"),
        "{header}"
    );
    // Line 2: the seeded row (y_m = elevation).
    let row = lines.next().expect("data row");
    assert!(
        row.starts_with(&format!("{nid},{},1.5,2,3", hier.petal_id)),
        "{row}"
    );
    assert_eq!(lines.next(), None, "exactly one data row");

    // Parquet twin (unconditionally compiled): status + content-type only;
    // deep parquet round-trips live in export_share_test.rs.
    let (status, ctype, _) = get_raw(
        &h,
        &format!("/api/v1/petals/{}/export.parquet", hier.petal_id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype, "application/vnd.apache.parquet");
}

// ---------------------------------------------------------------------------
// (f) query_guard limit violation errors rather than truncates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_byte_ceiling_errors_not_truncates() {
    // Cheapest deterministic endpoint-triggerable limit: the 8 MiB response
    // byte ceiling (row cap needs 10k rows; the rate limit is timing-based).
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let blob = "x".repeat(3_500_000); // 3 rows ≈ 10.5 MiB > 8 MiB ceiling
    for i in 0..3 {
        h.seed_node(
            &hier.petal_id,
            &format!("Big {i}"),
            [i as f64, 0.0, 0.0],
            Some(json!({ "blob": blob })),
        )
        .await
        .expect("seed big node");
    }
    let token = h.mint_token(&hier.verse_scope(), "viewer");

    let (status, body) = query(&h, &token, "SELECT * FROM node", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "must error, not truncate: {body}");
    let err = body["error"].as_str().unwrap_or_default();
    assert!(err.contains("result size exceeds ceiling (8 MiB)"), "{err}");
    assert!(body["data"].is_null(), "no partial data on limit violation");
}

/// GPX export over HTTP: a waypoint linked to its track by
/// `properties.gpx_track_id` nests under it and is still exported (as a
/// top-level `<wpt>`), and an orphan waypoint is exported as a root — each once.
#[tokio::test]
async fn gpx_export_emits_track_and_both_waypoints_exactly_once() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let p = &hier.petal_id;
    let track = h
        .seed_node(
            p,
            "Ridge",
            [0.0, 0.0, 0.0],
            Some(json!({ "gpx_type": "track" })),
        )
        .await
        .expect("seed track");
    h.seed_node(
        p,
        "Summit",
        [1.0, 0.0, 1.0],
        Some(json!({ "gpx_type": "waypoint", "name": "Summit", "gpx_track_id": track })),
    )
    .await
    .expect("seed track waypoint");
    h.seed_node(
        p,
        "Spring",
        [2.0, 0.0, 2.0],
        Some(json!({ "gpx_type": "waypoint", "name": "Spring", "gpx_track_id": ulid::Ulid::new().to_string() })),
    )
    .await
    .expect("seed orphan waypoint");
    let token = h.mint_token(&hier.verse_scope(), "editor");

    let (status, ctype, body) =
        get_raw(&h, &format!("/api/v1/petals/{p}/export/gpx"), &token).await;
    let xml = String::from_utf8(body).expect("utf-8 GPX");

    assert_eq!(status, StatusCode::OK, "{xml}");
    assert_eq!(ctype, "application/gpx+xml");
    assert_eq!(xml.matches("<trk>").count(), 1, "{xml}");
    assert_eq!(xml.matches("<wpt ").count(), 2, "{xml}");
    assert_eq!(xml.matches("<name>Summit</name>").count(), 1, "{xml}");
    assert_eq!(xml.matches("<name>Spring</name>").count(), 1, "{xml}");
}

// ---------------------------------------------------------------------------
// AP-3.4: TokenScopePolicy is the production scope gate (require_scope)
// ---------------------------------------------------------------------------

/// A foreign-verse Editor gets the uniform denial on two `require_scope`-gated
/// routes (a pure URI resolve and a DB-backed node read) while the own-verse
/// token passes — red if `require_scope` stops delegating to
/// `TokenScopePolicy` or its containment is inverted.
#[tokio::test]
async fn foreign_verse_token_is_denied_by_token_scope_policy() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed hierarchy");
    let node_id = h
        .seed_node(&own.petal_id, "scoped", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    let address = format!(
        "/api/v1/address?uri=fe://{}/{}/{}/{}",
        own.verse_id, own.fractal_id, own.petal_id, node_id
    );
    let read = format!("/api/v1/nodes/{node_id}");

    let foreign = h.mint_token("VERSE#other", "editor");
    let own_token = h.mint_token(&own.verse_scope(), "editor");
    for path in [&address, &read] {
        let (_, body) = h.get(path, Some(&foreign)).await;
        assert_eq!(body["ok"], false, "{path}: {body}");
        assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{path}");

        let (_, body) = h.get(path, Some(&own_token)).await;
        assert_eq!(body["ok"], true, "{path}: {body}");
    }
}

// ---------------------------------------------------------------------------
// T5-LOCK: REST PATCH .../transform cannot free-translate a promoted stamp
// ---------------------------------------------------------------------------

async fn patch_transform(
    h: &ApiHarness,
    token: &str,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
) -> serde_json::Value {
    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/api/v1/nodes/{}/transform",
            node_id.replace('#', "%23")
        ))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({ "position": position, "rotation": rotation, "scale": [2.0, 2.0, 2.0] })
                .to_string(),
        ))
        .expect("build request");
    serde_json::from_slice(&body_bytes(h.request(req).await).await).expect("json body")
}

/// Persisted transforms that reached the DB channel so far (fire-and-forget).
fn drained_persists(
    rx: &crossbeam::channel::Receiver<fe_runtime::messages::ApiCommand>,
) -> Vec<(String, [f32; 3])> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            fe_runtime::messages::ApiCommand::TransformPersist {
                node_id, rotation, ..
            } => Some((node_id, rotation)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn rest_transform_rejects_moving_a_promoted_stamp_but_rotates_it() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed hierarchy");
    let stamp = h
        .seed_promoted_stamp(&own.petal_id, 0, [5.0, 1.0, 7.0])
        .await
        .expect("seed stamp");
    let token = h.mint_token(&own.verse_scope(), "editor");
    let rx = h.api_command_receiver();

    let moved = patch_transform(&h, &token, &stamp, [6.0, 1.0, 7.0], [0.0; 3]).await;
    assert_eq!(moved["ok"], false, "{moved}");
    assert!(
        moved["error"]
            .as_str()
            .unwrap_or_default()
            .contains("position is locked"),
        "{moved}"
    );
    assert!(
        drained_persists(&rx).is_empty(),
        "a denied move must not persist"
    );

    let rotated = patch_transform(&h, &token, &stamp, [5.0, 1.0, 7.0], [0.0, 1.5, 0.0]).await;
    assert_eq!(rotated["ok"], true, "{rotated}");
    assert_eq!(
        drained_persists(&rx),
        vec![(stamp.clone(), [0.0, 1.5, 0.0])],
        "rotation-only change persists"
    );
}

// ---------------------------------------------------------------------------
// T5-PARITY (endpoint_api_surface Phase 3): API ops == UI ops, real-row reads
// ---------------------------------------------------------------------------

/// DELETE `/api/v1/nodes/{id}{query}`; answers the one DB request like the DB
/// thread and returns (response envelope, the `DbCommand` that reached it).
async fn delete_capturing_command(
    h: &ApiHarness,
    token: &str,
    node_id: &str,
    query: &str,
) -> (serde_json::Value, fe_runtime::messages::DbCommand) {
    use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
    let rx = h.api_command_receiver();
    let db_thread =
        std::thread::spawn(
            move || match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(ApiCommand::DbRequest { cmd, reply_tx }) => {
                    let node_id = match &cmd {
                        DbCommand::TombstoneNode { node_id, .. }
                        | DbCommand::CascadeTombstoneNode { node_id, .. } => node_id.clone(),
                        other => panic!("delete sent {other:?}"),
                    };
                    let _ = reply_tx.send(DbResult::NodeDeleted {
                        node_id,
                        petal_id: String::new(),
                    });
                    cmd
                }
                other => panic!("no DbRequest reached the DB channel: {other:?}"),
            },
        );
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/nodes/{node_id}{query}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("build request");
    let body = serde_json::from_slice(&body_bytes(h.request(req).await).await).expect("json");
    (body, db_thread.join().expect("db thread"))
}

/// 6a, API half: REST DELETE sends exactly the op the UI's `handle_delete`
/// sends (`TombstoneNode` / `CascadeTombstoneNode`), differing only in the
/// caller: `Identified{token sub, role}` vs the UI's `Local`. The DB half
/// (`node_lifecycle_test::ui_and_api_tombstones_write_the_same_op_log_entry`)
/// proves both callers then write the same op-log entry.
#[tokio::test]
async fn rest_delete_sends_the_ui_tombstone_op_with_an_identified_caller() {
    use fe_runtime::messages::{CallerAuth, DbCommand};
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed hierarchy");
    let node = h
        .seed_node(&own.petal_id, "doomed", [0.0; 3], None)
        .await
        .expect("seed node");
    let token = h.mint_token(&own.verse_scope(), "editor");
    let identified = |cmd: &DbCommand| match cmd {
        DbCommand::TombstoneNode { node_id, auth } => (false, node_id.clone(), auth.clone()),
        DbCommand::CascadeTombstoneNode { node_id, auth } => (true, node_id.clone(), auth.clone()),
        other => panic!("not a tombstone op: {other:?}"),
    };
    let caller = |auth: CallerAuth| match auth {
        CallerAuth::Identified { did, role } => (did, role),
        other => panic!("API caller must be Identified, got {other:?}"),
    };

    let (body, cmd) = delete_capturing_command(&h, &token, &node, "").await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["data"]["tombstoned"], true);
    let (cascade, node_id, auth) = identified(&cmd);
    assert!(!cascade);
    assert_eq!(node_id, node);
    assert_eq!(caller(auth), (h.did(), "editor".to_string()));

    let (body, cmd) = delete_capturing_command(&h, &token, &node, "?cascade=true").await;
    assert_eq!(body["data"]["cascade"], true, "{body}");
    let (cascade, node_id, auth) = identified(&cmd);
    assert!(cascade);
    assert_eq!(node_id, node);
    assert_eq!(caller(auth), (h.did(), "editor".to_string()));
}

/// 6b: the earthwork summary sums the volumes of the petal's LIVE region rows
/// (real SurrealDB aggregation, not a fixture). A tombstoned region, a region
/// in another petal and a plain node with volume-shaped keys are excluded.
#[tokio::test]
async fn earthwork_summary_sums_live_region_rows() {
    use fe_query::spatial_nodes::{CUT_VOLUME_KEY, FILL_VOLUME_KEY, KIND_KEY};
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed hierarchy");
    let other_petal = h
        .seed_petal(&own.fractal_id, "Other Petal", None)
        .await
        .expect("other petal");
    let region = |cut: f64, fill: f64| json!({ KIND_KEY: "earthwork_region", CUT_VOLUME_KEY: cut, FILL_VOLUME_KEY: fill });
    for (petal, cut, fill) in [
        (&own.petal_id, 120.5, 30.0),
        (&own.petal_id, 79.5, 70.25),
        (&other_petal, 1000.0, 1000.0),
    ] {
        h.seed_node(petal, "region", [0.0; 3], Some(region(cut, fill)))
            .await
            .expect("seed region");
    }
    let deleted = h
        .seed_node(
            &own.petal_id,
            "deleted region",
            [0.0; 3],
            Some(region(5000.0, 5000.0)),
        )
        .await
        .expect("seed deleted region");
    h.db.query(
        "UPDATE node SET tombstone = { hlc: 1, source_did: 'did:key:test' } WHERE node_id = $nid",
    )
    .bind(("nid", deleted))
    .await
    .expect("tombstone region")
    .check()
    .expect("tombstone check");
    h.seed_node(
        &own.petal_id,
        "plain",
        [0.0; 3],
        Some(json!({ CUT_VOLUME_KEY: 9.0 })),
    )
    .await
    .expect("seed plain");

    let token = h.mint_token(&own.verse_scope(), "viewer");
    let (_, body) = h
        .get(
            &format!("/api/v1/petals/{}/earthwork/summary", own.petal_id),
            Some(&token),
        )
        .await;
    assert_eq!(body["ok"], true, "{body}");
    let summary = &body["data"][0];
    assert_eq!(summary["region_count"], 2, "{body}");
    assert_eq!(summary["total_cut_m3"].as_f64(), Some(200.0), "{body}");
    assert_eq!(summary["total_fill_m3"].as_f64(), Some(100.25), "{body}");
}

/// 6c: `GET /api/v1/nodes/{id}` reads each object type through the one
/// generic endpoint: `kind` from `properties.node_kind`, and the
/// type-specific payload in `data.properties`.
#[tokio::test]
async fn read_node_returns_each_type_specific_payload() {
    use fe_query::spatial_nodes::{
        CUT_VOLUME_KEY, FILL_VOLUME_KEY, KIND_KEY, MATERIAL_KEY, STAMP_INSTANCE_INDEX_KEY,
        STAMP_PATH_ID_KEY,
    };
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed hierarchy");
    let stamp = h
        .seed_promoted_stamp(&own.petal_id, 3, [5.0, 1.0, 7.0])
        .await
        .expect("seed stamp");
    let region = h
        .seed_node(
            &own.petal_id,
            "cut",
            [0.0; 3],
            Some(json!({ KIND_KEY: "earthwork_region", CUT_VOLUME_KEY: 42.5,
                         FILL_VOLUME_KEY: 7.0, MATERIAL_KEY: "clay" })),
        )
        .await
        .expect("seed region");
    let points = json!([[0.0, 0.0, 0.0, 0.0], [10.0, 0.0, 5.0, 1.0]]);
    let path = h
        .seed_node(
            &own.petal_id,
            "ridge",
            [0.0; 3],
            Some(json!({ "gpx_type": "track", "gpx_points": points })),
        )
        .await
        .expect("seed path");
    let token = h.mint_token(&own.verse_scope(), "viewer");

    let mut dtos = Vec::new();
    for id in [&stamp, &region, &path] {
        let (_, body) = h
            .get(
                &format!("/api/v1/nodes/{}", id.replace('#', "%23")),
                Some(&token),
            )
            .await;
        assert_eq!(body["ok"], true, "{id}: {body}");
        assert_eq!(body["data"]["node_id"], id.as_str());
        dtos.push(body["data"].clone());
    }

    let stamp_dto = &dtos[0];
    assert_eq!(stamp_dto["kind"], "stamp");
    let props = &stamp_dto["data"]["properties"];
    assert_eq!(props[STAMP_INSTANCE_INDEX_KEY], 3);
    assert_eq!(
        props[STAMP_PATH_ID_KEY],
        stamp.split("#inst-").next().unwrap()
    );

    let region_dto = &dtos[1];
    assert_eq!(region_dto["kind"], "earthwork_region");
    let props = &region_dto["data"]["properties"];
    assert_eq!(props[CUT_VOLUME_KEY].as_f64(), Some(42.5));
    assert_eq!(props[FILL_VOLUME_KEY].as_f64(), Some(7.0));
    assert_eq!(props[MATERIAL_KEY], "clay");

    let path_dto = &dtos[2];
    assert!(
        path_dto["kind"].is_null(),
        "a path is a plain node: {path_dto}"
    );
    assert_eq!(path_dto["data"]["properties"]["gpx_type"], "track");
    assert_eq!(path_dto["data"]["properties"]["gpx_points"], points);
}
