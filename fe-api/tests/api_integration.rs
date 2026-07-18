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

    // Scoped read route: insufficient role and foreign scope both 403.
    let csv_path = format!("/api/v1/petals/{}/export.csv", hier.petal_id);
    let (status, _, _) = get_raw(&h, &csv_path, &none_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let foreign = h.mint_token("VERSE#someone-else", "viewer");
    let (status, _, _) = get_raw(&h, &csv_path, &foreign).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
