//! Web-bundle install through the store client (P-STORE): verify, kill-list check, blob store. See src/AGENTS.md §portal-install.
// Nothing calls this until the portal host's registry drain (S-HOST) lands.
#![allow(dead_code)]

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use fe_api::portal_bundles::{bundle_max_bytes, inspect_archive, InspectedBundle};
use fe_format::integrity;
use fe_format::portal_bundle::{Refusal as BundleRefusal, RefusalClass};
use fe_hexon::registry_source::FetchedPackage;
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex, BlobHash, BlobStoreHandle};

/// True when a lowercase-hex digest is on an accepted kill list (under any package id).
pub(crate) type KillListPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What the install path needs from the host: the local blob store and the verified kill list.
#[derive(Clone)]
pub(crate) struct InstallContext {
    pub blobs: BlobStoreHandle,
    pub kill_list: KillListPredicate,
}

static CONTEXT: OnceLock<InstallContext> = OnceLock::new();

/// Set the process-wide context once at startup; `false` when one is already set.
pub(crate) fn configure(context: InstallContext) -> bool {
    CONTEXT.set(context).is_ok()
}

/// A web bundle that verified and now sits in the local blob store; nothing was started or created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledBundle {
    pub package_id: String,
    pub version: String,
    /// BLAKE3 of the canonical manifest: the value the index lists and the kill list keys.
    pub content_hash: String,
    /// BLAKE3 of the archive bytes: the blob store key.
    pub blob_hash: BlobHash,
    pub blob_hash_hex: String,
    /// The sealing publisher (always present: unsealed bundles are refused).
    pub publisher_did: String,
    pub title: String,
    pub capability_digest: String,
}

