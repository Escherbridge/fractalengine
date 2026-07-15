//! Baked splat buffer serde shape + hexon-embedded storage (FR-2); see
//! `src/splat/AGENTS.md` §bake. Pure/bevy-free — mirrors [`crate::splat::synth::SplatBuffer`]
//! field-for-field but is a standalone serde type so it round-trips through
//! bincode without depending on the runtime buffer's non-serde internals.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};

use crate::splat::synth::SplatBuffer;

/// Dense baked splat buffer for one tile, written at hexon/tileset build time
/// and read back verbatim at runtime (FR-4) — no live fill cost. Optional per
/// tile: hexons/tiles built before this track, or tiles the bake step skipped,
/// simply have no entry and the runtime falls back to live `synthesize_splats`
/// (FR-5).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BakedSplatBuffer {
    /// Format version; bump on any incompatible field/encoding change so old
    /// readers can detect and skip (rather than misparse) a newer buffer.
    pub version: u16,
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub scales: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
}

/// Current on-disk format version written by this track's bake step.
pub const BAKED_SPLAT_FORMAT_VERSION: u16 = 1;

/// Web-Mercator tile edge length in real meters at `lat`/`zoom`. Pure
/// duplicate of the render-gated `terrain_plugin::tile_world_size_m` (bevy
/// dependency makes that one unreachable from the `fetch`-only bake path) —
/// keep the two formulas identical if either changes.
pub fn tile_world_size_m(lat: f64, zoom: u8) -> f64 {
    40_075_016.686 * lat.to_radians().cos() / 2f64.powi(zoom as i32)
}

