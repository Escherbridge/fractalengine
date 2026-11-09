//! Net journey through the public `Net` seam: one row per rule of net/AGENTS.md (§credentials, §ingress,
//! §dial-outs, §mqtt, §egress, §host-ceilings), on a manifest smuggled past validation (P2P-QA steer 2026-11-08 20:20).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fe_policy::RoleLevel;
use fe_sdk::manifest_v2::{AuthScheme, AuthSlot, ExtensionManifest, HttpEgress, Method, Qos, ReconnectMs};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::mqtt::{encode, Packet};
use super::*;
use crate::bus::sources::{AuditAction, AuditEntry, AuditSink, RoleSource};
use crate::bus::{HostError, InstanceSpec};
use crate::tick::{Clock, ManualClock};

const LOCAL: &str = "did:key:z6MkLocalHost";
const EDITOR: &str = "did:key:z6MkDeviceEditor";
const VIEWER: &str = "did:key:z6MkViewer";
/// Every DID with this prefix is an editor: rows that are not about rate get a fresh caller bucket.
const FLEET: &str = "did:key:z6MkFleet";
const PETAL: &str = "01JPETAL";
const PACKAGE: &str = "org.example.device-adapter";
const JSON: &str = "application/json";

/// Roles by DID, changeable mid-journey (a demotion after a websocket upgrade).
#[derive(Default)]
struct Roles(Mutex<HashMap<String, RoleLevel>>);

impl Roles {
    fn set(&self, did: &str, role: RoleLevel) {
        self.0.lock().unwrap().insert(did.to_string(), role);
    }
}

impl RoleSource for Roles {
    fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
        if did.starts_with(FLEET) {
            return RoleLevel::Editor;
        }
        self.0.lock().unwrap().get(did).copied().unwrap_or(RoleLevel::None)
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<AuditEntry>>);

