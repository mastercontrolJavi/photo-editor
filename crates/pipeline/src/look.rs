//! The camera's own starting look, as stored in the DNG:
//!
//! * **ProfileGainTableMap** (DNG 1.6 tag 52525): a coarse grid over the image,
//!   each cell holding a brightness → gain table. Apple uses it for local tone
//!   mapping (lifting shadows, holding highlights). It depends on position, so it
//!   is applied once at decode time, in the stored (un-rotated) frame.
//! * **ProfileToneCurve** (tag 50940): a global curve in linear light, applied at
//!   render time after exposure, so the Exposure slider works "under" it the way
//!   it does in Lightroom.

use std::sync::Arc;

use rayon::prelude::*;

use crate::image::LinearImage;
use crate::local::LocalMaps;

/// Rendering inputs that come from the file rather than from the user's sliders.
#[derive(Debug, Clone, Default)]
pub struct Look {
    /// DNG BaselineExposure in stops; added to the user's exposure.
    pub baseline_exposure: f32,
    /// The file's default tone curve, if it has one.
    pub tone_curve: Option<Arc<ToneCurve>>,
    /// Helper maps for the local sliders (Shadows, Highlights, Clarity, Dehaze).
    pub local: Option<Arc<LocalMaps>>,
}

impl Look {
    /// No tone curve, no exposure offset: plain linear → sRGB.
    pub fn neutral() -> Self {
        Self::default()
    }

    pub fn with_exposure(baseline_exposure: f32) -> Self {
        Look { baseline_exposure, ..Self::default() }
    }
}

// ───────────────────────────── tone curve ─────────────────────────────

const CURVE_LUT: usize = 4096;

/// A monotone curve on linear [0, 1], sampled into a lookup table.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurve {
    lut: Vec<f32>,
}

impl ToneCurve {
    /// Build from DNG (input, output) pairs. Returns `None` for curves that are
    /// malformed (too few points, not increasing, out of range).
    pub fn from_points(points: &[(f32, f32)]) -> Option<Self> {
        if points.len() < 2 || points.len() > 8192 {
            return None;
        }
        let ok_range = points.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
        let increasing = points.windows(2).all(|w| w[1].0 > w[0].0);
        if !ok_range || !increasing {
            return None;
        }
        let lut = (0..CURVE_LUT)
            .map(|i| {
                let x = i as f32 / (CURVE_LUT - 1) as f32;
                let j = points.partition_point(|p| p.0 <= x).clamp(1, points.len() - 1);
                let (x0, y0) = points[j - 1];
                let (x1, y1) = points[j];
                if x <= x0 {
                    y0
                } else if x >= x1 {
                    y1
                } else {
                    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
                }
            })
            .collect();
        Some(ToneCurve { lut })
    }

    /// The curve at one linear value (clamped to [0, 1]).
    #[inline]
    pub fn eval(&self, v: f32) -> f32 {
        let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        let f = v * (CURVE_LUT - 1) as f32;
        let i = (f as usize).min(CURVE_LUT - 2);
        let t = f - i as f32;
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t
    }

    /// Apply to an RGB pixel without shifting its hue: the largest and smallest
    /// channels go through the curve and the middle one keeps its relative place
    /// between them (the method Adobe's DNG reference code uses).
    #[inline]
    pub fn apply_rgb(&self, p: [f32; 3]) -> [f32; 3] {
        let p = p.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let (mut hi, mut mid, mut lo) = (0, 1, 2);
        if p[hi] < p[mid] {
            std::mem::swap(&mut hi, &mut mid);
        }
        if p[mid] < p[lo] {
            std::mem::swap(&mut mid, &mut lo);
        }
        if p[hi] < p[mid] {
            std::mem::swap(&mut hi, &mut mid);
        }
        let mut out = [0.0; 3];
        let h = self.eval(p[hi]);
        let l = self.eval(p[lo]);
        out[hi] = h;
        out[lo] = l;
        out[mid] = if p[hi] > p[lo] { l + (h - l) * (p[mid] - p[lo]) / (p[hi] - p[lo]) } else { h };
        out
    }
}

// ─────────────────────────── gain table map ───────────────────────────

