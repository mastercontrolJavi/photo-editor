//! Lens corrections: distortion, corner shading (vignetting) and colour
//! fringing from the lens (lateral chromatic aberration).
//!
//! All three depend only on how far a pixel is from the centre of the photo.
//! Distances are measured as a fraction of the half-diagonal, so the same
//! numbers work for the preview and the full-size photo.
//!
//! iPhone ProRAW files are already corrected by the phone before they are
//! saved, so for them these are creative or touch-up controls.

use rayon::prelude::*;

use crate::adjust::Adjustments;
use crate::image::LinearImage;

/// Largest colour misalignment looked for, as a fraction of the distance from
/// the centre (0.0015 is about 7 pixels at the corner of a 48 MP photo).
const CA_RANGE: f32 = 0.0015;
const CA_STEPS: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    cx: f32,
    cy: f32,
    inv_norm: f32,
    /// Distortion strength: sample at `distance × scale × (1 + k·r²)`.
    k: f32,
    scale: f32,
    /// Extra magnification for red and blue (0 = none).
    ca: [f32; 2],
    vignette: f32,
}

impl Lens {
    /// `None` when no lens correction is switched on. `chromatic` is the
    /// measured red/blue misalignment of this photo (see [`chromatic_scales`]).
    pub fn new(a: &Adjustments, chromatic: [f32; 2], width: usize, height: usize) -> Option<Self> {
        let o = &a.optics;
        let ca = if o.remove_ca { chromatic } else { [0.0; 2] };
        if o.distortion == 0.0 && o.vignetting == 0.0 && ca == [0.0; 2] {
            return None;
        }
        let (w, h) = (width as f32, height as f32);
        let norm = 0.5 * (w * w + h * h).sqrt();
        // Positive Distortion corrects barrel distortion (straight lines that bow outwards).
        let k = -0.2 * o.distortion / 100.0;
        // Zoom just enough that no empty corners or edges appear.
        let f = |r: f32| 1.0 + k * r * r;
        let nearest_edge = (w.min(h) * 0.5) / norm;
        let scale = 1.0 / f(nearest_edge).max(f(1.0));
        Some(Lens { cx: w * 0.5, cy: h * 0.5, inv_norm: 1.0 / norm, k, scale, ca, vignette: o.vignetting / 100.0 })
    }

    /// True when red and blue are sampled at their own positions.
    #[inline]
    pub fn splits_colours(&self) -> bool {
        self.ca != [0.0; 2]
    }

    /// For the pixel that should show the point at `(x, y)` (in image pixels):
    /// where to read green, red and blue from, and the brightness correction.
    #[inline]
    pub fn map(&self, x: f32, y: f32) -> ([(f32, f32); 3], f32) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let r2 = (dx * dx + dy * dy) * self.inv_norm * self.inv_norm;
        let f = self.scale * (1.0 + self.k * r2);
        let at = |m: f32| (self.cx + dx * m, self.cy + dy * m);
        let gain = if self.vignette >= 0.0 { 1.0 + self.vignette * r2 } else { 1.0 / (1.0 - self.vignette * r2) };
        ([at(f), at(f * (1.0 + self.ca[0])), at(f * (1.0 + self.ca[1]))], gain)
    }
}

