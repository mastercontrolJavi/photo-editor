//! Healing: covering a small circle of the photo (a dust spot, a blemish, a
//! stray wire) with another part of the same photo.
//!
//! **Clone** copies the source as it is. **Heal** copies its texture but
//! takes brightness and colour from what surrounds the spot, so the patch
//! disappears into a sky or a wall even when the source is a little lighter
//! or darker. The matching is done on a ring of points around each circle:
//! how much brighter the surroundings of the spot are than the surroundings
//! of the source, smoothly carried across the inside.
//!
//! Nothing is written into the photo: the renderer asks [`Healer::apply`] for
//! each pixel it reads.

use crate::adjust::{Spot, SpotMode};
use crate::image::LinearImage;

/// Points measured around each circle.
const RING: usize = 16;
/// The helper grid that says which spots can touch which part of the photo.
const GRID: usize = 48;

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

/// Average colour of a small patch around a point (smooths over noise and fine texture).
fn patch(img: &LinearImage, x: f32, y: f32, step: f32) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for j in -1..=1 {
        for i in -1..=1 {
            let p = bilinear(img, x + i as f32 * step, y + j as f32 * step);
            sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
        }
    }
    sum.map(|v| v / 9.0)
}

fn ring_directions() -> [(f32, f32); RING] {
    std::array::from_fn(|i| {
        let a = i as f32 / RING as f32 * std::f32::consts::TAU;
        (a.cos(), a.sin())
    })
}

/// Colours on a circle of radius `r` around `(cx, cy)`.
fn ring(img: &LinearImage, cx: f32, cy: f32, r: f32) -> [[f32; 3]; RING] {
    let step = (r * 0.12).max(1.0);
    ring_directions().map(|(dx, dy)| patch(img, cx + dx * r, cy + dy * r, step))
}

struct Ready {
    cx: f32,
    cy: f32,
    r2: f32,
    inv_r: f32,
    /// From the spot to its source, in pixels.
    dx: f32,
    dy: f32,
    /// Fraction of the radius that is fully covered; the rest fades out.
    inner: f32,
    inv_soft: f32,
    opacity: f32,
    /// Heal only: surroundings of the spot ÷ surroundings of the source, around the ring.
    gains: Option<[[f32; 3]; RING]>,
}

/// The healing spots of an edit, made ready for one image size.
pub struct Healer {
    spots: Vec<Ready>,
    /// For each cell of a coarse grid, the spots that reach into it.
    cells: Vec<Vec<u16>>,
    cell_w: f32,
    cell_h: f32,
    dirs: [(f32, f32); RING],
}

impl Healer {
    /// `None` when there is nothing to heal.
    pub fn new(spots: &[Spot], img: &LinearImage) -> Option<Self> {
        if spots.is_empty() || img.width < 2 || img.height < 2 {
            return None;
        }
        let (w, h) = (img.width as f32, img.height as f32);
        let long = w.max(h);
        let (cell_w, cell_h) = (w / GRID as f32, h / GRID as f32);
        let mut cells = vec![Vec::new(); GRID * GRID];
        let mut ready = Vec::with_capacity(spots.len());
        for s in spots {
            if s.opacity <= 0.0 {
                continue;
            }
            let (cx, cy) = (s.at[0] * w, s.at[1] * h);
            let r = (s.radius * long).max(1.0);
            let (dx, dy) = (s.source[0] * w - cx, s.source[1] * h - cy);
            // Never a harder edge than about a pixel and a half, so the patch isn't jagged.
            let inner = (1.0 - s.feather / 100.0).min(1.0 - 1.5 / r).clamp(0.0, 0.97);
            let gains = (s.mode == SpotMode::Heal).then(|| {
                let (around_spot, around_source) = (ring(img, cx, cy, r), ring(img, cx + dx, cy + dy, r));
                std::array::from_fn(|i| {
                    [0, 1, 2].map(|c| ((around_spot[i][c].max(0.0) + 0.002) / (around_source[i][c].max(0.0) + 0.002)).clamp(0.2, 5.0))
                })
            });
            let index = ready.len() as u16;
            ready.push(Ready { cx, cy, r2: r * r, inv_r: 1.0 / r, dx, dy, inner, inv_soft: 1.0 / (1.0 - inner), opacity: s.opacity / 100.0, gains });
            let cell = |v: f32, size: f32| ((v / size).floor().max(0.0) as usize).min(GRID - 1);
            for gy in cell(cy - r, cell_h)..=cell(cy + r, cell_h) {
                for gx in cell(cx - r, cell_w)..=cell(cx + r, cell_w) {
                    cells[gy * GRID + gx].push(index);
                }
            }
        }
        if ready.is_empty() {
            return None;
        }
        Some(Healer { spots: ready, cells, cell_w, cell_h, dirs: ring_directions() })
    }

