//! §existence-oracle (fe-api/AGENTS.md): over the real router, a verse-A token
//! asking about (i) an id that never existed and (ii) a REAL resource in verse
//! B must get byte-identical (status, body) on every scoped route — otherwise
//! the difference tells the caller that B's id exists.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fractalengine_test_harness::api::{body_bytes, ApiHarness};
use serde_json::json;

/// How a route's request body is shaped (the extractor must accept it, so the
/// handler's authz — not a 4xx from axum — answers).
#[derive(Clone, Copy)]
enum Payload {
    None,
    Json(fn() -> serde_json::Value),
    Multipart,
}

/// The ids substituted into a route template for one probe.
struct Probe {
    petal: String,
    node: String,
    verse: String,
    fractal: String,
    field_def: String,
    scope: String,
}

/// Every route that resolves a resource's scope from the DB, by family.
/// `{p}` petal, `{n}` node, `{v}` verse, `{f}` fractal, `{fd}` field def, `{s}`
/// scope string, `{va}`/`{fa}`/`{pa}` the caller's OWN verse/fractal/petal
/// (decoy-prefix creates, and second-id routes where the probe is a node id
/// riding inside an authorized petal's URL).
const ROUTES: &[(&str, &str, Payload)] = &[
    // petal-scoped GET/PUT/DELETE
    ("GET", "/api/v1/petals/{p}/terrain", Payload::None),
    (
        "PUT",
        "/api/v1/petals/{p}/terrain",
        Payload::Json(|| json!({})),
    ),
    ("DELETE", "/api/v1/petals/{p}/terrain", Payload::None),
    ("GET", "/api/v1/petals/{p}/visibility-groups", Payload::None),
    (
        "PUT",
        "/api/v1/petals/{p}/visibility-groups",
        Payload::Json(|| json!({ "groups": [] })),
    ),
    (
        "PATCH",
        "/api/v1/petals/{p}/visibility-groups/g1",
        Payload::Json(|| json!({ "name": "G" })),
    ),
    (
        "DELETE",
        "/api/v1/petals/{p}/visibility-groups/g1",
        Payload::None,
    ),
    (
        "POST",
        "/api/v1/petals/{p}/waypoints",
        Payload::Json(|| json!({ "name": "wp", "lat": 1.0, "lon": 2.0, "ele": 3.0 })),
    ),
    ("GET", "/api/v1/petals/{p}/nodes?kind=stamp", Payload::None),
    ("GET", "/api/v1/petals/{p}/earthwork/summary", Payload::None),
    (
        "POST",
        "/api/v1/petals/{p}/paths/01HZZZZZZZZZZZZZZZZZZZZZZZ/instances/0/promote",
        Payload::None,
    ),
    ("POST", "/api/v1/petals/{p}/assets", Payload::Multipart),
    ("POST", "/api/v1/petals/{p}/import", Payload::Multipart),
    (
        "POST",
        "/api/v1/verses/{va}/fractals/{fa}/petals/{p}/nodes",
        Payload::Json(|| json!({ "name": "planted" })),
    ),
    (
        "POST",
        "/api/v1/verses/{va}/fractals/{f}/petals",
        Payload::Json(|| json!({ "name": "planted" })),
    ),
    // second id inside the caller's OWN authorized petal: the probe is the path
    // (the DB tolerates an absent path but refuses a foreign one)
    (
        "POST",
        "/api/v1/petals/{pa}/paths/{n}/instances/0/promote",
        Payload::None,
    ),
    // export / BI egress / analytics / distributed query
    ("GET", "/api/v1/petals/{p}/export", Payload::None),
    ("GET", "/api/v1/petals/{p}/export.parquet", Payload::None),
    ("GET", "/api/v1/petals/{p}/export.csv", Payload::None),
    // gpx
    ("POST", "/api/v1/petals/{p}/import/gpx", Payload::Multipart),
    ("GET", "/api/v1/petals/{p}/export/gpx", Payload::None),
    // gis
    ("GET", "/api/v1/petals/{p}/gis/nodes", Payload::None),
    ("GET", "/api/v1/petals/{p}/gis/tracks", Payload::None),
    // iot
    (
        "POST",
        "/api/v1/petals/{p}/iot/readings",
        Payload::Json(|| json!({ "readings": [] })),
    ),
    // node-scoped
    (
        "PATCH",
        "/api/v1/nodes/{n}/transform",
        Payload::Json(
            || json!({ "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] }),
        ),
    ),
    ("GET", "/api/v1/nodes/{n}/transform", Payload::None),
    (
        "PATCH",
        "/api/v1/nodes/{n}/properties",
        Payload::Json(|| json!({ "key": "k", "value": 1 })),
    ),
    ("GET", "/api/v1/nodes/{n}/properties", Payload::None),
    ("DELETE", "/api/v1/nodes/{n}/properties/k", Payload::None),
    (
        "PATCH",
        "/api/v1/nodes/{n}/move",
        Payload::Json(|| json!({ "lat": 1.0, "lon": 2.0 })),
    ),
    ("GET", "/api/v1/nodes/{n}/elevation-profile", Payload::None),
    ("GET", "/api/v1/nodes/{n}/stats", Payload::None),
    ("GET", "/api/v1/nodes/{n}/asset", Payload::None),
    // endpoint surface
    ("GET", "/api/v1/nodes/{n}", Payload::None),
    ("DELETE", "/api/v1/nodes/{n}", Payload::None),
    ("GET", "/api/v1/nodes/{n}/address", Payload::None),
    // hierarchy create twins
    (
        "POST",
        "/api/v1/verses/{v}/fractals",
        Payload::Json(|| json!({ "name": "planted" })),
    ),
    // field defs
    (
        "PATCH",
        "/api/v1/field-defs/by-id/{fd}",
        Payload::Json(|| json!({ "value_type": "number", "default_val": null })),
    ),
    ("DELETE", "/api/v1/field-defs/by-id/{fd}", Payload::None),
    ("GET", "/api/v1/field-defs/{s}", Payload::None),
    // hexon tileset management
    (
        "POST",
        "/api/v1/hexons/tilesets/install?petal_id={p}",
        Payload::Multipart,
    ),
    (
        "DELETE",
        "/api/v1/hexons/tilesets/ts-1?petal_id={p}",
        Payload::None,
    ),
    (
        "PATCH",
        "/api/v1/hexons/tilesets/ts-1/seeding?petal_id={p}",
        Payload::Json(|| json!({ "enabled": true })),
    ),
    ("GET", "/api/v1/hexons/tilesets?petal_id={p}", Payload::None),
    ("GET", "/api/v1/hexons/storage?petal_id={p}", Payload::None),
    // hexon crate registry
    (
        "POST",
        "/api/v1/crates/c-1/install?petal_id={p}",
        Payload::None,
    ),
    (
        "DELETE",
        "/api/v1/crates/c-1/uninstall?petal_id={p}",
        Payload::None,
    ),
    ("GET", "/api/v1/crates/search?petal_id={p}", Payload::None),
    (
        "GET",
        "/api/v1/crates/installed?petal_id={p}",
        Payload::None,
    ),
    ("GET", "/api/v1/crates/c-1?petal_id={p}", Payload::None),
    (
        "GET",
        "/api/v1/crates/c-1/entries?petal_id={p}",
        Payload::None,
    ),
    (
        "GET",
        "/api/v1/crates/c-1/entries/e-1/asset?petal_id={p}",
        Payload::None,
    ),
    (
        "GET",
        "/api/v1/crates/available?petal_id={p}",
        Payload::None,
    ),
    // tile data plane
    (
        "GET",
        "/api/v1/tiles/elevation/ts-1/10/535/358.png?petal_id={p}",
        Payload::None,
    ),
    (
        "GET",
        "/api/v1/tiles/satellite/ts-1/10/535/358.jpg?petal_id={p}",
        Payload::None,
    ),
    ("GET", "/api/v1/tilesets?petal_id={p}", Payload::None),
    (
        "GET",
        "/api/v1/tilesets/ts-1/meta?petal_id={p}",
        Payload::None,
    ),
];

/// Routes that legitimately SUCCEED on the caller's own petal: the probe id
/// is a filter, so (i) and (ii) must answer identically (an empty listing),
/// not deny. A foreign `path_id` once returned verse B's stamp rows here.
const SAME_ANSWER_ROUTES: &[(&str, &str, Payload)] = &[(
    "GET",
    "/api/v1/petals/{pa}/nodes?kind=stamp&path_id={n}",
    Payload::None,
)];

/// Body-carrying routes outside the path-template table (petal id in the body).
/// Builds a route's JSON body around the probed petal id.
type BodyFor = fn(&str) -> serde_json::Value;

const BODY_ROUTES: &[(&str, BodyFor)] = &[
    (
        "/api/v1/analytics/query",
        |petal| json!({ "sql": "SELECT 1", "petal_id": petal }),
    ),
    (
        "/api/v1/query",
        |petal| json!({ "distributed": { "kind": "all_readings", "petal_id": petal } }),
    ),
    (
        "/api/v1/nodes",
        |petal| json!({ "petal_id": petal, "name": "planted" }),
    ),
];

const BOUNDARY: &str = "existence-oracle-boundary";

async fn send(
    h: &ApiHarness,
    method: &str,
    path: &str,
    token: &str,
    payload: Payload,
) -> (StatusCode, Vec<u8>) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let req = match payload {
        Payload::None => builder.body(Body::empty()),
        Payload::Json(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body().to_string())),
        Payload::Multipart => builder
            .header(
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(format!("--{BOUNDARY}--\r\n"))),
    }
    .expect("build request");
    let resp = h.request(req).await;
    let status = resp.status();
    (status, body_bytes(resp).await)
}

