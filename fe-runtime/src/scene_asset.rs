//! Private scene-asset transfer admission and native demand; see AGENTS.md.

use crate::{blob_store::BlobHash, scene_replication::ReplicaNodeProvenance};
use std::time::Instant;

pub const MAX_SCENE_ASSET_BYTES: u64 = 256 * 1024 * 1024;
pub const SCENE_ASSET_ROW_MARKER: &str = "_fe_scene_asset";
/// The MIME types a scene asset row may declare (transfer, publication and upload share it).
pub const SCENE_ASSET_MIMES: [&str; 3] = ["model/gltf-binary", "model/gltf+json", PORTAL_BUNDLE_MIME];
/// Sealed portal bundle MIME (mirrors `fe_format::portal_bundle::PORTAL_BUNDLE_MIME`; fe-format cannot be a dependency here).
pub const PORTAL_BUNDLE_MIME: &str = "application/vnd.fractalengine.portal-bundle+hexon";
/// Bundle size cap (OD-4 default); the per-node override is enforced at ingest.
pub const MAX_PORTAL_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

/// The DB resolves all identities and authority; a request is never an authority token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneAssetTransferPurpose {
    Publish { asset_id: String, petal_id: String },
    Fetch,
    Serve { requester_did: String },
}

#[derive(Debug, Clone)]
pub struct SceneAssetTransferRequest {
    pub verse_id: String,
    pub hash: BlobHash,
    pub node_id: Option<String>,
    pub purpose: SceneAssetTransferPurpose,
    pub deadline: Instant,
}

/// Exact current admission witness; callers must re-read before persisting or reporting readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneAssetTransferPermit {
    pub verse_id: String,
    pub petal_id: String,
    /// The live reference the request named; `None` for an anchored asset admitted on its
    /// own metadata (QA-0158: an upload is Verse content before its first placement).
    pub node_id: Option<String>,
    pub asset_id: String,
    pub content_hash: BlobHash,
    pub content_type: String,
    pub size_bytes: u64,
    pub asset_fingerprint: String,
}

/// Missing bytes under an actual current read; this hint grants no transfer authority.
#[derive(Debug, Clone)]
pub struct SceneAssetNeeded {
    pub verse_id: String,
    pub scope: String,
    pub node_id: String,
    pub asset_id: String,
    pub content_hash: BlobHash,
    pub size_bytes: u64,
    pub provenance: ReplicaNodeProvenance,
}

/// Owner-reviewed exact local-copy adoption; not a claim about the original uploader.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetAdoptionCandidate {
    pub node_id: String,
    pub asset_id: String,
    pub scope: String,
    pub content_hash: BlobHash,
    pub content_type: String,
    pub size_bytes: u64,
    pub asset_fingerprint: String,
    pub reference_fingerprints: Vec<SceneAssetReferenceWitness>,
    pub model_fingerprint: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetAdoptionReceipt {
    pub candidate: SceneAssetAdoptionCandidate,
    pub operation_id: String,
    pub adopter_did: String,
    pub adopted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneAssetReferenceWitness {
    pub node_id: String,
    pub fingerprint: String,
}

/// Verify bounded canonical bytes independently of any transport or metadata authority.
pub fn verify_scene_asset_bytes(
    bytes: &[u8],
    hash: &BlobHash,
    content_type: &str,
    size_bytes: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        size_bytes > 0 && size_bytes <= MAX_SCENE_ASSET_BYTES && bytes.len() as u64 == size_bytes,
        "scene asset size differs from bounded metadata"
    );
    anyhow::ensure!(
        *blake3::hash(bytes).as_bytes() == *hash,
        "scene asset digest mismatch"
    );
    if content_type == PORTAL_BUNDLE_MIME {
        // Bundles replicate as inert bytes: no glTF branch, no texture guard (see AGENTS.md scene-asset-transfer).
        return verify_portal_bundle_bytes(bytes);
    }
    let extension = match content_type {
        "model/gltf-binary" => {
            anyhow::ensure!(
                bytes.starts_with(b"glTF"),
                "GLB MIME requires a GLB container"
            );
            "glb"
        }
        "model/gltf+json" => {
            let document: serde_json::Value = serde_json::from_slice(bytes)?;
            anyhow::ensure!(
                document["asset"]["version"]
                    .as_str()
                    .is_some_and(|version| version.starts_with("2.")),
                "glTF MIME requires a glTF 2 document"
            );
            "gltf"
        }
        _ => anyhow::bail!("unsupported scene asset MIME"),
    };
    crate::ktx2_guard::check_asset_bytes(bytes, Some(extension))
        .map_err(|error| anyhow::anyhow!("scene asset refused: {error}"))?;
    Ok(())
}