impl BakedSplatBuffer {
    /// Number of splats in the buffer.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// True when the buffer carries no splats.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

impl From<&SplatBuffer> for BakedSplatBuffer {
    fn from(buf: &SplatBuffer) -> Self {
        Self {
            version: BAKED_SPLAT_FORMAT_VERSION,
            positions: buf.positions.clone(),
            colors: buf.colors.clone(),
            scales: buf.scales.clone(),
            normals: buf.normals.clone(),
        }
    }
}

impl From<BakedSplatBuffer> for SplatBuffer {
    fn from(buf: BakedSplatBuffer) -> Self {
        SplatBuffer {
            positions: buf.positions,
            colors: buf.colors,
            scales: buf.scales,
            normals: buf.normals,
        }
    }
}

impl From<&BakedSplatBuffer> for SplatBuffer {
    fn from(buf: &BakedSplatBuffer) -> Self {
        SplatBuffer {
            positions: buf.positions.clone(),
            colors: buf.colors.clone(),
            scales: buf.scales.clone(),
            normals: buf.normals.clone(),
        }
    }
}

/// Encode a baked splat buffer to bytes (bincode) for embedding in a hexon
/// archive's `terrain/splats/{cache_key}.bin` entry.
pub fn encode_baked_splats(buf: &BakedSplatBuffer) -> Result<Vec<u8>> {
    bincode::serialize(buf).context("failed to encode baked splat buffer")
}

/// Decode a baked splat buffer previously written by [`encode_baked_splats`].
/// A version mismatch is NOT a hard error here — callers should treat any
/// decode failure as "no baked data" and fall back to live synthesis (FR-5).
pub fn decode_baked_splats(bytes: &[u8]) -> Result<BakedSplatBuffer> {
    bincode::deserialize(bytes).context("failed to decode baked splat buffer")
}

// ---------------------------------------------------------------------------
// Hexon archive embedding — appends `terrain/splats/{cache_key}.bin` entries
// onto an already-built `.hexon` ZIP without touching `fe-format` (which is
// out of scope for this track; the tileset ZIP layout is otherwise owned
// there). Written by `TilesetBuilder` (FR-3), read by `HexonTileSource` (FR-4).
// ---------------------------------------------------------------------------

const SPLATS_ZIP_PREFIX: &str = "terrain/splats/";
const SPLATS_ZIP_SUFFIX: &str = ".bin";

/// Re-open `archive_bytes` (a `.hexon` ZIP produced by `fe_format::HexonArchive`)
/// and append one `terrain/splats/{cache_key}.bin` entry per baked tile.
/// Returns the new archive bytes. `entries` is `(cache_key, encoded_bytes)` —
/// callers should encode with [`encode_baked_splats`] first.
pub fn append_baked_splats_to_archive(
    archive_bytes: &[u8],
    entries: &[(String, Vec<u8>)],
) -> Result<Vec<u8>> {
    if entries.is_empty() {
        return Ok(archive_bytes.to_vec());
    }
    let reader = Cursor::new(archive_bytes);
    let mut src =
        zip::ZipArchive::new(reader).context("failed to open hexon archive for splat append")?;

    let out_buf = Vec::new();
    let mut writer = zip::ZipWriter::new(Cursor::new(out_buf));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Copy every existing entry through unchanged.
    for i in 0..src.len() {
        let mut file = src.by_index(i)?;
        let name = file.name().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        writer.start_file(name, stored)?;
        writer.write_all(&data)?;
    }

    // Append the baked splat buffers.
    for (cache_key, data) in entries {
        writer.start_file(
            format!("{SPLATS_ZIP_PREFIX}{cache_key}{SPLATS_ZIP_SUFFIX}"),
            stored,
        )?;
        writer.write_all(data)?;
    }

    let cursor = writer
        .finish()
        .context("failed to finalize splat-augmented hexon archive")?;
    Ok(cursor.into_inner())
}

/// Read every `terrain/splats/{cache_key}.bin` entry out of a `.hexon` archive,
/// returning `(cache_key, decoded_buffer)` pairs. Absent entries or a whole
/// archive with no splats section both yield an empty vec — never an error —
/// so callers can treat "no baked splats" as the normal FR-5 fallback case.
pub fn read_baked_splats_from_archive(
    archive_bytes: &[u8],
) -> Result<Vec<(String, BakedSplatBuffer)>> {
    let reader = Cursor::new(archive_bytes);
    let mut archive = match zip::ZipArchive::new(reader) {
        Ok(a) => a,
        Err(_) => return Ok(Vec::new()),
    };

    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();

    let mut out = Vec::new();
    for name in names.iter().filter(|n| {
        n.starts_with(SPLATS_ZIP_PREFIX)
            && n.ends_with(SPLATS_ZIP_SUFFIX)
            && n.len() > SPLATS_ZIP_PREFIX.len() + SPLATS_ZIP_SUFFIX.len()
    }) {
        let mut file = archive.by_name(name)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        let cache_key = name
            .strip_prefix(SPLATS_ZIP_PREFIX)
            .unwrap_or(name)
            .strip_suffix(SPLATS_ZIP_SUFFIX)
            .unwrap_or(name)
            .to_string();
        // A corrupt/future-version single entry should not sink the whole
        // archive load — skip it and let that tile fall back to live synth.
        if let Ok(buf) = decode_baked_splats(&data) {
            out.push((cache_key, buf));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_buf(n: usize) -> BakedSplatBuffer {
        BakedSplatBuffer {
            version: BAKED_SPLAT_FORMAT_VERSION,
            positions: (0..n).map(|i| [i as f32, 0.0, i as f32 * 2.0]).collect(),
            colors: vec![[0.5, 0.5, 0.5, 1.0]; n],
            scales: vec![[1.0, 1.0]; n],
            normals: vec![[0.0, 1.0, 0.0]; n],
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let buf = sample_buf(12);
        let bytes = encode_baked_splats(&buf).unwrap();
        let decoded = decode_baked_splats(&bytes).unwrap();
        assert_eq!(decoded.positions, buf.positions);
        assert_eq!(decoded.colors, buf.colors);
        assert_eq!(decoded.scales, buf.scales);
        assert_eq!(decoded.normals, buf.normals);
        assert_eq!(decoded.version, buf.version);
    }

    #[test]
    fn splat_buffer_conversion_roundtrip() {
        let mut sb = SplatBuffer::default();
        sb.positions.push([1.0, 2.0, 3.0]);
        sb.colors.push([1.0, 0.0, 0.0, 1.0]);
        sb.scales.push([0.5, 0.4]);
        sb.normals.push([0.0, 1.0, 0.0]);

        let baked: BakedSplatBuffer = (&sb).into();
        assert_eq!(baked.len(), 1);
        let back: SplatBuffer = baked.into();
        assert_eq!(back.positions, sb.positions);
        assert_eq!(back.colors, sb.colors);
    }

    #[test]
    fn decode_garbage_is_an_error_not_a_panic() {
        let garbage = vec![0xFFu8; 8];
        assert!(decode_baked_splats(&garbage).is_err());
    }

    fn build_minimal_hexon_zip() -> Vec<u8> {
        // A minimal ZIP with a single manifest.json entry, standing in for a
        // real fe_format::HexonArchive::export_tileset output for this test.
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", opts).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn append_and_read_baked_splats_roundtrip() {
        let base = build_minimal_hexon_zip();
        let buf_a = sample_buf(4);
        let buf_b = sample_buf(2);
        let entries = vec![
            (
                "10/512/340".to_string(),
                encode_baked_splats(&buf_a).unwrap(),
            ),
            (
                "10/513/340".to_string(),
                encode_baked_splats(&buf_b).unwrap(),
            ),
        ];
        let augmented = append_baked_splats_to_archive(&base, &entries).unwrap();

        // Original entry still present.
        let mut archive = zip::ZipArchive::new(Cursor::new(&augmented)).unwrap();
        assert!(archive.by_name("manifest.json").is_ok());

        let read_back = read_baked_splats_from_archive(&augmented).unwrap();
        assert_eq!(read_back.len(), 2);
        let map: std::collections::HashMap<_, _> = read_back.into_iter().collect();
        assert_eq!(map["10/512/340"].len(), 4);
        assert_eq!(map["10/513/340"].len(), 2);
    }

    #[test]
    fn empty_entries_leaves_archive_unchanged() {
        let base = build_minimal_hexon_zip();
        let augmented = append_baked_splats_to_archive(&base, &[]).unwrap();
        assert_eq!(augmented, base);
    }

    #[test]
    fn archive_without_splats_section_reads_as_empty() {
        let base = build_minimal_hexon_zip();
        let read_back = read_baked_splats_from_archive(&base).unwrap();
        assert!(read_back.is_empty());
    }

    #[test]
    fn non_zip_bytes_read_as_empty_not_error() {
        // FR-5: a malformed/absent splats section must never fail the whole
        // hexon load — always degrade to "no baked splats found".
        let read_back = read_baked_splats_from_archive(b"not a zip file").unwrap();
        assert!(read_back.is_empty());
    }
}