    /// The colour to show at `(x, y)` of the image, given the photo's own
    /// colour there (`base`). Outside every spot this is `base` unchanged.
    #[inline]
    pub fn apply(&self, img: &LinearImage, x: f32, y: f32, base: [f32; 3]) -> [f32; 3] {
        let gx = ((x / self.cell_w).max(0.0) as usize).min(GRID - 1);
        let gy = ((y / self.cell_h).max(0.0) as usize).min(GRID - 1);
        let here = &self.cells[gy * GRID + gx];
        if here.is_empty() {
            return base;
        }
        let mut out = base;
        for &i in here {
            let s = &self.spots[i as usize];
            let (px, py) = (x - s.cx, y - s.cy);
            let d2 = px * px + py * py;
            if d2 >= s.r2 {
                continue;
            }
            let mut patch = bilinear(img, x + s.dx, y + s.dy);
            if let Some(gains) = &s.gains {
                let g = self.gain_at(gains, px * s.inv_r, py * s.inv_r);
                patch = [patch[0] * g[0], patch[1] * g[1], patch[2] * g[2]];
            }
            let rho = d2.sqrt() * s.inv_r;
            let t = ((rho - s.inner) * s.inv_soft).clamp(0.0, 1.0);
            let alpha = s.opacity * (1.0 - t * t * (3.0 - 2.0 * t));
            out = [0, 1, 2].map(|c| out[c] + (patch[c] - out[c]) * alpha);
        }
        out
    }

    /// The ring's values carried smoothly to a point inside the circle
    /// (`(x, y)` in units of the radius). This is how heat would spread from
    /// the rim: near a rim point the value is that point's, and the centre
    /// gets the average.
    #[inline]
    fn gain_at(&self, gains: &[[f32; 3]; RING], x: f32, y: f32) -> [f32; 3] {
        let inside = (1.0 - (x * x + y * y)).max(0.0);
        let (mut sum, mut total) = ([0.0f32; 3], 0.0f32);
        for (g, (bx, by)) in gains.iter().zip(&self.dirs) {
            let (ex, ey) = (x - bx, y - by);
            let w = (inside + 1e-4) / (ex * ex + ey * ey).max(1e-4);
            sum = [sum[0] + g[0] * w, sum[1] + g[1] * w, sum[2] + g[2] * w];
            total += w;
        }
        sum.map(|v| v / total)
    }
}

