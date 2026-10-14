//! KTX2 intake guard: refuses glTF assets that would reach bevy's KTX2/zstd
//! decoder (RUSTSEC-2026-0344 interim mitigation). See src/AGENTS.md §ktx2-guard.

use serde::de::IgnoredAny;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// Largest JSON chunk (GLB) or `.gltf` document the guard will parse.
pub const MAX_GLTF_JSON_LEN: usize = 64 * 1024 * 1024;

/// KTX2 file identifier: `«KTX 20»\r\n\x1A\n`.
pub const KTX2_MAGIC: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];

const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;
const CHUNK_TYPE_JSON: &[u8; 4] = b"JSON";
const CHUNK_TYPE_BIN: &[u8; 4] = b"BIN\0";
const KTX2_MIME: &str = "image/ktx2";
const BASISU_EXTENSION: &str = "KHR_texture_basisu";

/// Why an asset was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ktx2GuardError {
    /// The bytes end before a header or chunk they declare.
    Truncated(&'static str),
    /// Not a binary glTF 2.0 container.
    NotGlb(&'static str),
    /// The GLB header's total length disagrees with the byte count.
    LengthMismatch { declared: u64, actual: u64 },
    /// The JSON exceeds [`MAX_GLTF_JSON_LEN`].
    JsonTooLarge { len: u64, max: usize },
    /// The glTF JSON does not parse (bevy's loader would refuse it too).
    InvalidJson(String),
    /// Structurally invalid in a way that could make the loader misbehave.
    Malformed(String),
    /// The asset carries a KTX2 / Basis Universal texture: the refusal itself.
    Ktx2Texture(String),
}

impl fmt::Display for Ktx2GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "truncated glTF asset: {what}"),
            Self::NotGlb(what) => write!(f, "not a glTF 2.0 binary: {what}"),
            Self::LengthMismatch { declared, actual } => write!(
                f,
                "GLB header declares {declared} bytes but the asset has {actual}"
            ),
            Self::JsonTooLarge { len, max } => {
                write!(f, "glTF JSON is {len} bytes; the limit is {max}")
            }
            Self::InvalidJson(e) => write!(f, "glTF JSON does not parse: {e}"),
            Self::Malformed(what) => write!(f, "malformed glTF asset: {what}"),
            Self::Ktx2Texture(evidence) => write!(
                f,
                "KTX2/Basis textures are refused until the ruzstd decompression-bomb \
                 fix (RUSTSEC-2026-0344) ships in bevy: {evidence}"
            ),
        }
    }
}

impl std::error::Error for Ktx2GuardError {}

/// Guard any bytes about to be handed to an asset loader. `extension` is the
/// requested path's extension (without the dot), if any.
pub fn check_asset_bytes(bytes: &[u8], extension: Option<&str>) -> Result<(), Ktx2GuardError> {
    if bytes.starts_with(GLB_MAGIC) {
        return check_glb(bytes);
    }
    if bytes.starts_with(&KTX2_MAGIC) {
        return Err(Ktx2GuardError::Ktx2Texture(
            "the asset is a raw KTX2 image".into(),
        ));
    }
    match extension.map(str::to_ascii_lowercase).as_deref() {
        Some("ktx2") | Some("basis") => Err(Ktx2GuardError::Ktx2Texture(
            "the asset path has a KTX2/Basis extension".into(),
        )),
        Some("gltf") => check_gltf_json(bytes, None),
        // `gltf::Gltf::from_slice` parses anything without the GLB magic as a
        // JSON glTF, so a `.glb` (or extensionless) path can carry one too.
        _ if looks_like_json_object(bytes) => check_gltf_json(bytes, None),
        _ => Ok(()),
    }
}

/// True when serde_json would read `bytes` as an object (leading JSON whitespace, then `{`).
fn looks_like_json_object(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        == Some(&b'{')
}

