//! The "detail" sliders: Texture, Sharpening, Noise Reduction and Defringe.
//!
//! Unlike the tone and colour sliders, these look at a pixel's neighbours, so
//! they work on a block of pixels at a time. They run on the photo's linear
//! values before any tone or colour change, which means moving Exposure or
//! Contrast afterwards doesn't change what counts as noise or an edge.
//!
//! Sizes are given in full-resolution pixels and divided by `px_scale` (how
//! many photo pixels one rendered pixel covers). In a fit-to-window preview a
//! 1-pixel sharpening radius is a fraction of a screen pixel, so sharpening and
//! noise reduction fade out there; zoom to 100 % to judge them. That is also
//! what happens to them when the exported photo is viewed small.
//!
//! Brightness is filtered as its square root, a rough stand-in for how bright
//! things look, so halos are balanced between the dark and the light side of
//! an edge.

use crate::adjust::Adjustments;
use crate::local::box_blur;

/// Never ask for more neighbouring rows than this, whatever the settings.
const MAX_MARGIN: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    /// −1 … 1.
    texture: f32,
    texture_fine: f32,
    texture_coarse: f32,
    /// Strength of the sharpening (0 = off).
    sharpen: f32,
    sharpen_sigma: f32,
    /// Differences smaller than about this are left alone (the Detail slider).
    sharpen_core: f32,
    /// 0 … 1: how much sharpening is held back away from edges (the Masking slider).
    sharpen_mask: f32,
    noise_luma: f32,
    noise_luma_radius: usize,
    noise_color: f32,
    noise_color_radius: usize,
    fringe_purple: f32,
    fringe_green: f32,
    fringe_radius: usize,
    /// How many rows above and below a block the filters need to see.
    pub margin: usize,
}

