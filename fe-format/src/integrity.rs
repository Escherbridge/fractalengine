//! D-76 Merkle-extended signature root: `seal` and `verify_archive` (see `src/AGENTS.md` §integrity).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek, Write};

use anyhow::Context;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::entries::EntryCatalog;
use crate::limits;
use crate::manifest::{HexonManifest, TilesetBundleIndex};
use crate::signature::{sign_manifest, verify_manifest, verifying_key_to_did};

/// Integrity-root format version.
pub const INTEGRITY_VERSION: u32 = 1;
/// The one digest algorithm a version-1 root uses.
pub const INTEGRITY_ALGO: &str = "blake3";
/// The signed member; covered by the signature, never by a digest.
pub const MANIFEST_PATH: &str = "manifest.json";
/// Entry catalog; when header-committed it names the self-certifying assets.
pub const ENTRIES_PATH: &str = "entries.json";
/// Body-member digest map written by [`seal`].
pub const MEMBERS_DOC_PATH: &str = "members.json";
/// Signed chunk table of a container (a serialized [`TilesetBundleIndex`]).
pub const CHUNK_INDEX_PATH: &str = "chunks/index.json";
/// Prefix of self-certifying content-addressed asset members.
pub const ASSETS_PREFIX: &str = "assets/";
/// The only members a root's `header` map may commit (small JSON, read in the header phase).
pub const HEADER_MEMBER_PATHS: &[&str] = &[
    ENTRIES_PATH,
    "license.json",
    "schema.json",
    "terrain/tileset_meta.json",
    "terrain/config.json",
    MEMBERS_DOC_PATH,
    CHUNK_INDEX_PATH,
];

/// Zip path of a container's chunk `seq`.
pub fn chunk_member_path(seq: u32) -> String {
    format!("chunks/{seq}.hexon")
}

/// Lowercase blake3 hex of `bytes` (the digest form every map in the root uses).
pub fn digest_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// `manifest.integrity`: digests of the header-phase members, by exact stored bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityRoot {
    /// Root format version ([`INTEGRITY_VERSION`]).
    pub v: u32,
    /// Digest algorithm ([`INTEGRITY_ALGO`]).
    pub algo: String,
    /// Header member path → blake3 hex (always includes `members.json`).
    pub header: BTreeMap<String, String>,
}

/// `members.json`: every body member's path → blake3 hex (a flat JSON object).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MembersDoc {
    pub members: BTreeMap<String, String>,
}

/// How strictly a reader treats the seal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IntegrityPolicy {
    /// Refuse unsealed archives; with `pin`, refuse any other publisher DID.
    RequireSealed { pin: Option<String> },
    /// Enforce a seal when present; let legacy unsealed archives through.
    #[default]
    VerifyIfSealed,
}

/// What a passing verification established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrityStatus {
    /// Every member checked under a signature by `publisher_did`.
    Sealed { publisher_did: String },
    /// Legacy archive with no integrity root; nothing was verified.
    Unsealed,
}

/// One variant per integrity failure class.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrityError {
    #[error("hexon is not sealed but the integrity policy requires a seal")]
    NotSealed,
    #[error("unsupported integrity root: {0}")]
    Unsupported(String),
    #[error("malformed hexon: {0}")]
    Malformed(String),
    #[error("manifest signature does not verify against its publisher_did")]
    BadSignature,
    #[error("publisher {found} is not the pinned publisher {expected}")]
    PublisherMismatch { expected: String, found: String },
    #[error("member {path} does not match its committed digest")]
    DigestMismatch { path: String },
    #[error("member {path} is not covered by the integrity root")]
    Uncovered { path: String },
    #[error("committed member {path} is missing")]
    MissingMember { path: String },
    #[error("asset {hash} is not referenced by the signed entries.json")]
    AssetNotReferenced { hash: String },
}

