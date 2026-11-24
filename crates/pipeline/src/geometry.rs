//! Crop, straighten, rotate and flip.
//!
//! Nothing here touches pixels. A [`Crop`] describes which part of the photo
//! ends up in the picture; the renderer and the on-screen preview both ask it
//! for coordinate mappings.
//!
//! Three coordinate spaces are involved:
//!
//! * **source** — the decoded photo, in pixels.
//! * **frame** — the source after the quarter turns and the mirror flip.
//! * **unit** — the finished picture, (0,0) top-left to (1,1) bottom-right.

use serde::{Deserialize, Serialize};

/// 2-D affine map: `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 0.0, e: 1.0, f: 0.0 };

    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.b * y + self.c, self.d * x + self.e * y + self.f)
    }

    /// Recover the affine map that a (known to be affine) function computes.
    pub fn from_fn(f: impl Fn(f64, f64) -> (f64, f64)) -> Self {
        let (ox, oy) = f(0.0, 0.0);
        let (xx, xy) = f(1.0, 0.0);
        let (yx, yy) = f(0.0, 1.0);
        Affine { a: xx - ox, b: yx - ox, c: ox, d: xy - oy, e: yy - oy, f: oy }
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Affine { a: sx, b: 0.0, c: 0.0, d: 0.0, e: sy, f: 0.0 }
    }

    /// `self` first, then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        Affine::from_fn(|x, y| {
            let (u, v) = self.apply(x, y);
            next.apply(u, v)
        })
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.e - self.b * self.d;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (a, b, d, e) = (self.e / det, -self.b / det, -self.d / det, self.a / det);
        Some(Affine { a, b, c: -(a * self.c + b * self.f), d, e, f: -(d * self.c + e * self.f) })
    }
}

/// Smallest crop side, as a fraction of the frame.
const MIN_SIDE: f32 = 0.02;
pub const ANGLE_RANGE: (f32, f32) = (-45.0, 45.0);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    /// Clockwise quarter turns, 0…3.
    pub quarter_turns: u8,
    /// Mirror left-to-right (applied before the turns).
    pub flip: bool,
    /// Straighten angle in degrees; positive turns the picture clockwise.
    pub angle: f32,
    /// Centre of the crop in the frame, 0…1.
    pub cx: f32,
    pub cy: f32,
    /// Size of the crop as a fraction of the frame's width and height.
    pub w: f32,
    pub h: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Self { quarter_turns: 0, flip: false, angle: 0.0, cx: 0.5, cy: 0.5, w: 1.0, h: 1.0 }
    }
}

impl Crop {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    pub fn sanitized(mut self) -> Self {
        let ok = [self.angle, self.cx, self.cy, self.w, self.h].iter().all(|v| v.is_finite());
        if !ok {
            return Self { quarter_turns: self.quarter_turns % 4, flip: self.flip, ..Self::default() };
        }
        self.quarter_turns %= 4;
        self.angle = self.angle.clamp(ANGLE_RANGE.0, ANGLE_RANGE.1);
        self.w = self.w.clamp(MIN_SIDE, 1.0);
        self.h = self.h.clamp(MIN_SIDE, 1.0);
        self.cx = self.cx.clamp(0.0, 1.0);
        self.cy = self.cy.clamp(0.0, 1.0);
        self
    }

    /// Frame size for a source of `sw × sh` (the sides swap on odd turns).
    pub fn frame_size(&self, sw: f64, sh: f64) -> (f64, f64) {
        if self.quarter_turns % 2 == 1 {
            (sh, sw)
        } else {
            (sw, sh)
        }
    }

    /// Size of the finished picture in source pixels (not yet rounded).
    pub fn output_size_f(&self, sw: f64, sh: f64) -> (f64, f64) {
        let (fw, fh) = self.frame_size(sw, sh);
        (self.w as f64 * fw, self.h as f64 * fh)
    }

    /// Size of the finished picture in whole pixels.
    pub fn output_size(&self, sw: usize, sh: usize) -> (usize, usize) {
        let (w, h) = self.output_size_f(sw as f64, sh as f64);
        ((w.round() as usize).max(1), (h.round() as usize).max(1))
    }

    /// A point in the frame → the same point in the source.
    pub fn frame_to_source(&self, fx: f64, fy: f64, sw: f64, sh: f64) -> (f64, f64) {
        let (sx, sy) = match self.quarter_turns % 4 {
            0 => (fx, fy),
            1 => (fy, sh - fx),
            2 => (sw - fx, sh - fy),
            _ => (sw - fy, fx),
        };
        (if self.flip { sw - sx } else { sx }, sy)
    }