const ZIP_LOCAL_SIGNATURE: &[u8; 4] = &[0x50, 0x4B, 0x03, 0x04];
const ZIP_EOCD_SIGNATURE: &[u8; 4] = &[0x50, 0x4B, 0x05, 0x06];
const ZIP_CENTRAL_SIGNATURE: &[u8; 4] = &[0x50, 0x4B, 0x01, 0x02];
const ZIP_EOCD_LEN: usize = 22;
const BUNDLE_MAX_MEMBERS: usize = 4096;
const BUNDLE_HEADER_MEMBER_MAX: u64 = 64 * 1024;
const BUNDLE_HEADER_MEMBERS: [&str; 5] = [
    "manifest.json",
    "members.json",
    "entries.json",
    "license.json",
    "plugin/extension.json",
];

fn le16(bytes: &[u8], at: usize) -> usize {
    u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize
}

fn le32(bytes: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as u64
}

/// Bounded structural check of a sealed portal bundle: ZIP magic, central directory, header members.
/// Signature and digest verification stay in the admission pipeline (`verify_sealed_header`).
pub fn verify_portal_bundle_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_PORTAL_BUNDLE_BYTES,
        "portal bundle exceeds the size cap"
    );
    anyhow::ensure!(
        bytes.starts_with(ZIP_LOCAL_SIGNATURE),
        "bundle MIME requires a ZIP container"
    );
    anyhow::ensure!(bytes.len() >= ZIP_EOCD_LEN, "bundle has no end record");
    let search_floor = bytes.len().saturating_sub(ZIP_EOCD_LEN + u16::MAX as usize);
    let eocd = (search_floor..=bytes.len() - ZIP_EOCD_LEN)
        .rev()
        .find(|&at| &bytes[at..at + 4] == ZIP_EOCD_SIGNATURE)
        .ok_or_else(|| anyhow::anyhow!("bundle has no end record"))?;
    anyhow::ensure!(
        eocd + ZIP_EOCD_LEN + le16(bytes, eocd + 20) == bytes.len(),
        "bundle end record comment is inconsistent"
    );
    let entries = le16(bytes, eocd + 10);
    let directory_size = le32(bytes, eocd + 12);
    let directory_offset = le32(bytes, eocd + 16);
    anyhow::ensure!(
        le16(bytes, eocd + 4) == 0 && le16(bytes, eocd + 6) == 0 && le16(bytes, eocd + 8) == entries,
        "multi-disk bundles are refused"
    );
    anyhow::ensure!(
        entries != 0xFFFF && directory_size != 0xFFFF_FFFF && directory_offset != 0xFFFF_FFFF,
        "zip64 bundles are refused"
    );
    anyhow::ensure!(
        (1..=BUNDLE_MAX_MEMBERS).contains(&entries)
            && directory_offset + directory_size == eocd as u64,
        "bundle central directory is out of bounds"
    );
    let mut cursor = directory_offset as usize;
    let mut names = std::collections::BTreeMap::<String, (usize, u64, u64, usize)>::new();
    let mut total_uncompressed = 0u64;
    for _ in 0..entries {
        anyhow::ensure!(
            cursor + 46 <= eocd && &bytes[cursor..cursor + 4] == ZIP_CENTRAL_SIGNATURE,
            "bundle central directory is malformed"
        );
        let (flags, method) = (le16(bytes, cursor + 8), le16(bytes, cursor + 10));
        let (compressed, uncompressed) = (le32(bytes, cursor + 20), le32(bytes, cursor + 24));
        let (name_len, extra_len, comment_len) = (
            le16(bytes, cursor + 28),
            le16(bytes, cursor + 30),
            le16(bytes, cursor + 32),
        );
        let local_offset = le32(bytes, cursor + 42) as usize;
        let name_end = cursor + 46 + name_len;
        anyhow::ensure!(
            flags & 1 == 0 && (method == 0 || method == 8),
            "bundle member is encrypted or uses an unsupported method"
        );
        anyhow::ensure!(
            name_len > 0 && name_end + extra_len + comment_len <= eocd,
            "bundle central directory is malformed"
        );
        let name = std::str::from_utf8(&bytes[cursor + 46..name_end])?;
        anyhow::ensure!(
            !name.starts_with('/')
                && !name.contains('\\')
                && !name.chars().any(char::is_control)
                && !name.split('/').any(|part| part == ".."),
            "bundle member path is unsafe"
        );
        anyhow::ensure!(
            local_offset + 30 <= directory_offset as usize
                && &bytes[local_offset..local_offset + 4] == ZIP_LOCAL_SIGNATURE,
            "bundle member has no local header"
        );
        total_uncompressed += uncompressed;
        anyhow::ensure!(
            total_uncompressed <= MAX_SCENE_ASSET_BYTES,
            "bundle expands past the scene asset cap"
        );
        anyhow::ensure!(
            names.insert(name.to_owned(), (method, compressed, uncompressed, local_offset)).is_none(),
            "bundle repeats a member path"
        );
        cursor = name_end + extra_len + comment_len;
    }
    anyhow::ensure!(cursor == eocd, "bundle central directory is malformed");
    for required in ["manifest.json", "plugin/extension.json"] {
        anyhow::ensure!(names.contains_key(required), "sealed bundle lacks {required}");
    }
    for header in BUNDLE_HEADER_MEMBERS {
        if let Some((_, _, uncompressed, _)) = names.get(header) {
            anyhow::ensure!(
                *uncompressed <= BUNDLE_HEADER_MEMBER_MAX,
                "bundle header member {header} is oversized"
            );
        }
    }
    // A stored manifest is readable without inflating: it must name a plugin hexon with an integrity root.
    let (method, compressed, _, local_offset) = names["manifest.json"];
    if method == 0 {
        let start = local_offset + 30 + le16(bytes, local_offset + 26) + le16(bytes, local_offset + 28);
        let end = start + compressed as usize;
        anyhow::ensure!(end <= directory_offset as usize, "bundle manifest is truncated");
        let manifest: serde_json::Value = serde_json::from_slice(&bytes[start..end])?;
        anyhow::ensure!(
            manifest["hexon_type"] == "plugin" && manifest["integrity"].is_object(),
            "bundle is not a sealed plugin hexon"
        );
    }
    Ok(())
}

