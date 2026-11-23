use rayon::prelude::*;

/// Scene-linear RGB image, f32 per channel, sRGB primaries, D65 white.
/// Values are not clipped: anything above 1.0 is brighter than sensor white.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearImage {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width * height` pixels.
    pub pixels: Vec<[f32; 3]>,
}

impl LinearImage {
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Self {
        assert_eq!(pixels.len(), width * height, "pixel count must match dimensions");
        Self { width, height, pixels }
    }

    pub fn get(&self, x: usize, y: usize) -> [f32; 3] {
        self.pixels[y * self.width + x]
    }

    /// Area-average downscale so the longest edge is at most `long_edge`.
    /// Averaging happens in linear light, which is physically correct.
    /// Returns a clone when the image is already small enough.
    pub fn downscale_to_fit(&self, long_edge: usize) -> LinearImage {
        let (w, h) = fit_within(self.width, self.height, long_edge);
        if w == self.width && h == self.height {
            return self.clone();
        }
        self.resize_box(w, h)
    }

    /// Area-average resize to exactly `w × h` (downscaling only).
    pub fn resize_box(&self, w: usize, h: usize) -> LinearImage {
        assert!(w >= 1 && h >= 1 && w <= self.width && h <= self.height);
        let (sw, sh) = (self.width, self.height);
        let mut out = vec![[0.0f32; 3]; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(oy, row)| {
            let y0 = oy * sh / h;
            let y1 = ((oy + 1) * sh / h).max(y0 + 1);
            for (ox, px) in row.iter_mut().enumerate() {
                let x0 = ox * sw / w;
                let x1 = ((ox + 1) * sw / w).max(x0 + 1);
                let mut acc = [0.0f64; 3];
                for y in y0..y1 {
                    for p in &self.pixels[y * sw + x0..y * sw + x1] {
                        acc[0] += p[0] as f64;
                        acc[1] += p[1] as f64;
                        acc[2] += p[2] as f64;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f64;
                *px = [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32];
            }
        });
        LinearImage::new(w, h, out)
    }

    /// Bake an EXIF/TIFF orientation code (1–8) into the pixels so every later
    /// step (preview, export) sees an upright image. Unknown codes are treated
    /// as 1 (no change).
    pub fn oriented(self, code: u16) -> LinearImage {
        let (w, h) = (self.width, self.height);
        let swap = matches!(code, 5..=8);
        let (ow, oh) = if swap { (h, w) } else { (w, h) };
        if !(2..=8).contains(&code) {
            return self;
        }
        // For each output pixel (x, y), which source pixel lands there.
        let src_of = |x: usize, y: usize| -> (usize, usize) {
            match code {
                2 => (w - 1 - x, y),         // mirror horizontally
                3 => (w - 1 - x, h - 1 - y), // rotate 180
                4 => (x, h - 1 - y),         // mirror vertically
                5 => (y, x),                 // transpose
                6 => (y, h - 1 - x),         // rotate 90 clockwise
                7 => (w - 1 - y, h - 1 - x), // transverse
                8 => (w - 1 - y, x),         // rotate 90 counter-clockwise
                _ => unreachable!(),
            }
        };
        let mut out = vec![[0.0f32; 3]; ow * oh];
        out.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let (sx, sy) = src_of(x, y);
                *px = self.pixels[sy * w + sx];
            }
        });
        LinearImage::new(ow, oh, out)
    }
}

/// Largest size with the same aspect ratio whose long edge is ≤ `long_edge`.
/// Never upscales.
pub fn fit_within(width: usize, height: usize, long_edge: usize) -> (usize, usize) {
    let long = width.max(height);
    if long <= long_edge {
        return (width, height);
    }
    let scale = long_edge as f64 / long as f64;
    let w = ((width as f64 * scale).round() as usize).clamp(1, long_edge);
    let h = ((height as f64 * scale).round() as usize).clamp(1, long_edge);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×2 image where each pixel stores its own (x, y) so moves are visible.
    fn coords() -> LinearImage {
        let mut px = Vec::new();
        for y in 0..2 {
            for x in 0..3 {
                px.push([x as f32, y as f32, 0.0]);
            }
        }
        LinearImage::new(3, 2, px)
    }

    #[test]
    fn rotate_90_cw_moves_top_left_to_top_right() {
        let r = coords().oriented(6);
        assert_eq!((r.width, r.height), (2, 3));
        // Source top-left (0,0) should now be at output top-right (1,0).
        assert_eq!(r.get(1, 0), [0.0, 0.0, 0.0]);
        // Source bottom-left (0,1) should be at output top-left.
        assert_eq!(r.get(0, 0), [0.0, 1.0, 0.0]);
        // Source top-right (2,0) should be at output bottom-right.
        assert_eq!(r.get(1, 2), [2.0, 0.0, 0.0]);
    }

    #[test]
    fn rotate_90_ccw_and_180() {
        let r = coords().oriented(8);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(r.get(0, 2), [0.0, 0.0, 0.0]); // top-left → bottom-left
        let r = coords().oriented(3);
        assert_eq!(r.get(0, 0), [2.0, 1.0, 0.0]); // bottom-right → top-left
    }

    #[test]
    fn every_orientation_is_a_permutation_and_inverse_pairs_cancel() {
        let base = coords();
        for code in 1..=8u16 {
            let r = base.clone().oriented(code);
            let mut seen: Vec<_> = r.pixels.iter().map(|p| (p[0] as i32, p[1] as i32)).collect();
            seen.sort();
            assert_eq!(seen.len(), 6);
            seen.dedup();
            assert_eq!(seen.len(), 6, "orientation {code} duplicated pixels");
        }
        // 6 then 8 (cw then ccw) is the identity; 2, 4, 5, 7 are self-inverse.
        assert_eq!(base.clone().oriented(6).oriented(8), base);
        for code in [2, 3, 4, 5, 7] {
            assert_eq!(base.clone().oriented(code).oriented(code), base, "code {code}");
        }
    }

    #[test]
    fn downscale_averages_in_linear_light() {
        let img = LinearImage::new(2, 1, vec![[0.0; 3], [1.0; 3]]);
        let s = img.resize_box(1, 1);
        assert_eq!(s.pixels[0], [0.5; 3]);
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_within(8064, 6048, 2560), (2560, 1920));
        assert_eq!(fit_within(100, 50, 2560), (100, 50));
        assert_eq!(fit_within(6048, 8064, 2560), (1920, 2560));
    }
}