    /// Unit coordinates of the finished picture → source pixels.
    pub fn unit_to_source(&self, sw: f64, sh: f64) -> Affine {
        let (fw, fh) = self.frame_size(sw, sh);
        let (ow, oh) = self.output_size_f(sw, sh);
        // The crop window is turned the opposite way to the picture.
        let (sin, cos) = (-(self.angle as f64)).to_radians().sin_cos();
        let (cx, cy) = (self.cx as f64 * fw, self.cy as f64 * fh);
        Affine::from_fn(|s, t| {
            let (dx, dy) = ((s - 0.5) * ow, (t - 0.5) * oh);
            self.frame_to_source(cx + cos * dx - sin * dy, cy + sin * dx + cos * dy, sw, sh)
        })
    }

    /// Where the four corners of the finished picture sit in the source, as
    /// fractions of its width and height: top-left, top-right, bottom-right,
    /// bottom-left.
    pub fn corners_uv(&self, sw: f64, sh: f64) -> [[f32; 2]; 4] {
        let t = self.unit_to_source(sw, sh);
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(s, u)| {
            let (x, y) = t.apply(s, u);
            [(x / sw) as f32, (y / sh) as f32]
        })
    }

    /// Half-extent of the (turned) crop window along the frame's axes, in pixels.
    fn half_extent(&self, fw: f64, fh: f64) -> (f64, f64) {
        let (sin, cos) = (self.angle as f64).to_radians().sin_cos();
        let (ow, oh) = (self.w as f64 * fw, self.h as f64 * fh);
        ((cos.abs() * ow + sin.abs() * oh) / 2.0, (sin.abs() * ow + cos.abs() * oh) / 2.0)
    }

    /// True when the whole crop window lies inside the frame.
    pub fn is_inside(&self, sw: f64, sh: f64) -> bool {
        let (fw, fh) = self.frame_size(sw, sh);
        let (ex, ey) = self.half_extent(fw, fh);
        let (cx, cy) = (self.cx as f64 * fw, self.cy as f64 * fh);
        let tol = 0.01;
        self.w >= MIN_SIDE
            && self.h >= MIN_SIDE
            && cx - ex >= -tol
            && cx + ex <= fw + tol
            && cy - ey >= -tol
            && cy + ey <= fh + tol
    }

    /// Shrink and nudge the crop until it lies inside the frame, keeping its shape.
    pub fn fit(&mut self, sw: f64, sh: f64) {
        let (fw, fh) = self.frame_size(sw, sh);
        let (ex, ey) = self.half_extent(fw, fh);
        let scale = (fw / (2.0 * ex)).min(fh / (2.0 * ey)).min(1.0);
        self.w = (self.w as f64 * scale) as f32;
        self.h = (self.h as f64 * scale) as f32;
        let (ex, ey) = self.half_extent(fw, fh);
        let (ex, ey) = ((ex / fw).min(0.5), (ey / fh).min(0.5));
        self.cx = (self.cx as f64).clamp(ex, 1.0 - ex) as f32;
        self.cy = (self.cy as f64).clamp(ey, 1.0 - ey) as f32;
    }

    /// The largest crop with the given shape (`ratio` = width ÷ height of the
    /// finished picture), centred as near to the current centre as possible.
    pub fn set_aspect(&mut self, ratio: f64, sw: f64, sh: f64) {
        let (fw, fh) = self.frame_size(sw, sh);
        // Start far too large with the right shape, then let `fit` shrink it.
        let big = 4.0 * fw.max(fh);
        let (ow, oh) = if ratio >= 1.0 { (big, big / ratio) } else { (big * ratio, big) };
        self.w = (ow / fw) as f32;
        self.h = (oh / fh) as f32;
        self.fit(sw, sh);
    }

    /// Width ÷ height of the finished picture.
    pub fn aspect(&self, sw: f64, sh: f64) -> f64 {
        let (ow, oh) = self.output_size_f(sw, sh);
        ow / oh.max(1e-9)
    }

    pub fn rotate_cw(&mut self) {
        self.quarter_turns = (self.quarter_turns + 1) % 4;
        (self.cx, self.cy) = (1.0 - self.cy, self.cx);
        std::mem::swap(&mut self.w, &mut self.h);
    }

    pub fn rotate_ccw(&mut self) {
        self.quarter_turns = (self.quarter_turns + 3) % 4;
        (self.cx, self.cy) = (self.cy, 1.0 - self.cx);
        std::mem::swap(&mut self.w, &mut self.h);
    }

    /// Mirror the picture as it is currently shown, left to right.
    pub fn flip_horizontal(&mut self) {
        self.quarter_turns = (4 - self.quarter_turns % 4) % 4;
        self.flip = !self.flip;
        self.cx = 1.0 - self.cx;
        self.angle = -self.angle;
    }

    /// Mirror the picture as it is currently shown, top to bottom.
    pub fn flip_vertical(&mut self) {
        self.quarter_turns = (6 - self.quarter_turns % 4) % 4;
        self.flip = !self.flip;
        self.cy = 1.0 - self.cy;
        self.angle = -self.angle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which source pixel lands at output pixel (x, y) of a `sw × sh` source.
    fn lands(c: &Crop, sw: usize, sh: usize, x: usize, y: usize) -> (i64, i64) {
        let (ow, oh) = c.output_size(sw, sh);
        let t = c.unit_to_source(sw as f64, sh as f64);
        let (sx, sy) = t.apply((x as f64 + 0.5) / ow as f64, (y as f64 + 0.5) / oh as f64);
        ((sx - 0.5).round() as i64, (sy - 0.5).round() as i64)
    }

    #[test]
    fn default_is_identity() {
        let c = Crop::default();
        assert_eq!(c.output_size(30, 20), (30, 20));
        assert_eq!(lands(&c, 30, 20, 7, 3), (7, 3));
        assert!(c.is_inside(30.0, 20.0));
    }

    #[test]
    fn quarter_turn_clockwise_moves_top_left_to_top_right() {
        let mut c = Crop::default();
        c.rotate_cw();
        assert_eq!(c.output_size(30, 20), (20, 30));
        // Output top-right shows the source's top-left pixel.
        assert_eq!(lands(&c, 30, 20, 19, 0), (0, 0));
        // Output top-left shows the source's bottom-left pixel.
        assert_eq!(lands(&c, 30, 20, 0, 0), (0, 19));
    }

    #[test]
    fn turns_and_flips_compose_like_real_ones() {
        let (sw, sh) = (30usize, 20usize);
        let mut c = Crop::default();
        for _ in 0..4 {
            c.rotate_cw();
        }
        assert!(c.is_identity());
        c.rotate_cw();
        c.rotate_ccw();
        assert!(c.is_identity());
        c.flip_horizontal();
        assert_eq!(lands(&c, sw, sh, 0, 0), (29, 0));
        c.flip_horizontal();
        assert!(c.is_identity());
        c.flip_vertical();
        assert_eq!(lands(&c, sw, sh, 0, 0), (0, 19));
        c.flip_vertical();
        assert!(c.is_identity());
        // Turn clockwise, then mirror what is on screen left-to-right:
        // the screen's top-left was the turned picture's top-right = source top-left.
        c.rotate_cw();
        c.flip_horizontal();
        assert_eq!(lands(&c, sw, sh, 0, 0), (0, 0));
        assert_eq!(lands(&c, sw, sh, 19, 29), (29, 19));
    }

    #[test]
    fn crop_follows_the_picture_through_a_turn() {
        // Crop the left half, then turn clockwise: it becomes the top half.
        let mut c = Crop { cx: 0.25, w: 0.5, ..Crop::default() };
        c.rotate_cw();
        assert_eq!(c.output_size(40, 20), (20, 20));
        assert_eq!(lands(&c, 40, 20, 19, 0), (0, 0));
        assert_eq!(lands(&c, 40, 20, 0, 19), (19, 19));
    }

    #[test]
    fn straightening_keeps_the_crop_inside_the_photo() {
        let mut c = Crop { angle: 10.0, ..Crop::default() };
        assert!(!c.is_inside(4000.0, 3000.0));
        c.fit(4000.0, 3000.0);
        assert!(c.is_inside(4000.0, 3000.0));
        assert!((c.aspect(4000.0, 3000.0) - 4.0 / 3.0).abs() < 1e-3, "shape is kept");
        for [u, v] in c.corners_uv(4000.0, 3000.0) {
            assert!((-1e-4..=1.0001).contains(&u) && (-1e-4..=1.0001).contains(&v), "{u} {v}");
        }
    }

    #[test]
    fn set_aspect_gives_the_largest_crop_of_that_shape() {
        let mut c = Crop::default();
        c.set_aspect(1.0, 4000.0, 3000.0);
        assert_eq!(c.output_size(4000, 3000), (3000, 3000));
        c.set_aspect(16.0 / 9.0, 4000.0, 3000.0);
        assert_eq!(c.output_size(4000, 3000), (4000, 2250));
    }

    #[test]
    fn affine_inverse_round_trips() {
        let t = Crop { angle: 7.0, cx: 0.4, cy: 0.55, w: 0.5, h: 0.6, quarter_turns: 3, flip: true }
            .unit_to_source(400.0, 300.0);
        let inv = t.inverse().unwrap();
        let (x, y) = t.apply(0.3, 0.8);
        let (s, u) = inv.apply(x, y);
        assert!((s - 0.3).abs() < 1e-9 && (u - 0.8).abs() < 1e-9);
    }
}
