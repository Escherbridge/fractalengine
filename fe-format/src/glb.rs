//! Cheap structural check of an untrusted binary glTF (GLB) file; see `AGENTS.md` §glb.

use std::fmt;

const HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;
const MAGIC: [u8; 4] = *b"glTF";
const CHUNK_JSON: [u8; 4] = *b"JSON";
const CHUNK_BIN: [u8; 4] = *b"BIN\0";

/// Facts read from a structurally valid GLB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlbFacts {
    /// Total file length in bytes.
    pub total_len: usize,
    /// Length of the JSON chunk payload.
    pub json_len: usize,
    /// Length of the BIN chunk payload (0 when absent).
    pub bin_len: usize,
    /// Length of the JSON `meshes` array (0 when absent).
    pub meshes: usize,
    /// Length of the JSON `materials` array (0 when absent).
    pub materials: usize,
    /// Length of the JSON `images` array (0 when absent).
    pub images: usize,
    /// Length of the JSON `nodes` array (0 when absent).
    pub nodes: usize,
    /// True when an image or buffer has a `uri` that is not a `data:` URI.
    pub external_uri: bool,
}

/// One refusal reason per variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlbError {
    TooLarge,
    TooShort,
    BadMagic,
    BadVersion,
    LengthMismatch,
    MissingJsonChunk,
    FirstChunkNotJson,
    ChunkMisaligned,
    JsonChunkOverrun,
    JsonInvalid,
    JsonNotObject,
    SecondChunkNotBin,
    BinChunkOverrun,
    TrailingBytes,
}

impl fmt::Display for GlbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            GlbError::TooLarge => "glb file exceeds the size limit",
            GlbError::TooShort => "glb file is shorter than its header",
            GlbError::BadMagic => "glb header magic is not glTF",
            GlbError::BadVersion => "glb container version is not 2",
            GlbError::LengthMismatch => "glb declared length differs from the file length",
            GlbError::MissingJsonChunk => "glb has no complete JSON chunk header",
            GlbError::FirstChunkNotJson => "glb first chunk is not a JSON chunk",
            GlbError::ChunkMisaligned => "glb chunk length is not a multiple of 4",
            GlbError::JsonChunkOverrun => "glb JSON chunk runs past the end of the file",
            GlbError::JsonInvalid => "glb JSON chunk is not valid JSON",
            GlbError::JsonNotObject => "glb JSON chunk is not a JSON object",
            GlbError::SecondChunkNotBin => "glb second chunk is not a BIN chunk",
            GlbError::BinChunkOverrun => "glb BIN chunk runs past the end of the file",
            GlbError::TrailingBytes => "glb has data after its last chunk",
        };
        f.write_str(text)
    }
}

impl std::error::Error for GlbError {}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Reads a chunk header at `at`; returns (payload length, type, payload start).
fn chunk_header(bytes: &[u8], at: usize) -> Option<(usize, [u8; 4], usize)> {
    let length = read_u32(bytes, at)? as usize;
    let kind = bytes.get(at + 4..at + CHUNK_HEADER_LEN)?;
    Some((length, [kind[0], kind[1], kind[2], kind[3]], at + CHUNK_HEADER_LEN))
}

fn array_len(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> usize {
    object
        .get(key)
        .and_then(|value| value.as_array())
        .map_or(0, |array| array.len())
}

fn has_external_uri(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    object
        .get(key)
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("uri")
                    .is_some_and(|uri| !uri.as_str().is_some_and(|text| text.starts_with("data:")))
            })
        })
}