/// Why an install was refused; nothing is stored for any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The host never called [`configure`]; fail closed.
    NotConfigured,
    /// The archive is not a web-bundle hexon (e.g. a plugin or tileset).
    WrongKind(String),
    /// The seal, a member digest or the archive structure failed verification.
    Tampered(String),
    /// The bundle failed another portal-bundle rule (README section 6).
    Invalid(String),
    /// Unsealed archives are never installed from a registry.
    NotSealed,
    /// The seal, id, version or content hash disagree with what the index listed.
    ListingMismatch(String),
    /// `(content_hash, digest)` is on an accepted kill list.
    KillListed { digest: String },
    /// The local blob store failed.
    Store(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("the install path is not configured on this host"),
            Self::WrongKind(detail) => write!(f, "not a web bundle: {detail}"),
            Self::Tampered(detail) => write!(f, "bundle failed verification: {detail}"),
            Self::Invalid(detail) => write!(f, "bundle refused: {detail}"),
            Self::NotSealed => f.write_str("bundle is not sealed by a publisher"),
            Self::ListingMismatch(detail) => write!(f, "bundle does not match its listing: {detail}"),
            Self::KillListed { digest } => write!(f, "bundle is on the kill list ({digest})"),
            Self::Store(detail) => write!(f, "could not store the bundle: {detail}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// Pinned seam (S-HOST): install with the host's configured context.
pub(crate) fn install_from_registry(fetched: &FetchedPackage) -> Result<InstalledBundle, Refusal> {
    let context = CONTEXT.get().ok_or(Refusal::NotConfigured)?;
    install_with(context, fetched)
}

/// Verify `fetched` as a sealed web-bundle hexon and store its bytes; starts nothing, creates no node.
pub(crate) fn install_with(context: &InstallContext, fetched: &FetchedPackage) -> Result<InstalledBundle, Refusal> {
    let listed = &fetched.package;
    // Cheap early refusal on the claimed hash; the verified hash is checked again below.
    if (context.kill_list)(&listed.content_hash.to_ascii_lowercase()) {
        return Err(Refusal::KillListed { digest: listed.content_hash.clone() });
    }
    let inspected = inspect_bytes(&fetched.bytes, bundle_max_bytes()).map_err(classify)?;

    let Some(sealed_by) = inspected.sealed_by.clone() else { return Err(Refusal::NotSealed) };
    if sealed_by != listed.publisher_did {
        return Err(Refusal::ListingMismatch(format!("sealed by {sealed_by}, listed {}", listed.publisher_did)));
    }
    if inspected.validated.package_id != listed.package_id || inspected.validated.version != listed.version {
        return Err(Refusal::ListingMismatch(format!(
            "bundle is {} {}, listed {} {}",
            inspected.validated.package_id, inspected.validated.version, listed.package_id, listed.version
        )));
    }
    if !inspected.content_hash.eq_ignore_ascii_case(&listed.content_hash) {
        return Err(Refusal::ListingMismatch(format!(
            "content hash {} differs from the listed {}",
            inspected.content_hash, listed.content_hash
        )));
    }

    let archive_digest = integrity::digest_hex(&fetched.bytes);
    let keys = std::iter::once(inspected.content_hash.to_ascii_lowercase())
        .chain(std::iter::once(archive_digest.clone()))
        .chain(inspected.members.iter().filter_map(|member| member.blake3.clone()));
    for key in keys {
        if (context.kill_list)(&key) {
            return Err(Refusal::KillListed { digest: key });
        }
    }

    let expected = hash_from_hex(&archive_digest).map_err(|e| Refusal::Store(format!("{e:#}")))?;
    let stored = context.blobs.add_blob(&fetched.bytes).map_err(|e| Refusal::Store(format!("{e:#}")))?;
    if stored != expected {
        return Err(Refusal::Store("blob store returned a different hash".into()));
    }
    Ok(InstalledBundle {
        package_id: inspected.validated.package_id.clone(),
        version: inspected.validated.version.clone(),
        content_hash: inspected.content_hash.clone(),
        blob_hash: stored,
        blob_hash_hex: hash_to_hex(&stored),
        publisher_did: sealed_by,
        title: inspected.validated.portal.title.clone(),
        capability_digest: inspected.validated.capability_digest.clone(),
    })
}

/// Map the shared portal verifier's refusal onto the install refusals.
fn classify(refusal: BundleRefusal) -> Refusal {
    let text = refusal.to_string();
    match refusal.class {
        RefusalClass::KindNotWebBundle => Refusal::WrongKind(refusal.detail),
        RefusalClass::Integrity | RefusalClass::ArchiveLimit | RefusalClass::TooLarge => Refusal::Tampered(refusal.detail),
        _ => Refusal::Invalid(text),
    }
}

/// Run the portal verifier over in-memory bytes (it reads from a path, so a scratch file stands in).
fn inspect_bytes(bytes: &[u8], max_bytes: u64) -> Result<InspectedBundle, BundleRefusal> {
    // The cap before any disk write: an over-cap download never reaches the scratch file.
    if bytes.len() as u64 > max_bytes {
        return Err(BundleRefusal {
            class: RefusalClass::TooLarge,
            detail: format!("{} bytes over cap {max_bytes}", bytes.len()),
        });
    }
    let scratch = ScratchFile::write(bytes).map_err(|e| BundleRefusal {
        class: RefusalClass::Integrity,
        detail: format!("scratch file: {e}"),
    })?;
    inspect_archive(&scratch.0, bytes.len() as u64, max_bytes)
}

/// A scratch archive removed on drop.
struct ScratchFile(PathBuf);

impl ScratchFile {
    fn write(bytes: &[u8]) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("fe-install-{}.hexon", ulid::Ulid::new()));
        std::fs::write(&path, bytes)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Write as _;
    use std::sync::Mutex;

    use ed25519_dalek::SigningKey;
    use fe_hexon::package::PluginPackage;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use serde_json::json;

    const PAGE: &[u8] = b"<!doctype html><title>docs</title>";

    fn extension_json(kind: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "manifest_version": 2, "id": "org.example.pump-docs", "version": "1.0.0", "kind": kind,
            "portal": {
                "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Pump docs",
                "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [], "wasm": false, "inline_styles": false },
                "navigation": [], "subscriptions": { "node": false, "readings": false, "queries": [] },
                "commands": {}, "hooks": [], "streams": [], "ui_intents": []
            }
        }))
        .expect("extension json")
    }

    /// A sealed archive with uncompressed members (so a byte flip lands in a known place).
    fn sealed_bundle(kind: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let manifest = json!({
            "schema_version": "1.0.0", "hexon_id": "org.example.pump-docs", "hexon_type": "plugin",
            "publisher_did": "did:key:z6Mkfixture", "version": "1.0.0", "name": "Pump docs",
            "created_at": "2026-11-08T00:00:00Z", "updated_at": "2026-11-08T00:00:00Z"
        });
        let members: [(&str, Vec<u8>); 4] = [
            ("manifest.json", serde_json::to_vec(&manifest).expect("manifest")),
            ("entries.json", br#"{"entries":[]}"#.to_vec()),
            ("plugin/extension.json", extension_json(kind)),
            ("web/index.html", PAGE.to_vec()),
        ];
        for (name, bytes) in &members {
            zip.start_file(*name, options).expect("start member");
            zip.write_all(bytes).expect("write member");
        }
        let plain = zip.finish().expect("finish zip").into_inner();
        integrity::seal(&plain, &SigningKey::from_bytes(&[7u8; 32])).expect("seal")
    }

    fn listing_for(bytes: &[u8]) -> PluginPackage {
        let inspected = inspect_bytes(bytes, bundle_max_bytes()).expect("fixture verifies");
        PluginPackage {
            package_id: inspected.validated.package_id,
            version: inspected.validated.version,
            publisher_did: inspected.sealed_by.expect("sealed"),
            content_hash: inspected.content_hash,
            component_digest: String::new(),
            extension_json: Vec::new(),
            license_json: None,
            component: Vec::new(),
            trusted_by: None,
        }
    }

    fn context(killed: Arc<Mutex<HashSet<String>>>) -> (InstallContext, Arc<MockBlobStore>) {
        let store = Arc::new(MockBlobStore::new());
        let context = InstallContext {
            blobs: store.clone(),
            kill_list: Arc::new(move |digest| killed.lock().expect("kill list").contains(digest)),
        };
        (context, store)
    }

    /// One journey: a sealed bundle installs and is stored; tampered, wrong-kind and kill-listed copies store nothing.
    #[test]
    fn registry_install_verifies_then_stores_and_refuses_bad_bundles() {
        let killed = Arc::new(Mutex::new(HashSet::new()));
        let (context, store) = context(killed.clone());

        // Wrong kind: a sealed archive whose extension.json is a plugin.
        let plugin = sealed_bundle("plugin");
        let wrong_kind = FetchedPackage {
            bytes: plugin,
            package: PluginPackage {
                package_id: "org.example.pump-docs".into(),
                version: "1.0.0".into(),
                publisher_did: "did:key:z6Mkfixture".into(),
                content_hash: "0".repeat(64),
                component_digest: String::new(),
                extension_json: Vec::new(),
                license_json: None,
                component: Vec::new(),
                trusted_by: None,
            },
        };
        assert!(matches!(install_with(&context, &wrong_kind), Err(Refusal::WrongKind(_))));

        // Tampered: one byte of the page flipped after sealing.
        let good = sealed_bundle("web-bundle");
        let listing = listing_for(&good);
        let mut tampered = good.clone();
        let at = tampered.windows(PAGE.len()).position(|w| w == PAGE).expect("page stored uncompressed");
        tampered[at + 15] ^= 0x20;
        let result = install_with(&context, &FetchedPackage { bytes: tampered, package: listing.clone() });
        assert!(matches!(result, Err(Refusal::Tampered(_))), "{result:?}");

        // Kill-listed: the listed content hash is on the accepted list.
        killed.lock().expect("kill list").insert(listing.content_hash.clone());
        let result = install_with(&context, &FetchedPackage { bytes: good.clone(), package: listing.clone() });
        assert!(matches!(result, Err(Refusal::KillListed { .. })), "{result:?}");
        killed.lock().expect("kill list").clear();
        assert!(store.is_empty(), "refusals store nothing");

        // Index listed a different hash than the archive carries.
        let mut lying = listing.clone();
        lying.content_hash = "f".repeat(64);
        let result = install_with(&context, &FetchedPackage { bytes: good.clone(), package: lying });
        assert!(matches!(result, Err(Refusal::ListingMismatch(_))), "{result:?}");

        // Happy path: stored once, identity reported.
        let installed = install_with(&context, &FetchedPackage { bytes: good.clone(), package: listing.clone() }).expect("installs");
        assert_eq!(installed.package_id, "org.example.pump-docs");
        assert_eq!(installed.content_hash, listing.content_hash);
        assert_eq!(store.bytes_for(&installed.blob_hash).as_deref(), Some(good.as_slice()));
        assert_eq!(store.len(), 1);
    }
}