/// Parsed ProfileGainTableMap. Layout (big-endian, DNG 1.6 §6):
/// MapPointsV u32, MapPointsH u32, MapSpacingV/H f64, MapOriginV/H f64,
/// MapPointsN u32, MapInputWeights 5×f32, then V×H×N f32 gains (N fastest).
#[derive(Debug, Clone)]
pub struct GainTableMap {
    points_v: usize,
    points_h: usize,
    spacing_v: f64,
    spacing_h: f64,
    origin_v: f64,
    origin_h: f64,
    points_n: usize,
    /// Weights for R, G, B, min(R,G,B), max(R,G,B).
    weights: [f32; 5],
    gains: Vec<f32>,
}

impl GainTableMap {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap()));
        let f64_at = |o: usize| b.get(o..o + 8).map(|s| f64::from_be_bytes(s.try_into().unwrap()));
        let f32_at = |o: usize| b.get(o..o + 4).map(|s| f32::from_be_bytes(s.try_into().unwrap()));
        let points_v = u32_at(0)? as usize;
        let points_h = u32_at(4)? as usize;
        let (spacing_v, spacing_h, origin_v, origin_h) = (f64_at(8)?, f64_at(16)?, f64_at(24)?, f64_at(32)?);
        let points_n = u32_at(40)? as usize;
        let mut weights = [0f32; 5];
        for (i, w) in weights.iter_mut().enumerate() {
            *w = f32_at(44 + 4 * i)?;
        }
        if !(1..=4096).contains(&points_v) || !(1..=4096).contains(&points_h) || !(2..=4096).contains(&points_n) {
            return None;
        }
        let count = points_v * points_h * points_n;
        // Gains are the last `count` floats; DNG 1.7 may put extra header fields first.
        let start = b.len().checked_sub(4 * count)?;
        if start < 64 {
            return None;
        }
        let gains: Vec<f32> = (0..count).map(|i| f32_at(start + 4 * i).unwrap()).collect();
        let finite = [spacing_v, spacing_h, origin_v, origin_h].iter().all(|v| v.is_finite())
            && spacing_v > 0.0
            && spacing_h > 0.0
            && weights.iter().all(|w| w.is_finite())
            && gains.iter().all(|g| g.is_finite() && *g >= 0.0);
        if !finite {
            return None;
        }
        Some(GainTableMap { points_v, points_h, spacing_v, spacing_h, origin_v, origin_h, points_n, weights, gains })
    }

    /// Grid size as (rows, columns, table entries).
    pub fn dims(&self) -> (usize, usize, usize) {
        (self.points_v, self.points_h, self.points_n)
    }

    /// Multiply every pixel by its local gain. `input_scale` scales the pixel
    /// values used to look the gain up (not the pixels themselves).
    pub fn apply(&self, img: &mut LinearImage, input_scale: f32) {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 {
            return;
        }
        // Column positions in map units, shared by every row.
        let cols: Vec<(usize, usize, f32)> =
            (0..w).map(|x| grid_pos(((x as f64 + 0.5) / w as f64 - self.origin_h) / self.spacing_h, self.points_h)).collect();
        let n = self.points_n;
        let wts = self.weights.map(|v| v * input_scale);
        img.pixels.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let (v0, v1, fv) = grid_pos(((y as f64 + 0.5) / h as f64 - self.origin_v) / self.spacing_v, self.points_v);
            for (px, &(h0, h1, fh)) in row.iter_mut().zip(&cols) {
                let [r, g, b] = *px;
                let t = wts[0] * r + wts[1] * g + wts[2] * b + wts[3] * r.min(g).min(b) + wts[4] * r.max(g).max(b);
                let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) } * (n - 1) as f32;
                let k = (t as usize).min(n - 2);
                let ft = t - k as f32;
                let at = |v: usize, hh: usize| {
                    let base = (v * self.points_h + hh) * n + k;
                    self.gains[base] + (self.gains[base + 1] - self.gains[base]) * ft
                };
                let top = at(v0, h0) + (at(v0, h1) - at(v0, h0)) * fh;
                let bottom = at(v1, h0) + (at(v1, h1) - at(v1, h0)) * fh;
                let gain = top + (bottom - top) * fv;
                *px = [r * gain, g * gain, b * gain];
            }
        });
    }
}