#[inline]
fn sample(img: &LinearImage, channel: usize, x: f32, y: f32) -> f32 {
    let fx = (x - 0.5).clamp(0.0, (img.width - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |xx: usize, yy: usize| img.pixels[yy * img.width + xx][channel];
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

/// Measure how much larger or smaller the red and blue images are than the
/// green one. The result is the extra magnification to read red and blue at
/// so that their edges line up with green's; `[0, 0]` means nothing to fix.
pub fn chromatic_scales(img: &LinearImage) -> [f32; 2] {
    let (w, h) = (img.width as f32, img.height as f32);
    if img.width < 64 || img.height < 64 {
        return [0.0; 2];
    }
    let (cx, cy) = (w * 0.5, h * 0.5);
    let norm = 0.5 * (w * w + h * h).sqrt();
    // How quickly a channel changes along the line from the centre, at a point.
    let slope = |channel: usize, x: f32, y: f32, ux: f32, uy: f32| {
        sample(img, channel, x + ux, y + uy) - sample(img, channel, x - ux, y - uy)
    };
    // Points away from the centre where green has a clear edge across that line.
    let mut points: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
    let step = (img.width.max(img.height) / 500).max(2);
    for yi in (4..img.height - 4).step_by(step) {
        for xi in (4..img.width - 4).step_by(step) {
            let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
            let (dx, dy) = (x - cx, y - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.4 * norm {
                continue;
            }
            let (ux, uy) = (dx / d, dy / d);
            let g = slope(1, x, y, ux, uy);
            let level = sample(img, 1, x, y).max(0.02);
            if g.abs() > 0.15 * level {
                points.push((x, y, ux, uy, g / level));
            }
        }
    }
    if points.len() < 200 {
        return [0.0; 2];
    }
    // How well the channel's edges, read `s` further out, match green's edges.
    // Dividing by the channel's own edge strength matters: reading between
    // pixels blurs a little, which would otherwise always favour "no shift".
    let score = |channel: usize, s: f32| -> f64 {
        let (matched, own) = points
            .par_iter()
            .map(|&(x, y, ux, uy, g)| {
                let (px, py) = (cx + (x - cx) * (1.0 + s), cy + (y - cy) * (1.0 + s));
                let level = sample(img, channel, px, py).max(0.02);
                let c = (slope(channel, px, py, ux, uy) / level) as f64;
                (g as f64 * c, c * c)
            })
            .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
        matched / own.sqrt().max(1e-12)
    };
    let measure = |channel: usize| -> f32 {
        let scores: Vec<f64> = (-CA_STEPS..=CA_STEPS).map(|i| score(channel, i as f32 / CA_STEPS as f32 * CA_RANGE)).collect();
        let (best, _) = scores.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
        let centre = CA_STEPS as usize;
        // Only trust a clear improvement over "no correction", found inside the search range.
        if best == 0 || best == scores.len() - 1 || scores[best] < scores[centre] * 1.002 || scores[centre] <= 0.0 {
            return 0.0;
        }
        // Fit a parabola through the best step and its neighbours for a finer answer.
        let (a, b, c) = (scores[best - 1], scores[best], scores[best + 1]);
        let offset = if (a - 2.0 * b + c).abs() > 1e-12 { 0.5 * (a - c) / (a - 2.0 * b + c) } else { 0.0 };
        let found = (best as f32 - centre as f32 + offset as f32) / CA_STEPS as f32 * CA_RANGE;
        // Under about half a pixel at the corner of a 48 MP photo isn't worth moving pixels for.
        if found.abs() < 0.0001 {
            0.0
        } else {
            found
        }
    };
    [measure(0), measure(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn optics(f: impl FnOnce(&mut Adjustments)) -> Adjustments {
        let mut a = Adjustments::default();
        a.sharpen.amount = 0.0;
        f(&mut a);
        a
    }

    #[test]
    fn off_by_default() {
        assert!(Lens::new(&Adjustments::default(), [0.001, -0.001], 400, 300).is_none());
        // Ticking "Remove chromatic aberration" on a photo with none still does nothing.
        assert!(Lens::new(&optics(|a| a.optics.remove_ca = true), [0.0; 2], 400, 300).is_none());
    }

    #[test]
    fn distortion_never_reads_outside_the_photo_and_leaves_the_centre_alone() {
        for amount in [-100.0, -40.0, 30.0, 100.0] {
            let lens = Lens::new(&optics(|a| a.optics.distortion = amount), [0.0; 2], 400, 300).unwrap();
            let ([centre, ..], _) = lens.map(200.0, 150.0);
            assert_eq!(centre, (200.0, 150.0));
            for (x, y) in [(0.0, 0.0), (400.0, 300.0), (200.0, 0.0), (0.0, 150.0), (400.0, 150.0), (130.0, 300.0)] {
                let ([(sx, sy), ..], _) = lens.map(x, y);
                assert!((-0.01..=400.01).contains(&sx) && (-0.01..=300.01).contains(&sy), "amount {amount}: ({x},{y}) -> ({sx},{sy})");
            }
        }
        // Positive values pull the middle of each edge in more than the corners (correcting barrel distortion).
        let lens = Lens::new(&optics(|a| a.optics.distortion = 100.0), [0.0; 2], 400, 300).unwrap();
        let ([(_, edge_y), ..], _) = lens.map(200.0, 0.0);
        let ([(_, corner_y), ..], _) = lens.map(0.0, 0.0);
        assert!(edge_y < 1.0 && corner_y > 10.0, "{edge_y} {corner_y}");
    }

    #[test]
    fn vignetting_brightens_or_darkens_the_corners_only() {
        let lens = Lens::new(&optics(|a| a.optics.vignetting = 100.0), [0.0; 2], 400, 300).unwrap();
        assert_eq!(lens.map(200.0, 150.0).1, 1.0);
        assert!((lens.map(0.0, 0.0).1 - 2.0).abs() < 1e-4);
        let lens = Lens::new(&optics(|a| a.optics.vignetting = -100.0), [0.0; 2], 400, 300).unwrap();
        assert!((lens.map(0.0, 0.0).1 - 0.5).abs() < 1e-4);
    }

    /// Grey squares on a darker background, with red and blue drawn slightly larger or smaller than green.
    fn fringed(red: f32, blue: f32) -> LinearImage {
        let (w, h) = (900usize, 600usize);
        let pattern = |x: f32, y: f32| -> f32 {
            // Soft-edged tiles, so the edges are well defined for sub-pixel shifts.
            let wave = (x * 0.09).sin() * (y * 0.11).sin();
            0.25 + 0.2 * (wave * 6.0).clamp(-1.0, 1.0)
        };
        let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let at = |m: f32| pattern(cx + (x - cx) * m, cy + (y - cy) * m);
                [at(1.0 + red), at(1.0), at(1.0 + blue)]
            })
            .collect();
        LinearImage::new(w, h, px)
    }

    #[test]
    fn measures_colour_fringing_and_its_direction() {
        // Red is drawn as if magnified by 1/(1+0.0009): reading it 0.0009 further out lines it up.
        let [r, b] = chromatic_scales(&fringed(0.0009, -0.0006));
        assert!((r + 0.0009).abs() < 0.0002, "red {r}");
        assert!((b - 0.0006).abs() < 0.0002, "blue {b}");
    }

    #[test]
    fn a_clean_photo_measures_as_clean() {
        assert_eq!(chromatic_scales(&fringed(0.0, 0.0)), [0.0, 0.0]);
        let flat = LinearImage::new(200, 200, vec![[0.3; 3]; 200 * 200]);
        assert_eq!(chromatic_scales(&flat), [0.0, 0.0]);
    }

    #[test]
    fn the_correction_lines_the_colours_back_up() {
        let img = fringed(0.0009, -0.0006);
        let lens = Lens::new(&optics(|a| a.optics.remove_ca = true), chromatic_scales(&img), img.width, img.height).unwrap();
        assert!(lens.splits_colours());
        // Near a corner the uncorrected colours disagree; after correction they agree.
        let mut before = 0.0;
        let mut after = 0.0;
        for i in 0..200 {
            let (x, y) = (60.0 + i as f32 * 0.7, 40.0 + i as f32 * 0.45);
            let ([g, r, _], _) = lens.map(x, y);
            before += (sample(&img, 0, x, y) - sample(&img, 1, x, y)).abs();
            after += (sample(&img, 0, r.0, r.1) - sample(&img, 1, g.0, g.1)).abs();
        }
        assert!(after < before * 0.4, "before {before}, after {after}");
    }
}
