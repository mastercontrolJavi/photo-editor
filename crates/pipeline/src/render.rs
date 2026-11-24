//! The one rendering function used by both the on-screen preview and the
//! exported JPEG. Keeping a single code path is what makes preview and export
//! agree.
//!
//! Order of operations for each pixel:
//!
//! 0. Reading the photo: the Geometry warp, lens corrections and healing
//!    spots, then the detail sliders (Texture, Sharpening, Noise Reduction,
//!    Defringe), which need each pixel's neighbours.
//! 1. Dehaze (on the photo's own values).
//! 2. White balance + exposure (one 3×3 matrix).
//! 3. Tone: Clarity, Shadows, Highlights, Whites, Blacks, Contrast. These work
//!    on brightness in stops and scale the colour as a whole, so hue doesn't shift.
//!    Then each mask's own sliders, as strongly as the mask says at that point.
//! 4. The camera's own tone curve from the file (or a plain clip to 0…1).
//! 5. Vignette.
//! 6. sRGB encoding, then the "display" steps: point curves, Point Color,
//!    Color Mixer, Vibrance/Saturation, B&W, Color Grading, LUT, Grain.
//!
//! These sliders are modelled on Lightroom's but the maths is our own, so the
//! same number gives a similar result, not an identical one.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::adjust::{curve_table, parametric_table, Adjustments, Calibration, Mask, PointColor, Wheel, BAND_HUES};
use crate::detail::Detail;
use crate::heal::Healer;
use crate::lens::Lens;
use crate::mask::MaskEval;
use crate::perspective::Warp;
use crate::color::{self, fast_exp2, fast_pow, Mat3};
use crate::geometry::{Affine, Crop};
use crate::image::LinearImage;
use crate::local::{stops, LocalMaps};
use crate::look::{Look, ToneCurve};
use crate::lut::Lut3D;

/// 8-bit sRGB-encoded pixels, row-major RGB.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// What to render from.
#[derive(Clone, Copy)]
pub struct Scene<'a> {
    pub image: &'a LinearImage,
    pub look: &'a Look,
    /// The look-up table named by `Adjustments::lut`, if it is loaded.
    pub lut: Option<&'a Lut3D>,
}

/// How output pixels relate to the photo and to the crop.
#[derive(Debug, Clone)]
pub struct View {
    /// Output pixel position → position in `Scene::image` (pixels). `None`
    /// means the output is the image itself, pixel for pixel.
    pub to_source: Option<Affine>,
    /// Output pixel position → position in the finished (cropped) picture,
    /// from (−1,−1) at its top-left to (1,1) at its bottom-right.
    pub to_crop: Affine,
    /// Size of the finished picture in full-resolution pixels.
    pub crop_px: (f32, f32),
    /// How many full-resolution pixels one output pixel covers (1 for export).
    pub px_scale: f32,
}

impl View {
    /// The whole image, no crop.
    pub fn whole(width: usize, height: usize) -> Self {
        View {
            to_source: None,
            to_crop: Affine { a: 2.0 / width as f64, b: 0.0, c: -1.0, d: 0.0, e: 2.0 / height as f64, f: -1.0 },
            crop_px: (width as f32, height as f32),
            px_scale: 1.0,
        }
    }

    /// The whole (uncropped) preview image. The crop is applied when the
    /// preview is drawn, but the vignette and grain need to know where it is.
    pub fn preview(crop: &Crop, preview_w: usize, preview_h: usize, full_w: usize, full_h: usize) -> Self {
        let (fw, fh) = (full_w as f64, full_h as f64);
        let unit = crop.unit_to_source(fw, fh).inverse().unwrap_or(Affine::IDENTITY);
        let to_crop = Affine::scale(fw / preview_w as f64, fh / preview_h as f64)
            .then(&unit)
            .then(&Affine { a: 2.0, b: 0.0, c: -1.0, d: 0.0, e: 2.0, f: -1.0 });
        let (ow, oh) = crop.output_size_f(fw, fh);
        View { to_source: None, to_crop, crop_px: (ow as f32, oh as f32), px_scale: (fw / preview_w as f64) as f32 }
    }

    /// Part of the finished picture, read from the full-size photo: what the
    /// app shows when zoomed in. `unit` is the part wanted as
    /// `[left, top, right, bottom]` in 0…1 of the finished picture, rendered
    /// into `out_w × out_h` pixels.
    pub fn region(crop: &Crop, full_w: usize, full_h: usize, unit: [f64; 4], out_w: usize, out_h: usize) -> Self {
        let (uw, uh) = (unit[2] - unit[0], unit[3] - unit[1]);
        // Output pixel → position in the finished picture (0…1).
        let to_unit = Affine { a: uw / out_w as f64, b: 0.0, c: unit[0], d: 0.0, e: uh / out_h as f64, f: unit[1] };
        let (ow, oh) = crop.output_size_f(full_w as f64, full_h as f64);
        View {
            to_source: Some(to_unit.then(&crop.unit_to_source(full_w as f64, full_h as f64))),
            to_crop: to_unit.then(&Affine { a: 2.0, b: 0.0, c: -1.0, d: 0.0, e: 2.0, f: -1.0 }),
            crop_px: (ow as f32, oh as f32),
            px_scale: (ow * uw / out_w as f64) as f32,
        }
    }

    /// The finished, cropped picture at full size. Returns the view and the
    /// output size in pixels.
    pub fn export(crop: &Crop, full_w: usize, full_h: usize) -> (Self, usize, usize) {
        if crop.is_identity() {
            return (Self::whole(full_w, full_h), full_w, full_h);
        }
        let (ow, oh) = crop.output_size(full_w, full_h);
        let to_unit = Affine::scale(1.0 / ow as f64, 1.0 / oh as f64);
        let view = View {
            to_source: Some(to_unit.then(&crop.unit_to_source(full_w as f64, full_h as f64))),
            to_crop: Affine { a: 2.0 / ow as f64, b: 0.0, c: -1.0, d: 0.0, e: 2.0 / oh as f64, f: -1.0 },
            crop_px: (ow as f32, oh as f32),
            px_scale: 1.0,
        };
        (view, ow, oh)
    }
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

const FLOAT_STEPS: usize = 1 << 14;

/// Linear [0, 1] → sRGB-encoded [0, 1] as a float table (read with interpolation).
fn encode_table() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| (0..=FLOAT_STEPS + 1).map(|i| color::srgb_encode((i as f32 / FLOAT_STEPS as f32).min(1.0))).collect())
}

#[inline]
fn encode_f(table: &[f32], v: f32) -> f32 {
    let f = v * FLOAT_STEPS as f32;
    let i = f as usize;
    table[i] + (table[i + 1] - table[i]) * (f - i as f32)
}

/// Combined per-pixel transform: white balance and exposure folded into one
/// 3×3 matrix (both are linear operations in linear light).
pub fn pixel_matrix(adj: &Adjustments, baseline_exposure_ev: f32) -> Mat3 {
    let gain = 2f32.powf(adj.exposure + baseline_exposure_ev);
    let mut m = color::white_balance_matrix(adj.temperature, adj.tint);
    if let Some(cal) = calibration_matrix(&adj.calibration) {
        m = color::mul(&cal, &m);
    }
    m.iter_mut().flatten().for_each(|v| *v *= gain);
    m
}

/// Lightroom's Calibration sliders as a colour matrix: each primary is nudged
/// towards a neighbour (hue) and towards or away from grey (saturation), and
/// white is kept white. This follows the direction and rough strength of
/// Lightroom's sliders; it is not Adobe's formula.
fn calibration_matrix(c: &Calibration) -> Option<Mat3> {
    if *c == Calibration::default() {
        return None;
    }
    let settings = [(c.red_hue, c.red_saturation), (c.green_hue, c.green_saturation), (c.blue_hue, c.blue_saturation)];
    let mut m = [[0.0f32; 3]; 3];
    for (i, (hue, saturation)) in settings.iter().enumerate() {
        // Positive hue: red → orange, green → cyan, blue → purple. Negative goes the other way.
        let mut primary = [0.0f32; 3];
        primary[i] = 1.0;
        primary[(i + 1) % 3] = (hue / 100.0).max(0.0) * 0.3;
        primary[(i + 2) % 3] = (-hue / 100.0).max(0.0) * 0.3;
        let total: f32 = primary.iter().sum();
        let strength = 1.0 + 0.5 * saturation / 100.0;
        for (row, value) in primary.iter().enumerate() {
            m[row][i] = 1.0 / 3.0 + (value / total - 1.0 / 3.0) * strength;
        }
    }
    for row in &mut m {
        let total: f32 = row.iter().sum();
        row.iter_mut().for_each(|v| *v /= total);
    }
    Some(m)
}

