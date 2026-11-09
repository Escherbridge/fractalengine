//! Ingress/egress journey through the public `Net` seam (spec R8/R9 tables; P2P-QA credential steer 2026-11-08 20:20).

use std::sync::{Arc, Mutex};

use fe_policy::RoleLevel;
use fe_sdk::manifest_v2::{ExtensionManifest, Method};
use serde_json::json;

use super::*;
use crate::bus::sources::{AuditAction, AuditEntry, AuditSink, RoleSource};
use crate::bus::{HostError, InstanceSpec};
use crate::tick::{Clock, ManualClock};

const LOCAL: &str = "did:key:z6MkLocalHost";
const EDITOR: &str = "did:key:z6MkDeviceEditor";
const VIEWER: &str = "did:key:z6MkViewer";
const PETAL: &str = "01JPETAL";
const PACKAGE: &str = "org.example.device-adapter";

struct Roles;

impl RoleSource for Roles {
    fn role_at(&self, did: &str, _petal_id: &str) -> RoleLevel {
        match did {
            LOCAL => RoleLevel::Owner,
            EDITOR => RoleLevel::Editor,
            VIEWER => RoleLevel::Viewer,
            _ => RoleLevel::None,
        }
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

/// Records frames and connection events; answers every frame with a small JSON body.
#[derive(Default)]
struct Device {
    frames: Vec<IngressFrame>,
    connections: Vec<(String, u64, ConnectionEvent)>,
}

impl NetHooks for Device {
    fn on_ingress(&mut self, frame: IngressFrame) -> Option<IngressReply> {
        self.frames.push(frame);
        Some(IngressReply { status: 200, content_type: Some("application/json".into()), body: br#"{"ok":true}"#.to_vec() })
    }

    fn on_connection(&mut self, endpoint: &str, connection: u64, event: ConnectionEvent) {
        self.connections.push((endpoint.to_string(), connection, event));
    }

    fn on_egress_result(&mut self, _request: u64, _outcome: Result<IncomingResponse, HostError>) {}
}

/// `forward_headers` names `authorization` on purpose: the schema refuses it (V16), the host must strip it anyway.
fn adapter() -> InstanceSpec {
    let manifest: ExtensionManifest = serde_json::from_value(json!({
        "manifest_version": 2, "id": PACKAGE, "version": "0.1.0", "name": "Device adapter", "kind": "device-adapter",
        "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
        "ingress": [
            { "id": "telemetry", "transport": "http-server", "methods": ["POST"], "content_types": ["application/json"],
              "max_body_bytes": 64, "rate_hz": 2, "role_floor": "editor", "forward_headers": ["x-device-id", "authorization", "x-fe-join-lease"] },
            { "id": "live", "transport": "websocket-accept", "max_frame_bytes": 16, "rate_hz": 2, "max_connections": 1 }
        ],
        "egress": [
            { "id": "sink", "transport": "http", "url": "https://bi.example.org/api/", "methods": ["POST"], "rate_hz": 1 }
        ]
    }))
    .expect("test manifest");
    InstanceSpec {
        installation_id: "inst-adapter".into(),
        content_hash: "cd".repeat(32),
        verse_id: "01JVERSE".into(),
        petal_id: PETAL.into(),
        identity_did: LOCAL.into(),
        manifest: Arc::new(manifest),
    }
}

fn post(caller: &str, endpoint: &str, content_type: &str, body: &[u8]) -> HttpIngressRequest {
    HttpIngressRequest {
        petal_id: PETAL.into(),
        package_id: PACKAGE.into(),
        endpoint: endpoint.into(),
        caller_did: caller.into(),
        method: "POST".into(),
        sub_path: String::new(),
        query: vec![("since".into(), "0".into())],
        headers: vec![
            ("X-Device-Id".into(), "d1".into()),
            ("Authorization".into(), "Bearer fe-api-token".into()),
            ("Cookie".into(), "session=1".into()),
            ("x-fe-join-lease".into(), "lease".into()),
        ],
        content_type: Some(content_type.into()),
        body: body.to_vec(),
    }
}

/// Ingress table, credential stripping, websocket accept limits, and egress refusals before any network I/O.
#[test]
fn net_journey_ingress_table_credentials_and_egress_refusals() {
    let clock = ManualClock::default();
    clock.set(1_000);
    let audit = Arc::new(Audit::default());
    let sources = NetSources { roles: Arc::new(Roles), audit: audit.clone(), credentials: Arc::new(NoCredentials), runtime: None };
    let net = Net::new(Arc::new(clock.clone()), sources);
    let spec = adapter();
    let key = spec.key();
    net.register_instance(&spec).unwrap();

    // Declared endpoint: the frame reaches the guest with only the declared, non-credential headers; the reply returns.
    let mut reply = net.accept_http(post(EDITOR, "telemetry", "application/json; charset=utf-8", br#"{"t":1}"#)).unwrap();
    let mut device = Device::default();
    assert_eq!(net.dispatch(&key, &mut device, 8), 1);
    let frame = &device.frames[0];
    assert_eq!(frame.headers, vec![("x-device-id".to_string(), "d1".to_string())], "credential headers never reach a guest");
    assert_eq!((frame.caller_did.as_deref(), frame.method.as_deref(), frame.path.as_deref()), (Some(EDITOR), Some("POST"), Some("")));
    assert_eq!(reply.try_recv().unwrap().map(|answer| answer.status), Some(200));

    // The R8 table: each refusal is distinct; only the role-floor refusal is audited.
    clock.set(clock.now_ms() + 1_000);
    let denials = audit.denials();
    let refusal = |request: HttpIngressRequest| net.accept_http(request).map(|_| ()).unwrap_err();
    assert_eq!(refusal(post(EDITOR, "telemetry", "application/json", &[b'x'; 65])), IngressRefusal::PayloadTooLarge);
    assert_eq!(refusal(HttpIngressRequest { method: "GET".into(), ..post(EDITOR, "telemetry", "application/json", b"{}") }), IngressRefusal::MethodNotAllowed);
    assert_eq!(refusal(post(EDITOR, "telemetry", "application/json", b"{}")), IngressRefusal::TooManyRequests, "rate_hz 2 per caller");
    clock.set(clock.now_ms() + 1_000);
    assert_eq!(refusal(post(EDITOR, "telemetry", "text/plain", b"{}")), IngressRefusal::UnsupportedMediaType);
    assert_eq!(refusal(post(EDITOR, "undeclared", "application/json", b"{}")), IngressRefusal::NotFound);
    assert_eq!(refusal(post(VIEWER, "telemetry", "application/json", b"{}")), IngressRefusal::NotFound, "below the floor looks undeclared");
    assert_eq!(audit.denials(), denials + 1);

    // Websocket accept: connection cap, over-size frames dropped and counted, replies go back on the same socket.
    let accept = WsAcceptRequest { petal_id: PETAL.into(), package_id: PACKAGE.into(), endpoint: "live".into(), caller_did: EDITOR.into() };
    let mut session = net.open_websocket(accept.clone()).unwrap();
    assert_eq!(net.open_websocket(accept).unwrap_err(), IngressRefusal::TooManyRequests, "max_connections 1");
    net.websocket_frame(session.connection, WsFrame::Text("x".repeat(17)));
    net.websocket_frame(session.connection, WsFrame::Text("hello".into()));
    let mut device = Device::default();
    assert_eq!(net.dispatch(&key, &mut device, 8), 2, "opened + one frame");
    assert_eq!(device.connections, vec![("live".to_string(), session.connection, ConnectionEvent::Opened)]);
    assert_eq!(device.frames[0].body, b"hello".to_vec());
    assert_eq!(session.outbound.try_recv().unwrap(), WsOutbound::Frame(WsFrame::Text(r#"{"ok":true}"#.into())));
    net.send(&key, "live", Some(session.connection), WsFrame::Text("ping".into())).unwrap();
    assert_eq!(session.outbound.try_recv().unwrap(), WsOutbound::Frame(WsFrame::Text("ping".into())));
    assert!(matches!(net.send(&key, "telemetry", None, WsFrame::Text("x".into())), Err(HostError::CapabilityDenied(_))));
    net.websocket_closed(session.connection);
    assert_eq!(net.counters_for_installation("inst-adapter").unwrap().ingress_dropped_size, 2, "the 65-byte POST and the 17-byte frame");

    // Egress: undeclared endpoints and methods are audited denials; paths and headers that leave the base are invalid.
    let denials = audit.denials();
    let request = |method: Method, path: &str, headers: Vec<(String, String)>| OutgoingRequest {
        method,
        path: path.into(),
        query: Vec::new(),
        headers,
        content_type: Some("application/json".into()),
        body: b"{}".to_vec(),
    };
    assert!(matches!(net.http_request(&key, "elsewhere", request(Method::Post, "v1", vec![])), Err(HostError::CapabilityDenied(_))));
    assert!(matches!(net.http_request(&key, "sink", request(Method::Get, "v1", vec![])), Err(HostError::CapabilityDenied(_))));
    assert_eq!(audit.denials(), denials + 2);
    for path in ["../admin", "v1/%2e%2e/admin", "//evil.example/x", "https://evil.example/", "a\\b"] {
        assert!(matches!(net.http_request(&key, "sink", request(Method::Post, path, vec![])), Err(HostError::InvalidInput(_))), "{path}");
    }
    let smuggled = vec![("Authorization".to_string(), "Bearer stolen".to_string())];
    assert!(matches!(net.http_request(&key, "sink", request(Method::Post, "v1", smuggled)), Err(HostError::InvalidInput(_))));
    assert!(matches!(net.http_request(&key, "sink", request(Method::Post, "v1", vec![])), Err(HostError::NotAvailable(_))), "no runtime, no client");
    let joined = super::egress::endpoint_url("https://bi.example.org/api/", "v1/push", &[("a".into(), "b c".into())]).unwrap();
    assert_eq!(joined.as_str(), "https://bi.example.org/api/v1/push?a=b+c");

    // Disabled = unregistered: the mount answers the uniform not-found.
    net.unregister_instance(&key);
    assert_eq!(net.accept_http(post(EDITOR, "telemetry", "application/json", b"{}")).map(|_| ()).unwrap_err(), IngressRefusal::NotFound);
}