/// Guard a binary glTF (GLB) container.
///
/// A GLB whose first chunk is not JSON passes: bevy's `gltf` parser refuses
/// it (`GlbError::ChunkType`), so it can never reach an image decoder.
pub fn check_glb(bytes: &[u8]) -> Result<(), Ktx2GuardError> {
    if bytes.len() < GLB_HEADER_LEN {
        return Err(Ktx2GuardError::Truncated(
            "shorter than the 12-byte GLB header",
        ));
    }
    if !bytes.starts_with(GLB_MAGIC) {
        return Err(Ktx2GuardError::NotGlb("missing the glTF magic"));
    }
    if read_u32(bytes, 4) != Some(2) {
        return Err(Ktx2GuardError::NotGlb("version is not 2"));
    }
    let declared = u64::from(read_u32(bytes, 8).unwrap_or(0));
    let actual = bytes.len() as u64;
    if declared != actual {
        return Err(Ktx2GuardError::LengthMismatch { declared, actual });
    }
    if bytes.len() == GLB_HEADER_LEN {
        return Ok(());
    }
    let Some(json_chunk) = read_chunk(bytes, GLB_HEADER_LEN)? else {
        return Ok(());
    };
    if json_chunk.chunk_type != *CHUNK_TYPE_JSON {
        return Ok(());
    }
    // Bound the JSON before slicing or parsing it.
    if json_chunk.declared_len > MAX_GLTF_JSON_LEN as u64 {
        return Err(Ktx2GuardError::JsonTooLarge {
            len: json_chunk.declared_len,
            max: MAX_GLTF_JSON_LEN,
        });
    }
    let json = json_chunk.data(bytes)?;
    let bin = match read_chunk(bytes, json_chunk.end)? {
        Some(chunk) if chunk.chunk_type == *CHUNK_TYPE_BIN => Some(chunk.data(bytes)?),
        _ => None,
    };
    inspect_gltf_json(json, bin)
}

/// Guard a glTF JSON document (`.gltf`, or a GLB's JSON chunk with its BIN chunk).
pub fn check_gltf_json(json: &[u8], bin: Option<&[u8]>) -> Result<(), Ktx2GuardError> {
    if json.len() > MAX_GLTF_JSON_LEN {
        return Err(Ktx2GuardError::JsonTooLarge {
            len: json.len() as u64,
            max: MAX_GLTF_JSON_LEN,
        });
    }
    inspect_gltf_json(json, bin)
}

/// The only glTF fields the guard reads; everything else is skipped unallocated.
#[derive(Deserialize)]
struct GltfRoot {
    #[serde(default, rename = "extensionsUsed")]
    extensions_used: Vec<String>,
    #[serde(default, rename = "extensionsRequired")]
    extensions_required: Vec<String>,
    #[serde(default)]
    images: Vec<GltfImage>,
    #[serde(default)]
    textures: Vec<GltfTexture>,
    #[serde(default, rename = "bufferViews")]
    buffer_views: Vec<GltfBufferView>,
    #[serde(default)]
    buffers: Vec<GltfBuffer>,
}

#[derive(Deserialize)]
struct GltfBuffer {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
struct GltfImage {
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default, rename = "bufferView")]
    buffer_view: Option<u64>,
}

#[derive(Deserialize)]
struct GltfTexture {
    #[serde(default)]
    extensions: Option<BTreeMap<String, IgnoredAny>>,
}

#[derive(Deserialize)]
struct GltfBufferView {
    buffer: u64,
    #[serde(default, rename = "byteOffset")]
    byte_offset: u64,
    #[serde(rename = "byteLength")]
    byte_length: u64,
}

fn inspect_gltf_json(json: &[u8], bin: Option<&[u8]>) -> Result<(), Ktx2GuardError> {
    let root: GltfRoot =
        serde_json::from_slice(json).map_err(|e| Ktx2GuardError::InvalidJson(e.to_string()))?;

    for list in [&root.extensions_used, &root.extensions_required] {
        if list.iter().any(|name| name == BASISU_EXTENSION) {
            return Err(Ktx2GuardError::Ktx2Texture(format!(
                "the asset declares the {BASISU_EXTENSION} extension"
            )));
        }
    }
    for (index, texture) in root.textures.iter().enumerate() {
        if texture
            .extensions
            .as_ref()
            .is_some_and(|extensions| extensions.contains_key(BASISU_EXTENSION))
        {
            return Err(Ktx2GuardError::Ktx2Texture(format!(
                "textures[{index}] uses {BASISU_EXTENSION}"
            )));
        }
    }
    // bevy_gltf `percent_decode_str(uri).decode_utf8().unwrap()`s every buffer
    // and image URI; with `panic = "abort"` an invalid one would kill the GUI.
    for (index, buffer) in root.buffers.iter().enumerate() {
        if let Some(uri) = buffer.uri.as_deref() {
            percent_decode_utf8(uri).ok_or_else(|| {
                Ktx2GuardError::Malformed(format!(
                    "buffers[{index}].uri is not UTF-8 once percent-decoded"
                ))
            })?;
        }
    }
    for (index, image) in root.images.iter().enumerate() {
        inspect_image(index, image, &root.buffer_views, bin)?;
    }
    Ok(())
}

