//! Masks: where a local adjustment applies, as a strength from 0 to 1 for
//! every point of the photo.
//!
//! Positions are fractions of the photo (0 … 1 across and down). Distances
//! are measured in "longer sides": a circle of radius 0.1 is a real circle,
//! a tenth of the photo's longer side across, on a wide or a tall photo.

use std::sync::{Arc, Mutex};

use crate::adjust::{BrushStroke, Geometry, Mask, MaskShape};
use crate::perspective::{warped_to_photo, Warp};

/// Longer side, in pixels, of the picture a brush mask is painted into.
/// Brush edges are soft, so this is plenty even for a 48 MP photo.
const BRUSH_MAP_EDGE: usize = 1024;

#[inline(always)]
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A painted mask: one strength per pixel of a small picture of the photo.
#[derive(Debug)]
pub struct BrushMap {
    w: usize,
    h: usize,
    alpha: Vec<f32>,
}

impl BrushMap {
    #[inline]
    fn sample(&self, u: f32, v: f32) -> f32 {
        let fx = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let a = &self.alpha;
        let top = a[y0 * self.w + x0] + (a[y0 * self.w + x1] - a[y0 * self.w + x0]) * tx;
        let bottom = a[y1 * self.w + x0] + (a[y1 * self.w + x1] - a[y1 * self.w + x0]) * tx;
        top + (bottom - top) * ty
    }
}

fn map_size(photo_w: usize, photo_h: usize) -> (usize, usize) {
    let long = photo_w.max(photo_h).max(1) as f64;
    let k = BRUSH_MAP_EDGE as f64 / long;
    (((photo_w as f64 * k).round() as usize).max(1), ((photo_h as f64 * k).round() as usize).max(1))
}

