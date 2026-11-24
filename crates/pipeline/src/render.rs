//! The one rendering function used by both the on-screen preview and the
//! exported JPEG. Keeping a single code path is what makes preview and export
//! agree.
//!
//! Order of operations for each pixel:
//!
//! 0. Lens corrections and the detail sliders (Texture, Sharpening, Noise
//!    Reduction, Defringe), which need each pixel's neighbours.
//! 1. Dehaze (on the photo's own values).
//! 2. White balance + exposure (one 3×3 matrix).
//! 3. Tone: Clarity, Shadows, Highlights, Whites, Blacks, Contrast. These work
//!    on brightness in stops and scale the colour as a whole, so hue doesn't shift.
//! 4. The camera's own tone curve from the file (or a plain clip to 0…1).
//! 5. Vignette.
//! 6. sRGB encoding, then the "display" steps: point curves, Color Mixer,
//!    Vibrance/Saturation, B&W, Color Grading, LUT, Grain.
//!
//! These sliders are modelled on Lightroom's but the maths is our own, so the
//! same number gives a similar result, not an identical one.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::adjust::{curve_table, parametric_table, Adjustments, Calibration, Wheel, BAND_HUES};
use crate::detail::Detail;
use crate::lens::Lens;
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
        let t = Tone {
            clarity: a.clarity / 100.0,
            shadows: a.shadows / 100.0,
            highlights: a.highlights / 100.0,
            whites: a.whites / 100.0,
            blacks: a.blacks / 100.0,
            slope: 2f32.powf(a.contrast / 100.0 * 0.8),
        };
        let active = [t.clarity, t.shadows, t.highlights, t.whites, t.blacks].iter().any(|v| *v != 0.0) || a.contrast != 0.0;
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
            o += self.shadows * 2.2 * (1.0 - ramp(-4.5, 1.0 / 5.5, m));
            o += self.highlights * 1.6 * ramp(-0.5, 1.0 / 2.8, m);
        }
        if self.whites != 0.0 {
            o += self.whites * 0.9 * ramp(-1.0, 1.0 / (X_WHITE + 1.0), o);
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
    grain: Option<Grain>,
}

const CURVE_STEPS: usize = 1024;

impl<'a> Prepared<'a> {
    fn new(scene: &Scene<'a>, a: &Adjustments, view: &View) -> Self {
        let tone = Tone::new(a);
        let dehaze = a.dehaze / 100.0;
        let wants_local = dehaze != 0.0 || tone.as_ref().is_some_and(Tone::needs_surroundings);
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
        let display = curves.is_some()
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
            grain,
        }
    }

    /// One pixel: `p` is the photo's linear colour, `(u, v)` its position in
    /// the photo (0…1) and `(nx, ny)` its position in the finished picture (−1…1).
    #[inline]
    fn shade(&self, mut p: [f32; 3], u: f32, v: f32, nx: f32, ny: f32, crop_px: (f32, f32)) -> [u8; 3] {
        let (ga, gb, dark) = match self.local {
            Some(m) => m.sample(u, v),
            None => (1.0, 0.0, 0.0),
        };
        if self.dehaze != 0.0 {
            p = self.remove_haze(p, dark);
        }
        let mut c = color::apply(&self.matrix, p);
        if let Some(tone) = &self.tone {
            let y = luma(c);
            if y > 1e-6 && y.is_finite() {
                let x = stops(y);
                // The helper maps describe the photo before exposure was applied.
                let xb = ga * x + gb + (1.0 - ga) * self.gain_ev;
                let k = fast_exp2(tone.map(x, xb) - x);
                c = [c[0] * k, c[1] * k, c[2] * k];
            }
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
        if !self.display {
            let lut = encode_lut();
            return d.map(|v| lut[(v * (LUT_SIZE - 1) as f32 + 0.5) as usize]);
        }

        let table = encode_table();
        let mut g = d.map(|v| encode_f(table, v));
        if let Some(curves) = &self.curves {
            g = [read_table(&curves[0], g[0]), read_table(&curves[1], g[1]), read_table(&curves[2], g[2])];
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
        g.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
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

/// Reads the photo for one output pixel: applies the view's mapping (crop,
/// zoom) and the lens corrections.
struct Fetch<'a> {
    img: &'a LinearImage,
    to_source: Option<Affine>,
    lens: Option<Lens>,
    inv_w: f32,
    inv_h: f32,
}

impl Fetch<'_> {
    /// The photo's colour for output pixel `(x, y)` and where in the photo it
    /// came from (0…1). `y` may lie above or below the output; the detail
    /// filters need a few such rows.
    #[inline]
    fn at(&self, x: usize, y: isize) -> ([f32; 3], f32, f32) {
        let (sx, sy) = match &self.to_source {
            None if self.lens.is_none() => {
                let row = y.clamp(0, self.img.height as isize - 1) as usize;
                return (self.img.pixels[row * self.img.width + x], (x as f32 + 0.5) * self.inv_w, (row as f32 + 0.5) * self.inv_h);
            }
            None => (x as f32 + 0.5, y as f32 + 0.5),
            Some(t) => {
                let (sx, sy) = t.apply(x as f64 + 0.5, y as f64 + 0.5);
                (sx as f32, sy as f32)
            }
        };
        let Some(lens) = &self.lens else {
            return (bilinear(self.img, sx, sy), sx * self.inv_w, sy * self.inv_h);
        };
        let ([g, r, b], gain) = lens.map(sx, sy);
        let mut p = bilinear(self.img, g.0, g.1);
        if lens.splits_colours() {
            // Red and blue are read from slightly different places to line them up with green.
            p[0] = bilinear(self.img, r.0, r.1)[0];
            p[2] = bilinear(self.img, b.0, b.1)[2];
        }
        ([p[0] * gain, p[1] * gain, p[2] * gain], g.0 * self.inv_w, g.1 * self.inv_h)
    }
}

/// Render `out_w × out_h` display-ready sRGB pixels.
pub fn render(scene: &Scene, adj: &Adjustments, view: &View, out_w: usize, out_h: usize) -> RenderedImage {
    let adj = adj.clone().sanitized();
    let img = scene.image;
    if view.to_source.is_none() {
        assert_eq!((out_w, out_h), (img.width, img.height), "a view without a mapping renders the image itself");
    }
    let prep = Prepared::new(scene, &adj, view);
    let fetch = Fetch {
        img,
        to_source: view.to_source,
        lens: Lens::new(&adj, scene.look.chromatic, img.width, img.height),
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
}