#[inline(always)]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// [`smoothstep`] with the edge given as a start and 1 ÷ width (no division per pixel).
#[inline(always)]
fn ramp(start: f32, inv_width: f32, x: f32) -> f32 {
    let t = ((x - start) * inv_width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Zero up to 0, then bending smoothly (over `width`) into a straight line
/// that rises one for one.
#[inline(always)]
fn knee(d: f32, width: f32) -> f32 {
    if d <= 0.0 {
        0.0
    } else if d < width {
        d * d / (2.0 * width)
    } else {
        d - 0.5 * width
    }
}

// ───────────────────────────────── tone ─────────────────────────────────

/// Stops (relative to middle grey) treated as display black and white by Contrast.
const X_BLACK: f32 = -7.0;
const X_WHITE: f32 = 2.474; // log2(1 / 0.18)

struct Tone {
    clarity: f32,
    shadows: f32,
    highlights: f32,
    whites: f32,
    blacks: f32,
    /// Contrast as a slope at middle grey (1 = unchanged).
    slope: f32,
}

impl Tone {
    fn new(a: &Adjustments) -> Option<Self> {
        Self::from_sliders(a.clarity, a.shadows, a.highlights, a.whites, a.blacks, a.contrast)
    }

    /// Each slider is −100 … +100.
    fn from_sliders(clarity: f32, shadows: f32, highlights: f32, whites: f32, blacks: f32, contrast: f32) -> Option<Self> {
        let t = Tone {
            clarity: clarity / 100.0,
            shadows: shadows / 100.0,
            // At −100 the bright tones keep 15 % of their spread (never none,
            // so a brighter tone never comes out darker than a dimmer one).
            highlights: highlights / 100.0 * if highlights < 0.0 { 0.85 } else { 0.8 },
            whites: whites / 100.0 * 0.8,
            blacks: blacks / 100.0,
            slope: 2f32.powf(contrast / 100.0 * 0.8),
        };
        let active = [t.clarity, t.shadows, t.highlights, t.whites, t.blacks].iter().any(|v| *v != 0.0) || contrast != 0.0;
        active.then_some(t)
    }

    fn needs_surroundings(&self) -> bool {
        self.clarity != 0.0 || self.shadows != 0.0 || self.highlights != 0.0
    }

    /// New brightness (stops) for a pixel at `x` stops whose surroundings are at `xb` stops.
    #[inline]
    fn map(&self, x: f32, xb: f32) -> f32 {
        let mut o = x;
        if self.clarity != 0.0 {
            // Stretch the difference from the surroundings, mostly in the mid-tones.
            let mid = ramp(-7.0, 1.0 / 3.5, xb) * (1.0 - ramp(1.0, 1.0 / 1.6, xb));
            o += self.clarity * 0.8 * (x - xb) * mid;
        }
        if self.shadows != 0.0 || self.highlights != 0.0 {
            // Decide "is this a shadow / a highlight?" mostly from the surroundings,
            // so fine detail inside a shadow is lifted together and keeps its contrast.
            let m = x + (xb - x) * 0.7;
            let lift = self.shadows * 2.2 * (1.0 - ramp(-4.5, 1.0 / 5.5, m));
            o += lift;
            // Highlights squeezes (or stretches) the spread of everything
            // brighter than half a stop under middle grey. The further above
            // that a tone is, the further it moves, so a blown-out sky comes
            // down a long way while the mid-tones barely change.
            o += self.highlights * knee(m + lift + 0.5, 1.0);
        }
        if self.whites != 0.0 {
            // Whites does the same from middle grey up, bending in more slowly:
            // it mostly moves the very brightest tones.
            o += self.whites * knee(o, 1.5);
        }
        if self.blacks != 0.0 {
            o += self.blacks * 1.4 * (1.0 - ramp(-6.0, 1.0 / 5.0, o));
        }
        if self.slope != 1.0 {
            // S-curve between display black and white, pivoting on middle grey.
            const SPAN: f32 = X_WHITE - X_BLACK;
            const PIVOT: f32 = -X_BLACK / SPAN;
            let u = (o - X_BLACK) * (1.0 / SPAN);
            if u > 1e-6 && u < 1.0 - 1e-6 {
                let v = if u < PIVOT {
                    PIVOT * fast_pow(u * (1.0 / PIVOT), self.slope)
                } else {
                    1.0 - (1.0 - PIVOT) * fast_pow((1.0 - u) * (1.0 / (1.0 - PIVOT)), self.slope)
                };
                o = X_BLACK + v * SPAN;
            }
        }
        o
    }
}

// ─────────────────────────────── vignette ───────────────────────────────

struct Vignette {
    stops: f32,
    start: f32,
    inv_width: f32,
    /// Exponent of the distance measure: 2 = ellipse, larger = follows the frame.
    power: f32,
    /// Scale applied to the longer axis so that +roundness tends to a circle.
    squeeze: (f32, f32),
}

impl Vignette {
    fn new(a: &Adjustments, crop_px: (f32, f32)) -> Option<Self> {
        let v = a.vignette;
        if v.amount == 0.0 {
            return None;
        }
        let centre = 0.35 + 0.9 * v.midpoint / 100.0;
        let half = 0.05 + 0.75 * v.feather / 100.0;
        let round = (v.roundness / 100.0).max(0.0);
        // On the longer side, distances grow by up to long ÷ short, which turns
        // the ellipse into a circle at roundness +100.
        let (w, h) = (crop_px.0.max(1.0), crop_px.1.max(1.0));
        let stretch = 1.0 + round * (w.max(h) / w.min(h) - 1.0);
        let squeeze = if w >= h { (stretch, 1.0) } else { (1.0, stretch) };
        Some(Vignette {
            stops: v.amount / 100.0 * if v.amount < 0.0 { 2.5 } else { 1.5 },
            start: centre - half,
            inv_width: 1.0 / (2.0 * half),
            power: 2.0 + (-v.roundness / 100.0).max(0.0) * 6.0,
            squeeze,
        })
    }

    #[inline]
    fn gain(&self, nx: f32, ny: f32) -> f32 {
        let (x, y) = ((nx * self.squeeze.0).abs(), (ny * self.squeeze.1).abs());
        let r = if self.power == 2.0 {
            (x * x + y * y).sqrt()
        } else {
            fast_pow(fast_pow(x.max(1e-6), self.power) + fast_pow(y.max(1e-6), self.power), 1.0 / self.power)
        };
        let mask = ramp(self.start, self.inv_width, r);
        if mask == 0.0 {
            1.0
        } else {
            fast_exp2(self.stops * mask)
        }
    }
}

// ───────────────────────────── display steps ─────────────────────────────

#[inline]
fn rgb_to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let d = mx - mn;
    if d < 1e-6 || mx <= 0.0 {
        return (0.0, 0.0, mx);
    }
    let inv = 1.0 / d;
    let h = if mx == c[0] {
        let h = (c[1] - c[2]) * inv;
        if h < 0.0 {
            h + 6.0
        } else {
            h
        }
    } else if mx == c[1] {
        (c[2] - c[0]) * inv + 2.0
    } else {
        (c[0] - c[1]) * inv + 4.0
    };
    (h * 60.0, d / mx, mx)
}

#[inline]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    // Hues arrive within one turn of 0…360, so one wrap either way is enough.
    let mut h6 = h * (1.0 / 60.0);
    if h6 < 0.0 {
        h6 += 6.0;
    } else if h6 >= 6.0 {
        h6 -= 6.0;
    }
    let i = h6 as i32;
    let f = h6 - i as f32;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Per-degree table of the Color Mixer: hue shift (degrees), saturation change,
/// luminance change and B&W brightness, each blended between neighbouring bands.
fn mixer_table(a: &Adjustments) -> Vec<[f32; 4]> {
    let n = BAND_HUES.len();
    let band = |k: usize| -> [f32; 4] {
        let b = a.mixer[k];
        let centre = BAND_HUES[k];
        let next = if k + 1 < n { BAND_HUES[k + 1] } else { 360.0 };
        let prev = if k > 0 { BAND_HUES[k - 1] } else { BAND_HUES[n - 1] - 360.0 };
        // Full slider travel moves a colour most of the way to its neighbour.
        let reach = if b.hue >= 0.0 { next - centre } else { centre - prev };
        [b.hue / 100.0 * 0.75 * reach, b.saturation / 100.0, b.luminance / 100.0, a.bw_mix[k] / 100.0]
    };
    (0..=360)
        .map(|deg| {
            let h = (deg % 360) as f32;
            let k = (0..n).rev().find(|&k| BAND_HUES[k] <= h).unwrap_or(0);
            let next = if k + 1 < n { BAND_HUES[k + 1] } else { 360.0 };
            let t = smoothstep(0.0, 1.0, (h - BAND_HUES[k]) / (next - BAND_HUES[k]));
            let (p, q) = (band(k), band((k + 1) % n));
            [0, 1, 2, 3].map(|i| p[i] + (q[i] - p[i]) * t)
        })
        .collect()
}

struct Grading {
    /// Colour pushed into shadows, mid-tones, highlights and everything.
    tint: [[f32; 3]; 4],
    lum: [f32; 4],
    start: f32,
    inv_width: f32,
}

impl Grading {
    fn new(a: &Adjustments) -> Option<Self> {
        let g = a.grading;
        let wheels = [g.shadows, g.midtones, g.highlights, g.global];
        if wheels.iter().all(|w| w.saturation == 0.0 && w.luminance == 0.0) {
            return None;
        }
        let tint = |w: Wheel| -> [f32; 3] {
            let c = hsv_to_rgb(w.hue, 1.0, 1.0);
            let l = luma(c);
            // A colour offset with no brightness of its own, so only the hue of the range moves.
            c.map(|v| (v - l) * w.saturation / 100.0 * 0.22)
        };
        Some(Grading {
            tint: wheels.map(tint),
            lum: wheels.map(|w| w.luminance / 100.0 * 0.25),
            start: 0.5 - g.balance / 100.0 * 0.3 - (0.08 + 0.42 * g.blending / 100.0),
            inv_width: 0.5 / (0.08 + 0.42 * g.blending / 100.0),
        })
    }

    #[inline]
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let hi = ramp(self.start, self.inv_width, luma(c));
        let lo = 1.0 - hi;
        let (ws, wm, wh) = (lo * lo, 2.0 * lo * hi, hi * hi);
        let (t, l) = (&self.tint, &self.lum);
        let shift = ws * l[0] + wm * l[1] + wh * l[2] + l[3];
        [
            c[0] + ws * t[0][0] + wm * t[1][0] + wh * t[2][0] + t[3][0] + shift,
            c[1] + ws * t[0][1] + wm * t[1][1] + wh * t[2][1] + t[3][1] + shift,
            c[2] + ws * t[0][2] + wm * t[1][2] + wh * t[2][2] + t[3][2] + shift,
        ]
    }
}