/// Neighbouring grid indices and blend weight for a map-unit position.
fn grid_pos(p: f64, points: usize) -> (usize, usize, f32) {
    let p = p.clamp(0.0, (points - 1) as f64);
    let i0 = (p.floor() as usize).min(points - 1);
    let i1 = (i0 + 1).min(points - 1);
    (i0, i1, (p - i0 as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gtm_bytes(pv: u32, ph: u32, pn: u32, weights: [f32; 5], gain: impl Fn(u32, u32, u32) -> f32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(pv.to_be_bytes());
        b.extend(ph.to_be_bytes());
        for v in [1.0 / pv as f64, 1.0 / ph as f64, 0.5 / pv as f64, 0.5 / ph as f64] {
            b.extend(v.to_be_bytes());
        }
        b.extend(pn.to_be_bytes());
        for w in weights {
            b.extend(w.to_be_bytes());
        }
        for v in 0..pv {
            for h in 0..ph {
                for k in 0..pn {
                    b.extend(gain(v, h, k).to_be_bytes());
                }
            }
        }
        b
    }

    #[test]
    fn identity_curve_changes_nothing() {
        let c = ToneCurve::from_points(&[(0.0, 0.0), (1.0, 1.0)]).unwrap();
        for p in [[0.1, 0.5, 0.9], [0.3, 0.3, 0.3], [0.0, 1.0, 0.25]] {
            let o = c.apply_rgb(p);
            for i in 0..3 {
                assert!((o[i] - p[i]).abs() < 1e-5, "{p:?} -> {o:?}");
            }
        }
    }

    #[test]
    fn curve_keeps_grays_gray_and_order_of_channels() {
        let c = ToneCurve::from_points(&[(0.0, 0.0), (0.25, 0.4), (1.0, 1.0)]).unwrap();
        let g = c.apply_rgb([0.25; 3]);
        assert!((g[0] - 0.4).abs() < 1e-4 && g[0] == g[1] && g[1] == g[2], "{g:?}");
        let o = c.apply_rgb([0.2, 0.6, 0.4]);
        assert!(o[1] > o[2] && o[2] > o[0], "{o:?}");
    }

    #[test]
    fn bad_curves_are_rejected() {
        assert!(ToneCurve::from_points(&[(0.0, 0.0)]).is_none());
        assert!(ToneCurve::from_points(&[(0.0, 0.0), (0.5, 0.2), (0.4, 0.3), (1.0, 1.0)]).is_none());
        assert!(ToneCurve::from_points(&[(0.0, 0.0), (1.0, 2.0)]).is_none());
    }

    #[test]
    fn gain_map_uses_position_and_brightness() {
        // 1 row × 2 columns; left column doubles everything, right column
        // doubles dark pixels and leaves bright ones alone.
        let bytes = gtm_bytes(1, 2, 2, [0.0, 0.0, 0.0, 0.0, 1.0], |_, h, k| if h == 0 || k == 0 { 2.0 } else { 1.0 });
        let map = GainTableMap::parse(&bytes).unwrap();
        assert_eq!(map.dims(), (1, 2, 2));
        let mut img = LinearImage::new(2, 1, vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
        img.pixels[0] = [0.25; 3];
        map.apply(&mut img, 1.0);
        assert!((img.pixels[0][0] - 0.5).abs() < 1e-5, "{:?}", img.pixels[0]);
        assert!((img.pixels[1][0] - 1.0).abs() < 1e-5, "{:?}", img.pixels[1]);
    }

    #[test]
    fn truncated_or_garbage_gain_maps_are_rejected() {
        let bytes = gtm_bytes(2, 2, 3, [0.2, 0.7, 0.1, 0.0, 0.0], |_, _, _| 1.0);
        assert!(GainTableMap::parse(&bytes).is_some());
        assert!(GainTableMap::parse(&bytes[..bytes.len() / 2]).is_none());
        assert!(GainTableMap::parse(&[0u8; 10]).is_none());
        let bad = gtm_bytes(2, 2, 3, [0.2, 0.7, 0.1, 0.0, 0.0], |_, _, _| f32::NAN);
        assert!(GainTableMap::parse(&bad).is_none());
    }
}
