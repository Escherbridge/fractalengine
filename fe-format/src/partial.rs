//! `HeaderDoc`: the one document both the HTTP `/meta` route and the P2P HashSeq
//! child 0 carry (PAS-P5 §3.2); `verify_header` is the one client-side check
//! both transports run over it. See `src/AGENTS.md` §partial.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrity::{
    verify_archive_with_budget, verify_sealed_header, IntegrityError, IntegrityPolicy,
    IntegrityStatus, SealCheck, SealedCoverage, CHUNK_INDEX_PATH, MANIFEST_PATH,
};
use crate::limits;
use crate::manifest::{HexonManifest, TilesetBundleIndex};

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
    /// Total size of the archive, in bytes — ADVISORY: outside the signature,
    /// never copied into [`VerifiedHeader`], never a cap or a security input.
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
/// chunk/member fetch is checked against. Every field is derived from the
/// manifest and signed coverage, never from the doc's own claims (the doc's
/// `archive_size` is advisory and deliberately absent; see `src/AGENTS.md` §partial).
#[derive(Debug, Clone)]
pub struct VerifiedHeader {
    /// `"{hexon_id}@{version}"` of the (signed, when sealed) manifest.
    pub uri: String,
    /// The manifest's `hexon_id`.
    pub hexon_id: String,
    /// The manifest's `version`.
    pub version: String,
    /// Derived from the signed chunk index (sealed) or the header's own
    /// `chunks/index.json` (unsealed).
    pub layout: Layout,
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
/// The doc's unauthenticated `uri` must equal the manifest's
/// `hexon_id@version` (no substitution) and its `layout` must equal the one
/// derived from the signed chunk index (no downgrade or over-claim);
/// `archive_size` is advisory and never reaches [`VerifiedHeader`].
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
    // `verify_sealed_header` already parsed (and, when sealed, signature-checked) these bytes.
    let manifest: HexonManifest = serde_json::from_str(&doc.manifest)
        .map_err(|e| IntegrityError::Malformed(format!("{MANIFEST_PATH}: {e}")))?;
    let signed_uri = format!("{}@{}", manifest.hexon_id, manifest.version);
    if doc.uri != signed_uri {
        return Err(IntegrityError::Substituted {
            claimed: doc.uri.clone(),
            signed: signed_uri,
        });
    }
    let (status, coverage, layout) = match check {
        SealCheck::Unsealed => {
            let layout = match doc.header_members.get(CHUNK_INDEX_PATH) {
                Some(text) => {
                    let index: TilesetBundleIndex = serde_json::from_str(text).map_err(|e| {
                        IntegrityError::Malformed(format!("{CHUNK_INDEX_PATH}: {e}"))
                    })?;
                    container_layout(&index)
                }
                None => Layout::Monolithic,
            };
            (IntegrityStatus::Unsealed, None, layout)
        }
        SealCheck::Sealed(coverage) => {
            // An uncommitted header member is Uncovered, as in `HexonArchiveIndex::open`.
            for (path, bytes) in &header_members {
                coverage.check_member(path, bytes)?;
            }
            let layout = coverage
                .chunk_index()
                .map_or(Layout::Monolithic, container_layout);
            let status = IntegrityStatus::Sealed {
                publisher_did: coverage.publisher_did().to_string(),
            };
            (status, Some(*coverage), layout)
        }
    };
    if doc.layout != layout {
        return Err(IntegrityError::Unsupported(format!(
            "header claims {:?} but the chunk index says {layout:?}",
            doc.layout
        )));
    }
    Ok(VerifiedHeader {
        uri: signed_uri,
        hexon_id: manifest.hexon_id,
        version: manifest.version,
        layout,
        status,
        coverage,
    })
}