/// Outcome of [`verify_sealed_header`].
#[derive(Debug, Clone)]
pub enum SealCheck {
    /// Signature, pin and header digests passed; check body members against this.
    Sealed(Box<SealedCoverage>),
    /// No integrity root and the policy allows that.
    Unsealed,
}

/// A verified header's commitments: what every other member must match.
#[derive(Debug, Clone)]
pub struct SealedCoverage {
    publisher_did: String,
    header: BTreeMap<String, String>,
    members: BTreeMap<String, String>,
    referenced_assets: BTreeSet<String>,
    chunk_index: Option<TilesetBundleIndex>,
}

impl SealedCoverage {
    /// The DID whose signature covers the root.
    pub fn publisher_did(&self) -> &str {
        &self.publisher_did
    }

    /// Committed digest for a header or body member, if any.
    pub fn committed_digest(&self, path: &str) -> Option<&str> {
        self.header
            .get(path)
            .or_else(|| self.members.get(path))
            .map(String::as_str)
    }

    /// Every body member `members.json` commits.
    pub fn body_members(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    /// The signed chunk table, when the archive is a container.
    pub fn chunk_index(&self) -> Option<&TilesetBundleIndex> {
        self.chunk_index.as_ref()
    }

    /// The signed blake3 hex of container chunk `seq`.
    pub fn chunk_digest(&self, seq: u32) -> Option<&str> {
        self.chunk_index
            .as_ref()?
            .bundles
            .iter()
            .find(|b| b.seq == seq)
            .map(|b| b.blake3_hex.as_str())
    }

    /// Check one member's blake3 hex against the coverage rules.
    pub fn check_member_digest(&self, path: &str, digest: &str) -> Result<(), IntegrityError> {
        if path == MANIFEST_PATH {
            return Ok(());
        }
        if let Some(hash) = path.strip_prefix(ASSETS_PREFIX) {
            // Assets are always self-certifying: this is the asset_hash check (A6).
            if !self.referenced_assets.contains(hash) {
                return Err(IntegrityError::AssetNotReferenced { hash: hash.into() });
            }
            return expect_digest(path, hash, digest);
        }
        match self.committed_digest(path) {
            Some(expected) => expect_digest(path, expected, digest),
            None => Err(IntegrityError::Uncovered { path: path.into() }),
        }
    }

    /// [`check_member_digest`](Self::check_member_digest) over raw member bytes.
    pub fn check_member(&self, path: &str, bytes: &[u8]) -> Result<(), IntegrityError> {
        self.check_member_digest(path, &digest_hex(bytes))
    }
}

fn expect_digest(path: &str, expected: &str, actual: &str) -> Result<(), IntegrityError> {
    if expected == actual {
        Ok(())
    } else {
        Err(IntegrityError::DigestMismatch { path: path.into() })
    }
}

fn malformed(context: &str, err: impl std::fmt::Display) -> IntegrityError {
    IntegrityError::Malformed(format!("{context}: {err:#}"))
}

/// Asset hashes the catalog references (`asset_hash` plus `sub_assets` values).
fn referenced_assets(catalog: &EntryCatalog) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in &catalog.entries {
        out.insert(entry.asset_hash.clone());
        if let Some(subs) = &entry.sub_assets {
            out.extend(subs.values().cloned());
        }
    }
    out
}

