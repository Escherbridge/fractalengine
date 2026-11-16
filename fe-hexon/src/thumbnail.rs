//! Untrusted PNG thumbnail decoder — see `conductor/tracks/model_library_20261115/spec.md` §6.

use std::io::Cursor;

/// Largest accepted thumbnail file.
pub const MAX_THUMBNAIL_BYTES: usize = 256 * 1024;
/// Largest accepted width or height in pixels.
pub const MAX_THUMBNAIL_SIDE: u32 = 512;
/// Decoder allocation ceiling; a 512 x 512 RGBA8 frame is exactly 1 MiB, the rest is decoder working room.
const DECODER_MAX_ALLOC_BYTES: u64 = 4 * 1024 * 1024;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// Signature, chunk length, `IHDR` tag, then width and height.
const IHDR_END: usize = 8 + 4 + 4 + 8;

/// Decoded thumbnail: tightly packed RGBA8 rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// One refusal reason per variant; never carries publisher bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ThumbnailError {
    #[error("thumbnail is larger than {MAX_THUMBNAIL_BYTES} bytes")]
    TooLarge,
    #[error("thumbnail is not a PNG")]
    NotPng,
    #[error("thumbnail header is missing or malformed")]
    MalformedHeader,
    #[error("thumbnail has a zero dimension")]
    ZeroDimension,
    #[error("thumbnail is wider or taller than {MAX_THUMBNAIL_SIDE} pixels")]
    DimensionsTooLarge,
    #[error("thumbnail could not be decoded")]
    Decode,
}

/// Decode an untrusted PNG thumbnail into RGBA8; every cap is checked before the decoder allocates.
pub fn decode(bytes: &[u8]) -> Result<Rgba, ThumbnailError> {
    if bytes.len() > MAX_THUMBNAIL_BYTES {
        return Err(ThumbnailError::TooLarge);
    }
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err(ThumbnailError::NotPng);
    }
    let (width, height) = read_ihdr_dimensions(bytes)?;
    if width == 0 || height == 0 {
        return Err(ThumbnailError::ZeroDimension);
    }
    if width > MAX_THUMBNAIL_SIDE || height > MAX_THUMBNAIL_SIDE {
        return Err(ThumbnailError::DimensionsTooLarge);
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_THUMBNAIL_SIDE);
    limits.max_image_height = Some(MAX_THUMBNAIL_SIDE);
    limits.max_alloc = Some(DECODER_MAX_ALLOC_BYTES);
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| ThumbnailError::Decode)?.to_rgba8();
    Ok(Rgba { width: decoded.width(), height: decoded.height(), pixels: decoded.into_raw() })
}

/// Width and height from the first chunk, which must be a 13-byte `IHDR`; read by hand, before any decoder runs.
fn read_ihdr_dimensions(bytes: &[u8]) -> Result<(u32, u32), ThumbnailError> {
    let header = bytes.get(8..IHDR_END).ok_or(ThumbnailError::MalformedHeader)?;
    let chunk_length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if chunk_length != 13 || &header[4..8] != b"IHDR" {
        return Err(ThumbnailError::MalformedHeader);
    }
    let width = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let height = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(format: image::ImageFormat, image: image::DynamicImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }

    fn valid_png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        encode(image::ImageFormat::Png, image::DynamicImage::ImageRgba8(image))
    }

    /// Signature plus an `IHDR` claiming `width` x `height`; nothing after it.
    fn header_only(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn decode_table() {
        let good = valid_png(3, 2);
        let mut wrong_signature = good.clone();
        wrong_signature[1] = b'X';
        let mut oversize = good.clone();
        oversize.resize(MAX_THUMBNAIL_BYTES + 1, 0);
        let truncated = good[..good.len() / 2].to_vec();
        let jpeg = encode(
            image::ImageFormat::Jpeg,
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))),
        );

        let cases: Vec<(&str, Vec<u8>, Result<(u32, u32, usize), ThumbnailError>)> = vec![
            ("valid", good, Ok((3, 2, 3 * 2 * 4))),
            ("wrong signature", wrong_signature, Err(ThumbnailError::NotPng)),
            ("oversize bytes", oversize, Err(ThumbnailError::TooLarge)),
            ("too wide", header_only(513, 8), Err(ThumbnailError::DimensionsTooLarge)),
            ("too tall", header_only(8, 513), Err(ThumbnailError::DimensionsTooLarge)),
            ("zero width", header_only(0, 8), Err(ThumbnailError::ZeroDimension)),
            ("zero height", header_only(8, 0), Err(ThumbnailError::ZeroDimension)),
            ("truncated", truncated, Err(ThumbnailError::Decode)),
            ("jpeg", jpeg, Err(ThumbnailError::NotPng)),
        ];
        for (name, bytes, expected) in cases {
            let got = decode(&bytes).map(|image| (image.width, image.height, image.pixels.len()));
            assert_eq!(got, expected, "case {name}");
        }
    }
}
