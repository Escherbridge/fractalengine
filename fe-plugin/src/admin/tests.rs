//! Admin channel journeys (batch R11-ADMIN: `cargo test -p fe-plugin -- admin::tests::`).

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::protocol::*;
use super::transport::*;

fn decode(value: serde_json::Value) -> Result<AdminRequest, AdminReply> {
    decode_request(&serde_json::to_vec(&value).unwrap())
}

fn refusal_code(reply: &AdminReply) -> &str {
    match &reply.outcome {
        ReplyOutcome::Refused { code, .. } => code.as_str(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn absolute_package_path() -> String {
    std::env::temp_dir().join("fleet-sim.hexon").display().to_string()
}

/// Every verb decodes and re-encodes to the same request; every refused shape answers its code and echoes the id.
#[test]
fn admin_request_table_accepts_the_eleven_verbs_and_refuses_everything_else() {
    let path = absolute_package_path();
    let accepted = [
        (json!({"v":1,"id":"a1","verb":"list"}), "list", None),
        (json!({"v":1,"id":"a2","verb":"status","args":{"installation_id":"01JINST"}}), "status", None),
        (json!({"v":1,"id":"a3","verb":"install-file","args":{"path":path}}), "install-file", Some("install")),
        (
            json!({"v":1,"id":"a4","verb":"install-registry","args":{"source_id":"default","package_id":"org.example.fleet-sim","version":"1.2.0"}}),
            "install-registry",
            Some("install"),
        ),
        (json!({"v":1,"id":"a5","verb":"enable","args":{"installation_id":"01JINST"}}), "enable", Some("enable")),
        (json!({"v":1,"id":"a6","verb":"disable","args":{"installation_id":"01JINST","binding_id":"b1"}}), "disable", Some("disable")),
        (json!({"v":1,"id":"a7","verb":"update","args":{"installation_id":"01JINST"}}), "update", Some("update")),
        (json!({"v":1,"id":"a8","verb":"unbind","args":{"installation_id":"01JINST","verse_id":"01JVERSE"}}), "unbind", Some("unbind")),
        (json!({"v":1,"id":"a9","verb":"uninstall","args":{"installation_id":"01JINST"}}), "uninstall", Some("uninstall")),
        (json!({"v":1,"id":"a10","verb":"disable-all","args":{}}), "disable-all", Some("disable")),
        (json!({"v":1,"id":"a11","verb":"enable-all"}), "enable-all", Some("enable")),
    ];
    for (wire, name, audit) in accepted {
        let request = decode(wire.clone()).unwrap_or_else(|reply| panic!("{name} refused: {reply:?}"));
        assert_eq!(request.verb.name(), name);
        assert_eq!(request.verb.audit_action(), audit, "{name}");
        let round_trip = decode_request(&encode_request(&request).unwrap()).unwrap();
        assert_eq!(round_trip, request, "{name} survives a round trip");
    }

    let refused = [
        // Consent and local acceptance are GUI-only: no verb grants them.
        (json!({"v":1,"id":"r1","verb":"consent","args":{"installation_id":"i","capability_digest":"d"}}), "r1", "bad-request"),
        (json!({"v":1,"id":"r2","verb":"bind","args":{"installation_id":"i","verse_id":"v"}}), "r2", "bad-request"),
        (json!({"v":1,"id":"r3","verb":"kill-list-override","args":{}}), "r3", "bad-request"),
        // No identity or credential ever rides the channel; the app stamps its own local DID.
        (json!({"v":1,"id":"r4","verb":"list","token":"secret"}), "r4", "bad-request"),
        (json!({"v":1,"id":"r5","verb":"enable","args":{"installation_id":"i","actor_did":"did:key:z6Mk"}}), "r5", "bad-request"),
        (json!({"v":2,"id":"r6","verb":"list"}), "r6", "unsupported-version"),
        (json!({"id":"r7","verb":"list"}), "r7", "bad-request"),
        (json!({"v":1,"id":"bad id!","verb":"list"}), "", "bad-request"),
        (json!({"v":1,"id":"r9","verb":"install-file","args":{"path":"relative/fleet.hexon"}}), "r9", "invalid-input"),
        (json!({"v":1,"id":"r10","verb":"install-registry","args":{"source_id":"s","package_id":"x.json"}}), "r10", "invalid-input"),
        (json!({"v":1,"id":"r11","verb":"install-registry","args":{"source_id":"s","package_id":"ok.pkg","version":"1.0.0+build"}}), "r11", "invalid-input"),
        (json!({"v":1,"id":"r12","verb":"status","args":{"installation_id":"has space"}}), "r12", "invalid-input"),
    ];
    for (wire, id, code) in refused {
        let reply = decode(wire.clone()).expect_err(&format!("{wire} must be refused"));
        assert_eq!(reply.id, id, "{wire}");
        assert_eq!(refusal_code(&reply), code, "{wire}");
    }
    assert!(decode_request(b"not json").is_err());

    for (value, enabled) in [(None, true), (Some("1"), true), (Some(""), true), (Some("0"), false), (Some(" 0 "), false)] {
        assert_eq!(super::channel_enabled_by(value), enabled, "FE_EXT_ADMIN_CHANNEL={value:?}");
    }
    assert!(LocalEndpoint::named("peer-a_1").is_ok());
    assert!(LocalEndpoint::named("../escape").is_err());
    assert!(LocalEndpoint::named(&"x".repeat(33)).is_err());
}

/// The 64 KiB cap holds on read (before allocating or waiting for the body) and on write; replies always fit.
#[tokio::test]
async fn admin_frames_enforce_the_cap_and_close_cleanly() {
    let (mut client, mut server) = tokio::io::duplex(1024);
    client.write_all(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes()).await.unwrap();
    // No body follows: the reader must refuse on the prefix alone instead of waiting for 64 KiB.
    assert!(matches!(read_frame(&mut server).await, Err(FrameError::TooLarge(n)) if n == MAX_FRAME_BYTES + 1));

    client.write_all(&0u32.to_be_bytes()).await.unwrap();
    assert!(matches!(read_frame(&mut server).await, Err(FrameError::Empty)));

    client.write_all(&10u32.to_be_bytes()).await.unwrap();
    client.write_all(b"abc").await.unwrap();
    drop(client);
    assert!(matches!(read_frame(&mut server).await, Err(FrameError::Truncated)));

    let (client, mut server) = tokio::io::duplex(1024);
    drop(client);
    assert!(matches!(read_frame(&mut server).await, Ok(None)), "a close between frames is clean");

    let (mut sink, _keep) = tokio::io::duplex(1024);
    assert!(matches!(write_frame(&mut sink, &vec![b'x'; MAX_FRAME_BYTES + 1]).await, Err(FrameError::TooLarge(_))));

    let huge = AdminReply::ok(
        "big",
        AdminResult { installations: Vec::new(), message: Some("x".repeat(MAX_FRAME_BYTES)) },
    );
    let body = encode_reply(&huge);
    assert!(body.len() <= MAX_FRAME_BYTES);
    assert_eq!(refusal_code(&decode_reply(&body).unwrap()), "reply-too-large");
}

/// Foreign peers never reach a request; a CLI install waits on the GUI dialog (approved, then declined); a bad frame
/// is answered then closed; the real OS transport admits this user and reports an absent app as not running.
#[tokio::test]
async fn admin_channel_journey_memory_listener_consent_and_platform_transport() {
    let (mut listener, connector) = memory_listener();

    let mut foreign = connector.connect(PeerUser::Other("uid 1001".into())).unwrap();
    let mut own = connector.connect(PeerUser::Same).unwrap();
    let mut served = listener.accept().await.unwrap();
    assert_eq!(listener.rejected_count(), 1, "the foreign connection was dropped before the own one was served");
    let mut probe = [0u8; 1];
    assert_eq!(foreign.read(&mut probe).await.unwrap(), 0, "the foreign client sees the channel close");

    let consent = PendingConsent {
        installation_id: "01JINST".into(),
        package_id: "org.example.fleet-sim".into(),
        name: "Fleet sim".into(),
        version: "1.2.0".into(),
        capability_digest: "b71e".into(),
        is_update: false,
    };
    let installed = InstallationSummary {
        installation_id: "01JINST".into(),
        package_id: "org.example.fleet-sim".into(),
        name: "Fleet sim".into(),
        version: "1.2.0".into(),
        status: "Running".into(),
        ..Default::default()
    };
    let server = {
        let consent = consent.clone();
        let installed = installed.clone();
        tokio::spawn(async move {
            // install-file: the dialog opens, the operator approves in the GUI.
            let Incoming::Request(request) = next_request(&mut served).await else { panic!("expected install-file") };
            assert!(matches!(request.verb, AdminVerb::InstallFile { .. }));
            write_reply(&mut served, &AdminReply::pending_consent(&request.id, consent.clone())).await.unwrap();
            let result = AdminResult { installations: vec![installed], message: None };
            write_reply(&mut served, &AdminReply::ok(&request.id, result)).await.unwrap();
            // update: the dialog opens, the operator declines.
            let Incoming::Request(request) = next_request(&mut served).await else { panic!("expected update") };
            write_reply(&mut served, &AdminReply::pending_consent(&request.id, PendingConsent { is_update: true, ..consent })).await.unwrap();
            write_reply(&mut served, &AdminReply::refused(&request.id, RefusalCode::ConsentDeclined, "declined in the app")).await.unwrap();
            // An oversize frame is refused, then the connection closes.
            let Incoming::Refuse(refusal) = next_request(&mut served).await else { panic!("expected a refusal") };
            write_reply(&mut served, &refusal).await.unwrap();
            refusal
        })
    };

    let mut pending = Vec::new();
    let install = AdminRequest::new("c1", AdminVerb::InstallFile { path: absolute_package_path() });
    let reply = exchange(&mut own, &install, |p| pending.push(p.clone())).await.unwrap();
    assert_eq!(pending, vec![consent.clone()], "the CLI was told to look at the GUI dialog exactly once");
    assert_eq!(reply.outcome, ReplyOutcome::Ok { result: AdminResult { installations: vec![installed], message: None } });

    pending.clear();
    let update = AdminRequest::new("c2", AdminVerb::Update { installation_id: "01JINST".into() });
    let reply = exchange(&mut own, &update, |p| pending.push(p.clone())).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(refusal_code(&reply), "consent-declined");

    own.write_all(&((MAX_FRAME_BYTES as u32) * 2).to_be_bytes()).await.unwrap();
    let refusal = server.await.unwrap();
    assert_eq!(refusal_code(&refusal), "frame-too-large");
    let body = read_frame(&mut own).await.unwrap().expect("the refusal arrives before the close");
    assert_eq!(refusal_code(&decode_reply(&body).unwrap()), "frame-too-large");

    // The OS transport, under a per-test instance so a running app is never touched.
    let instance = format!("t{}", ulid::Ulid::new().to_string().to_ascii_lowercase());
    let endpoint = LocalEndpoint::named(&instance).unwrap();
    assert!(matches!(connect_local(&endpoint).await, Err(ConnectError::NotRunning(_))), "no app yet");
    let mut os_listener = listen_local(&endpoint).unwrap();
    assert!(listen_local(&endpoint).is_err(), "a second app cannot take over a live channel");
    let os_server = tokio::spawn(async move {
        let mut stream = os_listener.accept().await.unwrap();
        let Incoming::Request(request) = next_request(&mut stream).await else { panic!("expected list") };
        write_reply(&mut stream, &AdminReply::ok(&request.id, AdminResult::default())).await.unwrap();
        os_listener.rejected_count()
    });
    let mut stream = connect_local(&endpoint).await.unwrap();
    let reply = exchange(&mut stream, &AdminRequest::new("os1", AdminVerb::List {}), |_| panic!("list never needs consent"))
        .await
        .unwrap();
    assert!(reply.is_final());
    assert_eq!(os_server.await.unwrap(), 0, "this OS user is admitted");
}

