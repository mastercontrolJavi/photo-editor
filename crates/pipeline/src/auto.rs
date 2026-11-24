//! "Auto": a starting point for the Light sliders, worked out from how bright
//! and how contrasty the unedited photo is.
//!
//! This is a simple rule of thumb based on the photo's brightness spread. It
//! is not Lightroom's Auto, which uses a trained model.

use crate::adjust::Adjustments;
use crate::render::RenderedImage;

/// Brightness (0…1, as displayed) below which the given fraction of pixels fall.
fn percentiles(img: &RenderedImage, fractions: &[f64]) -> Vec<f32> {
    let mut histogram = [0u32; 256];
    for p in img.rgb.as_chunks::<3>().0 {
        let luma = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        histogram[(luma.round() as usize).min(255)] += 1;
    }
    let total: u32 = histogram.iter().sum();
    fractions
        .iter()
        .map(|f| {
            let target = (*f * total as f64) as u32;
            let mut seen = 0;
            let level = histogram.iter().position(|count| {
                seen += count;
                seen > target
            });
            level.unwrap_or(255) as f32 / 255.0
        })
        .collect()
}

/// Suggest Light settings for a photo, given its unedited rendering (any size).
/// Only Exposure, Contrast, Highlights, Shadows, Whites, Blacks and Vibrance
/// are changed in `current`.
pub fn auto_tone(unedited: &RenderedImage, current: &Adjustments) -> Adjustments {
    let mut a = current.clone();
    if unedited.rgb.len() < 3 {
        return a;
    }
    let p = percentiles(unedited, &[0.01, 0.10, 0.50, 0.90, 0.99]);
    let (p1, p10, p50, p99) = (p[0], p[1], p[2], p[4]);

    // Bring the middle of the photo towards a comfortable brightness, but only part of the way:
    // a night scene should stay dark.
    let stops = 2.2 * (0.45 / p50.max(0.03)).log2();
    let exposure = (stops * 0.55).clamp(-1.5, 1.5);
    // Roughly where the other percentiles land after that exposure change.
    let moved = |v: f32| (v * 2f32.powf(exposure / 2.2)).min(1.0);
    let (p1, p10, p99) = (moved(p1), moved(p10), moved(p99));

    a.exposure = (exposure * 20.0).round() / 20.0;
    a.highlights = 0.0 - ((p99 - 0.86) * 450.0).clamp(0.0, 70.0).round();
    a.shadows = ((0.16 - p10) * 450.0).clamp(0.0, 60.0).round();
    a.whites = ((0.95 - p99) * 160.0).clamp(-20.0, 40.0).round();
    a.blacks = ((0.035 - p1) * 400.0).clamp(-40.0, 10.0).round();
    a.contrast = if p[3] - p[1] > 0.55 { 4.0 } else { 12.0 };
    a.vibrance = 10.0;
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo whose brightness is spread evenly between two levels (0…255).
    fn spread(lo: u8, hi: u8) -> RenderedImage {
        let rgb = (0..1000).flat_map(|i| [(lo as f32 + (hi - lo) as f32 * i as f32 / 999.0) as u8; 3]).collect();
        RenderedImage { width: 1000, height: 1, rgb }
    }

    #[test]
    fn a_dark_flat_photo_is_brightened_and_given_contrast() {
        let a = auto_tone(&spread(10, 90), &Adjustments::default());
        assert!(a.exposure > 0.4, "{}", a.exposure);
        assert!(a.whites > 0.0 && a.contrast >= 10.0);
        assert_eq!(a.highlights, 0.0);
    }

    #[test]
    fn a_bright_washed_out_photo_is_darkened_and_its_blacks_deepened() {
        let a = auto_tone(&spread(120, 255), &Adjustments::default());
        assert!(a.exposure < -0.3, "{}", a.exposure);
        assert!(a.blacks < 0.0, "washed-out blacks are deepened: {}", a.blacks);
        assert!(a.shadows == 0.0 && a.highlights <= 0.0);
    }

    #[test]
    fn bright_highlights_in_a_normal_photo_are_pulled_back_and_dark_shadows_lifted() {
        // Mostly mid-tones, with a tenth of the photo near white and a fifth near black.
        let rgb = (0..1000).flat_map(|i| [if i < 200 { 12u8 } else if i < 900 { 120 } else { 250 }; 3]).collect();
        let a = auto_tone(&RenderedImage { width: 1000, height: 1, rgb }, &Adjustments::default());
        assert!(a.highlights < -20.0, "{}", a.highlights);
        assert!(a.shadows > 20.0, "{}", a.shadows);
    }

    #[test]
    fn a_well_exposed_photo_is_barely_touched_and_other_settings_are_kept() {
        let mut current = Adjustments { temperature: 20.0, clarity: 15.0, ..Default::default() };
        current.crop.rotate_cw();
        let a = auto_tone(&spread(5, 235), &current);
        assert!(a.exposure.abs() < 0.2, "{}", a.exposure);
        assert_eq!((a.temperature, a.clarity, a.crop), (20.0, 15.0, current.crop));
    }
}