impl AuditSink for Audit {
    fn record(&self, entry: AuditEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

impl Audit {
    fn denials(&self) -> usize {
        self.0.lock().unwrap().iter().filter(|entry| entry.action == AuditAction::Denial).count()
    }
}

/// Operator-filled credential slots.
struct Slots(HashMap<String, Credential>);

impl CredentialStore for Slots {
    fn get(&self, slot: &str) -> Option<Credential> {
        self.0.get(slot).cloned()
    }
}

/// Records frames and connection events; answers every frame with a small JSON body.
#[derive(Default)]
struct Device {
    frames: Vec<IngressFrame>,
    connections: Vec<(String, u64, ConnectionEvent)>,
}

impl NetHooks for Device {
    fn on_ingress(&mut self, frame: IngressFrame) -> Option<IngressReply> {
        self.frames.push(frame);
        Some(IngressReply { status: 200, content_type: Some(JSON.into()), body: br#"{"ok":true}"#.to_vec() })
    }

    fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent) {
        self.connections.push((endpoint.to_string(), connection, event));
    }

    fn on_egress_result(&mut self, _request: u64, _outcome: Result<IncomingResponse, HostError>) {}
}

/// A manifest the validator would refuse (V5, V16, schema bounds), handed to the host anyway: the host's own
/// rules must hold without the validator. `forward_headers` names every credential header on purpose.
fn smuggled_manifest() -> ExtensionManifest {
    serde_json::from_value(json!({
        "manifest_version": 2, "id": PACKAGE, "version": "0.1.0", "name": "Device adapter", "kind": "device-adapter",
        "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
        "ingress": [
            { "id": "telemetry", "transport": "http-server", "methods": ["POST"], "content_types": [JSON],
              "max_body_bytes": 64, "rate_hz": 2, "role_floor": "editor",
              "forward_headers": ["x-device-id", "authorization", "proxy-authorization", "cookie", "set-cookie",
                                  "x-api-key", "x-fe-join-lease", "X-FE-Session"] },
            { "id": "live", "transport": "websocket-accept", "max_frame_bytes": 16, "rate_hz": 2, "max_connections": 1 },
            { "id": "feed", "transport": "websocket-dial", "url": "wss://feed.example.org/v1", "max_frame_bytes": 1024,
              "rate_hz": 5, "reconnect_ms": { "min": 0, "max": 999_999_999u64 } },
            { "id": "rogue", "transport": "websocket-dial", "url": "ws://feed.example.org/v1", "max_frame_bytes": 1024, "rate_hz": 5 },
            { "id": "bus", "transport": "mqtt", "broker": "mqtts://broker.example.org", "topics": ["site-a/#/x"],
              "max_payload_bytes": 256, "rate_hz": 5 }
        ],
        "egress": [
            { "id": "sink", "transport": "http", "url": "https://bi.example.org/api/", "methods": ["POST"], "rate_hz": 1,
              "max_request_bytes": 8 },
            { "id": "vault", "transport": "http", "url": "https://vault.example.org/", "methods": ["GET"], "rate_hz": 1,
              "auth": { "scheme": "bearer", "credential": "vault-token" } },
            { "id": "uplink", "transport": "mqtt", "broker": "mqtts://broker.example.org", "topics": ["site-a/+/command"],
              "max_payload_bytes": 8, "rate_hz": 1, "qos": "at-most-once" }
        ]
    }))
    .expect("test manifest")
}

fn instance(petal: &str, installation: &str) -> InstanceSpec {
    InstanceSpec {
        installation_id: installation.into(),
        content_hash: "cd".repeat(32),
        verse_id: "01JVERSE".into(),
        petal_id: petal.into(),
        identity_did: LOCAL.into(),
        manifest: Arc::new(smuggled_manifest()),
    }
}

/// A POST carrying every credential header a client could send, plus one declared header.
fn post(caller: &str, endpoint: &str, content_type: &str, body: &[u8]) -> HttpIngressRequest {
    let header = |name: &str, value: &str| (name.to_string(), value.to_string());
    HttpIngressRequest {
        petal_id: PETAL.into(),
        package_id: PACKAGE.into(),
        endpoint: endpoint.into(),
        caller_did: caller.into(),
        method: "POST".into(),
        sub_path: String::new(),
        query: vec![header("since", "0")],
        headers: vec![
            header("X-Device-Id", "d1"),
            header("Authorization", "Bearer fe-api-token"),
            header("Proxy-Authorization", "Basic cHJveHk="),
            header("Cookie", "session=1"),
            header("Set-Cookie", "session=2"),
            header("X-Api-Key", "k"),
            header("x-fe-join-lease", "lease"),
            header("X-FE-Session", "s"),
        ],
        content_type: Some(content_type.into()),
        body: body.to_vec(),
    }
}

/// Every net/AGENTS.md rule, one row each, before any network I/O; the MQTT cap runs over an in-memory stream.
#[test]
fn net_journey_ingress_table_credentials_and_egress_refusals() {
    let clock = ManualClock::default();
    clock.set(1_000);
    let roles = Arc::new(Roles::default());
    roles.set(LOCAL, RoleLevel::Owner);
    roles.set(EDITOR, RoleLevel::Editor);
    roles.set(VIEWER, RoleLevel::Viewer);
    let audit = Arc::new(Audit::default());
    let sources = NetSources { roles: roles.clone(), audit: audit.clone(), credentials: Arc::new(NoCredentials), runtime: None };
    let net = Net::new(Arc::new(clock.clone()), sources);
    let spec = instance(PETAL, "inst-adapter");
    let key = spec.key();
    net.register_instance(&spec).unwrap();
    let fleet = |n: u32| format!("{FLEET}{n}");
    let refusal = |request: HttpIngressRequest| net.accept_http(request).map(|_| ()).unwrap_err();

    // §dial-outs at registration: a plaintext target or a malformed filter never spawns a redial loop.
    let states: HashMap<String, EndpointState> = net.endpoint_states(&key).into_iter().collect();
    let unavailable = |id: &str, needle: &str| matches!(&states[id], EndpointState::NotAvailable(reason) if reason.contains(needle));
    assert_eq!(states["telemetry"], EndpointState::Mounted);
    assert!(unavailable("feed", "network runtime"), "a valid dial-out on a host without a runtime");
    assert!(unavailable("rogue", "plaintext"), "ws:// beyond loopback never dials");
    assert!(unavailable("bus", "topic filter"), "a malformed filter never subscribes");

    // §credentials, inbound: the smuggled forward_headers reach the guest minus every credential header.
    let mut reply = net.accept_http(post(EDITOR, "telemetry", "application/json; charset=utf-8", br#"{"t":1}"#)).unwrap();
    let mut device = Device::default();
    assert_eq!(net.dispatch(&key, &mut device, 8), 1);
    let frame = &device.frames[0];
    assert_eq!(frame.headers, vec![("x-device-id".to_string(), "d1".to_string())], "credential headers never reach a guest");
    assert_eq!((frame.caller_did.as_deref(), frame.method.as_deref(), frame.path.as_deref()), (Some(EDITOR), Some("POST"), Some("")));
    assert_eq!(reply.try_recv().unwrap().map(|answer| answer.status), Some(200));

    // §ingress: steps 1-3 answer the uniform not-found (only the floor is audited; undeclared is not, E5), then 405/413/415.
    let rows: Vec<(&str, HttpIngressRequest, IngressRefusal, usize)> = vec![
        ("undeclared endpoint", post(&fleet(1), "nope", JSON, b"{}"), IngressRefusal::NotFound, 0),
        ("a websocket endpoint over http", post(&fleet(2), "live", JSON, b"{}"), IngressRefusal::NotFound, 0),
        ("below the role floor", post(VIEWER, "telemetry", JSON, b"{}"), IngressRefusal::NotFound, 1),
        (
            "below the floor hides 405/413/415",
            HttpIngressRequest { method: "DELETE".into(), sub_path: "x".into(), ..post(VIEWER, "telemetry", "text/plain", &[0; 512]) },
            IngressRefusal::NotFound,
            1,
        ),
        ("sub-path without sub_paths", HttpIngressRequest { sub_path: "admin".into(), ..post(&fleet(3), "telemetry", JSON, b"{}") }, IngressRefusal::NotFound, 0),
        ("undeclared method", HttpIngressRequest { method: "GET".into(), ..post(&fleet(4), "telemetry", JSON, b"{}") }, IngressRefusal::MethodNotAllowed, 0),
        ("over max_body_bytes", post(&fleet(5), "telemetry", JSON, &[b'x'; 65]), IngressRefusal::PayloadTooLarge, 0),
        ("undeclared media type", post(&fleet(6), "telemetry", "text/plain", b"{}"), IngressRefusal::UnsupportedMediaType, 0),
    ];
    for (rule, request, expected, audited) in rows {
        let before = audit.denials();
        assert_eq!(refusal(request), expected, "{rule}");
        assert_eq!(audit.denials() - before, audited, "{rule}: audit entries");
    }

    // Per-caller rate (rate_hz 2, method case-insensitive): two pass, the third is 429 until the bucket refills.
    let rate_caller = fleet(7);
    for _ in 0..2 {
        assert!(net.accept_http(HttpIngressRequest { method: "post".into(), ..post(&rate_caller, "telemetry", JSON, b"{}") }).is_ok());
    }
    assert_eq!(refusal(post(&rate_caller, "telemetry", JSON, b"{}")), IngressRefusal::TooManyRequests);
    clock.set(clock.now_ms() + 1_000);
    assert!(net.accept_http(post(&rate_caller, "telemetry", JSON, b"{}")).is_ok(), "refilled");
    assert_eq!(net.dispatch(&key, &mut Device::default(), 8), 3);

    // A full inbox answers 429 and counts the drop.
    for n in 0..INBOX_CAPACITY as u32 {
        assert!(net.accept_http(post(&fleet(1_000 + n), "telemetry", JSON, b"{}")).is_ok());
    }
    assert_eq!(refusal(post(&fleet(9_999), "telemetry", JSON, b"{}")), IngressRefusal::TooManyRequests);
    assert_eq!(net.counters_for_installation("inst-adapter").unwrap().ingress_dropped_queue, 1);
    assert_eq!(net.dispatch(&key, &mut Device::default(), usize::MAX), INBOX_CAPACITY);

    // Websocket accept: the floor (audited), max_connections, size then rate drops, replies on the same socket.
    let accept = |caller: &str| WsAcceptRequest { petal_id: PETAL.into(), package_id: PACKAGE.into(), endpoint: "live".into(), caller_did: caller.into() };
    let before = audit.denials();
    assert_eq!(net.open_websocket(accept(VIEWER)).unwrap_err(), IngressRefusal::NotFound);
    assert_eq!(audit.denials(), before + 1);
    let mut session = net.open_websocket(accept(EDITOR)).unwrap();
    assert_eq!(net.open_websocket(accept(&fleet(8))).unwrap_err(), IngressRefusal::TooManyRequests, "max_connections 1");
    let dropped_before = net.counters_for_installation("inst-adapter").unwrap();
    net.websocket_frame(session.connection, WsFrame::Text("x".repeat(17)));
    for text in ["hello", "again", "third"] {
        net.websocket_frame(session.connection, WsFrame::Text(text.into()));
    }
    let dropped = net.counters_for_installation("inst-adapter").unwrap();
    assert_eq!(
        (dropped.ingress_dropped_size - dropped_before.ingress_dropped_size, dropped.ingress_dropped_rate - dropped_before.ingress_dropped_rate),
        (1, 1),
        "17 bytes over max_frame_bytes 16; the third frame over rate_hz 2"
    );
    let mut device = Device::default();
    assert_eq!(net.dispatch(&key, &mut device, 8), 3, "opened + two frames");
    assert_eq!(device.connections, vec![("live".to_string(), session.connection, ConnectionEvent::Opened)]);
    assert_eq!(device.frames.iter().map(|frame| frame.body.clone()).collect::<Vec<_>>(), vec![b"hello".to_vec(), b"again".to_vec()]);
    assert_eq!(session.outbound.try_recv().unwrap(), WsOutbound::Frame(WsFrame::Text(r#"{"ok":true}"#.into())), "application/json replies as text");
    session.outbound.try_recv().unwrap();
    let send = |endpoint: &str, connection: Option<u64>, frame: WsFrame| net.send(&key, endpoint, connection, frame);
    assert!(matches!(send("live", None, WsFrame::Text("x".into())), Err(HostError::InvalidInput(_))), "accept mode names a connection");
    assert!(matches!(send("live", Some(session.connection), WsFrame::Text("x".repeat(17))), Err(HostError::InvalidInput(_))), "over max_frame_bytes");
    assert!(matches!(send("telemetry", None, WsFrame::Text("x".into())), Err(HostError::CapabilityDenied(_))), "not a websocket endpoint");
    send("live", Some(session.connection), WsFrame::Text("ping".into())).unwrap();
    assert_eq!(session.outbound.try_recv().unwrap(), WsOutbound::Frame(WsFrame::Text("ping".into())));

    // A caller demoted after the upgrade is disconnected on its next frame (1008, audited); the guest hears `closed` once.
    roles.set(EDITOR, RoleLevel::Viewer);
    let before = audit.denials();
    net.websocket_frame(session.connection, WsFrame::Text("still here".into()));
    assert!(matches!(session.outbound.try_recv().unwrap(), WsOutbound::Close { code: 1008, .. }));
    assert_eq!(audit.denials(), before + 1);
    let mut device = Device::default();
    assert_eq!(net.dispatch(&key, &mut device, 8), 1);
    assert_eq!(device.connections, vec![("live".to_string(), session.connection, ConnectionEvent::Closed)]);
    assert!(device.frames.is_empty(), "the demoted caller's frame never reaches the guest");
    net.websocket_closed(session.connection);
    assert_eq!(net.dispatch(&key, &mut Device::default(), 8), 0, "the writer's late report is not a second `closed`");
    roles.set(EDITOR, RoleLevel::Editor);

    // A guest close never puts a reserved code (1006) or an over-long reason on the wire.
    let mut session = net.open_websocket(accept(EDITOR)).unwrap();
    net.close(&key, "live", Some(session.connection), 1006, &"r".repeat(200)).unwrap();
    match session.outbound.try_recv().unwrap() {
        WsOutbound::Close { code, reason } => assert_eq!((code, reason.len()), (1000, 123)),
        other => panic!("expected a close, got {other:?}"),
    }
    net.websocket_closed(session.connection);
    net.dispatch(&key, &mut Device::default(), 8);

    // §credentials, outbound: a guest sets no credential or host-owned header, and none comes back in a response.
    let request = |method: Method, path: &str, headers: Vec<(String, String)>, body: &[u8]| OutgoingRequest {
        method,
        path: path.into(),
        query: Vec::new(),
        headers,
        content_type: Some(JSON.into()),
        body: body.to_vec(),
    };
    for name in [
        "Authorization", "Proxy-Authorization", "Cookie", "Set-Cookie", "X-Api-Key", "X-FE-Join-Lease", "x-fe-anything",
        "Host", "Content-Length", "Transfer-Encoding", "Connection", "Proxy-Foo", "Sec-WebSocket-Key",
    ] {
        let smuggled = vec![(name.to_string(), "stolen".to_string())];
        assert!(matches!(net.http_request(&key, "sink", request(Method::Post, "v1", smuggled, b"{}")), Err(HostError::InvalidInput(_))), "{name}");
    }
    let mut response = HeaderMap::new();
    for (name, value) in [("set-cookie", "s=1"), ("authorization", "Bearer x"), ("x-fe-join-lease", "l"), ("transfer-encoding", "chunked"), ("etag", "\"v1\"")] {
        response.append(name, HeaderValue::from_static(value));
    }
    assert_eq!(super::egress::response_headers(&response), vec![("etag".to_string(), "\"v1\"".to_string())]);

    // §credentials, endpoint secrets: re-read per use; an unfilled, empty or ill-fitting slot keeps the endpoint down.
    let slot = |scheme: AuthScheme, credential: &str, header: Option<&str>| AuthSlot { scheme, credential: credential.into(), header: header.map(str::to_string) };
    let filled = |username: Option<&str>, secret: &str| Credential { username: username.map(str::to_string), secret: secret.into() };
    let store = Slots(HashMap::from([
        ("token".to_string(), filled(None, "s3cret")),
        ("blank".to_string(), filled(None, "")),
        ("aladdin".to_string(), filled(Some("Aladdin"), "open sesame")),
    ]));
    let operator = Net::new(Arc::new(clock.clone()), NetSources { roles: roles.clone(), audit: audit.clone(), credentials: Arc::new(store), runtime: None });
    for (rule, auth, expected) in [
        ("an unfilled slot", slot(AuthScheme::Bearer, "absent", None), None),
        ("an empty secret", slot(AuthScheme::Bearer, "blank", None), None),
        ("basic without a username", slot(AuthScheme::Basic, "token", None), None),
        ("bearer", slot(AuthScheme::Bearer, "token", None), Some(("authorization", "Bearer s3cret"))),
        ("basic, RFC 7617's example", slot(AuthScheme::Basic, "aladdin", None), Some(("authorization", "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="))),
        ("a named header", slot(AuthScheme::Header, "token", Some("X-Api-Key")), Some(("x-api-key", "s3cret"))),
        ("a header the host owns", slot(AuthScheme::Header, "token", Some("Host")), None),
    ] {
        let header = match operator.credential(Some(&auth)) {
            Err(state) => {
                assert_eq!(state, EndpointState::CredentialMissing, "{rule}");
                None
            }
            Ok(shaped) => shaped.and_then(|(scheme, credential, header)| auth_header(scheme, &credential, header.as_deref())),
        };
        assert_eq!(header, expected.map(|(name, value)| (name.to_string(), value.to_string())), "{rule}");
    }
    assert!(!format!("{:?}", filled(Some("Aladdin"), "open sesame")).contains("open sesame"), "Debug never prints a secret");
    let missing = net.http_request(&key, "vault", request(Method::Get, "", vec![], b""));
    assert!(matches!(missing, Err(HostError::NotAvailable(reason)) if reason == "credential-missing"));

    // §egress containment: anything that could leave the declared base is invalid-input; the join keeps the base.
    let base = "https://bi.example.org/api/";
    for path in [
        "../admin", "v1/../../admin", "v1/%2e%2e/admin", "v1/%2E%2E/admin", "v1/%252e%252e/admin", "v1/..;/admin", "v1/%2fadmin",
        "a\\b", "//evil.example/x", "https://evil.example/", "v1?x=1", "v1#top", "user@evil.example", "v1/a b", "v1/\u{0}",
    ] {
        assert!(super::egress::endpoint_url(base, path, &[]).is_err(), "{path}");
    }
    let joined = super::egress::endpoint_url(base, "/v1/push", &[("a".into(), "b c".into())]).unwrap();
    assert_eq!(joined.as_str(), "https://bi.example.org/api/v1/push?a=b+c");
    assert_eq!(super::egress::endpoint_url(base, "", &[]).unwrap().as_str(), base);
    let too_many: Vec<(String, String)> = (0..65).map(|n| (format!("k{n}"), String::new())).collect();
    assert!(super::egress::endpoint_url(base, "v1", &too_many).is_err(), "at most 64 query parameters");

    // Redirects are followed only inside the base: same scheme, host and port, no userinfo, under the base path.
    let base_url = reqwest::Url::parse(base).unwrap();
    for (location, followed) in [
        ("https://bi.example.org/api/v2/x", true),
        ("https://bi.example.org/api", true),
        ("https://bi.example.org/apix/x", false),
        ("https://bi.example.org/api/../admin", false),
        ("https://bi.example.org:8443/api/x", false),
        ("http://bi.example.org/api/x", false),
        ("https://evil.example/api/x", false),
        ("https://user@bi.example.org/api/x", false),
    ] {
        assert_eq!(super::egress::within_base(&reqwest::Url::parse(location).unwrap(), &base_url), followed, "{location}");
    }

    // An egress client is never built for plaintext beyond loopback, userinfo or a query in the base.
    for (url, rule) in [("http://bi.example.org/", "plaintext"), ("https://user:pw@bi.example.org/", "userinfo"), ("https://bi.example.org/?k=v", "query")] {
        let http: HttpEgress = serde_json::from_value(json!({ "id": "x", "url": url, "methods": ["GET"], "rate_hz": 1 })).unwrap();
        assert!(super::egress::build_client(&http).is_err(), "{rule}");
    }

    // §egress refusals: undeclared endpoints and methods are audited capability denials (R9 AC); every refusal is counted.
    let refused_before = net.counters_for_installation("inst-adapter").unwrap().egress_refused;
    let egress_rows = [
        ("undeclared endpoint", "elsewhere", request(Method::Post, "v1", vec![], b"{}"), "capability-denied", 1),
        ("undeclared method", "sink", request(Method::Get, "v1", vec![], b"{}"), "capability-denied", 1),
        ("a path that leaves the base", "sink", request(Method::Post, "../admin", vec![], b"{}"), "invalid-input", 0),
        ("over max_request_bytes", "sink", request(Method::Post, "v1", vec![], b"123456789"), "budget-exceeded", 0),
        ("no client on a host without a runtime", "sink", request(Method::Post, "v1", vec![], b"{}"), "not-available", 0),
        ("over rate_hz", "sink", request(Method::Post, "v1", vec![], b"{}"), "rate-limited", 0),
    ];
    let egress_calls = egress_rows.len() as u64;
    for (rule, endpoint, outgoing, code, audited) in egress_rows {
        let before = audit.denials();
        let error = net.http_request(&key, endpoint, outgoing).unwrap_err();
        assert_eq!((error.code(), audit.denials() - before), (code, audited), "{rule}");
    }
    assert_eq!(net.counters_for_installation("inst-adapter").unwrap().egress_refused - refused_before, egress_calls);

    // §egress MQTT: a declared filter, no wildcard publish, size, QoS and retain as declared, then rate.
    let topic = "site-a/d1/command";
    let publish_rows = [
        ("undeclared endpoint", "nowhere", topic, 1usize, Qos::AtMostOnce, false, "capability-denied", 1usize),
        ("a topic outside the filters", "uplink", "site-b/d1/command", 1, Qos::AtMostOnce, false, "capability-denied", 1),
        ("a wildcard topic", "uplink", "site-a/+/command", 1, Qos::AtMostOnce, false, "invalid-input", 0),
        ("over max_payload_bytes", "uplink", topic, 9, Qos::AtMostOnce, false, "budget-exceeded", 0),
        ("undeclared at-least-once", "uplink", topic, 1, Qos::AtLeastOnce, false, "invalid-input", 0),
        ("undeclared retain", "uplink", topic, 1, Qos::AtMostOnce, true, "invalid-input", 0),
        ("no session on a host without a runtime", "uplink", topic, 1, Qos::AtMostOnce, false, "not-available", 0),
        ("over rate_hz", "uplink", topic, 1, Qos::AtMostOnce, false, "rate-limited", 0),
    ];
    for (rule, endpoint, topic, size, qos, retain, code, audited) in publish_rows {
        let before = audit.denials();
        let error = net.mqtt_publish(&key, endpoint, topic, vec![0; size], qos, retain).unwrap_err();
        assert_eq!((error.code(), audit.denials() - before), (code, audited), "{rule}");
    }

    // §dial-outs: redial bounds are clamped; jittered backoff stays inside them and its ceiling grows per attempt.
    assert_eq!(reconnect_bounds(None), (1_000, 60_000));
    assert_eq!(reconnect_bounds(Some(&ReconnectMs { min: 0, max: 999_999_999 })), (250, 600_000), "a smuggled hot loop is clamped");
    assert_eq!(reconnect_bounds(Some(&ReconnectMs { min: 5_000, max: 1_000 })), (5_000, 5_000));
    for attempt in 1..=24u32 {
        let wait = backoff_ms(attempt, 1_000, 60_000);
        let ceiling = (1_000u64 << attempt.min(16)).min(60_000);
        assert!((1_000..=ceiling).contains(&wait), "attempt {attempt}: {wait} ms");
    }
    assert_eq!(backoff_ms(3, 250, 250), 250);

    // Plaintext only to loopback; credentials, queries and other schemes never dial.
    for (url, expected) in [
        ("wss://feed.example.org/v1", Some((true, true, "feed.example.org", 443))),
        ("ws://127.0.0.1:9001/", Some((false, true, "127.0.0.1", 9001))),
        ("mqtt://[::1]", Some((false, false, "::1", 1883))),
        ("mqtts://broker.example.org", Some((true, false, "broker.example.org", 8883))),
        ("ws://feed.example.org/", None),
        ("mqtt://broker.example.org", None),
        ("ws://localhost.evil.example/", None),
        ("wss://user:pw@feed.example.org/", None),
        ("wss://feed.example.org/?token=x", None),
        ("https://feed.example.org/", None),
    ] {
        let parsed = super::connect::target(url).ok().map(|target| (target.tls, target.websocket, target.host, target.port));
        assert_eq!(parsed, expected.map(|(tls, websocket, host, port)| (tls, websocket, host.to_string(), port)), "{url}");
    }

    // Generations: a dial-out task that outlives a re-registration never writes into the new instance.
    let second = instance("01JPETAL2", "inst-adapter-2");
    let second_key = second.key();
    net.register_instance(&second).unwrap();
    let stale = net.lock().instances[&second_key].generation;
    net.unregister_instance(&second_key);
    net.register_instance(&second).unwrap();
    let current = net.lock().instances[&second_key].generation;
    assert_ne!(stale, current);
    assert_eq!(net.dial_opened(&second_key, stale, "feed"), None, "the stale task is told to stop");
    net.set_endpoint_state(&second_key, stale, "feed", EndpointState::Up);
    net.count(&second_key, stale, |counters| counters.ingress_frames += 1);
    let feed = |net: &Net| net.endpoint_states(&second_key).into_iter().find(|(id, _)| id == "feed").map(|(_, state)| state);
    assert!(matches!(feed(&net), Some(EndpointState::NotAvailable(_))), "stale state writes are dropped");
    assert_eq!(net.counters_for_installation("inst-adapter-2").unwrap().ingress_frames, 0, "stale counts are dropped");
    let connection = net.dial_opened(&second_key, current, "feed").expect("the live task connects");
    assert_eq!(feed(&net), Some(EndpointState::Up));
    let mut device = Device::default();
    assert_eq!(net.dispatch(&second_key, &mut device, 8), 1);
    assert_eq!(device.connections, vec![("feed".to_string(), connection, ConnectionEvent::Opened)]);

    // §mqtt: an oversized packet is known from its fixed header and skipped as it streams past, never buffered.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let (mut broker, client) = tokio::io::duplex(64 * 1024);
        let mut stream: super::connect::Stream = Box::new(client);
        let mut reader = super::mqtt_session::Reader::new(64);
        let flood = Packet::Publish { topic: "t".into(), packet_id: None, qos: 0, retain: false, dup: false, payload: vec![7; 4_096] };
        broker.write_all(&encode(&flood).unwrap()).await.unwrap();
        broker.write_all(&encode(&Packet::PingResp).unwrap()).await.unwrap();
        assert_eq!(reader.next(&mut stream).await, Ok(Packet::PingResp));
        assert_eq!(reader.take_oversized(), 1);
        assert!(reader.buffered() <= 64, "nothing of the flood stays buffered");
        // A header announcing the protocol maximum (256 MiB), then the broker hangs up: skipped, never allocated.
        broker.write_all(&[0x30, 0xff, 0xff, 0xff, 0x7f]).await.unwrap();
        drop(broker);
        assert!(reader.next(&mut stream).await.is_err(), "the reader reports the close");
        assert_eq!((reader.take_oversized(), reader.buffered()), (1, 0));
    });

    // Disabled = unregistered: the mount answers the uniform not-found, unaudited (E5 default).
    let before = audit.denials();
    net.unregister_instance(&key);
    assert_eq!(refusal(post(EDITOR, "telemetry", JSON, b"{}")), IngressRefusal::NotFound);
    assert_eq!(audit.denials(), before);
}