/// Paint one stroke onto `alpha` (a `w × h` picture).
fn paint_stroke(alpha: &mut [f32], w: usize, h: usize, stroke: &BrushStroke) {
    if stroke.points.is_empty() {
        return;
    }
    let long = w.max(h) as f32;
    let r = (stroke.size * long).max(0.75);
    // The soft part of the edge is never thinner than a pixel and a half, so a hard brush isn't jagged.
    let hard = (r * (1.0 - stroke.feather / 100.0)).min(r - 1.5).max(0.0);
    let inv_soft = 1.0 / (r - hard);
    let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * w as f32, p[1] * h as f32)).collect();
    // Where the whole stroke can reach.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &pts {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let bx0 = ((x0 - r).floor().max(0.0)) as usize;
    let by0 = ((y0 - r).floor().max(0.0)) as usize;
    let bx1 = (((x1 + r).ceil()).min(w as f32)).max(0.0) as usize;
    let by1 = (((y1 + r).ceil()).min(h as f32)).max(0.0) as usize;
    if bx1 <= bx0 || by1 <= by0 {
        return;
    }
    let (bw, bh) = (bx1 - bx0, by1 - by0);
    // The stroke on its own first, so overlapping parts of one stroke don't add up.
    let mut own = vec![0.0f32; bw * bh];
    let mut stamp = |a: (f32, f32), b: (f32, f32)| {
        let sx0 = ((a.0.min(b.0) - r).floor().max(bx0 as f32)) as usize;
        let sy0 = ((a.1.min(b.1) - r).floor().max(by0 as f32)) as usize;
        let sx1 = ((a.0.max(b.0) + r).ceil().min(bx1 as f32)).max(0.0) as usize;
        let sy1 = ((a.1.max(b.1) + r).ceil().min(by1 as f32)).max(0.0) as usize;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        for y in sy0..sy1 {
            let py = y as f32 + 0.5;
            for x in sx0..sx1 {
                let px = x as f32 + 0.5;
                let t = if len2 > 1e-9 { (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (px - (a.0 + dx * t), py - (a.1 + dy * t));
                let d = (ex * ex + ey * ey).sqrt();
                if d < r {
                    let v = 1.0 - smooth((d - hard) * inv_soft);
                    let slot = &mut own[(y - by0) * bw + (x - bx0)];
                    if v > *slot {
                        *slot = v;
                    }
                }
            }
        }
    };
    if pts.len() == 1 {
        stamp(pts[0], pts[0]);
    }
    for pair in pts.windows(2) {
        stamp(pair[0], pair[1]);
    }
    for y in 0..bh {
        let row = &mut alpha[(by0 + y) * w + bx0..(by0 + y) * w + bx1];
        for (slot, v) in row.iter_mut().zip(&own[y * bw..(y + 1) * bw]) {
            *slot = if stroke.erase { *slot * (1.0 - v) } else { slot.max(*v) };
        }
    }
}

struct Painted {
    strokes: Vec<BrushStroke>,
    size: (usize, usize),
    /// The mask before its last stroke: while that stroke is being drawn only it is repainted.
    before_last: Arc<Vec<f32>>,
    map: Arc<BrushMap>,
}

/// Recently painted brush masks. Painting is the slow part, and the same mask
/// is asked for again on every preview while a slider is dragged.
static PAINTED: Mutex<Vec<Painted>> = Mutex::new(Vec::new());
const KEEP_PAINTED: usize = 6;

/// The painted picture for a brush mask on a photo of this shape.
pub fn brush_map(strokes: &[BrushStroke], photo_w: usize, photo_h: usize) -> Arc<BrushMap> {
    let size = map_size(photo_w, photo_h);
    let (w, h) = size;
    let mut cache = PAINTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = cache.iter().position(|p| p.size == size && p.strokes == strokes) {
        let hit = cache.remove(i);
        let map = hit.map.clone();
        cache.push(hit);
        return map;
    }
    let n = strokes.len();
    // Can an earlier result be continued? Either its last stroke grew, or one stroke was added.
    let mut start: Option<(Arc<Vec<f32>>, usize)> = None;
    if n > 0 {
        for p in cache.iter().rev().filter(|p| p.size == size) {
            if p.strokes.len() == n && p.strokes[..n - 1] == strokes[..n - 1] {
                start = Some((p.before_last.clone(), n - 1));
                break;
            }
            if p.strokes.len() + 1 == n && p.strokes[..] == strokes[..n - 1] {
                start = Some((Arc::new(p.map.alpha.clone()), n - 1));
                break;
            }
        }
    }
    let (mut alpha, from) = match start {
        Some((base, from)) => (base.as_ref().clone(), from),
        None => (vec![0.0f32; w * h], 0),
    };
    let mut before_last = Arc::new(alpha.clone());
    for (i, stroke) in strokes.iter().enumerate().skip(from) {
        if i + 1 == n {
            before_last = Arc::new(alpha.clone());
        }
        paint_stroke(&mut alpha, w, h, stroke);
    }
    let map = Arc::new(BrushMap { w, h, alpha });
    cache.push(Painted { strokes: strokes.to_vec(), size, before_last, map: map.clone() });
    if cache.len() > KEEP_PAINTED {
        cache.remove(0);
    }
    map
}

#[derive(Debug, Clone)]
enum Shape {
    /// `weight = 1 − smooth(dot(p − a, d))`, with `d` already divided by its length².
    Linear { a: (f32, f32), d: (f32, f32) },
    Radial { c: (f32, f32), cos: f32, sin: f32, inv_r: (f32, f32), inner: f32, inv_soft: f32 },
    Brush(Arc<BrushMap>),
}

/// A mask made ready for use: ask it for the strength at any point.
#[derive(Debug, Clone)]
pub struct MaskEval {
    shape: Shape,
    invert: bool,
    /// Photo fractions → "longer sides".
    kx: f32,
    ky: f32,
}

impl MaskEval {
    /// `photo_w × photo_h` is the photo's size; only its shape matters.
    pub fn new(mask: &Mask, photo_w: usize, photo_h: usize) -> Self {
        let long = photo_w.max(photo_h).max(1) as f32;
        let (kx, ky) = (photo_w as f32 / long, photo_h as f32 / long);
        let shape = match &mask.shape {
            MaskShape::Linear { from, to } => {
                let a = (from[0] * kx, from[1] * ky);
                let (dx, dy) = (to[0] * kx - a.0, to[1] * ky - a.1);
                // A gradient with no length is a sharp line; give it a hair's width.
                let len2 = (dx * dx + dy * dy).max(1e-8);
                Shape::Linear { a, d: (dx / len2, dy / len2) }
            }
            MaskShape::Radial { centre, radius, angle, feather } => {
                let (sin, cos) = angle.to_radians().sin_cos();
                let inner = (1.0 - feather / 100.0).clamp(0.0, 0.98);
                Shape::Radial {
                    c: (centre[0] * kx, centre[1] * ky),
                    cos,
                    sin,
                    inv_r: (1.0 / radius[0].max(1e-4), 1.0 / radius[1].max(1e-4)),
                    inner,
                    inv_soft: 1.0 / (1.0 - inner),
                }
            }
            MaskShape::Brush { strokes } => Shape::Brush(brush_map(strokes, photo_w, photo_h)),
        };
        MaskEval { shape, invert: mask.invert, kx, ky }
    }

    /// Strength of the mask (0 … 1) at a point of the photo.
    #[inline]
    pub fn weight(&self, u: f32, v: f32) -> f32 {
        let w = match &self.shape {
            Shape::Linear { a, d } => {
                let t = (u * self.kx - a.0) * d.0 + (v * self.ky - a.1) * d.1;
                1.0 - smooth(t)
            }
            Shape::Radial { c, cos, sin, inv_r, inner, inv_soft } => {
                let (qx, qy) = (u * self.kx - c.0, v * self.ky - c.1);
                let (ex, ey) = ((cos * qx + sin * qy) * inv_r.0, (cos * qy - sin * qx) * inv_r.1);
                let d2 = ex * ex + ey * ey;
                if d2 >= 1.0 {
                    0.0
                } else {
                    1.0 - smooth((d2.sqrt() - inner) * inv_soft)
                }
            }
            Shape::Brush(map) => map.sample(u, v),
        };
        if self.invert {
            1.0 - w
        } else {
            w
        }
    }
}

/// A small picture of where a mask applies, for showing it over the photo:
/// one byte per pixel (0 = not at all, 255 = fully). It is laid out like the
/// preview the app draws, that is, after the Geometry warp. Returns
/// `(width, height, strengths)`.
pub fn overlay(mask: &Mask, geometry: &Geometry, photo: (usize, usize), long_edge: usize) -> (usize, usize, Vec<u8>) {
    let long = photo.0.max(photo.1).max(1) as f64;
    let k = long_edge.max(8) as f64 / long;
    let (w, h) = (((photo.0 as f64 * k).round() as usize).max(1), ((photo.1 as f64 * k).round() as usize).max(1));
    let eval = MaskEval::new(mask, photo.0, photo.1);
    let warp = Warp::new(geometry, photo.0, photo.1);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if let Some((pu, pv)) = warped_to_photo(warp.as_ref(), u, v) {
                out[y * w + x] = (eval.weight(pu as f32, pv as f32).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjust::LocalAdjust;

    fn mask(shape: MaskShape) -> Mask {
        Mask { shape, invert: false, adjust: LocalAdjust::default() }
    }

    #[test]
    fn a_linear_gradient_fades_from_its_start_to_its_end() {
        // Top of the sky fully affected, fading out by the middle of the photo.
        let m = MaskEval::new(&mask(MaskShape::Linear { from: [0.5, 0.2], to: [0.5, 0.5] }), 4000, 3000);
        assert_eq!(m.weight(0.1, 0.0), 1.0);
        assert_eq!(m.weight(0.9, 0.2), 1.0);
        assert!((m.weight(0.3, 0.35) - 0.5).abs() < 1e-3);
        assert!(m.weight(0.5, 0.5) < 1e-4);
        assert_eq!(m.weight(0.5, 0.9), 0.0);
        // Strength only ever falls along the gradient.
        let along: Vec<f32> = (0..=20).map(|i| m.weight(0.5, 0.2 + 0.3 * i as f32 / 20.0)).collect();
        assert!(along.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn a_radial_gradient_is_round_on_a_photo_that_is_not_square() {
        let shape = MaskShape::Radial { centre: [0.5, 0.5], radius: [0.2, 0.2], angle: 0.0, feather: 50.0 };
        let m = MaskEval::new(&mask(shape.clone()), 4000, 2000);
        assert_eq!(m.weight(0.5, 0.5), 1.0);
        // 0.1 of the longer side from the centre: across that is 0.1 of the width, down it is 0.2 of the height.
        let (across, down) = (m.weight(0.6, 0.5), m.weight(0.5, 0.7));
        assert!((across - down).abs() < 1e-4 && across == 1.0, "{across} {down}");
        let (across, down) = (m.weight(0.65, 0.5), m.weight(0.5, 0.8));
        assert!((across - down).abs() < 1e-4 && (across - 0.5).abs() < 1e-3, "{across} {down}");
        assert_eq!(m.weight(0.71, 0.5), 0.0);
        // Inverted: everything except the circle.
        let inv = MaskEval::new(&Mask { invert: true, ..mask(shape) }, 4000, 2000);
        assert_eq!((inv.weight(0.5, 0.5), inv.weight(0.9, 0.5)), (0.0, 1.0));
    }

    #[test]
    fn a_turned_ellipse_follows_its_angle() {
        let shape = MaskShape::Radial { centre: [0.5, 0.5], radius: [0.3, 0.1], angle: 90.0, feather: 0.0 };
        let m = MaskEval::new(&mask(shape), 1000, 1000);
        // Long axis now runs up and down.
        assert!(m.weight(0.5, 0.75) > 0.9);
        assert_eq!(m.weight(0.75, 0.5), 0.0);
    }

    #[test]
    fn brush_strokes_paint_and_erase() {
        let paint = BrushStroke { points: vec![[0.2, 0.5], [0.8, 0.5]], size: 0.05, feather: 50.0, erase: false };
        let rub = BrushStroke { points: vec![[0.5, 0.3], [0.5, 0.7]], size: 0.05, feather: 0.0, erase: true };
        let m = MaskEval::new(&mask(MaskShape::Brush { strokes: vec![paint.clone()] }), 3000, 2000);
        assert!(m.weight(0.3, 0.5) > 0.99, "on the stroke");
        assert!(m.weight(0.3, 0.5 + 0.05 * 1.5 * 0.75).abs() > 0.2, "inside the soft edge");
        assert_eq!(m.weight(0.3, 0.7), 0.0);
        assert_eq!(m.weight(0.95, 0.5), 0.0);
        let m = MaskEval::new(&mask(MaskShape::Brush { strokes: vec![paint, rub] }), 3000, 2000);
        assert!(m.weight(0.3, 0.5) > 0.99);
        assert!(m.weight(0.5, 0.5) < 0.01, "rubbed out in the middle");
    }

    #[test]
    fn continuing_a_stroke_gives_the_same_mask_as_painting_it_in_one_go() {
        let stroke = |n: usize| BrushStroke {
            points: (0..n).map(|i| [0.1 + 0.02 * i as f32, 0.3 + 0.01 * i as f32]).collect(),
            size: 0.04,
            feather: 60.0,
            erase: false,
        };
        let first = BrushStroke { points: vec![[0.5, 0.1], [0.5, 0.9]], size: 0.03, feather: 30.0, erase: false };
        // As the app does it: the last stroke grows a point at a time.
        for n in 1..=30 {
            brush_map(&[first.clone(), stroke(n)], 1777, 1333);
        }
        let grown = brush_map(&[first.clone(), stroke(30)], 1777, 1333);
        let mut fresh = vec![0.0f32; grown.w * grown.h];
        paint_stroke(&mut fresh, grown.w, grown.h, &first);
        paint_stroke(&mut fresh, grown.w, grown.h, &stroke(30));
        assert_eq!(grown.alpha, fresh);
        assert!(fresh.iter().any(|v| *v > 0.99));
    }

    #[test]
    fn the_overlay_matches_the_mask() {
        let m = mask(MaskShape::Linear { from: [0.0, 0.5], to: [1.0, 0.5] });
        let (w, h, a) = overlay(&m, &Geometry::default(), (4000, 3000), 64);
        assert_eq!((w, h, a.len()), (64, 48, 64 * 48));
        assert!(a[24 * 64] > 250 && a[24 * 64 + 63] < 5);
        assert!((a[24 * 64 + 32] as i32 - 127).abs() < 8);
    }
}