struct Grain {
    amplitude: f32,
    /// Grain cells per full-resolution pixel.
    per_px: f32,
    roughness: f32,
}

#[inline(always)]
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32 * 2.0 - 1.0
}

impl Grain {
    fn new(a: &Adjustments, view: &View) -> Option<Self> {
        let g = a.grain;
        if g.amount == 0.0 {
            return None;
        }
        // Cell size scales with the picture so grain looks the same at 12 and 48 MP.
        let long = view.crop_px.0.max(view.crop_px.1).max(1.0);
        let cell = (1.0 + 3.0 * g.size / 100.0) * (long / 4000.0).max(0.25);
        // A preview pixel averages several cells, which makes grain fainter; match that.
        let fade = (cell / view.px_scale.max(1.0)).min(1.0);
        Some(Grain { amplitude: g.amount / 100.0 * 0.14 * fade, per_px: 1.0 / cell, roughness: g.roughness / 100.0 })
    }

    #[inline]
    fn at(&self, px: f32, py: f32) -> f32 {
        let (gx, gy) = (px * self.per_px, py * self.per_px);
        let (ix, iy) = (gx as i32 - (gx < 0.0) as i32, gy as i32 - (gy < 0.0) as i32);
        let (tx, ty) = (gx - ix as f32, gy - iy as f32);
        let (h00, h10, h01, h11) = (hash(ix, iy), hash(ix + 1, iy), hash(ix, iy + 1), hash(ix + 1, iy + 1));
        let top = h00 + (h10 - h00) * tx;
        let bottom = h01 + (h11 - h01) * tx;
        let smooth = (top + (bottom - top) * ty) * 1.5;
        (smooth + (h00 - smooth) * self.roughness) * self.amplitude
    }
}

// ───────────────────────────── point colour ─────────────────────────────

/// One picked colour, ready to be compared with pixels.
struct PointPrep {
    hue: f32,
    sat: f32,
    lum: f32,
    /// A grey pick has no hue worth matching; it is matched on saturation and brightness alone.
    use_hue: bool,
    hue_width: f32,
    sat_width: f32,
    lum_width: f32,
    /// Degrees.
    hue_shift: f32,
    sat_change: f32,
    lum_change: f32,
}

impl PointPrep {
    fn new(p: &PointColor) -> Option<Self> {
        if !p.changes_something() {
            return None;
        }
        let reach = p.range / 100.0;
        Some(PointPrep {
            hue: p.hue,
            sat: p.saturation,
            lum: p.luminance,
            use_hue: p.saturation >= 0.08,
            hue_width: 12.0 + 48.0 * reach,
            sat_width: 0.15 + 0.5 * reach,
            lum_width: 0.35 + 0.65 * reach,
            hue_shift: p.hue_shift / 100.0 * 60.0,
            sat_change: p.sat_shift / 100.0,
            lum_change: p.lum_shift / 100.0 * 0.5,
        })
    }

    /// How much a pixel counts as "this colour": 1 at the picked colour,
    /// fading to 0 as hue, saturation or brightness move away from it.
    #[inline]
    fn weight(&self, h: f32, s: f32, l: f32) -> f32 {
        let near = |distance: f32, width: f32| 1.0 - ramp(width * 0.5, 2.0 / width, distance);
        let mut w = near((s - self.sat).abs(), self.sat_width) * near((l - self.lum).abs(), self.lum_width);
        if w > 0.0 && self.use_hue {
            let mut d = (h - self.hue).abs();
            if d > 180.0 {
                d = 360.0 - d;
            }
            // Greys have no real hue, so they are left alone.
            w *= near(d, self.hue_width) * ramp(0.0, 1.0 / 0.08, s);
        }
        w
    }
}

// ──────────────────────────────── masks ────────────────────────────────

/// One mask, ready for rendering: where it applies and what it does there.
struct MaskPrep {
    eval: MaskEval,
    /// Exposure change in stops.
    ev: f32,
    white_balance: Option<Mat3>,
    tone: Option<Tone>,
    saturation: f32,
}

impl MaskPrep {
    /// `None` for a mask whose sliders are all at rest.
    fn new(mask: &Mask, width: usize, height: usize) -> Option<Self> {
        let l = &mask.adjust;
        if *l == Default::default() {
            return None;
        }
        Some(MaskPrep {
            eval: MaskEval::new(mask, width, height),
            ev: l.exposure,
            white_balance: (l.temperature != 0.0 || l.tint != 0.0).then(|| color::white_balance_matrix(l.temperature, l.tint)),
            tone: Tone::from_sliders(l.clarity, l.shadows, l.highlights, l.whites, l.blacks, l.contrast),
            saturation: l.saturation / 100.0,
        })
    }
}

// ─────────────────────────────── the renderer ───────────────────────────────

struct Prepared<'a> {
    matrix: Mat3,
    /// Exposure + baseline exposure, in stops.
    gain_ev: f32,
    local: Option<&'a LocalMaps>,
    dehaze: f32,
    tone: Option<Tone>,
    profile: Option<&'a ToneCurve>,
    vignette: Option<Vignette>,
    /// Whether any step after sRGB encoding is active.
    display: bool,
    curves: Option<[Vec<f32>; 3]>,
    mixer: Option<Vec<[f32; 4]>>,
    bw: bool,
    saturation: f32,
    vibrance: f32,
    grading: Option<Grading>,
    lut: Option<(&'a Lut3D, f32)>,
    /// The look chosen from the Profile menu, and how strongly to apply it.
    style: Option<(std::sync::Arc<Lut3D>, f32)>,
    grain: Option<Grain>,
    points: Vec<PointPrep>,
    masks: Vec<MaskPrep>,
}

const CURVE_STEPS: usize = 1024;

impl<'a> Prepared<'a> {
    fn new(scene: &Scene<'a>, a: &Adjustments, view: &View) -> Self {
        let tone = Tone::new(a);
        let dehaze = a.dehaze / 100.0;
        let masks: Vec<MaskPrep> = a.masks.iter().filter_map(|m| MaskPrep::new(m, scene.image.width, scene.image.height)).collect();
        let points: Vec<PointPrep> = if a.bw { Vec::new() } else { a.point_colors.iter().filter_map(PointPrep::new).collect() };
        let wants_local = dehaze != 0.0
            || tone.as_ref().is_some_and(Tone::needs_surroundings)
            || masks.iter().any(|m| m.tone.as_ref().is_some_and(Tone::needs_surroundings));
        let local = if wants_local { scene.look.local.as_deref() } else { None };

        let curves = (!a.curve.is_identity()).then(|| {
            let straight: Vec<f32> = (0..CURVE_STEPS).map(|i| i as f32 / (CURVE_STEPS - 1) as f32).collect();
            // The parametric sliders bend the curve first, then the point curve is applied.
            let master = match (parametric_table(&a.curve.parametric, CURVE_STEPS), curve_table(&a.curve.rgb, CURVE_STEPS)) {
                (Some(p), Some(points)) => p.iter().map(|&v| read_table(&points, v)).collect(),
                (Some(p), None) => p,
                (None, Some(points)) => points,
                (None, None) => straight.clone(),
            };
            [&a.curve.red, &a.curve.green, &a.curve.blue].map(|ch| match curve_table(ch, CURVE_STEPS) {
                // Master curve first, then the channel's own curve.
                Some(t) => master.iter().map(|&v| read_table(&t, v)).collect(),
                None => master.clone(),
            })
        });
        let mixer_active = a.mixer.iter().any(|b| *b != Default::default()) || (a.bw && a.bw_mix.iter().any(|v| *v != 0.0));
        let grading = Grading::new(a);
        let lut = match (&a.lut, scene.lut) {
            (Some(l), Some(table)) if l.amount > 0.0 => Some((table, l.amount / 100.0)),
            _ => None,
        };
        let grain = Grain::new(a, view);
        let vignette = Vignette::new(a, view.crop_px);
        let style = match crate::profile::table(&a.profile.name) {
            Some(table) if a.profile.amount > 0.0 => Some((table, a.profile.amount / 100.0)),
            _ => None,
        };
        let display = curves.is_some()
            || !points.is_empty()
            || style.is_some()
            || mixer_active
            || a.bw
            || a.saturation != 0.0
            || a.vibrance != 0.0
            || grading.is_some()
            || lut.is_some()
            || grain.is_some();
        Prepared {
            matrix: pixel_matrix(a, scene.look.baseline_exposure),
            gain_ev: a.exposure + scene.look.baseline_exposure,
            local,
            dehaze: if local.is_some() { dehaze } else { 0.0 },
            tone,
            profile: scene.look.tone_curve.as_deref(),
            vignette,
            display,
            curves,
            mixer: mixer_active.then(|| mixer_table(a)),
            bw: a.bw,
            saturation: a.saturation / 100.0,
            vibrance: a.vibrance / 100.0,
            grading,
            lut,
            style,
            grain,
            points,
            masks,
        }
    }

    /// One pixel: `p` is the photo's linear colour, `(u, v)` its position in
    /// the photo (0…1) and `(nx, ny)` its position in the finished picture (−1…1).
    #[inline]
    fn shade(&self, p: [f32; 3], u: f32, v: f32, nx: f32, ny: f32, crop_px: (f32, f32)) -> [u8; 3] {
        if u < 0.0 {
            // An empty corner left by the Geometry warp: no photo here.
            return [0; 3];
        }
        let d = self.to_display(p, u, v, nx, ny);
        if !self.display {
            let lut = encode_lut();
            return d.map(|v| lut[(v * (LUT_SIZE - 1) as f32 + 0.5) as usize]);
        }
        self.finish(d, nx, ny, crop_px).map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    }