/// 0 below `lo`, 1 above `hi`, straight line between.
fn fade(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

impl Detail {
    /// `None` when no detail slider is in use (or none would be visible at this scale).
    pub fn new(a: &Adjustments, px_scale: f32) -> Option<Self> {
        let s = px_scale.max(0.05);
        let sharpen_sigma = a.sharpen.radius / s;
        // Below about a third of a rendered pixel the result is indistinguishable from none, so skip the work.
        let sharpen = a.sharpen.amount / 100.0 * 1.6 * fade(sharpen_sigma, 0.35, 0.7);
        let texture_coarse = 6.0 / s;
        let texture = a.texture / 100.0 * fade(texture_coarse, 0.4, 1.0);
        let noise_luma_radius = (2.5 / s).round() as usize;
        let noise_luma = if noise_luma_radius == 0 { 0.0 } else { a.noise.luminance / 100.0 };
        let noise_color_radius = (5.0 / s).round() as usize;
        let noise_color = if noise_color_radius == 0 { 0.0 } else { a.noise.color / 100.0 };
        let fringe_radius = (3.0 / s).round() as usize;
        let on = if fringe_radius == 0 { 0.0 } else { 1.0 };
        let (fringe_purple, fringe_green) = (a.optics.defringe_purple / 20.0 * on, a.optics.defringe_green / 20.0 * on);
        if sharpen == 0.0 && texture == 0.0 && noise_luma == 0.0 && noise_color == 0.0 && fringe_purple == 0.0 && fringe_green == 0.0 {
            return None;
        }
        let mut margin = 0.0f32;
        if noise_color > 0.0 {
            margin += 2.0 * noise_color_radius as f32;
        }
        if noise_luma > 0.0 {
            margin += 2.0 * noise_luma_radius as f32;
        }
        if sharpen != 0.0 {
            margin += (2.5 * sharpen_sigma).ceil() * 3.0 + 1.0;
        }
        if texture != 0.0 {
            margin = margin.max(3.0 * texture_coarse);
        }
        if fringe_purple > 0.0 || fringe_green > 0.0 {
            margin += 2.0 * fringe_radius as f32;
        }
        Some(Detail {
            texture,
            texture_fine: 1.0 / s,
            texture_coarse,
            sharpen,
            sharpen_sigma,
            sharpen_core: 0.0003 + 0.004 * (1.0 - a.sharpen.detail / 100.0).powi(2),
            sharpen_mask: a.sharpen.masking / 100.0,
            noise_luma,
            noise_luma_radius,
            noise_color,
            noise_color_radius,
            fringe_purple,
            fringe_green,
            fringe_radius,
            margin: (margin.ceil() as usize).clamp(1, MAX_MARGIN),
        })
    }

    /// Filter a `w × h` block of linear RGB pixels in place.
    pub fn apply(&self, px: &mut [[f32; 3]], w: usize, h: usize) {
        debug_assert_eq!(px.len(), w * h);
        if w == 0 || h == 0 {
            return;
        }
        if self.noise_color > 0.0 {
            self.reduce_color_noise(px, w, h);
        }
        let y: Vec<f32> = px.iter().map(|p| luma(*p).max(0.0)).collect();
        let l: Vec<f32> = y.iter().map(|v| v.sqrt()).collect();

        // Luminance noise: an edge-aware blur, so flat areas are smoothed and edges are kept.
        let base = if self.noise_luma > 0.0 {
            // "Noise" is brightness variation smaller than about `level`.
            let level = 0.012 + 0.05 * self.noise_luma;
            let smooth = guided(&l, &l, w, h, self.noise_luma_radius, level * level);
            let k = (self.noise_luma * 2.0).min(1.0);
            l.iter().zip(&smooth).map(|(a, b)| a + (b - a) * k).collect()
        } else {
            l.clone()
        };

        let mut out = base.clone();
        if self.sharpen != 0.0 {
            let blurred = gaussian(&base, w, h, self.sharpen_sigma);
            let high: Vec<f32> = base.iter().zip(&blurred).map(|(a, b)| a - b).collect();
            let mask = (self.sharpen_mask > 0.0).then(|| {
                // How "edgy" the neighbourhood is: the average size of the fine detail nearby.
                let size: Vec<f32> = high.iter().map(|v| v.abs()).collect();
                gaussian(&size, w, h, self.sharpen_sigma * 2.0 + 0.7)
            });
            let core2 = self.sharpen_core * self.sharpen_core;
            let threshold = 0.0006 + 0.005 * self.sharpen_mask;
            for i in 0..w * h {
                let d = high[i];
                // Leave the tiniest differences (mostly noise) alone.
                let mut add = self.sharpen * d * (d * d / (d * d + core2));
                if let Some(m) = &mask {
                    add *= fade(m[i], threshold * 0.5, threshold * 1.5);
                }
                // Cap the boost so edges don't grow bright or dark rims.
                out[i] += add.clamp(-0.12, 0.12);
            }
        }
        if self.texture != 0.0 {
            // Medium-sized detail: what is left between a slight and a wide blur.
            let fine = gaussian(&base, w, h, self.texture_fine);
            let coarse = gaussian(&base, w, h, self.texture_coarse);
            for i in 0..w * h {
                out[i] += (self.texture * 0.75 * (fine[i] - coarse[i])).clamp(-0.07, 0.07);
            }
        }

        // Apply the brightness change as a gain, so colours keep their hue.
        for i in 0..w * h {
            if l[i] > 1e-4 {
                let ratio = out[i].max(0.0) / l[i];
                let gain = (ratio * ratio).clamp(0.2, 5.0);
                px[i] = [px[i][0] * gain, px[i][1] * gain, px[i][2] * gain];
            }
        }
        if self.fringe_purple > 0.0 || self.fringe_green > 0.0 {
            self.defringe(px, &l, w, h);
        }
    }

    /// Smooth colour while keeping brightness: each of red and blue is made to
    /// follow the local brightness pattern, and green is then set so the
    /// pixel's brightness is unchanged.
    fn reduce_color_noise(&self, px: &mut [[f32; 3]], w: usize, h: usize) {
        let y: Vec<f32> = px.iter().map(|p| luma(*p)).collect();
        let r: Vec<f32> = px.iter().map(|p| p[0]).collect();
        let b: Vec<f32> = px.iter().map(|p| p[2]).collect();
        let eps = 1e-5 + 4e-4 * self.noise_color;
        let rs = guided(&r, &y, w, h, self.noise_color_radius, eps);
        let bs = guided(&b, &y, w, h, self.noise_color_radius, eps);
        let k = (self.noise_color * 1.5).min(1.0);
        for i in 0..w * h {
            let nr = r[i] + (rs[i] - r[i]) * k;
            let nb = b[i] + (bs[i] - b[i]) * k;
            let ng = (y[i] - 0.2126 * nr - 0.0722 * nb) / 0.7152;
            // Where keeping brightness would need negative green, leave the pixel alone.
            if ng >= 0.0 && nr >= 0.0 && nb >= 0.0 {
                px[i] = [nr, ng, nb];
            }
        }
    }

    /// Remove purple and green colour fringes, which only appear beside strong edges.
    fn defringe(&self, px: &mut [[f32; 3]], l: &[f32], w: usize, h: usize) {
        let r = self.fringe_radius;
        let mean = box_blur(l, w, h, r);
        let contrast: Vec<f32> = l.iter().zip(&mean).map(|(a, b)| (a - b).abs()).collect();
        let edge = box_blur(&contrast, w, h, r);
        for i in 0..w * h {
            let near_edge = fade(edge[i], 0.015, 0.06);
            if near_edge == 0.0 {
                continue;
            }
            let [pr, pg, pb] = px[i];
            let purple = pr.min(pb) - pg;
            if purple > 0.0 && self.fringe_purple > 0.0 {
                let cut = purple * (self.fringe_purple * near_edge).min(1.0);
                px[i] = [pr - cut, pg, pb - cut];
            }
            let green = pg - pr.max(pb);
            if green > 0.0 && self.fringe_green > 0.0 {
                px[i][1] = pg - green * (self.fringe_green * near_edge).min(1.0);
            }
        }
    }
}

#[inline]
fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Gaussian blur. Small radii use the exact bell-shaped weights; large ones
/// use three box blurs, which is nearly the same and much faster.
pub(crate) fn gaussian(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return src.to_vec();
    }
    if sigma >= 3.0 {
        let r = (((1.0 + 4.0 * sigma * sigma).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
        return box_blur(&box_blur(&box_blur(src, w, h, r), w, h, r), w, h, r);
    }
    let radius = (2.5 * sigma).ceil() as usize;
    let mut kernel: Vec<f32> = (0..=radius).map(|i| (-(i as f32).powi(2) / (2.0 * sigma * sigma)).exp()).collect();
    let total: f32 = kernel[0] + 2.0 * kernel[1..].iter().sum::<f32>();
    kernel.iter_mut().for_each(|k| *k /= total);

    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut acc = row[x] * kernel[0];
            for (i, k) in kernel.iter().enumerate().skip(1) {
                acc += k * (row[x.saturating_sub(i)] + row[(x + i).min(w - 1)]);
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = tmp[y * w + x] * kernel[0];
            for (i, k) in kernel.iter().enumerate().skip(1) {
                acc += k * (tmp[y.saturating_sub(i) * w + x] + tmp[(y + i).min(h - 1) * w + x]);
            }
            out[y * w + x] = acc;
        }
    }
    out
}

/// Guided filter: smooth `input`, keeping the edges that `guide` has.
/// `eps` sets how large a variation in the guide counts as an edge.
fn guided(input: &[f32], guide: &[f32], w: usize, h: usize, radius: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    let mean_i = box_blur(guide, w, h, radius);
    let mean_p = box_blur(input, w, h, radius);
    let ii: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = guide.iter().zip(input).map(|(a, b)| a * b).collect();
    let mean_ii = box_blur(&ii, w, h, radius);
    let mean_ip = box_blur(&ip, w, h, radius);
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for i in 0..n {
        let var = (mean_ii[i] - mean_i[i] * mean_i[i]).max(0.0);
        let cov = mean_ip[i] - mean_i[i] * mean_p[i];
        a[i] = cov / (var + eps);
        b[i] = mean_p[i] - a[i] * mean_i[i];
    }
    let mean_a = box_blur(&a, w, h, radius);
    let mean_b = box_blur(&b, w, h, radius);
    (0..n).map(|i| mean_a[i] * guide[i] + mean_b[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings with only the named sliders on (the default sharpening is switched off).
    fn with(f: impl FnOnce(&mut Adjustments)) -> Detail {
        let mut a = Adjustments::default();
        a.sharpen.amount = 0.0;
        f(&mut a);
        Detail::new(&a, 1.0).expect("a detail slider is on")
    }

    /// Repeatable pseudo-random numbers in −1 … 1.
    fn noise(i: usize) -> f32 {
        let mut h = (i as u32).wrapping_mul(0x9E37_79B1);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        h as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn std_dev(v: &[f32]) -> f32 {
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    /// A slightly soft vertical edge from `lo` to `hi` in the middle of a 64 × 32 block.
    fn soft_edge(lo: f32, hi: f32) -> Vec<[f32; 3]> {
        (0..64 * 32)
            .map(|i| {
                let t = ((i % 64) as f32 - 31.5) / 1.2;
                [lo + (hi - lo) / (1.0 + (-t).exp()); 3]
            })
            .collect()
    }

    #[test]
    fn nothing_switched_on_means_no_filter() {
        let mut a = Adjustments::default();
        a.sharpen.amount = 0.0;
        assert!(Detail::new(&a, 1.0).is_none());
        // An unedited photo gets the default sharpening at full size…
        assert!(Detail::new(&Adjustments::default(), 1.0).is_some());
        // …but a 1-pixel radius can't be seen in a preview five times smaller.
        assert!(Detail::new(&Adjustments::default(), 5.0).is_none());
    }

    #[test]
    fn sharpening_steepens_an_edge_and_leaves_flat_areas_alone() {
        let mut px = soft_edge(0.1, 0.5);
        let before = px.clone();
        with(|a| a.sharpen.amount = 100.0).apply(&mut px, 64, 32);
        let row = |p: &[[f32; 3]], x: usize| p[16 * 64 + x][0];
        let slope = |p: &[[f32; 3]]| row(p, 33) - row(p, 30);
        assert!(slope(&px) > slope(&before) * 1.1, "{} vs {}", slope(&px), slope(&before));
        assert!((row(&px, 5) - row(&before, 5)).abs() < 1e-4 && (row(&px, 58) - row(&before, 58)).abs() < 1e-4);
        // Grey stays grey.
        assert!(px.iter().all(|p| (p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6));
    }

    #[test]
    fn masking_keeps_sharpening_off_fine_grain_but_on_edges() {
        // Faint grain on the left half, a strong edge at the centre.
        let make = || -> Vec<[f32; 3]> {
            soft_edge(0.2, 0.6).iter().enumerate().map(|(i, p)| [p[0] * (1.0 + 0.02 * noise(i)); 3]).collect()
        };
        let grain_of = |p: &[[f32; 3]]| std_dev(&(0..32).flat_map(|y| (2..20).map(move |x| y * 64 + x)).map(|i| p[i][0]).collect::<Vec<_>>());
        let mut plain = make();
        let mut masked = make();
        with(|a| a.sharpen.amount = 120.0).apply(&mut plain, 64, 32);
        with(|a| {
            a.sharpen.amount = 120.0;
            a.sharpen.masking = 80.0;
        })
        .apply(&mut masked, 64, 32);
        let original = grain_of(&make());
        assert!(grain_of(&plain) > original * 1.3, "plain sharpening should boost grain");
        assert!(grain_of(&masked) < original * 1.1, "masking should protect the grain: {} vs {original}", grain_of(&masked));
        let slope = |p: &[[f32; 3]]| p[16 * 64 + 33][0] - p[16 * 64 + 30][0];
        assert!(slope(&masked) > slope(&make()) * 1.1, "the edge is still sharpened");
    }

    #[test]
    fn luminance_noise_reduction_smooths_flat_areas_and_keeps_edges() {
        let make = || -> Vec<[f32; 3]> {
            (0..64 * 32).map(|i| [if i % 64 < 32 { 0.08 } else { 0.5 } * (1.0 + 0.08 * noise(i)); 3]).collect()
        };
        let mut px = make();
        with(|a| a.noise.luminance = 60.0).apply(&mut px, 64, 32);
        let flat = |p: &[[f32; 3]]| std_dev(&(0..32).flat_map(|y| (40..60).map(move |x| y * 64 + x)).map(|i| p[i][0]).collect::<Vec<_>>());
        assert!(flat(&px) < flat(&make()) * 0.5, "{} vs {}", flat(&px), flat(&make()));
        // The step is still a step: one pixel either side of it stays far apart.
        let (left, right) = (px[16 * 64 + 30][0], px[16 * 64 + 33][0]);
        assert!(right - left > 0.35, "{left} {right}");
    }

    #[test]
    fn colour_noise_reduction_removes_colour_speckle_but_not_brightness_or_real_colour() {
        // An orange area with random colour speckle of constant brightness.
        let make = || -> Vec<[f32; 3]> {
            (0..64 * 32)
                .map(|i| {
                    let (r, b) = (0.4 + 0.08 * noise(i), 0.1 + 0.05 * noise(i + 7919));
                    // Pick green so every pixel has the same brightness.
                    [r, (0.25 - 0.2126 * r - 0.0722 * b) / 0.7152, b]
                })
                .collect()
        };
        let mut px = make();
        with(|a| a.noise.color = 70.0).apply(&mut px, 64, 32);
        let channel = |p: &[[f32; 3]], c: usize| p.iter().map(|v| v[c]).collect::<Vec<_>>();
        assert!(std_dev(&channel(&px, 0)) < std_dev(&channel(&make(), 0)) * 0.4);
        for p in &px {
            assert!((luma(*p) - 0.25).abs() < 1e-3, "brightness changed: {p:?}");
        }
        let mean_r = channel(&px, 0).iter().sum::<f32>() / px.len() as f32;
        assert!((mean_r - 0.4).abs() < 0.01, "the area is still orange: {mean_r}");
    }

    #[test]
    fn texture_changes_medium_detail_in_both_directions() {
        // Ripples about 10 pixels wide.
        let make = || -> Vec<[f32; 3]> { (0..96 * 32).map(|i| [0.3 + 0.03 * ((i % 96) as f32 * 0.6).sin(); 3]).collect() };
        let ripple = |p: &[[f32; 3]]| std_dev(&p.iter().map(|v| v[0]).collect::<Vec<_>>());
        let (mut more, mut less) = (make(), make());
        with(|a| a.texture = 80.0).apply(&mut more, 96, 32);
        with(|a| a.texture = -80.0).apply(&mut less, 96, 32);
        assert!(ripple(&more) > ripple(&make()) * 1.2 && ripple(&less) < ripple(&make()) * 0.8);
    }

    #[test]
    fn defringe_removes_purple_only_beside_strong_edges() {
        // Dark left, bright right, with a purple pixel column on the edge and a purple object far from any edge.
        let mut px: Vec<[f32; 3]> = (0..64 * 32).map(|i| if i % 64 < 32 { [0.02; 3] } else { [0.8; 3] }).collect();
        for y in 0..32 {
            px[y * 64 + 31] = [0.35, 0.1, 0.45];
            px[y * 64 + 32] = [0.5, 0.25, 0.6];
        }
        let far = 16 * 64 + 56;
        px[far] = [0.8, 0.79, 0.8];
        let before = px.clone();
        with(|a| a.optics.defringe_purple = 20.0).apply(&mut px, 64, 32);
        let purple = |p: [f32; 3]| p[0].min(p[2]) - p[1];
        assert!(purple(px[16 * 64 + 31]) < purple(before[16 * 64 + 31]) * 0.3, "{:?}", px[16 * 64 + 31]);
        assert_eq!(px[16 * 64 + 5], before[16 * 64 + 5]);
        assert_eq!(px[far], before[far]);
    }

    #[test]
    fn gaussian_blur_keeps_the_average_and_matches_its_width() {
        let mut src = vec![0.0f32; 41 * 41];
        src[20 * 41 + 20] = 1.0;
        for sigma in [0.6f32, 1.5, 4.0] {
            let out = gaussian(&src, 41, 41, sigma);
            assert!((out.iter().sum::<f32>() - 1.0).abs() < 1e-3, "sigma {sigma}");
            // Measured spread (standard deviation along x) is close to the one asked for.
            let var: f32 = out.iter().enumerate().map(|(i, v)| v * ((i % 41) as f32 - 20.0).powi(2)).sum();
            assert!((var.sqrt() - sigma).abs() < 0.25 * sigma, "sigma {sigma}: got {}", var.sqrt());
        }
    }
}
