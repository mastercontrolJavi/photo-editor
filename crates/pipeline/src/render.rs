//! The one rendering function used by both the on-screen preview and the
//! exported JPEG. Keeping a single code path is what makes preview and export
//! agree.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::adjust::Adjustments;
use crate::color::{self, Mat3};
use crate::image::LinearImage;
use crate::look::Look;

/// 8-bit sRGB-encoded pixels, row-major RGB.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

const LUT_SIZE: usize = 1 << 16;

/// Linear [0, 1] → 8-bit sRGB, with 65 536 input steps (error < 0.06 of one
/// output level, so effectively exact).
fn encode_lut() -> &'static [u8] {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..LUT_SIZE)
            .map(|i| {
                let v = i as f32 / (LUT_SIZE - 1) as f32;
                (color::srgb_encode(v) * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

/// Combined per-pixel transform: white balance and exposure folded into one
/// 3×3 matrix (both are linear operations in linear light).
pub fn pixel_matrix(adj: &Adjustments, baseline_exposure_ev: f32) -> Mat3 {
    let adj = adj.sanitized();
    let gain = 2f32.powf(adj.exposure + baseline_exposure_ev);
    let mut m = color::white_balance_matrix(adj.temperature, adj.tint);
    m.iter_mut().flatten().for_each(|v| *v *= gain);
    m
}

/// Render scene-linear pixels to display-ready sRGB.
///
/// Steps per pixel: white balance + exposure (one matrix) → the file's tone
/// curve, if it has one (otherwise a plain clip to [0, 1]) → sRGB transfer
/// curve → 8-bit.
pub fn render_srgb8(img: &LinearImage, adj: &Adjustments, look: &Look) -> RenderedImage {
    let m = pixel_matrix(adj, look.baseline_exposure);
    let curve = look.tone_curve.as_deref();
    let lut = encode_lut();
    let scale = (LUT_SIZE - 1) as f32;
    let mut rgb = vec![0u8; img.width * img.height * 3];
    rgb.par_chunks_mut(3 * img.width.max(1))
        .zip(img.pixels.par_chunks(img.width.max(1)))
        .for_each(|(out_row, in_row)| {
            for (out, px) in out_row.as_chunks_mut::<3>().0.iter_mut().zip(in_row) {
                let mut v = color::apply(&m, *px);
                if let Some(curve) = curve {
                    v = curve.apply_rgb(v);
                }
                for c in 0..3 {
                    // NaN-safe: clamp maps NaN to NaN, so guard explicitly.
                    let x = if v[c].is_nan() { 0.0 } else { v[c].clamp(0.0, 1.0) };
                    out[c] = lut[(x * scale + 0.5) as usize];
                }
            }
        });
    RenderedImage { width: img.width, height: img.height, rgb }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: f32) -> LinearImage {
        LinearImage::new(2, 2, vec![[v; 3]; 4])
    }

    #[test]
    fn middle_gray_lands_at_srgb_118() {
        let r = render_srgb8(&flat(0.18), &Adjustments::default(), &Look::neutral());
        assert_eq!(&r.rgb[0..3], &[118, 118, 118]);
    }

    #[test]
    fn exposure_plus_one_doubles_linear_light() {
        let adj = Adjustments { exposure: 1.0, ..Default::default() };
        let a = render_srgb8(&flat(0.09), &adj, &Look::neutral());
        let b = render_srgb8(&flat(0.18), &Adjustments::default(), &Look::neutral());
        assert_eq!(a.rgb, b.rgb);
        // Baseline exposure from the file behaves the same way.
        let c = render_srgb8(&flat(0.09), &Adjustments::default(), &Look::with_exposure(1.0));
        assert_eq!(c.rgb, b.rgb);
    }

    #[test]
    fn out_of_range_and_nan_are_clipped_safely() {
        let img = LinearImage::new(3, 1, vec![[-1.0; 3], [7.0; 3], [f32::NAN; 3]]);
        let r = render_srgb8(&img, &Adjustments::default(), &Look::neutral());
        assert_eq!(r.rgb, vec![0, 0, 0, 255, 255, 255, 0, 0, 0]);
    }

    #[test]
    fn the_files_tone_curve_is_applied_after_exposure() {
        use crate::look::ToneCurve;
        use std::sync::Arc;
        // Curve maps linear 0.09 to 0.18 (middle gray, sRGB 118).
        let curve = ToneCurve::from_points(&[(0.0, 0.0), (0.09, 0.18), (1.0, 1.0)]).unwrap();
        let look = Look { baseline_exposure: 0.0, tone_curve: Some(Arc::new(curve)) };
        let r = render_srgb8(&flat(0.09), &Adjustments::default(), &look);
        assert_eq!(&r.rgb[0..3], &[118, 118, 118]);
        // Exposure goes in before the curve: -1 EV halves the input, not the output.
        let dark = render_srgb8(&flat(0.18), &Adjustments { exposure: -1.0, ..Default::default() }, &look);
        assert_eq!(dark.rgb, r.rgb);
    }

    #[test]
    fn lut_matches_exact_formula_within_one_level() {
        for i in 0..=1000 {
            let v = i as f32 / 1000.0;
            let exact = (color::srgb_encode(v) * 255.0).round() as i32;
            let r = render_srgb8(&flat(v), &Adjustments::default(), &Look::neutral());
            assert!((r.rgb[0] as i32 - exact).abs() <= 1, "v={v}");
        }
    }
}
