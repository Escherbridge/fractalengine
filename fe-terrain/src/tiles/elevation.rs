use anyhow::{Context, Result};

/// Trait for decoding elevation data from tile image pixels.
pub trait ElevationDecoder: Send + Sync {
    /// Decode elevation values from raw RGBA/RGB pixels.
    ///
    /// Input: raw pixel bytes (RGB or RGBA), width, height.
    /// Output: elevation values in meters, row-major order (width * height).
    fn decode(&self, pixels: &[u8], width: u32, height: u32) -> Vec<f32>;
}

/// Mapbox Terrain-RGB decoder.
///
/// Formula: `h = -10000 + (R * 65536 + G * 256 + B) * 0.1`
///
/// Expects RGB (3 bytes per pixel) or RGBA (4 bytes per pixel).
#[derive(Debug, Clone, Default)]
pub struct TerrainRgbDecoder;

impl ElevationDecoder for TerrainRgbDecoder {
    fn decode(&self, pixels: &[u8], width: u32, height: u32) -> Vec<f32> {
        let count = (width * height) as usize;
        let bpp = pixels.len() / count;
        let mut elevations = Vec::with_capacity(count);

        for i in 0..count {
            let offset = i * bpp;
            if offset + 2 >= pixels.len() {
                elevations.push(0.0);
                continue;
            }
            let r = pixels[offset] as f64;
            let g = pixels[offset + 1] as f64;
            let b = pixels[offset + 2] as f64;
            let h = -10000.0 + (r * 65536.0 + g * 256.0 + b) * 0.1;
            elevations.push(h as f32);
        }

        elevations
    }
}

/// Mapzen Terrarium decoder.
///
/// Formula: `h = R * 256 + G + B / 256 - 32768`
///
/// Expects RGB (3 bytes per pixel) or RGBA (4 bytes per pixel).
#[derive(Debug, Clone, Default)]
pub struct TerrariumDecoder;

impl ElevationDecoder for TerrariumDecoder {
    fn decode(&self, pixels: &[u8], width: u32, height: u32) -> Vec<f32> {
        let count = (width * height) as usize;
        let bpp = pixels.len() / count;
        let mut elevations = Vec::with_capacity(count);

        for i in 0..count {
            let offset = i * bpp;
            if offset + 2 >= pixels.len() {
                elevations.push(0.0);
                continue;
            }
            let r = pixels[offset] as f64;
            let g = pixels[offset + 1] as f64;
            let b = pixels[offset + 2] as f64;
            let h = r * 256.0 + g + b / 256.0 - 32768.0;
            elevations.push(h as f32);
        }

        elevations
    }
}

/// Decode a PNG image into raw RGB pixel data and dimensions.
pub fn decode_png_pixels(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(png_bytes).context("failed to decode PNG")?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    Ok((rgb.into_raw(), w, h))
}
