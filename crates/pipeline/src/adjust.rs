//! The edit a person makes to one photo.
//!
//! Every value is relative to the camera's own rendering, so
//! `Adjustments::default()` means "as shot". Slider scales follow Lightroom's
//! (mostly −100…+100) so imported presets map across one-to-one.

use serde::{Deserialize, Serialize};

use crate::geometry::Crop;

/// The eight colour bands of the Color Mixer, in slider order.
pub const BAND_NAMES: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
/// Centre hue of each band, in degrees.
pub const BAND_HUES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

pub const EXPOSURE_RANGE: (f32, f32) = (-5.0, 5.0);
pub const TEMPERATURE_RANGE: (f32, f32) = (-100.0, 100.0);
pub const TINT_RANGE: (f32, f32) = (-100.0, 100.0);
/// Most sliders: −100 … +100.
pub const PERCENT: (f32, f32) = (-100.0, 100.0);
/// One-sided sliders: 0 … 100.
pub const AMOUNT: (f32, f32) = (0.0, 100.0);

/// One Color Mixer band (each −100 … +100).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Band {
    pub hue: f32,
    pub saturation: f32,
    pub luminance: f32,
}

/// One Color Grading wheel.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    /// 0 … 360 degrees.
    pub hue: f32,
    /// 0 … 100.
    pub saturation: f32,
    /// −100 … +100.
    pub luminance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// 0 … 100: how much the three ranges overlap.
    pub blending: f32,
    /// −100 (favour shadows) … +100 (favour highlights).
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        let w = Wheel::default();
        Self { shadows: w, midtones: w, highlights: w, global: w, blending: 50.0, balance: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// −100 (dark corners) … +100 (light corners).
    pub amount: f32,
    /// 0 … 100: how far from the centre the effect starts.
    pub midpoint: f32,
    /// 0 … 100: how soft the transition is.
    pub feather: f32,
    /// −100 (follows the frame) … +100 (circular).
    pub roundness: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self { amount: 0.0, midpoint: 50.0, feather: 50.0, roundness: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    pub amount: f32,
    pub size: f32,
    pub roughness: f32,
}

impl Default for Grain {
    fn default() -> Self {
        Self { amount: 0.0, size: 25.0, roughness: 50.0 }
    }
}

/// The "parametric" curve: four sliders that each bend one part of the tone
/// range (each −100 … +100).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Parametric {
    pub highlights: f32,
    pub lights: f32,
    pub darks: f32,
    pub shadows: f32,
}

/// Point curves. Each is a list of `[input, output]` pairs in 0…1; an empty
/// list is a straight line (no change).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurves {
    pub rgb: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
    pub parametric: Parametric,
}

impl ToneCurves {
    pub fn is_identity(&self) -> bool {
        self.rgb.is_empty()
            && self.red.is_empty()
            && self.green.is_empty()
            && self.blue.is_empty()
            && self.parametric == Parametric::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpen {
    /// 0 … 150.
    pub amount: f32,
    /// 0.5 … 3.0 pixels.
    pub radius: f32,
    /// 0 … 100: how much very fine detail is sharpened too.
    pub detail: f32,
    /// 0 … 100: keep sharpening to edges, away from smooth areas.
    pub masking: f32,
}

/// Sharpening applied to a photo that hasn't been edited, as in Lightroom.
/// RAW data is slightly soft until it is sharpened; the camera's own JPEGs
/// are sharpened more than this.
pub const DEFAULT_SHARPENING: f32 = 40.0;

impl Default for Sharpen {
    fn default() -> Self {
        Self { amount: DEFAULT_SHARPENING, radius: 1.0, detail: 25.0, masking: 0.0 }
    }
}

pub const SHARPEN_AMOUNT: (f32, f32) = (0.0, 150.0);
pub const SHARPEN_RADIUS: (f32, f32) = (0.5, 3.0);
pub const DEFRINGE: (f32, f32) = (0.0, 20.0);

/// Noise reduction, each 0 … 100.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    pub luminance: f32,
    pub color: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Optics {
    /// Line up colour fringes caused by the lens.
    pub remove_ca: bool,
    /// −100 … +100; positive corrects barrel distortion.
    pub distortion: f32,
    /// −100 … +100; positive brightens the corners.
    pub vignetting: f32,
    /// 0 … 20.
    pub defringe_purple: f32,
    /// 0 … 20.
    pub defringe_green: f32,
}

/// Lightroom's "Calibration": shifts the camera's three primary colours.
/// Each value is −100 … +100. Mostly reached through imported presets.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub red_hue: f32,
    pub red_saturation: f32,
    pub green_hue: f32,
    pub green_saturation: f32,
    pub blue_hue: f32,
    pub blue_saturation: f32,
}

/// A colour look-up table (`.cube` file) applied on top of the edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LutUse {
    /// File name inside the app's `luts` folder.
    pub name: String,
    /// 0 … 100.
    #[serde(default = "hundred")]
    pub amount: f32,
}

