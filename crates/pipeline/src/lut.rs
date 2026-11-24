//! 3-D colour look-up tables in the `.cube` format (the common download format
//! for "LUTs"). A LUT maps every display colour to another colour; the editor
//! applies it at the end of the edit, with an Amount slider.

use std::path::Path;

#[derive(Clone, PartialEq)]
pub struct Lut3D {
    pub title: String,
    /// Points per side (the file's `LUT_3D_SIZE`).
    pub size: usize,
    /// `size³` colours, red changing fastest.
    data: Vec<[f32; 3]>,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
}

impl std::fmt::Debug for Lut3D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lut3D({:?}, {}³)", self.title, self.size)
    }
}

impl Lut3D {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        Self::parse(&String::from_utf8_lossy(&bytes))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut title = String::new();
        let mut size = 0usize;
        let (mut domain_min, mut domain_max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut data: Vec<[f32; 3]> = Vec::new();
        let triple = |rest: &str| -> Option<[f32; 3]> {
            let mut it = rest.split_whitespace().map(|v| v.parse::<f32>().ok());
            Some([it.next()??, it.next()??, it.next()??])
        };
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("TITLE") {
                title = rest.trim().trim_matches('"').to_string();
            } else if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size = rest.trim().parse().map_err(|_| "bad LUT_3D_SIZE")?;
                if !(2..=129).contains(&size) {
                    return Err(format!("LUT size {size} is outside the supported range (2 to 129)"));
                }
                data.reserve(size * size * size);
            } else if line.starts_with("LUT_1D_SIZE") {
                return Err("this is a 1-D LUT; only 3-D .cube LUTs are supported".into());
            } else if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                domain_min = triple(rest).ok_or("bad DOMAIN_MIN")?;
            } else if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                domain_max = triple(rest).ok_or("bad DOMAIN_MAX")?;
            } else if line.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.' || c == '+') {
                let v = triple(line).ok_or_else(|| format!("bad colour line: {line}"))?;
                if !v.iter().all(|c| c.is_finite()) {
                    return Err("the LUT contains a value that is not a number".into());
                }
                data.push(v);
            }
            // Other keywords (LUT_3D_INPUT_RANGE, …) are ignored.
        }
        if size == 0 {
            return Err("no LUT_3D_SIZE line; this doesn't look like a 3-D .cube file".into());
        }
        if data.len() != size * size * size {
            return Err(format!("expected {} colours for size {size}, found {}", size * size * size, data.len()));
        }
        if (0..3).any(|c| domain_max[c] - domain_min[c] <= 1e-6) {
            return Err("bad DOMAIN_MIN / DOMAIN_MAX".into());
        }
        Ok(Lut3D { title, size, data, domain_min, domain_max })
    }

    /// A table built in memory: `size³` colours, red changing fastest, for inputs 0…1.
    pub fn from_grid(title: &str, size: usize, data: Vec<[f32; 3]>) -> Self {
        assert_eq!(data.len(), size * size * size, "a LUT needs size³ colours");
        Lut3D { title: title.into(), size, data, domain_min: [0.0; 3], domain_max: [1.0; 3] }
    }

    /// A LUT that changes nothing (for tests).
    pub fn identity(size: usize) -> Self {
        let n = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([r as f32 / n, g as f32 / n, b as f32 / n]);
                }
            }
        }
        Lut3D { title: "identity".into(), size, data, domain_min: [0.0; 3], domain_max: [1.0; 3] }
    }

    /// Change every colour in the table (for tests).
    pub fn map(mut self, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        for v in &mut self.data {
            *v = f(*v);
        }
        self
    }

    /// Look a colour up, blending between the eight nearest table entries.
    #[inline]
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let mut i0 = [0usize; 3];
        let mut t = [0.0f32; 3];
        for c in 0..3 {
            let v = ((rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c])).clamp(0.0, 1.0);
            let f = v * (n - 1) as f32;
            i0[c] = (f as usize).min(n - 2);
            t[c] = f - i0[c] as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + g * n + b * n * n];
        let mut out = [0.0f32; 3];
        for (db, wb) in [(0, 1.0 - t[2]), (1, t[2])] {
            for (dg, wg) in [(0, 1.0 - t[1]), (1, t[1])] {
                for (dr, wr) in [(0, 1.0 - t[0]), (1, t[0])] {
                    let w = wr * wg * wb;
                    let v = at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                    out[0] += w * v[0];
                    out[1] += w * v[1];
                    out[2] += w * v[2];
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_cube_file() {
        let mut text = String::from("# comment\nTITLE \"Tiny\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text += &format!("{} {} {}\n", r, g, 1 - b); // blue inverted
                }
            }
        }
        let lut = Lut3D::parse(&text).unwrap();
        assert_eq!((lut.title.as_str(), lut.size), ("Tiny", 2));
        let o = lut.sample([0.25, 0.5, 0.2]);
        assert!((o[0] - 0.25).abs() < 1e-5 && (o[1] - 0.5).abs() < 1e-5 && (o[2] - 0.8).abs() < 1e-5, "{o:?}");
    }

    #[test]
    fn identity_changes_nothing() {
        let lut = Lut3D::identity(17);
        for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 0.71, 0.05]] {
            let o = lut.sample(c);
            assert!((0..3).all(|i| (o[i] - c[i]).abs() < 1e-5), "{c:?} -> {o:?}");
        }
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        assert!(Lut3D::parse("hello").unwrap_err().contains("LUT_3D_SIZE"));
        assert!(Lut3D::parse("LUT_3D_SIZE 2\n0 0 0\n").unwrap_err().contains("expected 8"));
        assert!(Lut3D::parse("LUT_1D_SIZE 16\n").unwrap_err().contains("1-D"));
        assert!(Lut3D::parse("LUT_3D_SIZE 2\n0 0 nan\n").is_err());
    }
}
