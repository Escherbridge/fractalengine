//! Existing-vs-proposed section profile sampler (Bevy-free); see `fe-terrain/src/AGENTS.md` §profile.

/// Hard cap on samples along one section line.
pub const PROFILE_MAX_SAMPLES: usize = 512;

/// Target spacing between samples, in world units.
pub const PROFILE_TARGET_STEP: f32 = 0.5;

/// One station along the section line (distance and heights in world units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfileSample {
    pub distance: f32,
    pub base_y: f32,
    pub proposed_y: f32,
}

/// Sampled section with cut/fill areas (meters when `scaled`, else world units).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionProfile {
    pub samples: Vec<ProfileSample>,
    /// Section length (meters when `scaled`, else world units).
    pub length: f64,
    /// Spacing between consecutive stations in world units.
    pub step: f32,
    pub cut_area: f64,
    pub fill_area: f64,
    pub scaled: bool,
}

/// Samples both surfaces along `a -> b` and integrates cut and fill areas.
pub fn sample_profile(
    base: &impl Fn(f32, f32) -> Option<f32>,
    proposed: &impl Fn(f32, f32) -> Option<f32>,
    a: [f32; 2],
    b: [f32; 2],
    max_samples: usize,
    world_scale: f64,
) -> Option<SectionProfile> {
    let dx = b[0] - a[0];
    let dz = b[1] - a[1];
    let world_length = (dx * dx + dz * dz).sqrt();
    if !world_length.is_finite() || world_length <= 0.0 {
        return None;
    }
    // Same global range every scale reader applies; out-of-range never claims meters (NFR-4).
    let scaled = fe_format::scale::is_world_scale_in_range(world_scale);
    let scale = if scaled { world_scale } else { 1.0 };

    let cap = max_samples.clamp(2, PROFILE_MAX_SAMPLES);
    let count = ((world_length / PROFILE_TARGET_STEP).ceil() as usize).clamp(2, cap);
    let step = world_length / (count - 1) as f32;

    let mut samples: Vec<ProfileSample> = Vec::with_capacity(count);
    for index in 0..count {
        let fraction = index as f32 / (count - 1) as f32;
        let (x, z) = (a[0] + dx * fraction, a[1] + dz * fraction);
        // A hole in either surface skips the station; no height is invented.
        if let (Some(base_y), Some(proposed_y)) = (base(x, z), proposed(x, z)) {
            if base_y.is_finite() && proposed_y.is_finite() {
                samples.push(ProfileSample {
                    distance: world_length * fraction,
                    base_y,
                    proposed_y,
                });
            }
        }
    }
    if samples.len() < 2 {
        return None;
    }

    let (mut cut_world, mut fill_world) = (0.0f64, 0.0f64);
    for pair in samples.windows(2) {
        let width = (pair[1].distance - pair[0].distance) as f64;
        let start_delta = (pair[0].proposed_y - pair[0].base_y) as f64;
        let end_delta = (pair[1].proposed_y - pair[1].base_y) as f64;
        accumulate_segment(width, start_delta, end_delta, &mut cut_world, &mut fill_world);
    }

    Some(SectionProfile {
        samples,
        length: world_length as f64 / scale,
        step,
        cut_area: cut_world / (scale * scale),
        fill_area: fill_world / (scale * scale),
        scaled,
    })
}