    /// The scene-referred steps: dehaze, white balance, exposure, tone, the
    /// camera's tone curve and the vignette. Returns linear light in 0…1.
    #[inline]
    fn to_display(&self, mut p: [f32; 3], u: f32, v: f32, nx: f32, ny: f32) -> [f32; 3] {
        let (ga, gb, dark) = match self.local {
            Some(m) => m.sample(u, v),
            None => (1.0, 0.0, 0.0),
        };
        if self.dehaze != 0.0 {
            p = self.remove_haze(p, dark);
        }
        let mut c = color::apply(&self.matrix, p);
        // How much brighter this pixel's surroundings are than the pixel, in stops.
        let mut surround = 0.0;
        if self.tone.is_some() || (self.local.is_some() && !self.masks.is_empty()) {
            let y = luma(c);
            if y > 1e-6 && y.is_finite() {
                let x = stops(y);
                // The helper maps describe the photo before exposure was applied.
                let xb = ga * x + gb + (1.0 - ga) * self.gain_ev;
                surround = xb - x;
                if let Some(tone) = &self.tone {
                    let k = fast_exp2(tone.map(x, xb) - x);
                    c = [c[0] * k, c[1] * k, c[2] * k];
                }
            }
        }
        if !self.masks.is_empty() {
            c = self.apply_masks(c, u, v, surround);
        }
        let mut d = match self.profile {
            Some(curve) => curve.apply_rgb(c),
            // NaN-safe: clamp maps NaN to NaN, so guard explicitly.
            None => c.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }),
        };
        if let Some(vig) = &self.vignette {
            let g = vig.gain(nx, ny);
            d = d.map(|v| (v * g).min(1.0));
        }
        d
    }

    /// Each mask's sliders, applied as strongly as the mask says at `(u, v)`.
    #[inline]
    fn apply_masks(&self, mut c: [f32; 3], u: f32, v: f32, surround: f32) -> [f32; 3] {
        for m in &self.masks {
            let w = m.eval.weight(u, v);
            if w <= 0.002 {
                continue;
            }
            if let Some(wb) = &m.white_balance {
                let t = color::apply(wb, c);
                c = [c[0] + (t[0] - c[0]) * w, c[1] + (t[1] - c[1]) * w, c[2] + (t[2] - c[2]) * w];
            }
            if m.ev != 0.0 {
                let k = fast_exp2(m.ev * w);
                c = [c[0] * k, c[1] * k, c[2] * k];
            }
            if let Some(tone) = &m.tone {
                let y = luma(c);
                if y > 1e-6 && y.is_finite() {
                    let x = stops(y);
                    let k = fast_exp2(w * (tone.map(x, x + surround) - x));
                    c = [c[0] * k, c[1] * k, c[2] * k];
                }
            }
            if m.saturation != 0.0 {
                let (l, f) = (luma(c), 1.0 + m.saturation * w);
                c = c.map(|v| (l + (v - l) * f).max(0.0));
            }
        }
        c
    }

    /// The first display steps: sRGB encoding, the profile's look and the
    /// curves. Point Color compares pixels with the picked colour at this stage.
    #[inline]
    fn encoded(&self, d: [f32; 3]) -> [f32; 3] {
        let table = encode_table();
        let mut g = d.map(|v| encode_f(table, v));
        if let Some((style, amount)) = &self.style {
            let o = style.sample(g);
            g = [0, 1, 2].map(|i| (g[i] + (o[i] - g[i]) * amount).clamp(0.0, 1.0));
        }
        if let Some(curves) = &self.curves {
            g = [read_table(&curves[0], g[0]), read_table(&curves[1], g[1]), read_table(&curves[2], g[2])];
        }
        g
    }

    /// Point Color: shift the pixels that are close to a picked colour.
    #[inline]
    fn shift_points(&self, mut g: [f32; 3]) -> [f32; 3] {
        let (h, s, v) = rgb_to_hsv(g);
        let l = luma(g);
        let (mut turn, mut sat, mut lum) = (0.0f32, 1.0f32, 1.0f32);
        for p in &self.points {
            let w = p.weight(h, s, l);
            if w > 0.0 {
                turn += w * p.hue_shift;
                sat *= 1.0 + w * p.sat_change;
                lum *= 1.0 + w * p.lum_change;
            }
        }
        if turn != 0.0 {
            g = hsv_to_rgb(h + turn.clamp(-180.0, 180.0), s, v);
        }
        if sat != 1.0 {
            let l = luma(g);
            g = g.map(|c| (l + (c - l) * sat).clamp(0.0, 1.0));
        }
        if lum != 1.0 {
            g = g.map(|c| (c * lum).clamp(0.0, 1.0));
        }
        g
    }

    /// The display steps, on sRGB-encoded values: profile look, curves,
    /// colour, look-up table, grain. Returns sRGB-encoded values (not yet clamped).
    #[inline]
    fn finish(&self, d: [f32; 3], nx: f32, ny: f32, crop_px: (f32, f32)) -> [f32; 3] {
        let mut g = self.encoded(d);
        if !self.points.is_empty() {
            g = self.shift_points(g);
        }
        let (mut h, mut s, mut val) = (0.0f32, 0.0f32, 0.0f32);
        let need_hsv = self.mixer.is_some() || self.vibrance != 0.0;
        if need_hsv {
            (h, s, val) = rgb_to_hsv(g);
        }
        let mut bw_gain = 1.0;
        if let Some(mixer) = &self.mixer {
            let i = (h as usize).min(359);
            let t = h - i as f32;
            let m = [0, 1, 2, 3].map(|k| mixer[i][k] + (mixer[i + 1][k] - mixer[i][k]) * t);
            if self.bw {
                bw_gain = 1.0 + m[3] * s * 0.7;
            } else {
                if m[0] != 0.0 {
                    g = hsv_to_rgb(h + m[0], s, val);
                }
                if m[1] != 0.0 {
                    // Towards (or away from) the colour's own brightness, so a
                    // desaturated blue ends up dark grey, not light grey.
                    let l = luma(g);
                    g = g.map(|v| (l + (v - l) * (1.0 + m[1])).clamp(0.0, 1.0));
                }
                if m[2] != 0.0 {
                    let k = 1.0 + m[2] * s * 0.6;
                    g = g.map(|v| (v * k).clamp(0.0, 1.0));
                }
            }
        }
        if self.bw {
            let l = (luma(g) * bw_gain).clamp(0.0, 1.0);
            g = [l; 3];
        } else if self.saturation != 0.0 || self.vibrance != 0.0 {
            let mut f = 1.0 + self.saturation;
            if self.vibrance != 0.0 {
                // Vibrance changes muted colours most, and goes easy on skin tones.
                let skin = 1.0 - 0.5 * (1.0 - ((h - 28.0).abs() / 25.0).min(1.0));
                let weight = if self.vibrance > 0.0 { (1.0 - s) * skin } else { 1.0 - s * 0.5 };
                f *= 1.0 + self.vibrance * weight;
            }
            let l = luma(g);
            g = g.map(|v| (l + (v - l) * f).clamp(0.0, 1.0));
        }
        if let Some(grading) = &self.grading {
            g = grading.apply(g).map(|v| v.clamp(0.0, 1.0));
        }
        if let Some((lut, amount)) = self.lut {
            let o = lut.sample(g);
            g = [0, 1, 2].map(|i| g[i] + (o[i] - g[i]) * amount);
        }
        if let Some(grain) = &self.grain {
            let n = grain.at((nx + 1.0) * 0.5 * crop_px.0, (ny + 1.0) * 0.5 * crop_px.1);
            // Grain shows most in the mid-tones, as on film.
            let l = luma(g).clamp(0.0, 1.0);
            let n = n * (0.35 + 0.65 * (4.0 * l * (1.0 - l)).sqrt());
            g = g.map(|v| v + n);
        }
        g
    }

    /// Dehaze: positive amounts subtract the estimated haze, negative amounts add some.
    #[inline]
    fn remove_haze(&self, p: [f32; 3], dark_nearby: f32) -> [f32; 3] {
        let air = self.local.map(|m| m.airlight).unwrap_or([1.0; 3]);
        if self.dehaze > 0.0 {
            let dark = p[0].min(p[1]).min(p[2]).min(dark_nearby).max(0.0);
            let air_level = (air[0] + air[1] + air[2]) / 3.0;
            let through = (1.0 - 0.9 * self.dehaze * (dark / air_level).min(1.0)).max(0.15);
            [0, 1, 2].map(|i| ((p[i] - air[i]) / through + air[i]).max(0.0))
        } else {
            let k = -self.dehaze * 0.45;
            [0, 1, 2].map(|i| p[i] + (air[i] - p[i]) * k)
        }
    }
}

#[inline]
fn read_table(table: &[f32], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * (table.len() - 1) as f32;
    let i = (f as usize).min(table.len() - 2);
    table[i] + (table[i + 1] - table[i]) * (f - i as f32)
}

#[inline]
fn bilinear(img: &LinearImage, sx: f32, sy: f32) -> [f32; 3] {
    let fx = (sx - 0.5).clamp(0.0, (img.width - 1) as f32);
    let fy = (sy - 0.5).clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let (a, b, c, d) = (img.pixels[y0 * img.width + x0], img.pixels[y0 * img.width + x1], img.pixels[y1 * img.width + x0], img.pixels[y1 * img.width + x1]);
    [0, 1, 2].map(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        top + (bottom - top) * ty
    })
}