/// The container layout a chunk index describes.
fn container_layout(index: &TilesetBundleIndex) -> Layout {
    Layout::Container {
        chunk_count: index.bundles.len() as u32,
    }
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

    fn tileset_meta(tile_count: u32) -> TilesetMeta {
        TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count,
            region_name: "Partial".into(),
            ..Default::default()
        }
    }

    /// A sealed two-chunk container's header document, as the lazy reader serves it.
    fn sealed_container_header_doc() -> HeaderDoc {
        let key = SigningKey::from_bytes(&[12u8; 32]);
        let chunks: Vec<crate::ChunkInput> = (0..2u32)
            .map(|seq| crate::ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [u64::from(seq), u64::from(seq)],
                archive_bytes: HexonArchive::export_tileset(
                    manifest(&format!("chunked-{seq}")),
                    &tileset_meta(1),
                    &[(format!("10/{seq}/0"), vec![seq as u8; 64])],
                    &[],
                    None,
                )
                .unwrap(),
            })
            .collect();
        let container =
            HexonArchive::export_chunked(manifest("chunked"), &tileset_meta(2), &chunks).unwrap();
        let sealed = seal(&container, &key).unwrap();
        let path = temp_path("container");
        std::fs::write(&path, &sealed).unwrap();
        let doc = HexonArchiveIndex::open(&path).unwrap().header_doc();
        let _ = std::fs::remove_file(&path);
        doc
    }

    /// S8 mandatory finding (1): a validly-signed header of hexon A served
    /// under uri B is refused — the uri is bound to the signed manifest.
    #[test]
    fn a_signed_header_served_under_another_uri_is_refused() {
        let (_key, honest) = sealed_monolithic_header_doc();
        let verified = verify_header(&honest, &IntegrityPolicy::VerifyIfSealed).expect("honest");
        assert_eq!(verified.uri, "mono@0.1.0");
        assert_eq!(verified.hexon_id, "mono");

        for claimed in ["other@0.1.0", "mono@9.9.9", "mono"] {
            let mut substituted = honest.clone();
            substituted.uri = claimed.into();
            let err = verify_header(&substituted, &IntegrityPolicy::VerifyIfSealed)
                .expect_err("a uri the signature does not cover must be refused");
            assert_eq!(
                err,
                IntegrityError::Substituted {
                    claimed: claimed.into(),
                    signed: "mono@0.1.0".into()
                }
            );
        }
    }

    /// S8 mandatory finding (2): claiming `Monolithic` over a signed chunk
    /// index (a downgrade that would let seq 0 skip the per-chunk digests),
    /// or over-claiming chunks, is refused; the honest claim verifies.
    #[test]
    fn a_layout_claim_that_contradicts_the_signed_chunk_index_is_refused() {
        let honest = sealed_container_header_doc();
        assert_eq!(honest.layout, Layout::Container { chunk_count: 2 });
        let verified = verify_header(&honest, &IntegrityPolicy::VerifyIfSealed).expect("honest");
        assert_eq!(verified.layout, Layout::Container { chunk_count: 2 });

        for lie in [Layout::Monolithic, Layout::Container { chunk_count: 3 }] {
            let mut lying = honest.clone();
            lying.layout = lie;
            let err = verify_header(&lying, &IntegrityPolicy::VerifyIfSealed)
                .expect_err("a layout the signed index contradicts must be refused");
            assert!(
                matches!(err, IntegrityError::Unsupported(_)),
                "{lie:?}: {err}"
            );
        }

        // A monolithic archive cannot be dressed up as a container either.
        let (_key, mono) = sealed_monolithic_header_doc();
        let mut upgraded = mono;
        upgraded.layout = Layout::Container { chunk_count: 1 };
        assert!(matches!(
            verify_header(&upgraded, &IntegrityPolicy::VerifyIfSealed),
            Err(IntegrityError::Unsupported(_))
        ));
    }

    /// S8 mandatory finding (3): `archive_size` is outside the signature, so
    /// any value verifies identically and nothing derived from it reaches the
    /// verified header (no fetch cap can be widened through it).
    #[test]
    fn archive_size_is_advisory_and_never_reaches_the_verified_header() {
        let honest = sealed_container_header_doc();
        let verified = |size: u64| {
            let mut doc = honest.clone();
            doc.archive_size = size;
            let v = verify_header(&doc, &IntegrityPolicy::VerifyIfSealed).expect("verifies");
            (
                v.uri.clone(),
                v.layout,
                v.status.clone(),
                v.chunk_digest(0).map(str::to_string),
                v.chunk_digest(1).map(str::to_string),
            )
        };
        assert_eq!(verified(0), verified(u64::MAX));
        assert_eq!(verified(0), verified(honest.archive_size));
    }

    /// S8: a header member the signed root does not commit (here a
    /// `schema.json` the archive never had) is refused rather than passed
    /// through to callers that read the doc's `header_members` map.
    #[test]
    fn an_uncommitted_header_member_in_a_sealed_doc_is_refused() {
        let (_key, honest) = sealed_monolithic_header_doc();
        let mut injected = honest.clone();
        injected
            .header_members
            .insert("schema.json".into(), "{}".into());
        assert!(!honest.header_members.contains_key("schema.json"));
        assert_eq!(
            verify_header(&injected, &IntegrityPolicy::VerifyIfSealed)
                .expect_err("uncommitted header member"),
            IntegrityError::Uncovered {
                path: "schema.json".into()
            }
        );
    }
}
