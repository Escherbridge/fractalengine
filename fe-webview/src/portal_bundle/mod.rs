//! Signed web bundles in the Petal Portal: origins, mounts, `fe-portal` protocol, CSP, navigation and the IPC bridge; see `AGENTS.md` here.

pub mod admission;
pub mod csp;
pub mod feeds;
pub mod ipc;
pub mod limits;
pub mod mount;
pub mod navigation;
pub mod origin;
pub mod protocol;

pub use ipc::{
    BindRequest, BridgeWebview, BrokerClient, BrokerEvent, DeliveryKind, Ingress, IpcCounters, IpcError, IpcStats,
    PendingReply, PortalBridge, PortalBroker, PortalIntent, UiIntent,
};
pub use mount::{
    bundle_max_bytes, prepare, AdmissionVerdict, BundleHost, BundleIdentity, BundleParts, BundleResolver,
    BundleSource, BundleWebview, Mount, MountMode, PortalMounts,
};
pub use navigation::{Attempt, Current, Decision, NavigationGate, RefuseReason, UNLOAD_URL};
pub use origin::{Label, LabelKind, SCHEME};

/// The external-browser fallback has no embedded surface: every Open reports Unavailable.
impl BundleWebview for crate::backends::stub::StubBackend {}

/// No embedded surface, so no bridge.
impl BridgeWebview for crate::backends::stub::StubBackend {}

/// The unfinished Servo backend renders no bundles.
#[cfg(feature = "backend-servo")]
impl BundleWebview for crate::backends::servo::ServoBackend {}

/// The unfinished Servo backend has no bridge.
#[cfg(feature = "backend-servo")]
impl BridgeWebview for crate::backends::servo::ServoBackend {}

/// Real fixture archives for in-crate tests (the integration journey keeps its own builder).
#[cfg(test)]
#[cfg_attr(not(feature = "backend-tauri"), allow(dead_code))]
pub(crate) mod test_fixture {
    use std::io::Write as _;
    use std::path::PathBuf;

    use super::mount::{bundle_max_bytes, prepare, BundleResolver, BundleSource, Mount};
    use crate::ipc::BundleState;

    /// An unsealed bundle archive on disk, removed on drop.
    pub(crate) struct FixtureBundle {
        pub(crate) path: PathBuf,
    }

    impl Drop for FixtureBundle {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// A docs-site bundle declaring `navigation`, with `web/index.html` and `web/app.js`.
    pub(crate) fn write_bundle(navigation: &[&str]) -> FixtureBundle {
        let extension = serde_json::json!({
            "manifest_version": 2, "id": "org.example.fixture", "version": "1.0.0", "kind": "web-bundle",
            "portal": {
                "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Fixture",
                "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [],
                         "wasm": false, "inline_styles": false },
                "navigation": navigation,
                "subscriptions": { "node": false, "readings": false, "queries": [] },
                "commands": {}, "hooks": [], "streams": [], "ui_intents": []
            }
        });
        let manifest = serde_json::json!({
            "schema_version": "1.0.0", "hexon_id": "org.example.fixture", "hexon_type": "plugin",
            "publisher_did": "did:key:z6Mkfixture", "version": "1.0.0", "name": "Fixture",
            "created_at": "2026-11-08T00:00:00Z", "updated_at": "2026-11-08T00:00:00Z"
        });
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let members: [(&str, Vec<u8>); 5] = [
            ("manifest.json", serde_json::to_vec(&manifest).expect("manifest")),
            ("entries.json", br#"{"entries":[]}"#.to_vec()),
            ("plugin/extension.json", serde_json::to_vec(&extension).expect("extension")),
            ("web/index.html", b"<!doctype html><title>Fixture</title>".to_vec()),
            ("web/app.js", b"document.title = 'Fixture';".to_vec()),
        ];
        for (name, bytes) in members {
            zip.start_file(name, options).expect("start member");
            zip.write_all(&bytes).expect("write member");
        }
        let bytes = zip.finish().expect("finish zip").into_inner();
        let path = std::env::temp_dir().join(format!("fe-portal-fixture-{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, bytes).expect("write fixture");
        FixtureBundle { path }
    }

    /// Resolves every node to one local archive; governance's default verdict (static sandbox).
    pub(crate) struct FixtureResolver(pub(crate) PathBuf);

    impl BundleResolver for FixtureResolver {
        fn resolve(&self, _node_id: &str) -> Result<BundleSource, BundleState> {
            Ok(BundleSource {
                asset_id: "asset-fixture".into(),
                blob_path: Some(self.0.clone()),
                revoked: None,
            })
        }
    }

    /// A prepared static mount of [`write_bundle`] (keep the bundle alive while the mount is used).
    pub(crate) fn static_mount(navigation: &[&str]) -> (FixtureBundle, Mount) {
        let bundle = write_bundle(navigation);
        let mount = prepare(&FixtureResolver(bundle.path.clone()), "node-fixture", bundle_max_bytes())
            .expect("fixture bundle mounts");
        (bundle, mount)
    }
}
