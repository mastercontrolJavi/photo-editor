//! Geometry: correcting perspective by hand (Lightroom's Vertical, Horizontal,
//! Rotate, Aspect, Scale and Offset sliders).
//!
//! The sliders describe a "projective" warp: the kind of change you get by
//! pointing the camera slightly differently. The warped picture has the same
//! size as the photo; the crop is then taken from the warped picture.
//!
//! Positions are in pixels of the image being rendered, so the same settings
//! give the same result on the preview and on the full-size photo.

use crate::adjust::Geometry;

type M3 = [[f64; 3]; 3];

/// How far Vertical/Horizontal ±100 tilt the imagined camera, in degrees.
const MAX_TILT: f64 = 25.0;
/// The imagined lens: focal length as a multiple of half the longer side
/// (1.6 is roughly a phone's main camera).
const FOCAL: f64 = 1.6;
/// The most "Constrain" will zoom in to hide empty corners.
const MAX_CONSTRAIN: f64 = 12.0;

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    o
}

fn invert(m: &M3) -> Option<M3> {
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-14 || !det.is_finite() {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [c(1, 1, 2, 2) * d, -c(0, 1, 2, 2) * d, c(0, 1, 1, 2) * d],
        [-c(1, 0, 2, 2) * d, c(0, 0, 2, 2) * d, -c(0, 0, 1, 2) * d],
        [c(1, 0, 2, 1) * d, -c(0, 0, 2, 1) * d, c(0, 0, 1, 1) * d],
    ])
}

#[inline]
fn project(m: &M3, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[2][0] * x + m[2][1] * y + m[2][2];
    if w <= 1e-9 {
        return None; // beyond the horizon of the tilted view
    }
    Some(((m[0][0] * x + m[0][1] * y + m[0][2]) / w, (m[1][0] * x + m[1][1] * y + m[1][2]) / w))
}

fn translate(x: f64, y: f64) -> M3 {
    [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]]
}

fn scale(x: f64, y: f64) -> M3 {
    [[x, 0.0, 0.0], [0.0, y, 0.0], [0.0, 0.0, 1.0]]
}

/// The warp for one image size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    to_source: M3,
    from_source: M3,
    w: f64,
    h: f64,
}