/// Every signed chunk must be a body member committed with the same digest.
fn check_chunk_index(
    index: &TilesetBundleIndex,
    members: &BTreeMap<String, String>,
) -> Result<(), IntegrityError> {
    for bundle in &index.bundles {
        let path = chunk_member_path(bundle.seq);
        match members.get(&path) {
            None => return Err(IntegrityError::MissingMember { path }),
            Some(digest) if *digest != bundle.blake3_hex => {
                return Err(IntegrityError::DigestMismatch { path })
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Header phase: verify the manifest signature, the pin and every header digest
/// from raw member bytes (the shape both transports carry); no body bytes needed.
pub fn verify_sealed_header(
    manifest_json: &[u8],
    header_members: &BTreeMap<String, Vec<u8>>,
    policy: &IntegrityPolicy,
) -> Result<SealCheck, IntegrityError> {
    let manifest: HexonManifest =
        serde_json::from_slice(manifest_json).map_err(|e| malformed(MANIFEST_PATH, e))?;
    let Some(root) = manifest.integrity else {
        return match policy {
            IntegrityPolicy::RequireSealed { .. } => Err(IntegrityError::NotSealed),
            IntegrityPolicy::VerifyIfSealed => Ok(SealCheck::Unsealed),
        };
    };
    if root.v != INTEGRITY_VERSION {
        return Err(IntegrityError::Unsupported(format!("version {}", root.v)));
    }
    if root.algo != INTEGRITY_ALGO {
        return Err(IntegrityError::Unsupported(format!("algo {}", root.algo)));
    }
    if let Some(path) = root
        .header
        .keys()
        .find(|p| !HEADER_MEMBER_PATHS.contains(&p.as_str()))
    {
        return Err(IntegrityError::Unsupported(format!(
            "header commits non-header member {path}"
        )));
    }

    let signature = manifest
        .signature
        .as_deref()
        .ok_or(IntegrityError::BadSignature)?;
    if !verify_manifest(manifest_json, signature, &manifest.publisher_did).unwrap_or(false) {
        return Err(IntegrityError::BadSignature);
    }
    if let IntegrityPolicy::RequireSealed { pin: Some(pin) } = policy {
        if *pin != manifest.publisher_did {
            return Err(IntegrityError::PublisherMismatch {
                expected: pin.clone(),
                found: manifest.publisher_did,
            });
        }
    }

    // Digests first, parsing second: a tampered member reports DigestMismatch.
    for (path, expected) in &root.header {
        let bytes = header_members
            .get(path)
            .ok_or_else(|| IntegrityError::MissingMember { path: path.clone() })?;
        expect_digest(path, expected, &digest_hex(bytes))?;
    }
    let committed = |path: &str| {
        root.header
            .contains_key(path)
            .then(|| &header_members[path])
    };
    let members = match committed(MEMBERS_DOC_PATH) {
        Some(bytes) => {
            serde_json::from_slice::<MembersDoc>(bytes)
                .map_err(|e| malformed(MEMBERS_DOC_PATH, e))?
                .members
        }
        None => BTreeMap::new(),
    };
    let referenced_assets = match committed(ENTRIES_PATH) {
        Some(bytes) => referenced_assets(
            &serde_json::from_slice(bytes).map_err(|e| malformed(ENTRIES_PATH, e))?,
        ),
        None => BTreeSet::new(),
    };
    let chunk_index = match committed(CHUNK_INDEX_PATH) {
        Some(bytes) => {
            let index: TilesetBundleIndex =
                serde_json::from_slice(bytes).map_err(|e| malformed(CHUNK_INDEX_PATH, e))?;
            check_chunk_index(&index, &members)?;
            Some(index)
        }
        None => None,
    };

    Ok(SealCheck::Sealed(Box::new(SealedCoverage {
        publisher_did: manifest.publisher_did,
        header: root.header,
        members,
        referenced_assets,
        chunk_index,
    })))
}

/// Read member `index` under `cap` and the shared `budget`.
fn read_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    cap: u64,
    budget: &mut u64,
) -> anyhow::Result<(String, Vec<u8>)> {
    let mut file = archive.by_index(index)?;
    let name = file.name().to_string();
    let declared = file.size();
    let bytes = limits::read_entry_capped(&mut file, declared, cap, budget)
        .with_context(|| format!("reading {name}"))?;
    Ok((name, bytes))
}

fn cap_for(path: &str) -> u64 {
    if HEADER_MEMBER_PATHS.contains(&path) || path == MANIFEST_PATH {
        limits::MAX_JSON_ENTRY_BYTES
    } else {
        limits::MAX_ENTRY_UNCOMPRESSED_BYTES
    }
}

/// Read member `name` for verification; absence is [`IntegrityError::MissingMember`].
fn read_for_verify<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    budget: &mut u64,
) -> Result<Vec<u8>, IntegrityError> {
    let index = archive
        .index_for_name(name)
        .ok_or_else(|| IntegrityError::MissingMember { path: name.into() })?;
    read_index(archive, index, cap_for(name), budget)
        .map(|(_, data)| data)
        .map_err(|e| malformed(name, e))
}

/// Verify a whole archive: header phase, then every member's coverage and digest.
pub fn verify_archive(
    bytes: &[u8],
    policy: &IntegrityPolicy,
) -> Result<IntegrityStatus, IntegrityError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| malformed("zip archive", e))?;
    limits::check_entry_count(archive.len()).map_err(|e| malformed("zip archive", e))?;
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;

    let manifest_json = match read_for_verify(&mut archive, MANIFEST_PATH, &mut budget) {
        Err(IntegrityError::MissingMember { .. }) => {
            return Err(IntegrityError::Malformed("missing manifest.json".into()))
        }
        other => other?,
    };
    let manifest: HexonManifest =
        serde_json::from_slice(&manifest_json).map_err(|e| malformed(MANIFEST_PATH, e))?;
    let mut header_members = BTreeMap::new();
    if let Some(root) = &manifest.integrity {
        for path in root.header.keys() {
            // Absent or non-header paths are reported by `verify_sealed_header`.
            if HEADER_MEMBER_PATHS.contains(&path.as_str())
                && archive.index_for_name(path).is_some()
            {
                let data = read_for_verify(&mut archive, path, &mut budget)?;
                header_members.insert(path.clone(), data);
            }
        }
    }

    let coverage = match verify_sealed_header(&manifest_json, &header_members, policy)? {
        SealCheck::Unsealed => return Ok(IntegrityStatus::Unsealed),
        SealCheck::Sealed(coverage) => coverage,
    };

    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    for name in &names {
        if name == MANIFEST_PATH || header_members.contains_key(name) {
            continue;
        }
        let data = read_for_verify(&mut archive, name, &mut budget)?;
        coverage.check_member(name, &data)?;
    }
    if let Some(path) = coverage
        .body_members()
        .find(|path| archive.index_for_name(path).is_none())
    {
        return Err(IntegrityError::MissingMember { path: path.into() });
    }

    Ok(IntegrityStatus::Sealed {
        publisher_did: coverage.publisher_did,
    })
}