/// Structural check of an untrusted GLB: container rules plus a JSON-object parse of the JSON chunk.
pub fn check(bytes: &[u8], max_len: usize) -> Result<GlbFacts, GlbError> {
    let total_len = bytes.len();
    if total_len > max_len {
        return Err(GlbError::TooLarge);
    }
    if total_len < HEADER_LEN {
        return Err(GlbError::TooShort);
    }
    if bytes[0..4] != MAGIC {
        return Err(GlbError::BadMagic);
    }
    if read_u32(bytes, 4) != Some(2) {
        return Err(GlbError::BadVersion);
    }
    if read_u32(bytes, 8).map(|declared| declared as usize) != Some(total_len) {
        return Err(GlbError::LengthMismatch);
    }

    let (json_len, json_kind, json_start) =
        chunk_header(bytes, HEADER_LEN).ok_or(GlbError::MissingJsonChunk)?;
    if json_kind != CHUNK_JSON {
        return Err(GlbError::FirstChunkNotJson);
    }
    if json_len % 4 != 0 {
        return Err(GlbError::ChunkMisaligned);
    }
    let json_end = json_start
        .checked_add(json_len)
        .filter(|end| *end <= total_len)
        .ok_or(GlbError::JsonChunkOverrun)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes[json_start..json_end])
        .map_err(|_| GlbError::JsonInvalid)?;
    let object = value.as_object().ok_or(GlbError::JsonNotObject)?;

    let mut bin_len = 0;
    let mut cursor = json_end;
    if cursor < total_len {
        let (length, kind, start) = chunk_header(bytes, cursor).ok_or(GlbError::TrailingBytes)?;
        if kind != CHUNK_BIN {
            return Err(GlbError::SecondChunkNotBin);
        }
        if length % 4 != 0 {
            return Err(GlbError::ChunkMisaligned);
        }
        cursor = start
            .checked_add(length)
            .filter(|end| *end <= total_len)
            .ok_or(GlbError::BinChunkOverrun)?;
        bin_len = length;
    }
    if cursor != total_len {
        return Err(GlbError::TrailingBytes);
    }

    Ok(GlbFacts {
        total_len,
        json_len,
        bin_len,
        meshes: array_len(object, "meshes"),
        materials: array_len(object, "materials"),
        images: array_len(object, "images"),
        nodes: array_len(object, "nodes"),
        external_uri: has_external_uri(object, "images") || has_external_uri(object, "buffers"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_JSON: &[u8] = br#"{"asset":{"version":"2.0"}}"#;

    /// Builds a GLB with a space-padded JSON chunk and an optional zero-padded BIN chunk.
    fn glb(json: &[u8], bin: Option<&[u8]>) -> Vec<u8> {
        let mut json_chunk = json.to_vec();
        while json_chunk.len() % 4 != 0 {
            json_chunk.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_chunk);
        if let Some(bin) = bin {
            let mut bin_chunk = bin.to_vec();
            while bin_chunk.len() % 4 != 0 {
                bin_chunk.push(0);
            }
            out.extend_from_slice(&(bin_chunk.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&bin_chunk);
        }
        let total = out.len() as u32;
        out[8..12].copy_from_slice(&total.to_le_bytes());
        out
    }

    fn with_u32(mut bytes: Vec<u8>, at: usize, value: u32) -> Vec<u8> {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    fn with_bytes(mut bytes: Vec<u8>, at: usize, value: &[u8]) -> Vec<u8> {
        bytes[at..at + value.len()].copy_from_slice(value);
        bytes
    }

    fn facts(total_len: usize, json_len: usize, bin_len: usize) -> GlbFacts {
        GlbFacts {
            total_len,
            json_len,
            bin_len,
            meshes: 0,
            materials: 0,
            images: 0,
            nodes: 0,
            external_uri: false,
        }
    }

    #[test]
    fn check_table() {
        let minimal = glb(MINIMAL_JSON, None);
        let with_bin = glb(MINIMAL_JSON, Some(&[1, 2, 3, 4, 5, 6, 7, 8]));
        let json_chunk_len = minimal.len() - 20;
        let bin_header_at = 20 + json_chunk_len;
        let external = glb(
            br#"{"images":[{"uri":"a.png"},{"uri":"data:image/png;base64,AAAA"}],"buffers":[{"byteLength":4}],"nodes":[{}],"meshes":[{}],"materials":[{}]}"#,
            None,
        );
        let external_expected = GlbFacts {
            total_len: external.len(),
            json_len: external.len() - 20,
            bin_len: 0,
            meshes: 1,
            materials: 1,
            images: 2,
            nodes: 1,
            external_uri: true,
        };

        let cases: Vec<(&str, Vec<u8>, usize, Result<GlbFacts, GlbError>)> = vec![
            ("minimal", minimal.clone(), usize::MAX, Ok(facts(minimal.len(), json_chunk_len, 0))),
            ("with bin", with_bin.clone(), usize::MAX, Ok(facts(with_bin.len(), json_chunk_len, 8))),
            ("bad magic", with_bytes(minimal.clone(), 0, b"glTX"), usize::MAX, Err(GlbError::BadMagic)),
            ("version 1", with_u32(minimal.clone(), 4, 1), usize::MAX, Err(GlbError::BadVersion)),
            ("too short", minimal[..8].to_vec(), usize::MAX, Err(GlbError::TooShort)),
            (
                "declared longer than file",
                with_u32(minimal.clone(), 8, minimal.len() as u32 + 4),
                usize::MAX,
                Err(GlbError::LengthMismatch),
            ),
            (
                "declared shorter than file",
                with_u32(minimal.clone(), 8, minimal.len() as u32 - 4),
                usize::MAX,
                Err(GlbError::LengthMismatch),
            ),
            (
                "json overrun",
                with_u32(minimal.clone(), 12, 4000),
                usize::MAX,
                Err(GlbError::JsonChunkOverrun),
            ),
            (
                "first chunk not json",
                with_bytes(minimal.clone(), 16, b"BIN\0"),
                usize::MAX,
                Err(GlbError::FirstChunkNotJson),
            ),
            ("json not object", glb(b"[1,2,3]", None), usize::MAX, Err(GlbError::JsonNotObject)),
            (
                "bin overrun",
                with_u32(with_bin.clone(), bin_header_at, 400),
                usize::MAX,
                Err(GlbError::BinChunkOverrun),
            ),
            ("over max_len", minimal.clone(), minimal.len() - 1, Err(GlbError::TooLarge)),
            ("external uri", external, usize::MAX, Ok(external_expected)),
        ];

        for (name, bytes, max_len, expected) in cases {
            assert_eq!(check(&bytes, max_len), expected, "case: {name}");
        }
    }
}