/// What an edit does to a list of colours (linear in, sRGB-encoded 0…1 out),
/// with no photo behind them: used to bake a profile's look into a table.
pub(crate) fn shade_colours(adj: &Adjustments, colours: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let look = Look::neutral();
    let blank = LinearImage::new(1, 1, vec![[0.0; 3]]);
    let scene = Scene { image: &blank, look: &look, lut: None };
    let view = View::whole(1, 1);
    let adj = adj.clone().sanitized();
    let prep = Prepared::new(&scene, &adj, &view);
    colours
        .iter()
        .map(|c| prep.finish(prep.to_display(*c, 0.0, 0.0, 0.0, 0.0), 0.0, 0.0, view.crop_px).map(|v| v.clamp(0.0, 1.0)))
        .collect()
}

/// Reads the photo for one output pixel: applies the view's mapping (crop,
/// zoom) and the lens corrections.
struct Fetch<'a> {
    img: &'a LinearImage,
    to_source: Option<Affine>,
    /// The Geometry warp: the picture being rendered → the photo.
    warp: Option<Warp>,
    lens: Option<Lens>,
    healer: Option<Healer>,
    /// No warp, lens correction or healing: pixels can be read as they are.
    plain: bool,
    inv_w: f32,
    inv_h: f32,
}

impl Fetch<'_> {
    /// The photo's colour at `(x, y)` (in its pixels), with healing spots applied.
    #[inline]
    fn read(&self, x: f32, y: f32) -> [f32; 3] {
        let base = bilinear(self.img, x, y);
        match &self.healer {
            Some(healer) => healer.apply(self.img, x, y, base),
            None => base,
        }
    }

    /// The photo's colour for output pixel `(x, y)` and where in the photo it
    /// came from (0…1). `y` may lie above or below the output; the detail
    /// filters need a few such rows. Where the Geometry warp leaves an empty
    /// corner, the position's first number is −1.
    #[inline]
    fn at(&self, x: usize, y: isize) -> ([f32; 3], f32, f32) {
        let (mut sx, mut sy) = match &self.to_source {
            None if self.plain => {
                let row = y.clamp(0, self.img.height as isize - 1) as usize;
                return (self.img.pixels[row * self.img.width + x], (x as f32 + 0.5) * self.inv_w, (row as f32 + 0.5) * self.inv_h);
            }
            None => (x as f64 + 0.5, y as f64 + 0.5),
            Some(t) => t.apply(x as f64 + 0.5, y as f64 + 0.5),
        };
        if let Some(warp) = &self.warp {
            match warp.to_source(sx, sy) {
                Some(p) => (sx, sy) = p,
                None => {
                    // An empty corner. It is drawn black, but the detail filters
                    // look at neighbouring pixels, so give them the nearest bit
                    // of photo: otherwise sharpening would draw a bright line
                    // along the edge of the picture.
                    let near = warp.to_source_unbounded(sx, sy).map(|(x, y)| {
                        self.read(x.clamp(0.0, self.img.width as f64) as f32, y.clamp(0.0, self.img.height as f64) as f32)
                    });
                    return (near.unwrap_or([0.0; 3]), -1.0, 0.0);
                }
            }
        }
        let (sx, sy) = (sx as f32, sy as f32);
        let Some(lens) = &self.lens else {
            return (self.read(sx, sy), sx * self.inv_w, sy * self.inv_h);
        };
        let ([g, r, b], gain) = lens.map(sx, sy);
        let mut p = self.read(g.0, g.1);
        if lens.splits_colours() {
            // Red and blue are read from slightly different places to line them up with green.
            p[0] = self.read(r.0, r.1)[0];
            p[2] = self.read(b.0, b.1)[2];
        }
        ([p[0] * gain, p[1] * gain, p[2] * gain], g.0 * self.inv_w, g.1 * self.inv_h)
    }
}

/// The colour the photo shows at a point, for Point Color's picker: hue in
/// degrees, then saturation and brightness (0…1). `(u, v)` is the place in the
/// photo (0…1 across and down); a few pixels around it are averaged so a
/// single noisy pixel doesn't decide. It is measured where Point Color itself
/// works: after the Light sliders, the profile and the curves.
pub fn pick_colour(preview: &LinearImage, adj: &Adjustments, look: &Look, full_size: (usize, usize), u: f32, v: f32) -> [f32; 3] {
    let mut adj = adj.clone().sanitized();
    adj.point_colors.clear();
    let scene = Scene { image: preview, look, lut: None };
    let (w, h) = (preview.width, preview.height);
    let view = View::preview(&adj.crop, w, h, full_size.0, full_size.1);
    let prep = Prepared::new(&scene, &adj, &view);
    let (cx, cy) = ((u * w as f32) as isize, (v * h as f32) as isize);
    // A few pixels either side on a normal preview; none on a tiny image.
    let reach = (w.max(h) / 600) as isize;
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for y in (cy - reach).max(0)..=(cy + reach).min(h as isize - 1) {
        for x in (cx - reach).max(0)..=(cx + reach).min(w as isize - 1) {
            let p = preview.pixels[y as usize * w + x as usize];
            sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
            n += 1.0;
        }
    }
    if n == 0.0 {
        return [0.0, 0.0, 0.5];
    }
    let p = sum.map(|c| c / n);
    // Where this point sits in the finished picture (the vignette depends on it).
    let (px, py) = match Warp::new(&adj.geometry, w, h) {
        Some(warp) => warp.from_source((u * w as f32) as f64, (v * h as f32) as f64),
        None => ((u * w as f32) as f64, (v * h as f32) as f64),
    };
    let (nx, ny) = view.to_crop.apply(px, py);
    let g = prep.encoded(prep.to_display(p, u, v, nx as f32, ny as f32));
    let (hue, saturation, _) = rgb_to_hsv(g);
    [hue, saturation, luma(g).clamp(0.0, 1.0)]
}

/// Render `out_w × out_h` display-ready sRGB pixels.
pub fn render(scene: &Scene, adj: &Adjustments, view: &View, out_w: usize, out_h: usize) -> RenderedImage {
    let adj = adj.clone().sanitized();
    let img = scene.image;
    if view.to_source.is_none() {
        assert_eq!((out_w, out_h), (img.width, img.height), "a view without a mapping renders the image itself");
    }
    let prep = Prepared::new(scene, &adj, view);
    let warp = Warp::new(&adj.geometry, img.width, img.height);
    let lens = Lens::new(&adj, scene.look.chromatic, img.width, img.height);
    let healer = Healer::new(&adj.spots, img);
    let fetch = Fetch {
        img,
        to_source: view.to_source,
        plain: warp.is_none() && lens.is_none() && healer.is_none(),
        warp,
        lens,
        healer,
        inv_w: 1.0 / img.width as f32,
        inv_h: 1.0 / img.height as f32,
    };
    let detail = Detail::new(&adj, view.px_scale);
    let mut rgb = vec![0u8; out_w * out_h * 3];
    let width = out_w.max(1);
    let crop = &view.to_crop;
    // Position in the finished picture at the first pixel of a row, and the step per pixel.
    let crop_row = |y: usize| {
        let (nx0, ny0) = crop.apply(0.5, y as f64 + 0.5);
        (nx0 as f32, ny0 as f32, crop.a as f32, crop.d as f32)
    };

    let Some(detail) = detail else {
        rgb.par_chunks_mut(3 * width).enumerate().for_each(|(y, out_row)| {
            let (nx0, ny0, nxs, nys) = crop_row(y);
            for (x, out) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let (px, u, v) = fetch.at(x, y as isize);
                *out = prep.shade(px, u, v, nx0 + nxs * x as f32, ny0 + nys * x as f32, view.crop_px);
            }
        });
        return RenderedImage { width: out_w, height: out_h, rgb };
    };

    // The detail filters need neighbouring pixels, so the picture is worked on
    // in horizontal strips, each read with a few extra rows above and below.
    let strip_rows = (out_h / (rayon::current_num_threads() * 2).max(1)).clamp(16, 64);
    let margin = detail.margin;
    rgb.par_chunks_mut(3 * width * strip_rows).enumerate().for_each(|(index, out_strip)| {
        let y0 = index * strip_rows;
        let rows = out_strip.len() / (3 * width);
        let strip_h = rows + 2 * margin;
        let mut strip = Vec::with_capacity(width * strip_h);
        let mut places = Vec::with_capacity(width * rows);
        for sy in 0..strip_h {
            let y = y0 as isize + sy as isize - margin as isize;
            let inside = sy >= margin && sy < margin + rows;
            for x in 0..out_w {
                let (px, u, v) = fetch.at(x, y);
                strip.push(px);
                if inside {
                    places.push((u, v));
                }
            }
        }
        detail.apply(&mut strip, width, strip_h);
        for (row, out_row) in out_strip.chunks_mut(3 * width).enumerate() {
            let (nx0, ny0, nxs, nys) = crop_row(y0 + row);
            for (x, out) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let (u, v) = places[row * width + x];
                let px = strip[(margin + row) * width + x];
                *out = prep.shade(px, u, v, nx0 + nxs * x as f32, ny0 + nys * x as f32, view.crop_px);
            }
        }
    });
    RenderedImage { width: out_w, height: out_h, rgb }
}

/// Render a downscaled copy of the photo the way the app's preview does:
/// the whole frame, with everything scaled as it will be in the full-size
/// picture (`full_size` is the photo's real size in pixels).
pub fn render_preview(
    preview: &LinearImage,
    adj: &Adjustments,
    look: &Look,
    lut: Option<&Lut3D>,
    full_size: (usize, usize),
) -> RenderedImage {
    let scene = Scene { image: preview, look, lut };
    let view = View::preview(&adj.crop, preview.width, preview.height, full_size.0, full_size.1);
    render(&scene, adj, &view, preview.width, preview.height)
}