#[inline]
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Pick a part of the photo to heal a spot from: somewhere nearby whose
/// surroundings look like the spot's surroundings, and that is itself smooth.
/// `at` is the spot's centre (0 … 1 across and down) and `radius` its size as
/// a fraction of the longer side. Returns the source's centre.
pub fn suggest_source(img: &LinearImage, at: [f32; 2], radius: f32) -> [f32; 2] {
    let (w, h) = (img.width as f32, img.height as f32);
    let r = (radius * w.max(h)).max(1.0);
    let (cx, cy) = (at[0] * w, at[1] * h);
    let around: [f32; RING] = ring(img, cx, cy, r).map(|c| (luma(c).max(0.0) + 0.002).ln());
    let dirs = ring_directions();
    let mut best: Option<(f32, f32, f32)> = None;
    for (step, distance) in [2.3f32, 3.2, 4.5, 6.5].iter().enumerate() {
        for (k, (dx, dy)) in dirs.iter().enumerate() {
            let (sx, sy) = (cx + dx * distance * r, cy + dy * distance * r);
            if sx - r < 0.0 || sy - r < 0.0 || sx + r > w || sy + r > h {
                continue;
            }
            // Do the surroundings match, apart from being evenly lighter or darker?
            let there: [f32; RING] = ring(img, sx, sy, r).map(|c| (luma(c).max(0.0) + 0.002).ln());
            let diff: [f32; RING] = std::array::from_fn(|i| around[i] - there[i]);
            let mean = diff.iter().sum::<f32>() / RING as f32;
            let uneven = diff.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / RING as f32;
            // Is the candidate itself free of features that would be copied in?
            let inside: [f32; RING] = ring(img, sx, sy, r * 0.5).map(|c| (luma(c).max(0.0) + 0.002).ln());
            let centre = inside.iter().sum::<f32>() / RING as f32;
            let busy = inside.iter().map(|d| (d - centre).powi(2)).sum::<f32>() / RING as f32;
            // Ties go to the nearest candidate, and then to a fixed order, so the choice is repeatable.
            let score = uneven.sqrt() + 0.3 * mean.abs() + busy.sqrt() + 0.004 * step as f32 + 0.0001 * k as f32;
            if best.is_none_or(|(b, _, _)| score < b) {
                best = Some((score, sx, sy));
            }
        }
    }
    match best {
        Some((_, sx, sy)) => [sx / w, sy / h],
        // A spot too large for anything to fit beside it: offer the nearest place that is inside the photo.
        None => {
            let side = if cx < w * 0.5 { 1.0 } else { -1.0 };
            [((cx + side * 2.3 * r) / w).clamp(0.0, 1.0), at[1]]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth wall, slightly lighter on the right, with a fine weave.
    fn wall(w: usize, h: usize) -> LinearImage {
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let level = 0.20 + 0.10 * x as f32 / w as f32;
                let weave = if (x / 2 + y / 2) % 2 == 0 { 1.03 } else { 0.97 };
                [level * weave, level * weave * 0.9, level * weave * 0.8]
            })
            .collect();
        LinearImage::new(w, h, px)
    }

    fn with_blemish(mut img: LinearImage, cx: usize, cy: usize, r: usize) -> LinearImage {
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                if (x as f32 - cx as f32).hypot(y as f32 - cy as f32) <= r as f32 {
                    img.pixels[y * img.width + x] = [0.02, 0.02, 0.02];
                }
            }
        }
        img
    }

    fn read(healer: &Healer, img: &LinearImage, x: usize, y: usize) -> [f32; 3] {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        healer.apply(img, fx, fy, bilinear(img, fx, fy))
    }

    #[test]
    fn healing_removes_a_blemish_and_matches_the_surroundings() {
        let clean = wall(400, 300);
        let img = with_blemish(clean.clone(), 150, 150, 8);
        // Source 120 px to the right, where the wall is noticeably lighter.
        let spot = Spot { at: [150.0 / 400.0, 0.5], source: [270.0 / 400.0, 0.5], radius: 14.0 / 400.0, feather: 30.0, opacity: 100.0, mode: SpotMode::Heal };
        let healer = Healer::new(&[spot], &img).unwrap();
        let mut worst = 0.0f32;
        for y in 130..170 {
            for x in 130..170 {
                let got = luma(read(&healer, &img, x, y));
                let want = luma(clean.pixels[y * 400 + x]);
                worst = worst.max((got / want - 1.0).abs());
            }
        }
        // Within the weave's own ±3 % (plus a little): the blemish is gone and no lighter disc is left.
        assert!(worst < 0.08, "healed area is up to {:.0} % off", worst * 100.0);
        // A plain copy of the same source would have been about 15 % too light.
        let cloned = Healer::new(&[Spot { mode: SpotMode::Clone, ..spot }], &img).unwrap();
        let got = luma(read(&cloned, &img, 150, 150)) / luma(clean.pixels[150 * 400 + 150]);
        assert!(got > 1.08, "clone should copy the lighter wall as it is: {got}");
        assert_eq!(read(&cloned, &img, 150, 150), img.pixels[150 * 400 + 270]);
    }

    #[test]
    fn pixels_outside_every_spot_are_untouched() {
        let img = wall(400, 300);
        let spot = Spot { at: [0.5, 0.5], source: [0.7, 0.5], radius: 0.05, ..Spot::default() };
        let healer = Healer::new(&[spot], &img).unwrap();
        for (x, y) in [(0, 0), (399, 299), (200, 120), (225, 150), (100, 150)] {
            assert_eq!(read(&healer, &img, x, y), img.pixels[y * 400 + x], "({x}, {y})");
        }
        assert!(Healer::new(&[], &img).is_none());
        assert!(Healer::new(&[Spot { opacity: 0.0, ..spot }], &img).is_none());
    }

    #[test]
    fn opacity_and_feather_blend_the_patch_in() {
        let img = with_blemish(wall(400, 300), 150, 150, 8);
        let spot = Spot { at: [150.0 / 400.0, 0.5], source: [0.6, 0.5], radius: 20.0 / 400.0, feather: 100.0, opacity: 50.0, mode: SpotMode::Clone };
        let healer = Healer::new(&[spot], &img).unwrap();
        // Half strength at the centre: halfway between the blemish and the wall.
        let centre = read(&healer, &img, 150, 150)[0];
        let (dark, light) = (0.02, img.pixels[150 * 400 + 240][0]);
        assert!((centre - (dark + light) / 2.0).abs() < 0.01, "{centre}");
        // Fully feathered: the patch fades all the way from the centre to the rim.
        let strength = |x: usize| {
            let (before, after) = (img.pixels[150 * 400 + x][0], read(&healer, &img, x, 150)[0]);
            (after - before).abs()
        };
        assert!(strength(167) < strength(160) * 0.5);
    }

    #[test]
    fn the_suggested_source_is_clean_and_inside_the_photo() {
        // A dark line runs down the photo just right of the spot; the source must avoid it.
        let mut img = with_blemish(wall(400, 300), 150, 150, 6);
        for y in 0..300 {
            for x in 185..192 {
                img.pixels[y * 400 + x] = [0.01; 3];
            }
        }
        let (at, radius) = ([150.0 / 400.0, 0.5], 10.0 / 400.0);
        let source = suggest_source(&img, at, radius);
        let (sx, sy) = (source[0] * 400.0, source[1] * 300.0);
        assert!(sx >= 10.0 && sy >= 10.0 && sx <= 390.0 && sy <= 290.0, "{sx} {sy}");
        assert!((sx - 150.0).hypot(sy - 150.0) >= 20.0, "source overlaps the spot");
        assert!(!(175.0..=202.0).contains(&sx), "source sits on the dark line: {sx}");
        // The same question gets the same answer.
        assert_eq!(source, suggest_source(&img, at, radius));
        // In a corner, the source still lands inside the photo.
        let corner = suggest_source(&img, [0.02, 0.03], radius);
        assert!(corner[0] * 400.0 >= 10.0 && corner[1] * 300.0 >= 10.0);
    }
}
