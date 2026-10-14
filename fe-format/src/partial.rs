//! `HeaderDoc`: the one document both the HTTP `/meta` route and the P2P HashSeq
//! child 0 carry (PAS-P5 §3.2); `verify_header` is the one client-side check
//! both transports run over it. See `src/AGENTS.md` §partial.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrity::{
    verify_archive_with_budget, verify_sealed_header, IntegrityError, IntegrityPolicy,
    IntegrityStatus, SealCheck, SealedCoverage, MANIFEST_PATH,
};
use crate::limits;

/// Header-document format version.
pub const HEADER_DOC_VERSION: u32 = 1;

/// How a hexon's body is organized: one fetchable unit, or a signed container
/// of independently-fetchable chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Layout {
    /// The whole archive is one unit; a chunk/member route serves it directly.
    Monolithic,
    /// A container: `chunk_count` entries under `chunks/{seq}.hexon`.
    Container { chunk_count: u32 },
}

/// The header-phase document: raw `manifest.json` text plus raw header-member
/// text (exactly the bytes [`crate::integrity::verify_sealed_header`] hashes —
/// nothing here is re-serialized, so client-side hashing matches the signed
/// digests bit for bit). The HTTP `/meta` route returns this as JSON; the P2P
/// path stores it as HashSeq child 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderDoc {
    /// Format version ([`HEADER_DOC_VERSION`]).
    pub v: u32,
    /// `"{hexon_id}@{version}"`.
    pub uri: String,
    /// Raw `manifest.json` text, byte-identical to the archive member.
    pub manifest: String,
    /// Zip path → raw member text, for every `HEADER_MEMBER_PATHS` member
    /// present in the archive (entries, license, schema, tileset_meta,
    /// terrain config, members.json, chunks/index.json).
    pub header_members: BTreeMap<String, String>,
    /// Total size of the archive this header describes, in bytes.
    pub archive_size: u64,
    pub layout: Layout,
}

impl HeaderDoc {
    /// The exact bytes every P2P publisher stores as HashSeq child 0 (one
    /// encoder, so a hexon seeded by any host has one root).
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("HeaderDoc serializes (strings, ints, a BTreeMap)")
    }
}

/// A [`HeaderDoc`] that passed [`verify_header`]: what every subsequent
/// chunk/member fetch is checked against.
#[derive(Debug, Clone)]
pub struct VerifiedHeader {
    pub uri: String,
    pub layout: Layout,
    pub archive_size: u64,
    pub status: IntegrityStatus,
    coverage: Option<SealedCoverage>,
}

impl VerifiedHeader {
    /// The signed coverage every chunk/member byte is checked against; `None`
    /// for an unsealed archive accepted under [`IntegrityPolicy::VerifyIfSealed`]
    /// (there is nothing signed to check against — the policy already decided
    /// that was acceptable).
    pub fn coverage(&self) -> Option<&SealedCoverage> {
        self.coverage.as_ref()
    }

    /// The signed blake3 hex of container chunk `seq`, when sealed and a container.
    pub fn chunk_digest(&self, seq: u32) -> Option<&str> {
        self.coverage.as_ref()?.chunk_digest(seq)
    }
}

/// Verify a [`HeaderDoc`]'s signature, pin and header digests — the one check
/// both the HTTP and P2P partial-fetch paths run before trusting anything the
/// header claims. No archive bytes are needed; this is the header-phase shape
/// both transports carry.
///
/// Refuses a `Container` layout whose claimed `chunk_count` disagrees with the
/// signed `chunks/index.json` bundle count — a transport cannot under- or
/// over-claim chunks relative to what the publisher actually signed.
pub fn verify_header(
    doc: &HeaderDoc,
    policy: &IntegrityPolicy,
) -> Result<VerifiedHeader, IntegrityError> {
    if doc.v != HEADER_DOC_VERSION {
        return Err(IntegrityError::Unsupported(format!(
            "header doc version {}",
            doc.v
        )));
    }
    let header_members: BTreeMap<String, Vec<u8>> = doc
        .header_members
        .iter()
        .map(|(path, text)| (path.clone(), text.as_bytes().to_vec()))
        .collect();
    let check = verify_sealed_header(doc.manifest.as_bytes(), &header_members, policy)?;
    let (status, coverage) = match check {
        SealCheck::Unsealed => (IntegrityStatus::Unsealed, None),
        SealCheck::Sealed(coverage) => {
            if let Layout::Container { chunk_count } = doc.layout {
                let signed = coverage.chunk_index().map(|idx| idx.bundles.len() as u32);
                if signed != Some(chunk_count) {
                    return Err(IntegrityError::Unsupported(format!(
                        "header claims {chunk_count} chunks but the signed index has {signed:?}"
                    )));
                }
            }
            let status = IntegrityStatus::Sealed {
                publisher_did: coverage.publisher_did().to_string(),
            };
            (status, Some(*coverage))
        }
    };
    Ok(VerifiedHeader {
        uri: doc.uri.clone(),
        layout: doc.layout,
        archive_size: doc.archive_size,
        status,
        coverage,
    })
}