/// Percent-decode like `percent_encoding::percent_decode_str(..).decode_utf8()`
/// (a `%` not followed by two hex digits stays literal); `None` if not UTF-8.
fn percent_decode_utf8(uri: &str) -> Option<String> {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let raw = uri.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let decoded = (raw[i] == b'%')
            .then(|| Some((hex(*raw.get(i + 1)?)? << 4) | hex(*raw.get(i + 2)?)?))
            .flatten();
        match decoded {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(raw[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn inspect_image(
    index: usize,
    image: &GltfImage,
    buffer_views: &[GltfBufferView],
    bin: Option<&[u8]>,
) -> Result<(), Ktx2GuardError> {
    // bevy_image maps the mime type case-insensitively (`from_mime_type`).
    if image
        .mime_type
        .as_deref()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case(KTX2_MIME))
    {
        return Err(Ktx2GuardError::Ktx2Texture(format!(
            "images[{index}].mimeType is {KTX2_MIME}"
        )));
    }
    if let Some(uri) = image.uri.as_deref() {
        inspect_image_uri(index, uri)?;
    }
    if let Some(view_index) = image.buffer_view {
        let view = usize::try_from(view_index)
            .ok()
            .and_then(|i| buffer_views.get(i))
            .ok_or_else(|| {
                Ktx2GuardError::Malformed(format!(
                    "images[{index}].bufferView {view_index} does not exist"
                ))
            })?;
        // Only buffer 0 of a GLB is the embedded BIN chunk; other buffers are
        // external or data URIs, and their images are covered by the mime check.
        if let (0, Some(bin)) = (view.buffer, bin) {
            let start = view.byte_offset;
            let end = start.checked_add(view.byte_length);
            if end.is_none_or(|end| end > bin.len() as u64) {
                return Err(Ktx2GuardError::Malformed(format!(
                    "images[{index}].bufferView {view_index} lies outside the BIN chunk"
                )));
            }
            let start = start as usize;
            let image_bytes = &bin[start..start + view.byte_length as usize];
            if image_bytes.starts_with(&KTX2_MAGIC) {
                return Err(Ktx2GuardError::Ktx2Texture(format!(
                    "images[{index}] bufferView {view_index} holds KTX2 bytes"
                )));
            }
        }
    }
    Ok(())
}

fn inspect_image_uri(index: usize, raw_uri: &str) -> Result<(), Ktx2GuardError> {
    // bevy_gltf percent-decodes the URI before it tests for `data:` or
    // resolves a path, so `data:image%2Fktx2...` must be judged decoded.
    let uri = percent_decode_utf8(raw_uri).ok_or_else(|| {
        Ktx2GuardError::Malformed(format!(
            "images[{index}].uri is not UTF-8 once percent-decoded"
        ))
    })?;
    let uri = uri.as_str();
    if let Some(rest) = uri.strip_prefix("data:") {
        let (header, payload) = rest.split_once(',').unwrap_or((rest, ""));
        let mime = header.split(';').next().unwrap_or("");
        if mime.trim().eq_ignore_ascii_case(KTX2_MIME) {
            return Err(Ktx2GuardError::Ktx2Texture(format!(
                "images[{index}] is a {KTX2_MIME} data URI"
            )));
        }
        // 16 base64 characters decode to exactly the 12-byte KTX2 identifier.
        if header.ends_with(";base64")
            && payload.get(..16).and_then(decode_base64_prefix) == Some(KTX2_MAGIC)
        {
            return Err(Ktx2GuardError::Ktx2Texture(format!(
                "images[{index}] data URI holds KTX2 bytes"
            )));
        }
        return Ok(());
    }
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".ktx2") || lower.ends_with(".basis") {
        return Err(Ktx2GuardError::Ktx2Texture(format!(
            "images[{index}].uri points at a KTX2/Basis file"
        )));
    }
    Ok(())
}

/// Decode exactly 16 standard-alphabet base64 characters into 12 bytes.
fn decode_base64_prefix(text: &str) -> Option<[u8; 12]> {
    fn sextet(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let chars: &[u8; 16] = text.as_bytes().try_into().ok()?;
    let (quads, _) = chars.as_chunks::<4>();
    let mut out = [0u8; 12];
    for (group, quad) in quads.iter().enumerate() {
        let mut word = 0u32;
        for &c in quad {
            word = (word << 6) | sextet(c)?;
        }
        out[group * 3] = (word >> 16) as u8;
        out[group * 3 + 1] = (word >> 8) as u8;
        out[group * 3 + 2] = word as u8;
    }
    Some(out)
}

/// One GLB chunk: its type, declared length and where its data starts/ends.
struct Chunk {
    chunk_type: [u8; 4],
    declared_len: u64,
    start: usize,
    end: usize,
}

impl Chunk {
    /// The chunk's bytes, or `Truncated` when the declared length overruns the asset.
    fn data<'a>(&self, bytes: &'a [u8]) -> Result<&'a [u8], Ktx2GuardError> {
        bytes
            .get(self.start..self.end)
            .ok_or(Ktx2GuardError::Truncated(
                "a chunk is longer than the asset",
            ))
    }
}

/// Read the chunk header at `offset`; `None` when no bytes remain.
fn read_chunk(bytes: &[u8], offset: usize) -> Result<Option<Chunk>, Ktx2GuardError> {
    if offset >= bytes.len() {
        return Ok(None);
    }
    let header = bytes
        .get(offset..offset + CHUNK_HEADER_LEN)
        .ok_or(Ktx2GuardError::Truncated("a chunk header is cut short"))?;
    let declared_len = u64::from(u32::from_le_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let chunk_type = [header[4], header[5], header[6], header[7]];
    let start = offset + CHUNK_HEADER_LEN;
    // `declared_len` is at most u32::MAX, so this cannot overflow a 64-bit usize;
    // saturate anyway so a 32-bit target reads it as "too long", never wraps.
    let end = start.saturating_add(usize::try_from(declared_len).unwrap_or(usize::MAX));
    Ok(Some(Chunk {
        chunk_type,
        declared_len,
        start,
        end,
    }))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let b = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Assemble a GLB from a JSON document and an optional BIN payload.
    pub(crate) fn build_glb(json: &str, bin: Option<&[u8]>) -> Vec<u8> {
        let mut json_bytes = json.as_bytes().to_vec();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let mut body = Vec::new();
        body.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(CHUNK_TYPE_JSON);
        body.extend_from_slice(&json_bytes);
        if let Some(bin) = bin {
            let mut bin = bin.to_vec();
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            body.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            body.extend_from_slice(CHUNK_TYPE_BIN);
            body.extend_from_slice(&bin);
        }
        let mut glb = GLB_MAGIC.to_vec();
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((GLB_HEADER_LEN + body.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&body);
        glb
    }

    /// A GLB whose first chunk header claims `json_len` bytes it does not carry.
    fn glb_with_json_len(json_len: u32) -> Vec<u8> {
        let mut glb = GLB_MAGIC.to_vec();
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&28u32.to_le_bytes());
        glb.extend_from_slice(&json_len.to_le_bytes());
        glb.extend_from_slice(CHUNK_TYPE_JSON);
        glb.extend_from_slice(b"{}      ");
        glb
    }

    fn ktx2_payload() -> Vec<u8> {
        let mut bytes = KTX2_MAGIC.to_vec();
        bytes.extend_from_slice(&[0u8; 20]);
        bytes
    }

    const PNG_IMAGE_IN_BIN: &str = r#"{"asset":{"version":"2.0"},
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":32}],
        "images":[{"bufferView":0,"mimeType":"image/png"}],
        "textures":[{"source":0}]}"#;

    #[derive(Debug, PartialEq)]
    enum Verdict {
        Pass,
        Refused,
        Malformed,
    }

    fn verdict(result: Result<(), Ktx2GuardError>) -> Verdict {
        match result {
            Ok(()) => Verdict::Pass,
            Err(Ktx2GuardError::Ktx2Texture(_)) => Verdict::Refused,
            Err(_) => Verdict::Malformed,
        }
    }

    #[test]
    fn glb_guard_table() {
        let mut header_only_short = build_glb("{}", None);
        header_only_short.truncate(8);
        let mut wrong_total = build_glb("{}", None);
        wrong_total[8] = wrong_total[8].wrapping_add(4);
        let mut cut_chunk_header = GLB_MAGIC.to_vec();
        cut_chunk_header.extend_from_slice(&2u32.to_le_bytes());
        cut_chunk_header.extend_from_slice(&16u32.to_le_bytes());
        cut_chunk_header.extend_from_slice(&[4, 0, 0, 0]);
        let ktx2_data_uri = format!(
            r#"{{"images":[{{"uri":"data:application/octet-stream;base64,{}"}}]}}"#,
            "q0tUWCAyMLsNChoK" // base64 of KTX2_MAGIC
        );

        let cases: Vec<(&str, Vec<u8>, Verdict)> = vec![
            (
                "plain GLB, PNG in BIN",
                build_glb(PNG_IMAGE_IN_BIN, Some(&[0x89; 32])),
                Verdict::Pass,
            ),
            (
                "JSON only, no images",
                build_glb(r#"{"asset":{"version":"2.0"}}"#, None),
                Verdict::Pass,
            ),
            (
                "12-byte header, no chunks",
                {
                    let mut g = build_glb("{}", None);
                    g.truncate(12);
                    g[8..12].copy_from_slice(&12u32.to_le_bytes());
                    g
                },
                Verdict::Pass,
            ),
            (
                "first chunk not JSON (unloadable)",
                {
                    let mut g = build_glb("{}", None);
                    g[16..20].copy_from_slice(b"XXXX");
                    g
                },
                Verdict::Pass,
            ),
            (
                "image/ktx2 mimeType",
                build_glb(
                    r#"{"images":[{"bufferView":0,"mimeType":"image/ktx2"}],"bufferViews":[{"buffer":0,"byteLength":4}]}"#,
                    Some(&[0; 4]),
                ),
                Verdict::Refused,
            ),
            (
                "mimeType case-insensitive",
                build_glb(
                    r#"{"images":[{"uri":"a.png","mimeType":"Image/KTX2"}]}"#,
                    None,
                ),
                Verdict::Refused,
            ),
            (
                "basisu in extensionsUsed",
                build_glb(r#"{"extensionsUsed":["KHR_texture_basisu"]}"#, None),
                Verdict::Refused,
            ),
            (
                "basisu on a texture only",
                build_glb(
                    r#"{"textures":[{"extensions":{"KHR_texture_basisu":{"source":0}}}]}"#,
                    None,
                ),
                Verdict::Refused,
            ),
            (
                "KTX2 magic behind a png mimeType",
                build_glb(PNG_IMAGE_IN_BIN, Some(&ktx2_payload())),
                Verdict::Refused,
            ),
            (
                "ktx2 data URI mime",
                build_glb(
                    r#"{"images":[{"uri":"data:image/ktx2;base64,AAAA"}]}"#,
                    None,
                ),
                Verdict::Refused,
            ),
            (
                "KTX2 magic inside a data URI",
                build_glb(&ktx2_data_uri, None),
                Verdict::Refused,
            ),
            (
                "external .ktx2 uri",
                build_glb(r#"{"images":[{"uri":"tex/a.KTX2?v=1"}]}"#, None),
                Verdict::Refused,
            ),
            (
                "percent-encoded ktx2 data URI (bevy decodes first)",
                build_glb(
                    r#"{"images":[{"uri":"data:image%2Fktx2%3Bbase64,AAAA"}]}"#,
                    None,
                ),
                Verdict::Refused,
            ),
            (
                "percent-encoded .ktx2 path",
                build_glb(r#"{"images":[{"uri":"tex/a%2Ektx2"}]}"#, None),
                Verdict::Refused,
            ),
            (
                "literal % in a png uri",
                build_glb(r#"{"images":[{"uri":"100%25 done%.png"}]}"#, None),
                Verdict::Pass,
            ),
            (
                "texture extensions null",
                build_glb(r#"{"textures":[{"extensions":null}]}"#, None),
                Verdict::Pass,
            ),
            (
                "image uri not UTF-8 once decoded (bevy unwrap panic)",
                build_glb(r#"{"images":[{"uri":"a%FF.png"}]}"#, None),
                Verdict::Malformed,
            ),
            (
                "buffer uri not UTF-8 once decoded (bevy unwrap panic)",
                build_glb(r#"{"buffers":[{"uri":"b%C3.bin","byteLength":4}]}"#, None),
                Verdict::Malformed,
            ),
            (
                "shorter than the header",
                header_only_short,
                Verdict::Malformed,
            ),
            ("total length mismatch", wrong_total, Verdict::Malformed),
            (
                "chunk header cut short",
                cut_chunk_header,
                Verdict::Malformed,
            ),
            (
                "JSON chunk overruns the asset",
                glb_with_json_len(4096),
                Verdict::Malformed,
            ),
            (
                "JSON chunk claims u32::MAX",
                glb_with_json_len(u32::MAX),
                Verdict::Malformed,
            ),
            (
                "invalid JSON",
                build_glb("{not json", None),
                Verdict::Malformed,
            ),
            (
                "image bufferView missing",
                build_glb(
                    r#"{"images":[{"bufferView":7,"mimeType":"image/png"}]}"#,
                    None,
                ),
                Verdict::Malformed,
            ),
            (
                "image bufferView outside BIN",
                build_glb(
                    r#"{"images":[{"bufferView":0,"mimeType":"image/png"}],"bufferViews":[{"buffer":0,"byteOffset":18446744073709551615,"byteLength":2}]}"#,
                    Some(&[0; 4]),
                ),
                Verdict::Malformed,
            ),
        ];
        for (label, bytes, want) in cases {
            assert_eq!(verdict(check_glb(&bytes)), want, "{label}");
        }
    }

    #[test]
    fn json_chunk_over_the_cap_is_refused_before_parsing() {
        let too_big = u32::try_from(MAX_GLTF_JSON_LEN + 4).expect("cap fits u32");
        assert_eq!(
            check_glb(&glb_with_json_len(too_big)),
            Err(Ktx2GuardError::JsonTooLarge {
                len: u64::from(too_big),
                max: MAX_GLTF_JSON_LEN
            })
        );
    }

    #[test]
    fn asset_bytes_dispatch_table() {
        let ktx2_gltf = br#"{"images":[{"uri":"x.png","mimeType":"image/ktx2"}]}"#;
        let cases: Vec<(&str, Vec<u8>, Option<&str>, Verdict)> = vec![
            (
                "GLB by magic, any extension",
                build_glb(PNG_IMAGE_IN_BIN, Some(&[1; 32])),
                Some("bin"),
                Verdict::Pass,
            ),
            (
                "ktx2 GLB by magic",
                build_glb(r#"{"extensionsRequired":["KHR_texture_basisu"]}"#, None),
                Some("glb"),
                Verdict::Refused,
            ),
            (
                "raw KTX2 image",
                ktx2_payload(),
                Some("png"),
                Verdict::Refused,
            ),
            (
                ".ktx2 path, unknown bytes",
                vec![0; 8],
                Some("ktx2"),
                Verdict::Refused,
            ),
            (
                ".gltf JSON with ktx2 image",
                ktx2_gltf.to_vec(),
                Some("GLTF"),
                Verdict::Refused,
            ),
            (
                "JSON glTF served as .glb (gltf sniffs, not the extension)",
                ktx2_gltf.to_vec(),
                Some("glb"),
                Verdict::Refused,
            ),
            (
                "JSON glTF, leading whitespace, no extension",
                [b" \r\n\t".as_slice(), ktx2_gltf].concat(),
                None,
                Verdict::Refused,
            ),
            (
                "plain JSON glTF served as .glb",
                br#"{"asset":{"version":"2.0"}}"#.to_vec(),
                Some("glb"),
                Verdict::Pass,
            ),
            (
                "PNG bytes",
                vec![0x89, b'P', b'N', b'G', 0, 0],
                Some("png"),
                Verdict::Pass,
            ),
            (
                "no extension, unknown bytes",
                vec![1, 2, 3],
                None,
                Verdict::Pass,
            ),
        ];
        for (label, bytes, extension, want) in cases {
            assert_eq!(
                verdict(check_asset_bytes(&bytes, extension)),
                want,
                "{label}"
            );
        }
    }

    #[test]
    fn percent_decoding_matches_percent_encoding_crate() {
        let cases = [
            ("image%2Fktx2", Some("image/ktx2")),
            ("%41%7a", Some("Az")),
            ("50%", Some("50%")),
            ("%G1%4", Some("%G1%4")),
            ("caf%C3%A9", Some("café")),
            ("%FF", None),
        ];
        for (input, want) in cases {
            assert_eq!(percent_decode_utf8(input).as_deref(), want, "{input}");
        }
    }

    #[test]
    fn base64_prefix_decodes_the_ktx2_identifier() {
        assert_eq!(decode_base64_prefix("q0tUWCAyMLsNChoK"), Some(KTX2_MAGIC));
        assert_eq!(decode_base64_prefix("q0tUWCAyMLsNCho"), None);
        assert_eq!(decode_base64_prefix("q0tUWCAyMLsNCh=K"), None);
    }
}
