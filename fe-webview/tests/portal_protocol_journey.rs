//! Portal bundle journey over the real mount table, gate and `fe-portal` protocol; only the webview is faked.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use fe_format::portal_bundle::PortalCsp;
use fe_webview::ipc::{BlockedReason, BrowserEvent, BundleState, RevokedReason};
use fe_webview::portal_bundle::csp::{compose, UnsafeSource};
use fe_webview::portal_bundle::origin::{self, admitted_label, static_label, LabelKind};
use fe_webview::portal_bundle::protocol::serve;
use fe_webview::portal_bundle::{
    prepare, AdmissionVerdict, Attempt, BundleHost, BundleIdentity, BundleParts, BundleResolver, BundleSource,
    BundleWebview, Decision, MountMode, NavigationGate, PortalMounts, RefuseReason, UNLOAD_URL,
};
use http::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use http::StatusCode;
use serde_json::{json, Value};

const PACKAGE: &str = "org.example.flow-dashboard";
const MEDIA: &[u8] = b"0123456789abcdef";

// ---------------------------------------------------------------------------
// Fixture archives (README §1-§3 layout), built and sealed in-process
// ---------------------------------------------------------------------------

/// A bundle archive on disk, removed on drop.
struct Fixture {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl Fixture {
    fn write(bytes: Vec<u8>) -> Self {
        let path = std::env::temp_dir().join(format!("fe-portal-journey-{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, &bytes).expect("write fixture");
        Self { path, bytes }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn portal(navigation: &[&str], wasm: bool) -> Value {
    json!({
        "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Flow dashboard",
        "csp": { "connect": [], "img": [], "font": ["https://fonts.example.net"], "media": [], "frame": [],
                 "wasm": wasm, "inline_styles": false },
        "navigation": navigation,
        "subscriptions": { "node": false, "readings": false, "queries": [] },
        "commands": {}, "hooks": [], "streams": [], "ui_intents": []
    })
}

/// An unsealed archive: header members, `plugin/extension.json` and the `web/` body.
fn archive(version: &str, portal: Value, page: &str) -> Vec<u8> {
    let extension = json!({ "manifest_version": 2, "id": PACKAGE, "version": version, "kind": "web-bundle", "portal": portal });
    let manifest = json!({
        "schema_version": "1.0.0", "hexon_id": PACKAGE, "hexon_type": "plugin",
        "publisher_did": "did:key:z6Mkunsealed", "version": version, "name": "Flow dashboard",
        "created_at": "2026-11-08T00:00:00Z", "updated_at": "2026-11-08T00:00:00Z"
    });
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let members: [(&str, Vec<u8>); 7] = [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("entries.json", br#"{"entries":[]}"#.to_vec()),
        ("plugin/extension.json", serde_json::to_vec(&extension).unwrap()),
        ("web/index.html", format!("<!doctype html><title>{page}</title>").into_bytes()),
        ("web/app.js", b"document.body.dataset.ready = '1';".to_vec()),
        ("web/docs/index.html", b"<!doctype html><title>docs</title>".to_vec()),
        ("web/media.bin", MEDIA.to_vec()),
    ];
    for (name, bytes) in members {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn publisher_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// Flip the first byte of `target` and re-zip it with a fresh CRC: only the seal can catch it.
fn tamper(bytes: &[u8], target: &str) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let (name, method, mut data) = {
            let mut file = source.by_index(index).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (file.name().to_owned(), file.compression(), data)
        };
        if name == target {
            data[0] ^= 0xFF;
            out.start_file(name.as_str(), zip::write::SimpleFileOptions::default().compression_method(method))
                .unwrap();
            out.write_all(&data).unwrap();
        } else {
            out.raw_copy_file(source.by_index_raw(index).unwrap()).unwrap();
        }
    }
    out.finish().unwrap().into_inner()
}

// ---------------------------------------------------------------------------
// The faked edge: a webview that records what the host asked of it
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeWebview {
    loads: Vec<String>,
    shown: bool,
}

impl BundleWebview for FakeWebview {
    fn bundle_load(&mut self, url: &str) -> anyhow::Result<()> {
        self.loads.push(url.to_owned());
        Ok(())
    }
    fn bundle_show(&mut self) -> anyhow::Result<()> {
        self.shown = true;
        Ok(())
    }
    fn bundle_hide(&mut self) -> anyhow::Result<()> {
        self.shown = false;
        Ok(())
    }
}

/// Governance as the resolver reports it: admit one publisher, or none.
struct Resolver {
    path: Option<PathBuf>,
    admit: Option<String>,
    revoked: Option<RevokedReason>,
}

impl BundleResolver for Resolver {
    fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
        Ok(BundleSource {
            asset_id: "asset-1".into(),
            blob_path: self.path.clone(),
            revoked: self.revoked,
        })
    }
    fn verdict(&self, _identity: &BundleIdentity) -> AdmissionVerdict {
        match &self.admit {
            Some(publisher_did) => AdmissionVerdict::Admitted { publisher_did: publisher_did.clone() },
            None => AdmissionVerdict::Unadmitted { can_admit: true },
        }
    }
}

fn resolver(path: &Fixture, admit: Option<&str>) -> Arc<dyn BundleResolver> {
    Arc::new(Resolver { path: Some(path.path.clone()), admit: admit.map(str::to_owned), revoked: None })
}

fn states(events: &[BrowserEvent]) -> Vec<(String, BundleState)> {
    events
        .iter()
        .filter_map(|event| match event {
            BrowserEvent::BundleState { node_id, state, .. } => Some((node_id.clone(), state.clone())),
            _ => None,
        })
        .collect()
}

/// Poll until a non-Closed state arrives (the open runs on a worker thread).
fn poll_until_state(host: &mut BundleHost, webview: &mut FakeWebview) -> Vec<BrowserEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(host.poll(&mut *webview));
        if states(&seen).iter().any(|(_, state)| *state != BundleState::Closed) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no bundle state within 10 s: {seen:?}");
}

fn get(mounts: &PortalMounts, uri: &str, headers: &[(&str, &str)]) -> Option<http::Response<Vec<u8>>> {
    send(mounts, "GET", uri, headers)
}

/// `None` when `http` refuses the URI itself (wry then answers 400 before the handler: desk check F-2).
fn send(mounts: &PortalMounts, method: &str, uri: &str, headers: &[(&str, &str)]) -> Option<http::Response<Vec<u8>>> {
    let mut builder = http::Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Vec::new()).ok()?;
    let response = serve(mounts, &request);
    // F-1: wry drops any header whose value is not visible ASCII, so every value must round-trip.
    for (name, value) in response.headers() {
        assert!(value.to_str().is_ok(), "{name} must be visible ASCII");
    }
    for name in [
        "content-security-policy",
        "x-content-type-options",
        "cross-origin-opener-policy",
        "cross-origin-resource-policy",
        "referrer-policy",
        "permissions-policy",
        "cache-control",
    ] {
        assert!(response.headers().contains_key(name), "{name} on every response ({uri})");
    }
    Some(response)
}

fn status(response: Option<http::Response<Vec<u8>>>) -> Option<StatusCode> {
    response.map(|response| response.status())
}

fn main_frame(raw_url: &str) -> Attempt<'_> {
    Attempt { raw_url, is_redirect: false, is_main_frame: true }
}

// ---------------------------------------------------------------------------
// Journeys
// ---------------------------------------------------------------------------

/// R6 labels and §5 CSP composition, through the real prepare path where identity matters.
#[test]
fn bundle_origins_and_csp_follow_the_contract() {
    let key = publisher_key();
    let publisher = fe_format::verifying_key_to_did(&key.verifying_key());
    let v1 = Fixture::write(fe_format::seal(&archive("1.0.0", portal(&[], false), "v1"), &key).unwrap());
    let v2 = Fixture::write(fe_format::seal(&archive("1.1.0", portal(&[], false), "v2"), &key).unwrap());
    let admitted_v1 = prepare(resolver(&v1, Some(publisher.as_str())).as_ref(), "n", u64::MAX).unwrap();
    let admitted_v2 = prepare(resolver(&v2, Some(publisher.as_str())).as_ref(), "n", u64::MAX).unwrap();
    let static_v1 = prepare(resolver(&v1, None).as_ref(), "n", u64::MAX).unwrap();
    assert_eq!(admitted_v1.mode, MountMode::Admitted);
    assert_ne!(admitted_v1.content_hash, admitted_v2.content_hash, "two builds");
    assert_eq!(admitted_v1.label, admitted_v2.label, "a publisher's updates keep one origin");
    assert_eq!(admitted_v1.label, admitted_label(&publisher, PACKAGE));
    assert_ne!(admitted_label("did:key:other", PACKAGE), admitted_v1.label, "publishers never share an origin");
    assert_eq!(static_v1.label.kind(), LabelKind::Static);
    assert_ne!(static_v1.label, admitted_v1.label, "static never shares the admitted origin");
    assert_eq!(Some(static_v1.label.clone()), static_label(&static_v1.content_hash));

    let unsealed = Fixture::write(archive("1.0.0", portal(&[], false), "unsealed"));
    let unsealed_mount = prepare(resolver(&unsealed, Some(publisher.as_str())).as_ref(), "n", u64::MAX).unwrap();
    assert_eq!(unsealed_mount.mode, MountMode::Static, "an unsealed build is never admitted");
    assert_eq!(unsealed_mount.content_hash, blake3::hash(&unsealed.bytes).to_hex().to_string());

    let label = admitted_v1.label.as_str();
    for (raw, accepted) in [
        (format!("https://fe-portal.{label}.localhost/index.html"), true),
        (format!("fe-portal://{label}.localhost/"), true),
        (format!("https://fe-portal.{label}.localhost?q=1"), true),
        (format!("https://user@fe-portal.{label}.localhost/"), false),
        (format!("https://fe-portal.{label}.localhost@evil.example/"), false),
        (format!("https://fe-portal.{label}.localhost:443/"), false),
        (format!("https://fe-portal.{label}.localhost./"), false),
        (format!("https://fe-portal.x.{label}.localhost/"), false),
        (format!("https://fe-portal.{}.localhost/", label.to_uppercase()), false),
        (format!("https://fe-portal.b-{}.localhost/", &label[2..]), false),
        (format!("https://fe-portal.{}.localhost/", &label[..27]), false),
        (format!("https://fe-portal.{label}a.localhost/"), false),
        (format!("http://fe-portal.{label}.localhost/"), false),
        (format!("wss://fe-portal.{label}.localhost/"), false),
    ] {
        assert_eq!(origin::parse(&raw).is_some(), accepted, "{raw}");
    }

    let empty = PortalCsp {
        connect: vec![], img: vec![], font: vec![], media: vec![], frame: vec![], wasm: false, inline_styles: false,
    };
    let rich = PortalCsp {
        connect: vec!["https://api.example.org".into()],
        img: vec!["https://img.example.org".into()],
        font: vec!["https://fonts.example.net".into()],
        media: vec!["https://media.example.org".into()],
        frame: vec!["https://maps.example.org".into()],
        wasm: true,
        inline_styles: true,
    };
    let feed = vec!["http://127.0.0.1:8765".to_owned()];
    let tail = "worker-src 'self'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'";
    for (csp, mode, feeds, expected) in [
        (&empty, MountMode::Static, &[][..], format!(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; font-src 'self'; \
             media-src 'self'; connect-src 'self'; frame-src 'none'; {tail}")),
        (&rich, MountMode::Admitted, &feed[..], format!(
            "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: blob: https://img.example.org; font-src 'self' https://fonts.example.net; \
             media-src 'self' https://media.example.org; connect-src 'self' https://api.example.org; \
             frame-src https://maps.example.org; {tail}")),
        (&empty, MountMode::Static, &feed[..], format!(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; font-src 'self'; \
             media-src 'self'; connect-src 'self' http://127.0.0.1:8765; frame-src 'none'; {tail}")),
    ] {
        assert_eq!(compose(csp, mode, feeds).unwrap(), expected);
    }
    let smuggled = PortalCsp { connect: vec!["https://a.example; script-src *".into()], ..empty.clone() };
    assert_eq!(
        compose(&smuggled, MountMode::Static, &[]),
        Err(UnsafeSource("https://a.example; script-src *".into())),
        "a source can never add a directive"
    );
}

/// Open → serve → navigate → leave/fault/close, as the host runs it each frame.
#[test]
fn open_serve_navigate_close_journey() {
    let key = publisher_key();
    let publisher = fe_format::verifying_key_to_did(&key.verifying_key());
    let sealed_bytes = fe_format::seal(&archive("1.0.0", portal(&["https://docs.example.org"], true), "admitted"), &key)
        .unwrap();
    let sealed = Fixture::write(sealed_bytes.clone());
    let mounts = PortalMounts::new();
    let gate = Rc::new(NavigationGate::new(mounts.clone()));
    let parts = BundleParts { mounts: mounts.clone(), gate: Rc::clone(&gate), supported: true };
    let mut host = BundleHost::new(parts.clone());
    let mut webview = FakeWebview::default();

    // Open the sealed build this peer admitted: entry loaded and shown under the admitted origin.
    assert!(host.open("node-1", Some(resolver(&sealed, Some(publisher.as_str()))), &mut webview).is_empty());
    let opened = states(&poll_until_state(&mut host, &mut webview));
    assert_eq!(opened, vec![("node-1".to_owned(), BundleState::Admitted { publisher: publisher.clone() })]);
    let mount = mounts.current().expect("mounted");
    assert_eq!(mount.label, admitted_label(&publisher, PACKAGE));
    assert_eq!(webview.loads, vec![mount.entry_url.clone()]);
    assert!(webview.shown);
    assert!(mount.csp().to_str().unwrap().contains("'wasm-unsafe-eval'"));

    // Navigation: only the host's Open enters, once; reload stays; generation bumps per document.
    let generation = mounts.generation();
    assert_eq!(gate.decide(&main_frame(&mount.entry_url)), Decision::AllowEnter);
    assert_eq!(gate.decide(&main_frame(&mount.entry_url)), Decision::AllowSame, "reload of the same origin");
    assert_eq!(mounts.generation(), generation + 2);

    // Serving (R5): exact members with the composed CSP; everything else is refused.
    let base = format!("fe-portal://{}.localhost", mount.label);
    let entry = get(&mounts, &format!("{base}/index.html"), &[]).unwrap();
    assert_eq!(entry.status(), StatusCode::OK);
    assert_eq!(entry.headers()[CONTENT_SECURITY_POLICY], *mount.csp());
    assert_eq!(entry.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(entry.body(), b"<!doctype html><title>admitted</title>");
    let head = send(&mounts, "HEAD", &format!("{base}/app.js"), &[]).unwrap();
    assert!(head.body().is_empty());
    assert_ne!(head.headers()[CONTENT_LENGTH], "0", "HEAD keeps the length");
    let ranged = get(&mounts, &format!("{base}/media.bin"), &[("range", "bytes=2-5")]).unwrap();
    assert_eq!((ranged.status(), ranged.body().as_slice()), (StatusCode::PARTIAL_CONTENT, &MEDIA[2..=5]));
    assert_eq!(ranged.headers()[CONTENT_RANGE], format!("bytes 2-5/{}", MEDIA.len()).as_str());
    for (path, headers, expected) in [
        ("/", &[][..], Some(StatusCode::OK)),
        ("/docs/", &[][..], Some(StatusCode::OK)),
        ("/media.bin", &[("range", "bytes=99-")][..], Some(StatusCode::RANGE_NOT_SATISFIABLE)),
        ("/media.bin", &[("range", "bytes=0-1,4-5")][..], Some(StatusCode::OK)),
        ("/missing.js", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/../index.html", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/%2e%2e/index.html", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/%2E%2E/index.html", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/.%2e/index.html", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/docs/..%5cindex.html", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/.hidden.js", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/__fe/feeds.json", &[][..], Some(StatusCode::NOT_FOUND)),
        ("/app.js", &[("service-worker", "script")][..], Some(StatusCode::FORBIDDEN)),
    ] {
        assert_eq!(status(get(&mounts, &format!("{base}{path}"), headers)), expected, "{path} {headers:?}");
    }
    let backslash = status(get(&mounts, &format!("{base}/docs\\index.html"), &[]));
    assert!(matches!(backslash, None | Some(StatusCode::NOT_FOUND)), "a backslash never names a member");
    assert_eq!(status(send(&mounts, "POST", &format!("{base}/index.html"), &[])), Some(StatusCode::METHOD_NOT_ALLOWED));
    let foreign = admitted_label("did:key:other", PACKAGE);
    assert_eq!(status(get(&mounts, &format!("fe-portal://{foreign}.localhost/"), &[])), Some(StatusCode::NOT_FOUND));
    assert_eq!(status(get(&mounts, "fe-portal://evil.localhost/", &[])), Some(StatusCode::NOT_FOUND));

    // Leave for a declared origin: the mount goes, the chrome hears Closed, and re-entry needs Open.
    assert_eq!(gate.decide(&main_frame("https://evil.example.com/")), Decision::Refuse(RefuseReason::Undeclared));
    assert_eq!(gate.decide(&main_frame("https://docs.example.org/guide")), Decision::AllowLeave);
    assert_eq!(states(&host.poll(&mut webview)), vec![("node-1".to_owned(), BundleState::Closed)]);
    assert!(mounts.current().is_none());
    assert_eq!(status(get(&mounts, &format!("{base}/index.html"), &[])), Some(StatusCode::NOT_FOUND));
    assert_eq!(
        gate.decide(&main_frame(&mount.entry_url)),
        Decision::Refuse(RefuseReason::EnterWithoutOpen)
    );

    // A tampered member is never served: 500, the mount stops serving, and the next frame Blocks it.
    let tampered = Fixture::write(tamper(&sealed_bytes, "web/app.js"));
    host.open("node-2", Some(resolver(&tampered, None)), &mut webview);
    let opened = states(&poll_until_state(&mut host, &mut webview));
    assert_eq!(opened, vec![("node-2".to_owned(), BundleState::StaticSandbox { can_admit: true })]);
    let static_mount = mounts.current().expect("static mount");
    let static_base = format!("fe-portal://{}.localhost", static_mount.label);
    assert_eq!(
        status(get(&mounts, &format!("{static_base}/__fe/portal-v1.js"), &[])),
        Some(StatusCode::NOT_FOUND),
        "a static origin never gets the page API"
    );
    assert_eq!(status(get(&mounts, &format!("{static_base}/app.js"), &[])), Some(StatusCode::INTERNAL_SERVER_ERROR));
    assert_eq!(status(get(&mounts, &format!("{static_base}/index.html"), &[])), Some(StatusCode::NOT_FOUND));
    assert_eq!(
        states(&host.poll(&mut webview)),
        vec![("node-2".to_owned(), BundleState::Blocked(BlockedReason::Integrity))]
    );
    assert_eq!(webview.loads.last().map(String::as_str), Some(UNLOAD_URL), "the dying document is unloaded");
    assert!(!webview.shown);
    assert_eq!(gate.decide(&main_frame(UNLOAD_URL)), Decision::AllowHostUnload);
    assert_eq!(gate.decide(&main_frame(UNLOAD_URL)), Decision::DeferToRawPolicy, "the unload token is one-shot");

    // Open twice before the first resolves: only the latest Open mounts.
    host.open("node-3", Some(resolver(&sealed, Some(publisher.as_str()))), &mut webview);
    let superseded = host.open("node-4", Some(resolver(&sealed, None)), &mut webview);
    assert_eq!(states(&superseded), vec![("node-3".to_owned(), BundleState::Closed)]);
    let opened = states(&poll_until_state(&mut host, &mut webview));
    assert_eq!(opened, vec![("node-4".to_owned(), BundleState::StaticSandbox { can_admit: true })]);
    assert_eq!(mounts.current().map(|mount| mount.node_id.clone()).as_deref(), Some("node-4"));

    // Close unloads (bundles never keep running hidden) and voids the origin.
    let static_entry = mounts.current().unwrap().entry_url.clone();
    assert_eq!(states(&host.close(&mut webview)), vec![("node-4".to_owned(), BundleState::Closed)]);
    assert_eq!(webview.loads.last().map(String::as_str), Some(UNLOAD_URL));
    assert!(!webview.shown && mounts.current().is_none());
    assert_eq!(gate.decide(&main_frame(&static_entry)), Decision::Refuse(RefuseReason::EnterWithoutOpen));

    // Open outcomes that never mount (admission.md §2 order), one row each.
    let waiting: Arc<dyn BundleResolver> = Arc::new(Resolver { path: None, admit: None, revoked: None });
    let revoked: Arc<dyn BundleResolver> =
        Arc::new(Resolver { path: Some(sealed.path.clone()), admit: None, revoked: Some(RevokedReason::KillSwitch) });
    let malformed = Fixture::write(archive("2.0.0", json!({ "schema": "fe-portal-bundle/2" }), "bad"));
    for (resolver, cap, expected) in [
        (waiting, u64::MAX, BundleState::WaitingForBytes),
        (revoked, u64::MAX, BundleState::Revoked(RevokedReason::KillSwitch)),
        (resolver(&sealed, None), 16, BundleState::Blocked(BlockedReason::TooLarge)),
        (resolver(&malformed, None), u64::MAX, BundleState::Blocked(BlockedReason::MalformedManifest)),
    ] {
        assert_eq!(prepare(resolver.as_ref(), "node-x", cap).unwrap_err(), expected);
    }
    let mut unsupported = BundleHost::new(BundleParts { supported: false, ..parts.clone() });
    assert_eq!(
        states(&unsupported.open("node-5", Some(resolver(&sealed, None)), &mut webview)),
        vec![("node-5".to_owned(), BundleState::Blocked(BlockedReason::UnsupportedPlatform))]
    );
    assert_eq!(
        states(&host.open("node-6", None, &mut webview)),
        vec![("node-6".to_owned(), BundleState::Unavailable)],
        "no resolver injected"
    );
}