/// Check that `archive` is exactly the sealed archive a verified header
/// describes: its raw `manifest.json` equals `signed_manifest` byte for byte
/// (no same-publisher substitution) and the whole archive passes
/// `RequireSealed { pin: publisher_did }` within `total_budget`. The one
/// whole-archive check every monolithic partial fetch runs (fe-hexon's
/// `FetchStrategy`, fe-terrain's P2P install and streamed decode).
pub fn verify_archive_against_header(
    archive: &[u8],
    signed_manifest: &str,
    publisher_did: &str,
    total_budget: u64,
) -> Result<(), IntegrityError> {
    let malformed = |e: &dyn std::fmt::Display| IntegrityError::Malformed(format!("{e:#}"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| malformed(&e))?;
    let manifest = zip
        .by_name(MANIFEST_PATH)
        .map_err(|_| IntegrityError::MissingMember {
            path: MANIFEST_PATH.into(),
        })?;
    let declared = manifest.size();
    let mut budget = limits::MAX_JSON_ENTRY_BYTES;
    let served = limits::read_entry_capped(
        manifest,
        declared,
        limits::MAX_JSON_ENTRY_BYTES,
        &mut budget,
    )
    .map_err(|e| malformed(&e))?;
    if served != signed_manifest.as_bytes() {
        return Err(IntegrityError::DigestMismatch {
            path: MANIFEST_PATH.into(),
        });
    }
    verify_archive_with_budget(
        archive,
        &IntegrityPolicy::RequireSealed {
            pin: Some(publisher_did.to_string()),
        },
        total_budget,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_index::HexonArchiveIndex;
    use crate::integrity::seal;
    use crate::manifest::{ElevationEncoding, HexonType, TilesetMeta};
    use crate::HexonArchive;
    use ed25519_dalek::SigningKey;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn manifest(id: &str) -> crate::manifest::HexonManifest {
        crate::manifest::HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mkpartial".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Partial".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "fe_partial_{tag}_{}_{nanos}.hexon",
            std::process::id()
        ))
    }

    fn sealed_monolithic_header_doc() -> (SigningKey, HeaderDoc) {
        let key = SigningKey::from_bytes(&[11u8; 32]);
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 1,
            region_name: "Partial".into(),
            ..Default::default()
        };
        let bytes = HexonArchive::export_tileset(
            manifest("mono"),
            &meta,
            &[("10/0/0".into(), vec![9u8; 128])],
            &[],
            None,
        )
        .unwrap();
        let sealed = seal(&bytes, &key).unwrap();
        let path = temp_path("mono");
        std::fs::write(&path, &sealed).unwrap();
        let index = HexonArchiveIndex::open(&path).unwrap();
        let doc = index.header_doc();
        let _ = std::fs::remove_file(&path);
        (key, doc)
    }

    /// A4/the one client-side check both transports run: signature, pin and
    /// header digests all pass for an honest sealed header.
    #[test]
    fn a_sealed_monolithic_header_verifies_under_its_publisher() {
        let (key, doc) = sealed_monolithic_header_doc();
        assert_eq!(doc.layout, Layout::Monolithic);
        let verified = verify_header(&doc, &IntegrityPolicy::VerifyIfSealed).expect("verifies");
        assert_eq!(
            verified.status,
            IntegrityStatus::Sealed {
                publisher_did: crate::verifying_key_to_did(&key.verifying_key())
            }
        );
        assert!(verified.coverage().is_some());
    }

    /// RequireSealed with a pin refuses a header signed by a different publisher.
    #[test]
    fn a_pin_mismatch_is_refused() {
        let (_key, doc) = sealed_monolithic_header_doc();
        let err = verify_header(
            &doc,
            &IntegrityPolicy::RequireSealed {
                pin: Some("did:key:z6Mksomeoneelse".into()),
            },
        )
        .expect_err("wrong pin must be refused");
        assert!(matches!(err, IntegrityError::PublisherMismatch { .. }));
    }

    /// A tampered manifest byte (flipped after the header doc was captured)
    /// fails signature verification, not a parse error.
    #[test]
    fn a_tampered_manifest_text_fails_signature_verification() {
        let (_key, mut doc) = sealed_monolithic_header_doc();
        // Swap one ASCII byte deep inside the signed JSON's `signature` value
        // (base64 — a single-byte ASCII swap stays valid UTF-8, unlike XOR).
        let pos = doc
            .manifest
            .find("\"signature\"")
            .expect("signature field present")
            + 20;
        let mut bytes = doc.manifest.into_bytes();
        bytes[pos] = if bytes[pos] == b'A' { b'B' } else { b'A' };
        doc.manifest = String::from_utf8(bytes).expect("ASCII swap stays valid UTF-8");
        let err =
            verify_header(&doc, &IntegrityPolicy::VerifyIfSealed).expect_err("must be refused");
        assert!(matches!(
            err,
            IntegrityError::BadSignature | IntegrityError::Malformed(_)
        ));
    }
}