/// Private metadata-only audit protocol; see AGENTS.md scene-asset-publication-audit.
pub const SCENE_ASSET_AUDIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(8);
pub const SCENE_ASSET_AUDIT_RESPONSE_MAX: usize = 256 * 1024;
pub const SCENE_ASSET_AUDIT_INTERNAL_MAX: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SceneAssetAuditCaller {
    pub did: String,
    pub scope: String,
    pub role_ceiling: String,
}

#[derive(Debug, Clone)]
pub struct SceneAssetAuditRequest {
    pub node_id: String,
    pub operation_id: Option<String>,
    pub caller: SceneAssetAuditCaller,
    pub deadline: Instant,
}
impl SceneAssetAuditRequest {
    pub fn is_current(&self) -> bool {
        let now = Instant::now();
        self.deadline > now && self.deadline <= now + SCENE_ASSET_AUDIT_LIMIT
    }
    pub fn validate_ids(&self) -> bool {
        crate::safe_id::is_safe_path_id(&self.node_id)
            && self
                .operation_id
                .as_ref()
                .is_none_or(|id| is_canonical_audit_ulid(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum SceneAssetField<T> {
    Absent,
    Null,
    Value(T),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditProvenance {
    pub kind: String,
    pub operation_id: String,
    pub adopter_did: String,
    pub adopted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditReference {
    pub node_id: String,
    pub petal_id: String,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetAuditOperation {
    pub operation_id: String,
    pub op_type: String,
    pub node_id: String,
    pub scope: String,
    pub asset_id: String,
    pub content_hash: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub provenance: SceneAssetAuditProvenance,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneAssetPublicationAudit {
    pub protocol_version: u8,
    pub request_nonce: String,
    pub digest_algorithm: String,
    pub node_id: String,
    pub asset_id: String,
    pub petal_id: String,
    pub scope: String,
    pub content_hash: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub owner_did: SceneAssetField<String>,
    pub asset_petal_id: SceneAssetField<String>,
    pub publication_provenance: SceneAssetField<SceneAssetAuditProvenance>,
    pub asset_state_digest: String,
    pub references: Vec<SceneAssetAuditReference>,
    pub model_state_digest: Option<String>,
    pub payload_reference_count: u8,
    pub adoption_operation: Option<SceneAssetAuditOperation>,
    pub bytes_verified_by_this_read: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneAssetAuditFailure {
    Unavailable,
    Expired,
    Busy,
    InvalidRequest,
}

/// Canonical uppercase Crockford ULID, including its 128-bit leading-digit bound.
pub fn is_canonical_audit_ulid(id: &str) -> bool {
    id.len() == 26
        && id.as_bytes()[0] <= b'7'
        && id
            .bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}