/// Seal an archive: raw-copy every member, write `members.json`, set and sign
/// `manifest.integrity` (publisher = the key's DID). Always the LAST transform.
pub fn seal(archive_bytes: &[u8], key: &SigningKey) -> anyhow::Result<Vec<u8>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(archive_bytes)).context("seal: failed to open ZIP")?;
    limits::check_entry_count(archive.len())?;
    let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;

    let mut manifest: Option<HexonManifest> = None;
    let mut header_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut body_digests: Vec<(String, String)> = Vec::new();
    let mut copy_order: Vec<usize> = Vec::new();
    for index in 0..archive.len() {
        let name = archive
            .name_for_index(index)
            .unwrap_or_default()
            .to_string();
        if name == MEMBERS_DOC_PATH {
            continue; // re-seal: recomputed below
        }
        let (name, data) = read_index(&mut archive, index, cap_for(&name), &mut budget)?;
        if name == MANIFEST_PATH {
            manifest = Some(serde_json::from_slice(&data).context("seal: invalid manifest.json")?);
            continue;
        }
        copy_order.push(index);
        if HEADER_MEMBER_PATHS.contains(&name.as_str()) {
            header_bytes.insert(name, data);
        } else {
            body_digests.push((name, digest_hex(&data)));
        }
    }
    let mut manifest = manifest.context("seal: missing manifest.json")?;

    let referenced = match header_bytes.get(ENTRIES_PATH) {
        Some(bytes) => {
            referenced_assets(&serde_json::from_slice(bytes).context("seal: invalid entries.json")?)
        }
        None => BTreeSet::new(),
    };
    let mut members = BTreeMap::new();
    for (path, digest) in body_digests {
        if let Some(hash) = path.strip_prefix(ASSETS_PREFIX) {
            if !referenced.contains(hash) {
                return Err(IntegrityError::AssetNotReferenced { hash: hash.into() }.into());
            }
            expect_digest(&path, hash, &digest)?;
        } else {
            members.insert(path, digest);
        }
    }
    if let Some(bytes) = header_bytes.get(CHUNK_INDEX_PATH) {
        let index: TilesetBundleIndex =
            serde_json::from_slice(bytes).context("seal: invalid chunks/index.json")?;
        check_chunk_index(&index, &members)?;
    }

    let members_json = serde_json::to_vec(&MembersDoc { members })?;
    if members_json.len() as u64 > limits::MAX_JSON_ENTRY_BYTES {
        anyhow::bail!(
            "seal: members.json would be {} bytes (cap {}); split the tileset into a chunked container",
            members_json.len(),
            limits::MAX_JSON_ENTRY_BYTES
        );
    }
    let mut header: BTreeMap<String, String> = header_bytes
        .iter()
        .map(|(path, bytes)| (path.clone(), digest_hex(bytes)))
        .collect();
    header.insert(MEMBERS_DOC_PATH.into(), digest_hex(&members_json));

    manifest.publisher_did = verifying_key_to_did(&key.verifying_key());
    manifest.integrity = Some(IntegrityRoot {
        v: INTEGRITY_VERSION,
        algo: INTEGRITY_ALGO.into(),
        header,
    });
    manifest.signature = None;
    let unsigned = serde_json::to_vec(&manifest)?;
    manifest.signature = Some(sign_manifest(&unsigned, key)?);

    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    out.start_file(MANIFEST_PATH, deflated)?;
    out.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    for index in copy_order {
        out.raw_copy_file(archive.by_index_raw(index)?)?;
    }
    out.start_file(MEMBERS_DOC_PATH, deflated)?;
    out.write_all(&members_json)?;
    Ok(out.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{AssetEntry, EntryKind};
    use crate::manifest::HexonType;
    use crate::HexonArchive;

    fn manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "unit".into(),
            hexon_type: HexonType::Model,
            publisher_did: "did:key:z6Mkunit".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Unit".into(),
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

    fn entry(asset_hash: &str) -> AssetEntry {
        AssetEntry {
            entry_id: "e".into(),
            kind: EntryKind::Model,
            asset_hash: asset_hash.into(),
            format: "glb".into(),
            label: "e".into(),
            tags: vec![],
            description: None,
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }
    }

    /// A21 asset_hash: seal refuses an asset whose bytes are not its catalogued hash,
    /// and a sealed model with a content-addressed asset verifies.
    #[test]
    fn seal_enforces_asset_hash_and_verifies_a_self_certifying_asset() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let blob = b"glb-bytes".to_vec();
        let real = digest_hex(&blob);
        let fake = digest_hex(b"something else");

        let honest = HexonArchive::export(
            manifest(),
            vec![entry(&real)],
            vec![(real.clone(), blob.clone())],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let sealed = seal(&honest, &key).expect("honest model seals");
        let status = verify_archive(&sealed, &IntegrityPolicy::default()).expect("verifies");
        assert_eq!(
            status,
            IntegrityStatus::Sealed {
                publisher_did: verifying_key_to_did(&key.verifying_key())
            }
        );

        let lying = HexonArchive::export(
            manifest(),
            vec![entry(&fake)],
            vec![(fake.clone(), blob)],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let err = seal(&lying, &key).expect_err("asset_hash mismatch must not seal");
        assert_eq!(
            err.downcast_ref::<IntegrityError>(),
            Some(&IntegrityError::DigestMismatch {
                path: format!("assets/{fake}")
            })
        );
    }
}
