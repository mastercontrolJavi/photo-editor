//! Small helper maps for the "local" sliders (Shadows, Highlights, Clarity, Dehaze).
//!
//! Those sliders need to know what the neighbourhood of each pixel looks like,
//! not just the pixel itself. Working that out at full size on every slider
//! move would be slow, so it is done once when a photo opens, on a small copy:
//!
//! * `a`, `b` — an edge-aware blur of brightness (a "guided filter"). For any
//!   pixel with brightness `x` (in stops), `a·x + b` is the brightness of its
//!   surroundings. Near strong edges `a` → 1, which is what prevents halos.
//! * `dark` — the darkest colour channel nearby, which estimates haze.
//!
//! The maps are looked up by position (0…1), so the preview and the full-size
//! export read exactly the same values.

use crate::image::LinearImage;

/// Long edge of the helper maps, in pixels.
const MAP_EDGE: usize = 640;
/// Brightness differences smaller than about √EPS stops are treated as texture
/// (blurred); larger ones as edges (kept).
const EPS: f32 = 0.35;

#[derive(Clone)]
pub struct LocalMaps {
    pub width: usize,
    pub height: usize,
    a: Vec<f32>,
    b: Vec<f32>,
    dark: Vec<f32>,
    /// Colour of the haze ("airlight"), in the photo's linear RGB.
    pub airlight: [f32; 3],
}

impl std::fmt::Debug for LocalMaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LocalMaps({} x {}, airlight {:?})", self.width, self.height, self.airlight)
    }
}

/// Brightness in stops relative to middle grey (0.18).
#[inline]
pub fn stops(y: f32) -> f32 {
    crate::color::fast_log2(y.max(1e-5) / 0.18)
}

impl LocalMaps {
    pub fn compute(img: &LinearImage) -> Self {
        let small = img.downscale_to_fit(MAP_EDGE);
        let (w, h) = (small.width, small.height);
        let r = ((w.max(h) as f32 * 0.02).round() as usize).max(2);

        let x: Vec<f32> = small
            .pixels
            .iter()
            .map(|p| stops(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).clamp(-12.0, 6.0))
            .collect();
        let xx: Vec<f32> = x.iter().map(|v| v * v).collect();
        let mean = box_blur(&x, w, h, r);
        let mean_sq = box_blur(&xx, w, h, r);
        let mut a = vec![0.0f32; w * h];
        let mut b = vec![0.0f32; w * h];
        for i in 0..w * h {
            let var = (mean_sq[i] - mean[i] * mean[i]).max(0.0);
            a[i] = var / (var + EPS);
            b[i] = mean[i] * (1.0 - a[i]);
        }
        let a = box_blur(&a, w, h, r);
        let b = box_blur(&b, w, h, r);

        let r2 = (r / 2).max(1);
        let min_rgb: Vec<f32> = small.pixels.iter().map(|p| p[0].min(p[1]).min(p[2]).max(0.0)).collect();
        let dark = box_blur(&min_filter(&min_rgb, w, h, r2), w, h, r2);

        // Haze colour: the average of the pixels whose dark channel is brightest.
        let mut order: Vec<usize> = (0..w * h).collect();
        order.sort_by(|&i, &j| dark[j].partial_cmp(&dark[i]).unwrap_or(std::cmp::Ordering::Equal));
        let n = (w * h / 200).max(1);
        let mut air = [0.0f64; 3];
        for &i in &order[..n] {
            for c in 0..3 {
                air[c] += small.pixels[i][c] as f64;
            }
        }
        let airlight = air.map(|v| ((v / n as f64) as f32).clamp(0.05, 4.0));

        LocalMaps { width: w, height: h, a, b, dark, airlight }
    }

    /// `(a, b, dark)` at a position given as fractions of the photo's width and height.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> (f32, f32, f32) {
        let fx = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (i00, i10, i01, i11) = (y0 * self.width + x0, y0 * self.width + x1, y1 * self.width + x0, y1 * self.width + x1);
        let lerp2 = |m: &[f32]| {
            let top = m[i00] + (m[i10] - m[i00]) * tx;
            let bottom = m[i01] + (m[i11] - m[i01]) * tx;
            top + (bottom - top) * ty
        };
        (lerp2(&self.a), lerp2(&self.b), lerp2(&self.dark))
    }
}

/// Mean over a (2r+1)² window; windows are cut off at the image edge.
pub(crate) fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    let mut prefix = vec![0.0f64; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + src[y * w + x] as f64;
        }
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = ((prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64) as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for x in 0..w {
        for y in 0..h {
            prefix[y + 1] = prefix[y] + tmp[y * w + x] as f64;
        }
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            out[y * w + x] = ((prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64) as f32;
        }
    }
    out
}

/// Minimum over a (2r+1)² window.
fn min_filter(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = src[y * w + lo..=y * w + hi].iter().copied().fold(f32::INFINITY, f32::min);
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (lo..=hi).map(|yy| tmp[yy * w + x]).fold(f32::INFINITY, f32::min);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_image_has_flat_surroundings() {
        let img = LinearImage::new(64, 48, vec![[0.18; 3]; 64 * 48]);
        let m = LocalMaps::compute(&img);
        let (a, b, dark) = m.sample(0.3, 0.7);
        // Surroundings of a pixel at 0 stops are at 0 stops.
        assert!((a * 0.0 + b).abs() < 1e-3, "a {a} b {b}");
        assert!((dark - 0.18).abs() < 1e-3);
    }

    #[test]
    fn a_strong_edge_is_kept_but_fine_texture_is_smoothed() {
        // Left half dark, right half bright (5 stops apart), with a faint checker texture.
        let (w, h) = (200usize, 100usize);
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let base = if x < w / 2 { 0.02 } else { 0.64 };
                [base * if (x + y) % 2 == 0 { 1.05 } else { 0.95 }; 3]
            })
            .collect();
        let m = LocalMaps::compute(&LinearImage::new(w, h, px));
        let surround = |u: f32, y: f32| {
            let (a, b, _) = m.sample(u, 0.5);
            a * stops(y) + b
        };
        // Well inside each half the surroundings ignore the texture…
        let (bright_hi, bright_lo) = (surround(0.85, 0.64 * 1.05), surround(0.85, 0.64 * 0.95));
        assert!((bright_hi - bright_lo).abs() < 0.03, "texture leaked into the base: {bright_hi} vs {bright_lo}");
        // …and stay on their own side of the edge (no halo): about −3.2 and +1.8 stops.
        assert!((surround(0.15, 0.02) - stops(0.02)).abs() < 0.2);
        assert!((bright_hi - stops(0.64)).abs() < 0.2);
        // Right at the edge, the surroundings follow the pixel instead of averaging the two sides.
        let at_edge = surround(0.49, 0.02);
        assert!(at_edge < -2.0, "dark side of the edge was pulled up to {at_edge}");
    }
}