/// Render a whole image at full size with no crop and no look-up table.
/// For a downscaled copy use [`render_preview`], or sharpening and grain
/// would be sized for the wrong number of pixels.
pub fn render_srgb8(img: &LinearImage, adj: &Adjustments, look: &Look) -> RenderedImage {
    let mut adj = adj.clone();
    adj.crop = Crop::default();
    let scene = Scene { image: img, look, lut: None };
    render(&scene, &adj, &View::whole(img.width, img.height), img.width, img.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: f32) -> LinearImage {
        LinearImage::new(2, 2, vec![[v; 3]; 4])
    }

    fn one(rgb: [f32; 3], adj: &Adjustments) -> [u8; 3] {
        let r = render_srgb8(&LinearImage::new(1, 1, vec![rgb]), adj, &Look::neutral());
        [r.rgb[0], r.rgb[1], r.rgb[2]]
    }

    fn gray(v: f32, adj: &Adjustments) -> u8 {
        one([v; 3], adj)[0]
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
        // The same pixels through every display step must not panic either.
        let busy = Adjustments { contrast: 50.0, saturation: 30.0, vibrance: 30.0, shadows: 40.0, ..Default::default() };
        let r = render_srgb8(&img, &busy, &Look::neutral());
        assert_eq!(r.rgb.len(), 9);
    }

    #[test]
    fn the_files_tone_curve_is_applied_after_exposure() {
        use crate::look::ToneCurve;
        use std::sync::Arc;
        // Curve maps linear 0.09 to 0.18 (middle gray, sRGB 118).
        let curve = ToneCurve::from_points(&[(0.0, 0.0), (0.09, 0.18), (1.0, 1.0)]).unwrap();
        let look = Look { tone_curve: Some(Arc::new(curve)), ..Look::neutral() };
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
            // The float path used by the display steps must agree with it.
            let via_display = gray(v, &Adjustments { saturation: 1.0, ..Default::default() });
            assert!((via_display as i32 - exact).abs() <= 1, "display path v={v}");
        }
    }

    #[test]
    fn contrast_pivots_on_middle_gray() {
        let more = Adjustments { contrast: 60.0, ..Default::default() };
        let less = Adjustments { contrast: -60.0, ..Default::default() };
        assert_eq!(gray(0.18, &more), 118);
        assert!(gray(0.5, &more) > gray(0.5, &Adjustments::default()));
        assert!(gray(0.03, &more) < gray(0.03, &Adjustments::default()));
        assert!(gray(0.5, &less) < gray(0.5, &Adjustments::default()));
        assert!(gray(0.03, &less) > gray(0.03, &Adjustments::default()));
    }

    #[test]
    fn tone_sliders_act_on_their_own_range_and_keep_order() {
        let base = Adjustments::default();
        let shadows = Adjustments { shadows: 80.0, ..Default::default() };
        let highlights = Adjustments { highlights: -80.0, ..Default::default() };
        let whites = Adjustments { whites: 60.0, ..Default::default() };
        let blacks = Adjustments { blacks: -60.0, ..Default::default() };
        // Shadows lifts dark tones a lot and bright tones not at all.
        assert!(gray(0.01, &shadows) as i32 - gray(0.01, &base) as i32 > 15);
        assert_eq!(gray(0.7, &shadows), gray(0.7, &base));
        // Highlights pulls bright tones down and leaves dark tones alone.
        assert!((gray(0.8, &base) as i32) - (gray(0.8, &highlights) as i32) > 15);
        assert_eq!(gray(0.02, &highlights), gray(0.02, &base));
        // …and brings back detail above sensor white.
        assert!(gray(1.5, &highlights) < 255);
        assert!(gray(0.6, &whites) > gray(0.6, &base) && gray(0.01, &whites) == gray(0.01, &base));
        assert!(gray(0.01, &blacks) < gray(0.01, &base) && gray(0.5, &blacks) == gray(0.5, &base));
        // Brighter in never comes out darker.
        let all = Adjustments { shadows: 100.0, highlights: -100.0, whites: -100.0, blacks: 100.0, contrast: 100.0, ..Default::default() };
        let ramp: Vec<u8> = (1..200).map(|i| gray(i as f32 / 100.0, &all)).collect();
        assert!(ramp.windows(2).all(|w| w[1] >= w[0]), "{ramp:?}");
    }

    #[test]
    fn saturation_vibrance_and_bw() {
        let colour = [0.5, 0.2, 0.1];
        let base = one(colour, &Adjustments::default());
        let spread = |c: [u8; 3]| *c.iter().max().unwrap() as i32 - *c.iter().min().unwrap() as i32;
        let none = one(colour, &Adjustments { saturation: -100.0, ..Default::default() });
        assert_eq!(spread(none), 0);
        assert!(spread(one(colour, &Adjustments { saturation: 50.0, ..Default::default() })) > spread(base));
        assert!(spread(one(colour, &Adjustments { vibrance: 60.0, ..Default::default() })) > spread(base));
        assert_eq!(spread(one(colour, &Adjustments { bw: true, ..Default::default() })), 0);
        // Greys stay grey whatever the saturation.
        assert_eq!(spread(one([0.3; 3], &Adjustments { saturation: 100.0, vibrance: 100.0, ..Default::default() })), 0);
    }

    #[test]
    fn color_mixer_changes_only_its_own_colour() {
        let mut adj = Adjustments::default();
        adj.mixer[5].saturation = -100.0; // Blue
        let blue = one([0.05, 0.05, 0.6], &adj);
        let red = one([0.6, 0.08, 0.05], &adj);
        assert!(blue[2] as i32 - blue[0] as i32 <= 2, "blue should be grey now: {blue:?}");
        assert_eq!(red, one([0.6, 0.08, 0.05], &Adjustments::default()));
        // Luminance: darken blues only.
        let mut adj = Adjustments::default();
        adj.mixer[5].luminance = -80.0;
        assert!(one([0.05, 0.1, 0.6], &adj)[2] < one([0.05, 0.1, 0.6], &Adjustments::default())[2]);
        // B&W mix: make reds brighter in black & white.
        let mut adj = Adjustments { bw: true, ..Default::default() };
        let plain = one([0.6, 0.08, 0.05], &adj)[0];
        adj.bw_mix[0] = 100.0;
        assert!(one([0.6, 0.08, 0.05], &adj)[0] > plain);
    }

    #[test]
    fn point_curve_and_channel_curves() {
        let mut adj = Adjustments::default();
        adj.curve.rgb = vec![[0.0, 0.2], [1.0, 1.0]]; // lifted ("faded") blacks
        assert!(gray(0.0, &adj) >= 50);
        assert_eq!(gray(1.0, &adj), 255);
        let mut adj = Adjustments::default();
        adj.curve.blue = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let c = one([0.2; 3], &adj);
        assert!(c[2] > c[0] && c[0] == c[1], "{c:?}");
    }

    #[test]
    fn color_grading_tints_the_chosen_range() {
        let mut adj = Adjustments::default();
        adj.grading.shadows = Wheel { hue: 240.0, saturation: 80.0, luminance: 0.0 }; // blue shadows
        let dark = one([0.02; 3], &adj);
        let bright = one([0.9; 3], &adj);
        assert!(dark[2] > dark[0] + 5, "{dark:?}");
        assert!((bright[2] as i32 - bright[0] as i32).abs() <= 1, "{bright:?}");
    }

    #[test]
    fn a_lookup_table_is_applied_by_amount() {
        use crate::adjust::LutUse;
        let invert = Lut3D::identity(9).map(|c| c.map(|v| 1.0 - v));
        let img = LinearImage::new(1, 1, vec![[0.0; 3]]);
        let look = Look::neutral();
        let shade = |amount: f32| {
            let adj = Adjustments { lut: Some(LutUse { name: "x.cube".into(), amount }), ..Default::default() };
            let scene = Scene { image: &img, look: &look, lut: Some(&invert) };
            render(&scene, &adj, &View::whole(1, 1), 1, 1).rgb[0]
        };
        assert_eq!(shade(100.0), 255);
        assert_eq!(shade(0.0), 0);
        assert!((120..=135).contains(&shade(50.0)));
    }

    #[test]
    fn a_profile_is_applied_by_its_amount_and_under_the_sliders() {
        use crate::adjust::ProfileUse;
        let colour = [0.5, 0.2, 0.1];
        let spread = |c: [u8; 3]| *c.iter().max().unwrap() as i32 - *c.iter().min().unwrap() as i32;
        let with = |name: &str, amount: f32| Adjustments { profile: ProfileUse { name: name.into(), amount }, ..Default::default() };
        let plain = one(colour, &Adjustments::default());
        assert_eq!(one(colour, &with("vivid", 0.0)), plain);
        let (half, full, double) = (one(colour, &with("vivid", 50.0)), one(colour, &with("vivid", 100.0)), one(colour, &with("vivid", 200.0)));
        assert!(spread(plain) < spread(half) && spread(half) < spread(full) && spread(full) < spread(double));
        assert_eq!(spread(one(colour, &with("mono", 100.0))), 0);
        // An unknown profile name (say, from a newer version) is ignored, not an error.
        assert_eq!(one(colour, &with("from-the-future", 100.0)), plain);
        // The sliders still work on top: Saturation −100 over Vivid is grey.
        let grey = Adjustments { saturation: -100.0, ..with("vivid", 100.0) };
        assert_eq!(spread(one(colour, &grey)), 0);
    }

    #[test]
    fn vignette_darkens_corners_not_the_centre() {
        let img = LinearImage::new(101, 101, vec![[0.5; 3]; 101 * 101]);
        let mut adj = Adjustments::default();
        adj.vignette.amount = -60.0;
        let r = render_srgb8(&img, &adj, &Look::neutral());
        let at = |x: usize, y: usize| r.rgb[(y * 101 + x) * 3];
        assert_eq!(at(50, 50), render_srgb8(&img, &Adjustments::default(), &Look::neutral()).rgb[0]);
        assert!(at(0, 0) + 30 < at(50, 50));
        assert_eq!(at(0, 0), at(100, 100));
    }

    #[test]
    fn grain_adds_texture_without_changing_average_brightness() {
        let img = LinearImage::new(200, 200, vec![[0.2; 3]; 200 * 200]);
        let mut adj = Adjustments::default();
        adj.grain.amount = 60.0;
        let r = render_srgb8(&img, &adj, &Look::neutral());
        let plain = render_srgb8(&img, &Adjustments::default(), &Look::neutral()).rgb[0] as f64;
        let mean = r.rgb.iter().map(|v| *v as f64).sum::<f64>() / r.rgb.len() as f64;
        let distinct: std::collections::BTreeSet<u8> = r.rgb.iter().copied().collect();
        assert!((mean - plain).abs() < 1.5, "mean {mean} vs {plain}");
        assert!(distinct.len() > 10);
    }

    #[test]
    fn clarity_and_dehaze_use_the_helper_maps() {
        use std::sync::Arc;
        // Fine grey stripes on a hazy (lifted, low-contrast) background.
        let (w, h) = (240usize, 160usize);
        let px = (0..w * h).map(|i| if (i % w) / 4 % 2 == 0 { [0.30, 0.32, 0.36] } else { [0.22, 0.24, 0.28] }).collect();
        let img = LinearImage::new(w, h, px);
        let look = Look { local: Some(Arc::new(LocalMaps::compute(&img))), ..Look::neutral() };
        let contrast_of = |adj: &Adjustments| {
            let r = render_srgb8(&img, adj, &look);
            let at = |x: usize| r.rgb[(80 * w + x) * 3 + 1] as i32;
            at(121) - at(125)
        };
        let base = contrast_of(&Adjustments::default());
        assert!(contrast_of(&Adjustments { clarity: 80.0, ..Default::default() }) > base, "clarity adds local contrast");
        assert!(contrast_of(&Adjustments { clarity: -80.0, ..Default::default() }) < base);
        assert!(contrast_of(&Adjustments { dehaze: 70.0, ..Default::default() }) > base, "dehaze adds contrast");
        assert!(contrast_of(&Adjustments { dehaze: -70.0, ..Default::default() }) < base);
    }

    #[test]
    fn export_view_crops_turns_and_keeps_pixels_exact() {
        // 4×2 image; every pixel has its own brightness.
        let img = LinearImage::new(4, 2, (0..8).map(|i| [i as f32 / 10.0; 3]).collect());
        let look = Look::neutral();
        let scene = Scene { image: &img, look: &look, lut: None };
        // No sharpening here: this test is about pixels arriving unchanged.
        let mut unsharpened = Adjustments::default();
        unsharpened.sharpen.amount = 0.0;
        let plain = render_srgb8(&img, &unsharpened, &look);
        let level = |x: usize, y: usize| plain.rgb[(y * 4 + x) * 3];

        let mut adj = unsharpened.clone();
        adj.crop.rotate_cw();
        let (view, ow, oh) = View::export(&adj.crop, 4, 2);
        assert_eq!((ow, oh), (2, 4));
        let turned = render(&scene, &adj, &view, ow, oh);
        // Source top-left is now top-right; source bottom-left is top-left.
        assert_eq!(turned.rgb[3], level(0, 0));
        assert_eq!(turned.rgb[0], level(0, 1));

        let adj = Adjustments { crop: Crop { cx: 0.75, w: 0.5, ..Crop::default() }, ..unsharpened }; // right half
        let (view, ow, oh) = View::export(&adj.crop, 4, 2);
        assert_eq!((ow, oh), (2, 2));
        let cropped = render(&scene, &adj, &view, ow, oh);
        assert_eq!([cropped.rgb[0], cropped.rgb[3], cropped.rgb[6], cropped.rgb[9]], [level(2, 0), level(3, 0), level(2, 1), level(3, 1)]);
    }

    #[test]
    fn a_zoomed_in_region_matches_the_same_part_of_the_export() {
        // A varied 120 × 80 photo, cropped, with a vignette and the default sharpening.
        let px = (0..120 * 80).map(|i| [0.2 + 0.3 * ((i % 120) as f32 * 0.4).sin().abs(), 0.3, 0.1 + (i / 120) as f32 / 200.0]).collect();
        let img = LinearImage::new(120, 80, px);
        let look = Look::neutral();
        let scene = Scene { image: &img, look: &look, lut: None };
        let mut adj = Adjustments { crop: Crop { cx: 0.5, cy: 0.5, w: 0.8, h: 0.75, ..Crop::default() }, ..Default::default() };
        adj.vignette.amount = -50.0;
        let (view, ow, oh) = View::export(&adj.crop, 120, 80);
        let export = render(&scene, &adj, &view, ow, oh);
        assert_eq!((ow, oh), (96, 60));
        // The middle half of the finished picture, at one output pixel per photo pixel.
        let region = View::region(&adj.crop, 120, 80, [0.25, 0.25, 0.75, 0.75], 48, 30);
        assert!((region.px_scale - 1.0).abs() < 1e-4);
        let part = render(&scene, &adj, &region, 48, 30);
        // Away from the region's own border (where the filters see fewer neighbours) the pixels are the same.
        for y in 6..24 {
            for x in 6..42 {
                let a = &part.rgb[(y * 48 + x) * 3..][..3];
                let b = &export.rgb[((y + 15) * 96 + x + 24) * 3..][..3];
                assert!((0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= 1), "({x},{y}): {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn preview_and_export_put_the_vignette_in_the_same_place() {
        // Crop the right half and add a vignette: the centre of the *crop* stays untouched.
        let img = LinearImage::new(200, 100, vec![[0.5; 3]; 200 * 100]);
        let look = Look::neutral();
        let scene = Scene { image: &img, look: &look, lut: None };
        let mut adj = Adjustments { crop: Crop { cx: 0.75, w: 0.5, ..Crop::default() }, ..Default::default() };
        adj.vignette.amount = -80.0;
        let preview = render(&scene, &adj, &View::preview(&adj.crop, 200, 100, 200, 100), 200, 100);
        let (view, ow, oh) = View::export(&adj.crop, 200, 100);
        let export = render(&scene, &adj, &view, ow, oh);
        for (ex, ey) in [(50usize, 50usize), (2, 2), (97, 50), (20, 80)] {
            let p = preview.rgb[(ey * 200 + ex + 100) * 3] as i32;
            let e = export.rgb[(ey * ow + ex) * 3] as i32;
            assert!((p - e).abs() <= 1, "({ex},{ey}): preview {p} export {e}");
        }
    }

    // ---- masks, Point Color, Geometry and healing, through the renderer

    use crate::adjust::{Geometry, LocalAdjust, Mask, MaskShape, PointColor, Spot, SpotMode};

    fn grey_photo(w: usize, h: usize) -> LinearImage {
        LinearImage::new(w, h, vec![[0.18; 3]; w * h])
    }

    fn px(r: &RenderedImage, x: usize, y: usize) -> [u8; 3] {
        let i = (y * r.width + x) * 3;
        [r.rgb[i], r.rgb[i + 1], r.rgb[i + 2]]
    }

    #[test]
    fn a_mask_changes_only_its_own_part_of_the_photo() {
        let img = grey_photo(200, 100);
        // Darken the top by a stop, fading out between a quarter and half way down.
        let sky = Mask {
            shape: MaskShape::Linear { from: [0.5, 0.25], to: [0.5, 0.5] },
            invert: false,
            adjust: LocalAdjust { exposure: -1.0, ..Default::default() },
        };
        let adj = Adjustments { masks: vec![sky.clone()], ..Default::default() };
        let r = render_srgb8(&img, &adj, &Look::neutral());
        let plain = gray(0.18, &Adjustments::default());
        assert_eq!(px(&r, 100, 5)[0], gray(0.09, &Adjustments::default()), "full strength at the top");
        assert_eq!(px(&r, 100, 90)[0], plain, "untouched at the bottom");
        let middle = px(&r, 100, 37)[0];
        assert!(middle > px(&r, 100, 5)[0] && middle < plain, "fades in between: {middle}");
        // Inverted, it is the bottom that changes.
        let adj = Adjustments { masks: vec![Mask { invert: true, ..sky.clone() }], ..Default::default() };
        let r = render_srgb8(&img, &adj, &Look::neutral());
        assert_eq!((px(&r, 100, 5)[0], px(&r, 100, 90)[0]), (plain, gray(0.09, &Adjustments::default())));
        // A mask with no slider moved changes nothing at all.
        let idle = Adjustments { masks: vec![Mask { adjust: LocalAdjust::default(), ..sky }], ..Default::default() };
        assert_eq!(render_srgb8(&img, &idle, &Look::neutral()).rgb, render_srgb8(&img, &Adjustments::default(), &Look::neutral()).rgb);
    }

    #[test]
    fn mask_sliders_work_like_the_main_ones_inside_the_mask() {
        let img = LinearImage::new(1, 1, vec![[0.3, 0.2, 0.1]]);
        let everywhere = |adjust: LocalAdjust| Adjustments {
            masks: vec![Mask { shape: MaskShape::Radial { centre: [0.5, 0.5], radius: [3.0, 3.0], angle: 0.0, feather: 0.0 }, invert: false, adjust }],
            ..Default::default()
        };
        let look = Look::neutral();
        let render1 = |a: &Adjustments| px(&render_srgb8(&img, a, &look), 0, 0);
        // Exposure and white balance match the global sliders exactly.
        assert_eq!(render1(&everywhere(LocalAdjust { exposure: 0.7, ..Default::default() })), render1(&Adjustments { exposure: 0.7, ..Default::default() }));
        let warm = render1(&everywhere(LocalAdjust { temperature: 40.0, ..Default::default() }));
        assert_eq!(warm, render1(&Adjustments { temperature: 40.0, ..Default::default() }));
        // The tone sliders too (a single pixel has no surroundings to complicate things).
        let local = render1(&everywhere(LocalAdjust { contrast: 40.0, highlights: -30.0, shadows: 25.0, ..Default::default() }));
        let global = render1(&Adjustments { contrast: 40.0, highlights: -30.0, shadows: 25.0, ..Default::default() });
        assert_eq!(local, global);
        // Saturation: −100 leaves grey, and more spreads the channels apart.
        let none = render1(&everywhere(LocalAdjust { saturation: -100.0, ..Default::default() }));
        assert!(none[0] == none[1] && none[1] == none[2], "{none:?}");
        let more = render1(&everywhere(LocalAdjust { saturation: 60.0, ..Default::default() }));
        let base = render1(&Adjustments::default());
        assert!(more[0] as i32 - more[2] as i32 > base[0] as i32 - base[2] as i32);
    }

    #[test]
    fn point_color_changes_the_picked_colour_and_leaves_others_alone() {
        // Three stripes: orange, blue, grey.
        let (orange, blue, grey) = ([0.6, 0.25, 0.05], [0.05, 0.15, 0.6], [0.2, 0.2, 0.2]);
        let img = LinearImage::new(3, 1, vec![orange, blue, grey]);
        let look = Look::neutral();
        let base = render_srgb8(&img, &Adjustments::default(), &look);
        // Pick the orange stripe, as the app would.
        let [hue, saturation, luminance] = pick_colour(&img, &Adjustments::default(), &look, (3, 1), 0.17, 0.5);
        assert!((10.0..40.0).contains(&hue) && saturation > 0.5, "picked hue {hue}, saturation {saturation}");
        let pick = PointColor { hue, saturation, luminance, ..Default::default() };
        // Desaturate it fully: the orange goes grey, blue and grey stay exactly as they were.
        let adj = Adjustments { point_colors: vec![PointColor { sat_shift: -100.0, ..pick }], ..Default::default() };
        let r = render_srgb8(&img, &adj, &look);
        let o = px(&r, 0, 0);
        assert!(o[0] == o[1] && o[1] == o[2], "{o:?}");
        assert_eq!((px(&r, 1, 0), px(&r, 2, 0)), (px(&base, 1, 0), px(&base, 2, 0)));
        // Shift its hue: orange turns towards yellow-green; the others stay.
        let adj = Adjustments { point_colors: vec![PointColor { hue_shift: 80.0, ..pick }], ..Default::default() };
        let r = render_srgb8(&img, &adj, &look);
        let o = px(&r, 0, 0);
        assert!(o[1] as i32 > px(&base, 0, 0)[1] as i32 + 20, "more green than before: {o:?}");
        assert_eq!((px(&r, 1, 0), px(&r, 2, 0)), (px(&base, 1, 0), px(&base, 2, 0)));
        // Darken it.
        let adj = Adjustments { point_colors: vec![PointColor { lum_shift: -100.0, ..pick }], ..Default::default() };
        assert!(px(&render_srgb8(&img, &adj, &look), 0, 0)[0] < px(&base, 0, 0)[0] - 30);
        // A pick with nothing changed costs nothing and changes nothing.
        let adj = Adjustments { point_colors: vec![pick], ..Default::default() };
        assert_eq!(render_srgb8(&img, &adj, &look).rgb, base.rgb);
    }

    #[test]
    fn a_grey_pick_is_matched_by_brightness_not_hue() {
        let img = LinearImage::new(3, 1, vec![[0.2, 0.2, 0.2], [0.02, 0.02, 0.02], [0.6, 0.1, 0.1]]);
        let look = Look::neutral();
        let base = render_srgb8(&img, &Adjustments::default(), &look);
        let [hue, saturation, luminance] = pick_colour(&img, &Adjustments::default(), &look, (3, 1), 0.17, 0.5);
        assert!(saturation < 0.05);
        let adj = Adjustments { point_colors: vec![PointColor { hue, saturation, luminance, lum_shift: 100.0, range: 20.0, ..Default::default() }], ..Default::default() };
        let r = render_srgb8(&img, &adj, &look);
        assert!(px(&r, 0, 0)[0] > px(&base, 0, 0)[0] + 20, "the mid grey is lifted");
        assert_eq!(px(&r, 2, 0), px(&base, 2, 0), "the red is not");
    }

    #[test]
    fn geometry_moves_the_picture_and_leaves_empty_corners_black() {
        // Left half dark, right half bright.
        let (w, h) = (200usize, 100usize);
        let img = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { [0.05; 3] } else { [0.5; 3] }).collect());
        let look = Look::neutral();
        // Slide the picture right by a fifth of its width: the edge between the halves moves 40 px.
        let adj = Adjustments { geometry: Geometry { x_offset: 40.0, ..Default::default() }, ..Default::default() };
        let r = render_srgb8(&img, &adj, &look);
        let (dark, bright) = (gray(0.05, &Adjustments::default()), gray(0.5, &Adjustments::default()));
        assert_eq!((px(&r, 130, 50)[0], px(&r, 150, 50)[0]), (dark, bright));
        assert_eq!(px(&r, 10, 50), [0, 0, 0], "nothing to show where the picture moved away from");
        // Constrain zooms in instead of showing empty space.
        let adj = Adjustments { geometry: Geometry { rotate: 8.0, vertical: -40.0, constrain: true, ..Default::default() }, ..Default::default() };
        let bright_everywhere = LinearImage::new(w, h, vec![[0.5; 3]; w * h]);
        let r = render_srgb8(&bright_everywhere, &adj, &look);
        assert!(r.rgb.iter().all(|v| *v == bright), "an empty corner shows");
        // The same warp through the preview path and the export path gives the same picture.
        let adj = Adjustments { geometry: Geometry { vertical: 30.0, rotate: -5.0, ..Default::default() }, ..Default::default() };
        let whole = render_srgb8(&img, &adj, &look);
        let (view, ow, oh) = View::export(&adj.crop, w, h);
        let exported = render(&Scene { image: &img, look: &look, lut: None }, &adj, &view, ow, oh);
        assert_eq!(whole.rgb, exported.rgb);
    }

    #[test]
    fn a_mask_stays_on_the_same_part_of_the_photo_when_it_is_warped_or_cropped() {
        let (w, h) = (200usize, 100usize);
        let img = grey_photo(w, h);
        let look = Look::neutral();
        // Darken a disc around a point left of centre.
        let disc = Mask {
            shape: MaskShape::Radial { centre: [0.3, 0.5], radius: [0.08, 0.08], angle: 0.0, feather: 0.0 },
            invert: false,
            adjust: LocalAdjust { exposure: -2.0, ..Default::default() },
        };
        let plain = gray(0.18, &Adjustments::default());
        // Slide the picture right by 20 % of its width (40 px): the dark disc moves with it.
        let adj = Adjustments { masks: vec![disc.clone()], geometry: Geometry { x_offset: 40.0, ..Default::default() }, ..Default::default() };
        let r = render_srgb8(&img, &adj, &look);
        assert!(px(&r, 100, 50)[0] < plain - 40, "the disc should now be at x = 100");
        assert_eq!(px(&r, 60, 50)[0], plain, "and no longer at x = 60");
        // Crop to the right half: the disc is outside the picture and nothing is dark.
        let mut adj = Adjustments { masks: vec![disc], ..Default::default() };
        adj.crop = Crop { cx: 0.75, cy: 0.5, w: 0.5, h: 1.0, ..Crop::default() };
        let (view, ow, oh) = View::export(&adj.crop, w, h);
        let r = render(&Scene { image: &img, look: &look, lut: None }, &adj, &view, ow, oh);
        assert!(r.rgb.iter().all(|v| *v == plain));
    }

    #[test]
    fn healing_spots_are_applied_when_the_photo_is_rendered() {
        let (w, h) = (200usize, 100usize);
        let mut img = grey_photo(w, h);
        for y in 45..55 {
            for x in 55..65 {
                img.pixels[y * w + x] = [0.01; 3];
            }
        }
        let look = Look::neutral();
        let plain = gray(0.18, &Adjustments::default());
        assert!(px(&render_srgb8(&img, &Adjustments::default(), &look), 60, 50)[0] < 40);
        for mode in [SpotMode::Heal, SpotMode::Clone] {
            let spot = Spot { at: [0.3, 0.5], source: [0.6, 0.5], radius: 0.06, feather: 20.0, opacity: 100.0, mode };
            let adj = Adjustments { spots: vec![spot], ..Default::default() };
            let r = render_srgb8(&img, &adj, &look);
            for (x, y) in [(60, 50), (56, 46), (64, 54), (100, 50), (20, 20)] {
                assert!((px(&r, x, y)[0] as i32 - plain as i32).abs() <= 1, "{mode:?} at ({x}, {y}): {:?}", px(&r, x, y));
            }
            // The sharpening path reads the photo the same way.
            let sharp = Adjustments { sharpen: crate::adjust::Sharpen { amount: 80.0, ..Default::default() }, ..adj };
            let r = render_srgb8(&img, &sharp, &look);
            assert!((px(&r, 60, 50)[0] as i32 - plain as i32).abs() <= 2);
        }
    }
}