fn hundred() -> f32 {
    100.0
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    // ---- Light
    /// Exposure change in stops (EV). +1 doubles the light.
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub curve: ToneCurves,
    // ---- Color
    /// White-balance temperature shift, −100 (cooler) … +100 (warmer).
    pub temperature: f32,
    /// White-balance tint shift, −100 (greener) … +100 (more magenta).
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    /// Black & white conversion.
    pub bw: bool,
    pub mixer: [Band; 8],
    /// Brightness of each colour band in black & white (−100 … +100).
    pub bw_mix: [f32; 8],
    pub grading: ColorGrading,
    pub calibration: Calibration,
    // ---- Effects
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub vignette: Vignette,
    pub grain: Grain,
    // ---- Detail
    pub sharpen: Sharpen,
    pub noise: NoiseReduction,
    // ---- Optics
    pub optics: Optics,
    // ---- Look
    pub lut: Option<LutUse>,
    // ---- Geometry
    pub crop: Crop,
}

fn fix(v: f32, (lo, hi): (f32, f32), default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

fn fix_points(points: &mut Vec<[f32; 2]>) {
    points.retain(|p| p[0].is_finite() && p[1].is_finite());
    for p in points.iter_mut() {
        *p = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)];
    }
    points.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
    points.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-4);
    points.truncate(32);
    let straight = points.len() == 2 && points[0] == [0.0, 0.0] && points[1] == [1.0, 1.0];
    if points.len() < 2 || straight {
        points.clear();
    }
}