/// Trapezoid of |dh| over one segment, split at a sign change so cut and fill never cancel.
fn accumulate_segment(width: f64, start_delta: f64, end_delta: f64, cut: &mut f64, fill: &mut f64) {
    if start_delta * end_delta < 0.0 {
        let crossing = start_delta.abs() / (start_delta.abs() + end_delta.abs());
        let first = 0.5 * width * crossing * start_delta.abs();
        let second = 0.5 * width * (1.0 - crossing) * end_delta.abs();
        if start_delta > 0.0 {
            *fill += first;
            *cut += second;
        } else {
            *cut += first;
            *fill += second;
        }
    } else {
        let area = 0.5 * width * (start_delta.abs() + end_delta.abs());
        // Both ends share a sign (or touch zero): the sum of signs picks the bucket.
        if start_delta + end_delta >= 0.0 {
            *fill += area;
        } else {
            *cut += area;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE_A: [f32; 2] = [0.0, 0.0];
    const LINE_B: [f32; 2] = [30.0, 0.0];

    fn flat(_x: f32, _z: f32) -> Option<f32> {
        Some(0.0)
    }

    /// Plateau of `height` over [10, 20] with 1-unit ramps centred on 10 and 20 (area-exact on the 0.5 lattice).
    fn plateau(height: f32) -> impl Fn(f32, f32) -> Option<f32> {
        move |x, _| Some(height * ((x - 9.5).clamp(0.0, 1.0) - (x - 19.5).clamp(0.0, 1.0)))
    }

    #[test]
    fn section_profile_cut_fill_table() {
        type Surface = Box<dyn Fn(f32, f32) -> Option<f32>>;
        // (name, proposed surface, base surface, expected cut, expected fill)
        let cases: Vec<(&str, Surface, Surface, f64, f64)> = vec![
            ("fill plateau", Box::new(plateau(2.0)), Box::new(flat), 0.0, 20.0),
            ("cut plateau", Box::new(plateau(-2.0)), Box::new(flat), 20.0, 0.0),
            (
                "crossing mid-segment",
                Box::new(|x, _| Some(x - 15.25)),
                Box::new(flat),
                0.5 * 15.25 * 15.25,
                0.5 * 14.75 * 14.75,
            ),
            (
                "hole in base skipped",
                Box::new(|_, _| Some(2.0)),
                Box::new(|x, _| if (14.9..15.1).contains(&x) { None } else { Some(0.0) }),
                0.0,
                60.0,
            ),
        ];
        for (name, proposed_surface, base_surface, expected_cut, expected_fill) in cases {
            let profile = sample_profile(&base_surface, &proposed_surface, LINE_A, LINE_B, 512, 1.0)
                .unwrap_or_else(|| panic!("{name}: profile expected"));
            assert!((profile.cut_area - expected_cut).abs() < 1e-2, "{name}: cut {}", profile.cut_area);
            assert!((profile.fill_area - expected_fill).abs() < 1e-2, "{name}: fill {}", profile.fill_area);
            assert!((profile.length - 30.0).abs() < 1e-6, "{name}: length");
            assert!(profile.scaled, "{name}: scaled");
        }
    }

    #[test]
    fn section_profile_honesty_cases() {
        let proposed = |_x: f32, _z: f32| Some(2.0f32);
        // Fewer than two surviving samples -> None, never an invented height.
        let none_surface = |_x: f32, _z: f32| None;
        assert!(sample_profile(&none_surface, &proposed, LINE_A, LINE_B, 512, 1.0).is_none());
        let one_survivor = |x: f32, _z: f32| if x == 0.0 { Some(0.0) } else { None };
        assert!(sample_profile(&one_survivor, &proposed, LINE_A, LINE_B, 512, 1.0).is_none());
        // Unscaled world stays in world units and says so.
        let unscaled = sample_profile(&flat, &proposed, LINE_A, LINE_B, 512, 0.0).unwrap();
        assert!(!unscaled.scaled);
        assert!((unscaled.length - 30.0).abs() < 1e-6);
        assert!((unscaled.fill_area - 60.0).abs() < 1e-3);
        // Scaled world converts lengths and areas to meters (world = meters * scale).
        let scaled = sample_profile(&flat, &proposed, LINE_A, LINE_B, 512, 0.5).unwrap();
        assert!(scaled.scaled);
        assert!((scaled.length - 60.0).abs() < 1e-6);
        assert!((scaled.fill_area - 240.0).abs() < 1e-3);
        // Sample cap is honoured.
        let capped = sample_profile(&flat, &proposed, LINE_A, LINE_B, 8, 1.0).unwrap();
        assert_eq!(capped.samples.len(), 8);
    }
}
