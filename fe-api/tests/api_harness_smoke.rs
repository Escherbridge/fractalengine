//! Smoke test for the API-integration harness
//! (fe-test-harness/src/AGENTS.md §api-harness).

use axum::http::StatusCode;
use fractalengine_test_harness::api::ApiHarness;

#[tokio::test]
async fn harness_health_ready_and_authed_roundtrip() {
    let h = ApiHarness::spawn().await.expect("spawn harness");

    // Public health endpoint.
    let (status, body) = h.get("/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    // Readiness probe rides the db_reader ping.
    let (status, body) = h.get("/ready", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");

    // Authed route without a token -> 401 from the real auth middleware.
    let hier = h.seed_hierarchy().await.expect("seed hierarchy");
    let gis_path = format!("/api/v1/petals/{}/gis/nodes", hier.petal_id);
    let (status, _) = h.get(&gis_path, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Seed a node and read it back through the full router with a minted token.
    let node_id = h
        .seed_node(&hier.petal_id, "Smoke Node", [1.0, 5.0, -2.0], None)
        .await
        .expect("seed node");
    let token = h.mint_token(&hier.verse_scope(), "viewer");
    let (status, body) = h.get(&gis_path, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "gis nodes body: {body}");
    let nodes = body["data"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], node_id);
    assert_eq!(nodes[0]["display_name"], "Smoke Node");

    // Out-of-scope token -> 404, the unknown-petal answer (scope enforcement
    // is live; §existence-oracle).
    let foreign = h.mint_token("VERSE#someone-else", "viewer");
    let (status, _) = h.get(&gis_path, Some(&foreign)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let usage_dir = tempfile::tempdir().expect("usage tempdir");
    let usage_log = usage_dir.path().join("usage.jsonl");
    let (api_cmd_tx, commands) = crossbeam::channel::bounded(8);
    let (listening_tx, listening_rx) = crossbeam::channel::bounded(1);
    let (_, revocation_rx) = tokio::sync::broadcast::channel(1);
    let mut owned = fe_api::spawn_api_service(fe_api::ApiConfig {
        bind_addr: "127.0.0.1:0".into(),
        api_cmd_tx,
        transform_broadcast_tx: h.state.transform_broadcast_tx.clone(),
        entity_change_tx: h.state.entity_change_tx.clone(),
        verifying_key: h.state.verifying_key,
        revocation_rx,
        blob_store: None,
        cors_origins: None,
        api_db_reader: h.state.db_reader.clone(),
        entity_store: None,
        tileset_registry: None,
        hexon_registry: None,
        announcement_store: None,
        replication_tx: None,
        distributed_tx: None,
        sim_control_tx: None,
        share_signer: h.keypair.clone(),
        elevated_query_allowed: false,
        upload_quota_bytes: fe_api::upload::DEFAULT_UPLOAD_QUOTA_BYTES,
        screenshot: fe_api::screenshot::ScreenshotSurface::disabled(),
        input: fe_api::input::InputSurface::disabled(),
        listening_tx: Some(listening_tx),
        extension_host: None,
        binding_host: None,
        import_hooks: None,
        network_ops: fe_api::network_ops::NetworkOps {
            usage_sink: std::sync::Arc::new(
                fe_runtime::usage::JsonLinesUsageSink::open(&usage_log).expect("open usage log"),
            ),
            ..Default::default()
        },
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let address = loop {
        if let Ok(address) = listening_rx.try_recv() { break address; }
        assert!(std::time::Instant::now() < deadline, "owned API listener did not start");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    // Usage metering: one public and one authenticated request through the owned service's real layers.
    async fn raw_get(address: std::net::SocketAddr, path: &str, token: Option<&str>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\n{auth}Connection: close\r\n\r\n");
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(15), stream.read_to_end(&mut bytes)).await.unwrap().unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }
    assert!(raw_get(address, "/api/v1/health", None).await.starts_with("HTTP/1.1 200"));
    let viewer = h.mint_token(&hier.verse_scope(), "viewer");
    assert!(raw_get(address, &gis_path, Some(&viewer)).await.starts_with("HTTP/1.1 200"));
    // Events are written by detached tasks: wait on the observable line count, not on timing.
    let usage_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let usage_lines: Vec<serde_json::Value> = loop {
        let text = std::fs::read_to_string(&usage_log).unwrap_or_default();
        // The writer thread may be mid-line: parse only newline-terminated lines.
        let complete = &text[..text.rfind('\n').map_or(0, |end| end + 1)];
        let lines: Vec<serde_json::Value> = complete.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        if lines.len() >= 2 { break lines; }
        assert!(std::time::Instant::now() < usage_deadline, "usage events were not written");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(usage_lines.len(), 2, "exactly one event per request: {usage_lines:?}");
    let health = usage_lines.iter().find(|line| line["route_class"] == "/api/v1/health").expect("health event");
    assert!(health["caller_did"].is_null() && health["scope"].is_null(), "public route has no caller: {health}");
    let authed = usage_lines
        .iter()
        .find(|line| line["route_class"] == "/api/v1/petals/{petal_id}/gis/nodes")
        .expect("authenticated event carries the route template, never the concrete id");
    assert!(authed["caller_did"].is_string(), "authenticated route names its caller: {authed}");
    assert!(!authed.to_string().contains(&hier.petal_id), "no path id leaks into usage events");
    assert_eq!(authed["status"], 200);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let editor = h.mint_token(&hier.verse_scope(), "editor");
    let body = serde_json::json!({ "readings": [{ "node_id": node_id, "metric": "maintenance_probe", "value": 7.0 }] }).to_string();
    let request = format!("POST /api/v1/petals/{}/iot/readings HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {editor}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", hier.petal_id, body.len());
    let response = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(15), stream.read_to_end(&mut bytes)).await.unwrap().unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let (command, reply) = loop {
        if let Ok(fe_runtime::messages::ApiCommand::DbRequest { cmd, reply_tx }) = commands.try_recv() { break (cmd, reply_tx); }
        assert!(std::time::Instant::now() < deadline, "controlled IoT must use the DB executor despite an available reader");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    let fe_runtime::messages::DbCommand::InsertIotReadings { petal_id, verse_id, source_did, readings, source_class, correlation_id } = command else { panic!("expected controlled IoT command"); };
    assert_eq!(petal_id, hier.petal_id);
    assert_eq!(verse_id.as_deref(), Some(hier.verse_id.as_str()));
    assert_eq!(readings.len(), 1);
    fe_database::op_log::init_hlc(0);
    let written = fe_database::handlers::iot_reading::insert_readings_with_replication(
        &h.db, &petal_id, verse_id.as_deref(), &source_did, source_class, &readings, None, None,
    ).await.expect("real durable executor write");
    reply.send(fe_runtime::messages::DbResult::IotReadingsInserted { petal_id, written, correlation_id }).unwrap();
    assert!(response.await.unwrap().starts_with("HTTP/1.1 200"));
    let mut stored = h.db.query("SELECT metric, value FROM iot_reading WHERE node_id = $node")
        .bind(("node", node_id.clone())).await.unwrap();
    let rows: Vec<serde_json::Value> = stored.take(0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["metric"], "maintenance_probe");
    assert_eq!(rows[0]["value"], 7.0);
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await.expect("real pending WebSocket upgrade");
    use futures_util::StreamExt;
    let greeting = tokio::time::timeout(std::time::Duration::from_secs(1), socket.next()).await
        .expect("auth greeting deadline").expect("auth greeting frame").expect("auth greeting transport");
    assert!(greeting.to_text().unwrap().contains("auth_required"));
    owned.request_stop();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !owned.try_join().expect("owned API exits without panic") {
        assert!(std::time::Instant::now() < deadline, "live WebSocket retained the stopped API runtime");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(tokio::net::TcpStream::connect(address).await.is_err(), "the joined API must release its listener");
    let closed = tokio::time::timeout(std::time::Duration::from_secs(1), socket.next()).await;
    assert!(matches!(closed, Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))))));
}