/// The caller's own (authorized) verse/fractal/petal ids.
struct Own<'a> {
    verse: &'a str,
    fractal: &'a str,
    petal: &'a str,
}

fn render(template: &str, probe: &Probe, own: &Own<'_>) -> String {
    template
        .replace("{va}", own.verse)
        .replace("{fa}", own.fractal)
        .replace("{pa}", own.petal)
        .replace("{fd}", &probe.field_def)
        .replace("{f}", &probe.fractal)
        .replace("{p}", &probe.petal)
        .replace("{n}", &probe.node)
        .replace("{v}", &probe.verse)
        .replace("{s}", &probe.scope)
}

/// A denial in either convention: real-status 404, or the house 200 envelope
/// carrying the one text. Guards against both probes "agreeing" on, e.g., a 422.
fn is_uniform_denial(status: StatusCode, body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    status == StatusCode::NOT_FOUND
        || (status == StatusCode::OK && text.contains(fe_api::rest::NOT_FOUND_OR_DENIED))
}

#[tokio::test]
async fn unknown_and_foreign_ids_are_indistinguishable_on_every_scoped_route() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed verse A");
    let foreign = h.seed_hierarchy().await.expect("seed verse B");
    let foreign_node = h
        .seed_node(&foreign.petal_id, "B node", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed B node");
    // A stamp in B following B's node as its path: a cross-tenant
    // `?path_id=` listing must not return it.
    h.seed_node(
        &foreign.petal_id,
        "B stamp",
        [0.0, 0.0, 0.0],
        Some(json!({ "node_kind": "stamp", "path_id": foreign_node, "instance_index": 0 })),
    )
    .await
    .expect("seed B stamp");
    let foreign_field_def = ulid::Ulid::new().to_string();
    h.db.query(
        "CREATE field_def CONTENT { field_def_id: $id, scope: $scope, entity_type: 'node', \
         key: 'k', value_type: 'string', created_by: 'did:key:z6MkB', created_at: 'now' }",
    )
    .bind(("id", foreign_field_def.clone()))
    .bind(("scope", foreign.verse_scope()))
    .await
    .expect("seed field_def")
    .check()
    .expect("field_def row");
    // Owner clears every route's role floor, so only scope can deny.
    let token = h.mint_token(&own.verse_scope(), "owner");

    let unknown = Probe {
        petal: ulid::Ulid::new().to_string(),
        node: ulid::Ulid::new().to_string(),
        verse: ulid::Ulid::new().to_string(),
        fractal: ulid::Ulid::new().to_string(),
        field_def: ulid::Ulid::new().to_string(),
        scope: format!("VERSE%23{}", ulid::Ulid::new()),
    };
    let real_in_b = Probe {
        petal: foreign.petal_id.clone(),
        node: foreign_node,
        verse: foreign.verse_id.clone(),
        fractal: foreign.fractal_id.clone(),
        field_def: foreign_field_def,
        scope: foreign.verse_scope().replace('#', "%23"),
    };

    let own_ids = Own {
        verse: &own.verse_id,
        fractal: &own.fractal_id,
        petal: &own.petal_id,
    };
    let mut failures = Vec::new();
    for &(method, template, payload) in ROUTES.iter().chain(SAME_ANSWER_ROUTES) {
        let must_deny = !SAME_ANSWER_ROUTES.iter().any(|r| r.1 == template);
        let path_i = render(template, &unknown, &own_ids);
        let path_ii = render(template, &real_in_b, &own_ids);
        let (status_i, body_i) = send(&h, method, &path_i, &token, payload).await;
        let (status_ii, body_ii) = send(&h, method, &path_ii, &token, payload).await;
        if (status_i, &body_i) != (status_ii, &body_ii)
            || (must_deny && !is_uniform_denial(status_i, &body_i))
        {
            failures.push(format!(
                "{method} {template}: unknown=({status_i}, {}) foreign=({status_ii}, {})",
                String::from_utf8_lossy(&body_i),
                String::from_utf8_lossy(&body_ii)
            ));
        }
    }
    for &(path, body) in BODY_ROUTES {
        let (status_i, body_i) = post_body(&h, path, &token, body(&unknown.petal)).await;
        let (status_ii, body_ii) = post_body(&h, path, &token, body(&real_in_b.petal)).await;
        if (status_i, &body_i) != (status_ii, &body_ii) || !is_uniform_denial(status_i, &body_i) {
            failures.push(format!(
                "POST {path}: unknown=({status_i}, {}) foreign=({status_ii}, {})",
                String::from_utf8_lossy(&body_i),
                String::from_utf8_lossy(&body_ii)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "existence oracle on {} route(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

async fn post_body(
    h: &ApiHarness,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let resp = h.request(req).await;
    let status = resp.status();
    (status, body_bytes(resp).await)
}

/// The second-id gates must not over-deny: on the caller's own petal, a path
/// that IS one of its nodes still promotes (the command reaches the DB thread)
/// and still lists its own stamps.
#[tokio::test]
async fn own_petal_path_still_promotes_and_lists_its_stamps() {
    use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};

    let h = ApiHarness::spawn().await.expect("spawn harness");
    let own = h.seed_hierarchy().await.expect("seed verse A");
    let path = h
        .seed_node(&own.petal_id, "A path", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed path");
    let stamp = h
        .seed_node(
            &own.petal_id,
            "A stamp",
            [0.0, 0.0, 0.0],
            Some(json!({ "node_kind": "stamp", "path_id": path, "instance_index": 0 })),
        )
        .await
        .expect("seed stamp");
    let token = h.mint_token(&own.verse_scope(), "editor");

    // Answer PromoteInstance like the DB thread would (the harness has none).
    let rx = h.api_command_receiver();
    std::thread::spawn(move || {
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            if let DbCommand::PromoteInstance {
                petal_id,
                path_id,
                instance_index,
                ..
            } = cmd
            {
                let _ = reply_tx.send(DbResult::NodePromoted {
                    node_id: format!("{path_id}#inst-{instance_index}"),
                    petal_id,
                    path_id,
                    instance_index,
                    newly_promoted: true,
                });
            }
        }
    });

    let (status, body) = send(
        &h,
        "POST",
        &format!(
            "/api/v1/petals/{}/paths/{path}/instances/0/promote",
            own.petal_id
        ),
        &token,
        Payload::None,
    )
    .await;
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["data"]["node_id"], format!("{path}#inst-0"));

    let (_, body) = send(
        &h,
        "GET",
        &format!(
            "/api/v1/petals/{}/nodes?kind=stamp&path_id={path}",
            own.petal_id
        ),
        &token,
        Payload::None,
    )
    .await;
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["data"][0]["node_id"], stamp, "{json}");
}