impl Warp {
    /// `None` when the Geometry sliders are all at rest.
    pub fn new(g: &Geometry, width: usize, height: usize) -> Option<Self> {
        if g.is_identity() || width == 0 || height == 0 {
            return None;
        }
        let (w, h) = (width as f64, height as f64);
        let norm = w.max(h) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let f = FOCAL;

        // Tilting the camera up/down and left/right.
        let (sv, cv) = (-(g.vertical as f64) / 100.0 * MAX_TILT).to_radians().sin_cos();
        let (sh, ch) = (-(g.horizontal as f64) / 100.0 * MAX_TILT).to_radians().sin_cos();
        let pitch: M3 = [[f, 0.0, 0.0], [0.0, f * cv, -f * f * sv], [0.0, sv, f * cv]];
        let yaw: M3 = [[f * ch, 0.0, -f * f * sh], [0.0, f, 0.0], [sh, 0.0, f * ch]];
        let mut m = mul(&yaw, &pitch);
        // Keep the middle of the photo in the middle, at about its own size.
        let (mx, my) = project(&m, 0.0, 0.0)?;
        m = mul(&translate(-mx, -my), &m);
        let e = 1e-4;
        let (ax, ay) = project(&m, e, 0.0)?;
        let (bx, by) = project(&m, 0.0, e)?;
        let area = ((ax * by - ay * bx) / (e * e)).abs();
        if !(area.is_finite() && area > 1e-9) {
            return None;
        }
        let keep = 1.0 / area.sqrt();
        m = mul(&scale(keep, keep), &m);

        let (sr, cr) = (g.rotate as f64).to_radians().sin_cos();
        let roll: M3 = [[cr, -sr, 0.0], [sr, cr, 0.0], [0.0, 0.0, 1.0]];
        let stretch = 2f64.powf(g.aspect as f64 / 100.0 * 0.5).sqrt();
        let zoom = (g.scale as f64 / 100.0).max(0.05);
        let shift = translate(g.x_offset as f64 / 100.0 * 0.5 * w / norm, -(g.y_offset as f64) / 100.0 * 0.5 * h / norm);
        for step in [roll, scale(1.0 / stretch, stretch), scale(zoom, zoom), shift] {
            m = mul(&step, &m);
        }

        // From "middle of the photo = 0, half the longer side = 1" to pixels and back.
        let to_unit: M3 = [[1.0 / norm, 0.0, -cx / norm], [0.0, 1.0 / norm, -cy / norm], [0.0, 0.0, 1.0]];
        let to_px: M3 = [[norm, 0.0, cx], [0.0, norm, cy], [0.0, 0.0, 1.0]];
        let from_source = mul(&to_px, &mul(&m, &to_unit));
        let mut to_source = invert(&from_source)?;

        if g.constrain {
            // Zoom in about the middle until all four corners of the frame show photo.
            let zoomed = |k: f64| mul(&to_source, &mul(&to_px, &mul(&scale(1.0 / k, 1.0 / k), &to_unit)));
            let covers = |k: f64| {
                let t = zoomed(k);
                [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().all(|&(x, y)| {
                    project(&t, x, y).is_some_and(|(sx, sy)| sx >= -0.01 && sy >= -0.01 && sx <= w + 0.01 && sy <= h + 0.01)
                })
            };
            if !covers(1.0) {
                let (mut lo, mut hi) = (1.0, MAX_CONSTRAIN);
                if covers(hi) {
                    for _ in 0..40 {
                        let mid = (lo + hi) / 2.0;
                        if covers(mid) {
                            hi = mid;
                        } else {
                            lo = mid;
                        }
                    }
                }
                to_source = zoomed(hi);
            }
        }
        let from_source = invert(&to_source)?;
        Some(Warp { to_source, from_source, w, h })
    }

    /// Where in the photo the warped picture's point `(x, y)` comes from, or
    /// `None` if that point shows no photo (an empty corner).
    #[inline]
    pub fn to_source(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (sx, sy) = project(&self.to_source, x, y)?;
        (sx >= 0.0 && sy >= 0.0 && sx <= self.w && sy <= self.h).then_some((sx, sy))
    }

    /// Like [`Warp::to_source`], but also answers for points that fall outside
    /// the photo (used for handles dragged past its edge).
    pub fn to_source_unbounded(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        project(&self.to_source, x, y)
    }

    /// Where a point of the photo ends up in the warped picture.
    pub fn from_source(&self, x: f64, y: f64) -> (f64, f64) {
        project(&self.from_source, x, y).unwrap_or((x, y))
    }
}

/// A point of the warped picture → the photo, both as fractions (0 … 1) of
/// the width and height. Works with or without a warp.
pub fn warped_to_photo(warp: Option<&Warp>, u: f64, v: f64) -> Option<(f64, f64)> {
    match warp {
        None => Some((u, v)),
        Some(w) => w.to_source(u * w.w, v * w.h).map(|(x, y)| (x / w.w, y / w.h)),
    }
}

/// A point of the photo → the warped picture, both as fractions (0 … 1).
pub fn photo_to_warped(warp: Option<&Warp>, u: f64, v: f64) -> (f64, f64) {
    match warp {
        None => (u, v),
        Some(w) => {
            let (x, y) = w.from_source(u * w.w, v * w.h);
            (x / w.w, y / w.h)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 4000;
    const H: usize = 3000;

    fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn sliders_at_rest_mean_no_warp() {
        assert!(Warp::new(&Geometry::default(), W, H).is_none());
        // "Constrain" on its own has nothing to do.
        assert!(Warp::new(&Geometry { constrain: true, ..Default::default() }, W, H).is_none());
    }

    #[test]
    fn there_and_back_again() {
        let g = Geometry { vertical: -40.0, horizontal: 25.0, rotate: 3.0, aspect: 20.0, scale: 90.0, x_offset: 5.0, y_offset: -8.0, constrain: false };
        let warp = Warp::new(&g, W, H).unwrap();
        for (x, y) in [(2000.0, 1500.0), (900.0, 700.0), (3100.0, 2200.0)] {
            let (wx, wy) = warp.from_source(x, y);
            let (bx, by) = project(&warp.to_source, wx, wy).unwrap();
            assert!(dist((x, y), (bx, by)) < 1e-6, "{x} {y} → {bx} {by}");
        }
    }

    #[test]
    fn negative_vertical_widens_the_top_and_keeps_the_middle() {
        let warp = Warp::new(&Geometry { vertical: -60.0, ..Default::default() }, W, H).unwrap();
        let top = dist(warp.from_source(1000.0, 300.0), warp.from_source(3000.0, 300.0));
        let bottom = dist(warp.from_source(1000.0, 2700.0), warp.from_source(3000.0, 2700.0));
        assert!(top > bottom * 1.2, "top {top} bottom {bottom}");
        assert!(dist(warp.from_source(2000.0, 1500.0), (2000.0, 1500.0)) < 1e-6);
        // Vertical lines that lean in at the top (a building seen from below) come out more upright.
        let (tx, _) = warp.from_source(1200.0, 300.0);
        let (bx, _) = warp.from_source(1000.0, 2700.0);
        assert!((tx - bx).abs() < 200.0 - 50.0, "lean went from 200 px to {}", (tx - bx).abs());
        // And the other way round for positive values and for Horizontal.
        let warp = Warp::new(&Geometry { horizontal: -60.0, ..Default::default() }, W, H).unwrap();
        let left = dist(warp.from_source(400.0, 500.0), warp.from_source(400.0, 2500.0));
        let right = dist(warp.from_source(3600.0, 500.0), warp.from_source(3600.0, 2500.0));
        assert!(left > right * 1.2, "left {left} right {right}");
    }

    #[test]
    fn rotate_scale_and_offset_move_the_picture_the_way_the_sliders_say() {
        // Clockwise: a point right of the middle moves down.
        let warp = Warp::new(&Geometry { rotate: 10.0, ..Default::default() }, W, H).unwrap();
        let (x, y) = warp.from_source(3000.0, 1500.0);
        assert!(y > 1500.0 + 100.0 && x < 3000.0, "{x} {y}");
        let warp = Warp::new(&Geometry { scale: 150.0, ..Default::default() }, W, H).unwrap();
        let (x, y) = warp.from_source(3000.0, 1500.0);
        assert!((x - 3500.0).abs() < 1e-6 && (y - 1500.0).abs() < 1e-6);
        let warp = Warp::new(&Geometry { x_offset: 20.0, y_offset: 20.0, ..Default::default() }, W, H).unwrap();
        let (x, y) = warp.from_source(2000.0, 1500.0);
        assert!((x - 2400.0).abs() < 1e-6 && (y - 1200.0).abs() < 1e-6, "{x} {y}");
        // Positive Aspect makes things taller and narrower.
        let warp = Warp::new(&Geometry { aspect: 100.0, ..Default::default() }, W, H).unwrap();
        let (x, y) = warp.from_source(3000.0, 2500.0);
        assert!(x < 3000.0 && y > 2500.0);
    }

    #[test]
    fn empty_corners_are_reported_and_constrain_removes_them() {
        let g = Geometry { vertical: -70.0, rotate: 6.0, ..Default::default() };
        let loose = Warp::new(&g, W, H).unwrap();
        let corners = [(0.5, 0.5), (W as f64 - 0.5, 0.5), (W as f64 - 0.5, H as f64 - 0.5), (0.5, H as f64 - 0.5)];
        assert!(corners.iter().any(|&(x, y)| loose.to_source(x, y).is_none()), "this warp should leave an empty corner");
        let tight = Warp::new(&Geometry { constrain: true, ..g }, W, H).unwrap();
        assert!(corners.iter().all(|&(x, y)| tight.to_source(x, y).is_some()));
        // It zooms in no further than needed: some corner sits right at the edge of the photo.
        let nearest = corners
            .iter()
            .map(|&(x, y)| {
                let (sx, sy) = tight.to_source(x, y).unwrap();
                sx.min(sy).min(W as f64 - sx).min(H as f64 - sy)
            })
            .fold(f64::MAX, f64::min);
        assert!(nearest < 3.0, "nearest corner is {nearest} px inside the photo");
    }

    #[test]
    fn the_same_settings_give_the_same_picture_at_any_size() {
        let g = Geometry { vertical: 35.0, horizontal: -20.0, rotate: -4.0, scale: 110.0, x_offset: 10.0, ..Default::default() };
        let (big, small) = (Warp::new(&g, W, H).unwrap(), Warp::new(&g, W / 4, H / 4).unwrap());
        let (bx, by) = big.from_source(3200.0, 800.0);
        let (sx, sy) = small.from_source(800.0, 200.0);
        assert!((bx / 4.0 - sx).abs() < 1e-6 && (by / 4.0 - sy).abs() < 1e-6);
    }
}
