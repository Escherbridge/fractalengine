//! Pure bilinear upsampling of elevation grids for close-range mesh density; see `src/AGENTS.md` §terrain_plugin.

/// Bilinearly upsample a row-major `w×h` elevation grid by an integer `factor` (≥1).
///
/// Output dims are `((w-1)*factor+1) × ((h-1)*factor+1)` so the original samples
/// land exactly on output grid nodes (endpoints preserved, no drift). Returns the
/// grid unchanged when `factor <= 1` or the grid is too small to interpolate.
pub fn upsample_bilinear(src: &[f32], w: usize, h: usize, factor: u32) -> (Vec<f32>, usize, usize) {
    let factor = factor.max(1) as usize;
    if factor == 1 || w < 2 || h < 2 || src.len() < w * h {
        return (src.to_vec(), w, h);
    }
    let new_w = (w - 1) * factor + 1;
    let new_h = (h - 1) * factor + 1;
    let mut out = vec![0.0f32; new_w * new_h];
    let inv = 1.0f32 / factor as f32;

    for j in 0..new_h {
        let sy = j as f32 * inv; // source-space row coordinate
        let y0 = (sy.floor() as usize).min(h - 1);
        let y1 = (y0 + 1).min(h - 1);
        let ty = sy - y0 as f32;
        for i in 0..new_w {
            let sx = i as f32 * inv; // source-space column coordinate
            let x0 = (sx.floor() as usize).min(w - 1);
            let x1 = (x0 + 1).min(w - 1);
            let tx = sx - x0 as f32;

            let v00 = src[y0 * w + x0];
            let v10 = src[y0 * w + x1];
            let v01 = src[y1 * w + x0];
            let v11 = src[y1 * w + x1];
            let top = v00 + (v10 - v00) * tx;
            let bot = v01 + (v11 - v01) * tx;
            out[j * new_w + i] = top + (bot - top) * ty;
        }
    }
    (out, new_w, new_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_one_is_identity() {
        let src = vec![1.0, 2.0, 3.0, 4.0];
        let (out, w, h) = upsample_bilinear(&src, 2, 2, 1);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, src);
    }

    #[test]
    fn factor_two_dims_and_midpoint_average() {
        // 2x2 corners; midpoint of a bilinear patch is the mean of the four corners.
        let src = vec![0.0, 10.0, 20.0, 30.0];
        let (out, w, h) = upsample_bilinear(&src, 2, 2, 2);
        assert_eq!((w, h), (3, 3));
        // Corners preserved exactly.
        assert_eq!(out[0], 0.0);
        assert_eq!(out[2], 10.0);
        assert_eq!(out[6], 20.0);
        assert_eq!(out[8], 30.0);
        // Center = mean of corners.
        assert!((out[4] - 15.0).abs() < 1e-5);
        // Edge midpoints = mean of the two edge samples.
        assert!((out[1] - 5.0).abs() < 1e-5); // between 0 and 10
        assert!((out[3] - 10.0).abs() < 1e-5); // between 0 and 20
    }

    #[test]
    fn linear_ramp_is_reproduced_exactly() {
        // A horizontal ramp upsampled stays linear.
        let src = vec![0.0, 4.0, 0.0, 4.0]; // 2x2, columns 0 -> 4
        let (out, w, _h) = upsample_bilinear(&src, 2, 2, 4);
        assert_eq!(w, 5);
        // Row 0 should be 0,1,2,3,4.
        for (i, expected) in [0.0, 1.0, 2.0, 3.0, 4.0].iter().enumerate() {
            assert!((out[i] - expected).abs() < 1e-5, "col {i} = {}", out[i]);
        }
    }

    #[test]
    fn too_small_grid_returns_unchanged() {
        let src = vec![7.0];
        let (out, w, h) = upsample_bilinear(&src, 1, 1, 4);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, src);
    }
}