impl Adjustments {
    /// Clamp every value into its slider range and replace NaN with the
    /// default, so a hand-edited or damaged edit file can't break rendering.
    pub fn sanitized(mut self) -> Self {
        let pct = |v: f32| fix(v, PERCENT, 0.0);
        self.exposure = fix(self.exposure, EXPOSURE_RANGE, 0.0);
        self.contrast = pct(self.contrast);
        self.highlights = pct(self.highlights);
        self.shadows = pct(self.shadows);
        self.whites = pct(self.whites);
        self.blacks = pct(self.blacks);
        for c in [&mut self.curve.rgb, &mut self.curve.red, &mut self.curve.green, &mut self.curve.blue] {
            fix_points(c);
        }
        self.temperature = fix(self.temperature, TEMPERATURE_RANGE, 0.0);
        self.tint = fix(self.tint, TINT_RANGE, 0.0);
        self.vibrance = pct(self.vibrance);
        self.saturation = pct(self.saturation);
        for b in &mut self.mixer {
            *b = Band { hue: pct(b.hue), saturation: pct(b.saturation), luminance: pct(b.luminance) };
        }
        for v in &mut self.bw_mix {
            *v = pct(*v);
        }
        let g = &mut self.grading;
        for w in [&mut g.shadows, &mut g.midtones, &mut g.highlights, &mut g.global] {
            w.hue = if w.hue.is_finite() { w.hue.rem_euclid(360.0) } else { 0.0 };
            w.saturation = fix(w.saturation, AMOUNT, 0.0);
            w.luminance = pct(w.luminance);
        }
        g.blending = fix(g.blending, AMOUNT, 50.0);
        g.balance = pct(g.balance);
        let p = &mut self.curve.parametric;
        *p = Parametric { highlights: pct(p.highlights), lights: pct(p.lights), darks: pct(p.darks), shadows: pct(p.shadows) };
        let c = &mut self.calibration;
        *c = Calibration {
            red_hue: pct(c.red_hue),
            red_saturation: pct(c.red_saturation),
            green_hue: pct(c.green_hue),
            green_saturation: pct(c.green_saturation),
            blue_hue: pct(c.blue_hue),
            blue_saturation: pct(c.blue_saturation),
        };
        self.texture = pct(self.texture);
        let s = &mut self.sharpen;
        *s = Sharpen {
            amount: fix(s.amount, SHARPEN_AMOUNT, DEFAULT_SHARPENING),
            radius: fix(s.radius, SHARPEN_RADIUS, 1.0),
            detail: fix(s.detail, AMOUNT, 25.0),
            masking: fix(s.masking, AMOUNT, 0.0),
        };
        self.noise = NoiseReduction { luminance: fix(self.noise.luminance, AMOUNT, 0.0), color: fix(self.noise.color, AMOUNT, 0.0) };
        let o = &mut self.optics;
        o.distortion = pct(o.distortion);
        o.vignetting = pct(o.vignetting);
        o.defringe_purple = fix(o.defringe_purple, DEFRINGE, 0.0);
        o.defringe_green = fix(o.defringe_green, DEFRINGE, 0.0);
        self.clarity = pct(self.clarity);
        self.dehaze = pct(self.dehaze);
        let v = &mut self.vignette;
        *v = Vignette {
            amount: pct(v.amount),
            midpoint: fix(v.midpoint, AMOUNT, 50.0),
            feather: fix(v.feather, AMOUNT, 50.0),
            roundness: pct(v.roundness),
        };
        let gr = &mut self.grain;
        *gr = Grain {
            amount: fix(gr.amount, AMOUNT, 0.0),
            size: fix(gr.size, AMOUNT, 25.0),
            roughness: fix(gr.roughness, AMOUNT, 50.0),
        };
        if let Some(l) = &mut self.lut {
            l.amount = fix(l.amount, AMOUNT, 100.0);
            if l.name.trim().is_empty() {
                self.lut = None;
            }
        }
        self.crop = self.crop.sanitized();
        self
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The same edit with the crop of `other` (used by presets and "Reset",
    /// which change the look but leave the framing alone).
    pub fn with_crop_of(mut self, other: &Adjustments) -> Self {
        self.crop = other.crop;
        self
    }
}

/// Sample the parametric curve into `n` evenly spaced values on 0…1.
/// Returns `None` when all four sliders are at zero.
pub fn parametric_table(p: &Parametric, n: usize) -> Option<Vec<f32>> {
    if *p == Parametric::default() {
        return None;
    }
    // Each slider lifts or lowers a soft bump centred on its part of the range.
    let regions = [(0.12, p.shadows), (0.37, p.darks), (0.63, p.lights), (0.88, p.highlights)];
    let mut table: Vec<f32> = (0..n)
        .map(|i| {
            let x = i as f32 / (n - 1) as f32;
            // Black and white stay where they are.
            let ends = (x / 0.05).min((1.0 - x) / 0.05).min(1.0);
            let shift: f32 = regions
                .iter()
                .map(|(centre, value)| {
                    let d = ((x - centre) / 0.3).abs().min(1.0);
                    value / 100.0 * 0.16 * (1.0 - d * d).powi(2)
                })
                .sum();
            (x + shift * ends).clamp(0.0, 1.0)
        })
        .collect();
    // Opposite sliders pushed hard could make the curve dip; never let it.
    for i in 1..n {
        table[i] = table[i].max(table[i - 1]);
    }
    Some(table)
}

/// Sample a point curve into `n` evenly spaced values on 0…1, using a smooth
/// curve that never overshoots between points (monotone cubic). Returns `None`
/// for an empty (straight-line) curve.
pub fn curve_table(points: &[[f32; 2]], n: usize) -> Option<Vec<f32>> {
    if points.len() < 2 {
        return None;
    }
    let k = points.len();
    let slope: Vec<f32> = points.windows(2).map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0]).max(1e-6)).collect();
    let mut tangent = vec![0.0f32; k];
    tangent[0] = slope[0];
    tangent[k - 1] = slope[k - 2];
    for i in 1..k - 1 {
        let (a, b) = (slope[i - 1], slope[i]);
        tangent[i] = if a * b <= 0.0 {
            0.0
        } else {
            let (h0, h1) = (points[i][0] - points[i - 1][0], points[i + 1][0] - points[i][0]);
            let (w1, w2) = (2.0 * h1 + h0, h1 + 2.0 * h0);
            (w1 + w2) / (w1 / a + w2 / b)
        };
    }
    let table = (0..n)
        .map(|i| {
            let x = i as f32 / (n - 1) as f32;
            if x <= points[0][0] {
                return points[0][1];
            }
            if x >= points[k - 1][0] {
                return points[k - 1][1];
            }
            let j = points.partition_point(|p| p[0] <= x).clamp(1, k - 1);
            let (p0, p1) = (points[j - 1], points[j]);
            let h = p1[0] - p0[0];
            let t = (x - p0[0]) / h;
            let (t2, t3) = (t * t, t * t * t);
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p0[1]
                + (t3 - 2.0 * t2 + t) * h * tangent[j - 1]
                + (-2.0 * t3 + 3.0 * t2) * p1[1]
                + (t3 - t2) * h * tangent[j];
            y.clamp(0.0, 1.0)
        })
        .collect();
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_and_drops_nan() {
        let a = Adjustments { exposure: 99.0, temperature: f32::NAN, tint: -500.0, contrast: 400.0, ..Default::default() }
            .sanitized();
        assert_eq!(a, Adjustments { exposure: 5.0, temperature: 0.0, tint: -100.0, contrast: 100.0, ..Default::default() });
    }

    #[test]
    fn straight_line_curves_count_as_no_curve() {
        let mut a = Adjustments::default();
        a.curve.rgb = vec![[0.0, 0.0], [1.0, 1.0]];
        a.curve.red = vec![[0.5, f32::NAN]];
        assert!(a.sanitized().is_default());
    }

    #[test]
    fn curve_table_passes_through_its_points_without_overshoot() {
        let pts = [[0.0, 0.1], [0.25, 0.2], [0.75, 0.9], [1.0, 0.9]];
        let t = curve_table(&pts, 1001).unwrap();
        assert!((t[0] - 0.1).abs() < 1e-4 && (t[250] - 0.2).abs() < 1e-3 && (t[750] - 0.9).abs() < 1e-3);
        assert!(t.windows(2).all(|w| w[1] >= w[0] - 1e-5), "must not dip between rising points");
        assert!(t.iter().all(|v| (0.1 - 1e-4..=0.9 + 1e-4).contains(v)));
        assert!(curve_table(&[], 16).is_none());
    }

    #[test]
    fn parametric_curve_bends_only_its_region_and_never_dips() {
        let lift = Parametric { shadows: 80.0, ..Default::default() };
        let t = parametric_table(&lift, 101).unwrap();
        assert!(t[12] > 0.12 + 0.08, "shadows lifted: {}", t[12]);
        assert!((t[75] - 0.75).abs() < 0.01 && t[0] == 0.0 && t[100] == 1.0);
        let wild = Parametric { shadows: 100.0, darks: -100.0, lights: 100.0, highlights: -100.0 };
        let t = parametric_table(&wild, 501).unwrap();
        assert!(t.windows(2).all(|w| w[1] >= w[0]));
        assert!(parametric_table(&Parametric::default(), 16).is_none());
    }

    #[test]
    fn old_edit_files_still_load() {
        let a: Adjustments = serde_json::from_str(r#"{"exposure":0.5,"temperature":10,"tint":-3}"#).unwrap();
        assert_eq!(a, Adjustments { exposure: 0.5, temperature: 10.0, tint: -3.0, ..Default::default() });
    }
}
