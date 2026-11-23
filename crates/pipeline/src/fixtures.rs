//! Generates small synthetic DNG files for automated tests and manual trials.
//!
//! These are *generic* DNGs (uncompressed, one IFD). They exercise the same
//! decoder paths as iPhone ProRAW (LinearRaw) and DJI-style Bayer DNGs, but
//! they are not real camera files: passing tests on them does not prove
//! compatibility with real ProRAW.

use crate::color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    /// 3 samples per pixel, PhotometricInterpretation = LinearRaw (like ProRAW).
    Linear,
    /// 1 sample per pixel, RGGB Bayer mosaic (like most camera RAWs).
    Bayer,
}

#[derive(Debug, Clone)]
pub struct FixtureSpec {
    pub width: usize,
    pub height: usize,
    pub kind: FixtureKind,
    /// TIFF/EXIF orientation code 1–8.
    pub orientation: u16,
    pub as_shot_neutral: [f64; 3],
    /// XYZ(D65) → camera matrix, row-major. `srgb_identity_matrix()` makes the
    /// camera space equal linear sRGB, which keeps test expectations simple.
    pub color_matrix: [f64; 9],
    pub baseline_exposure: f64,
    pub black_level: u16,
    pub white_level: u16,
    pub make: String,
    pub model: String,
}

/// Colour matrix under which camera RGB == linear sRGB.
pub fn srgb_identity_matrix() -> [f64; 9] {
    let m = color::xyz_to_srgb();
    [m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2]].map(|v| v as f64)
}

impl FixtureSpec {
    pub fn new(width: usize, height: usize, kind: FixtureKind) -> Self {
        Self {
            width,
            height,
            kind,
            orientation: 1,
            as_shot_neutral: [1.0, 1.0, 1.0],
            color_matrix: srgb_identity_matrix(),
            baseline_exposure: 0.0,
            black_level: 0,
            white_level: 65535,
            make: "Fixture".into(),
            model: "Synthetic DNG".into(),
        }
    }
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn short(tag: u16, vals: &[u16]) -> Entry {
    Entry { tag, typ: 3, count: vals.len() as u32, data: vals.iter().flat_map(|v| v.to_le_bytes()).collect() }
}
fn long(tag: u16, vals: &[u32]) -> Entry {
    Entry { tag, typ: 4, count: vals.len() as u32, data: vals.iter().flat_map(|v| v.to_le_bytes()).collect() }
}
fn bytes(tag: u16, vals: &[u8]) -> Entry {
    Entry { tag, typ: 1, count: vals.len() as u32, data: vals.to_vec() }
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut data = s.as_bytes().to_vec();
    data.push(0);
    Entry { tag, typ: 2, count: data.len() as u32, data }
}
fn rational(tag: u16, vals: &[f64]) -> Entry {
    let data = vals
        .iter()
        .flat_map(|v| {
            let d = 1_000_000u32;
            let n = (v * d as f64).round().max(0.0) as u32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        })
        .collect();
    Entry { tag, typ: 5, count: vals.len() as u32, data }
}
fn srational(tag: u16, vals: &[f64]) -> Entry {
    let data = vals
        .iter()
        .flat_map(|v| {
            let d = 1_000_000i32;
            let n = (v * d as f64).round() as i32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        })
        .collect();
    Entry { tag, typ: 10, count: vals.len() as u32, data }
}

/// Little-endian TIFF with a single IFD followed by its out-of-line values
/// and one uncompressed image strip.
fn build_tiff(mut entries: Vec<Entry>, image: &[u8]) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);
    let ifd_len = 2 + 12 * entries.len() + 4;
    let extra_len: usize = entries.iter().filter(|e| e.data.len() > 4).map(|e| (e.data.len() + 1) & !1).sum();
    let image_off = (8 + ifd_len + extra_len) as u32;
    for e in entries.iter_mut() {
        if e.tag == 273 {
            e.data = image_off.to_le_bytes().to_vec();
        }
    }

    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    let mut extra = Vec::new();
    out.extend((entries.len() as u16).to_le_bytes());
    for e in &entries {
        out.extend(e.tag.to_le_bytes());
        out.extend(e.typ.to_le_bytes());
        out.extend(e.count.to_le_bytes());
        if e.data.len() <= 4 {
            let mut v = e.data.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            let off = (8 + ifd_len + extra.len()) as u32;
            out.extend(off.to_le_bytes());
            extra.extend(&e.data);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    out.extend(0u32.to_le_bytes()); // no next IFD
    out.extend(extra);
    debug_assert_eq!(out.len() as u32, image_off);
    out.extend(image);
    out
}

/// Build a DNG whose scene is given by `scene(x, y)` → camera-space linear
/// RGB in [0, 1] (in *stored* orientation, before the orientation tag is applied).
pub fn make_dng(spec: &FixtureSpec, scene: impl Fn(usize, usize) -> [f64; 3]) -> Vec<u8> {
    let (w, h) = (spec.width, spec.height);
    let (black, white) = (spec.black_level as f64, spec.white_level as f64);
    let encode = |v: f64| -> u16 { (black + v.clamp(0.0, 1.0) * (white - black)).round() as u16 };
    let cpp = match spec.kind {
        FixtureKind::Linear => 3,
        FixtureKind::Bayer => 1,
    };
    let mut image = Vec::with_capacity(w * h * cpp * 2);
    for y in 0..h {
        for x in 0..w {
            let px = scene(x, y);
            match spec.kind {
                FixtureKind::Linear => {
                    for c in px {
                        image.extend(encode(c).to_le_bytes());
                    }
                }
                FixtureKind::Bayer => {
                    // RGGB: even row = R G, odd row = G B.
                    let c = match (y % 2, x % 2) {
                        (0, 0) => 0,
                        (1, 1) => 2,
                        _ => 1,
                    };
                    image.extend(encode(px[c]).to_le_bytes());
                }
            }
        }
    }

    let mut e = vec![
        long(254, &[0]),
        long(256, &[w as u32]),
        long(257, &[h as u32]),
        short(258, &vec![16; cpp]),
        short(259, &[1]),
        short(262, &[if cpp == 3 { 34892 } else { 32803 }]),
        ascii(271, &spec.make),
        ascii(272, &spec.model),
        long(273, &[0]), // patched by build_tiff
        short(274, &[spec.orientation]),
        short(277, &[cpp as u16]),
        long(278, &[h as u32]),
        long(279, &[image.len() as u32]),
        short(284, &[1]),
        bytes(50706, &[1, 4, 0, 0]),
        ascii(50708, &format!("{} {}", spec.make, spec.model)),
        short(50714, &vec![spec.black_level; cpp]),
        short(50717, &vec![spec.white_level; cpp]),
        srational(50721, &spec.color_matrix),
        rational(50728, &spec.as_shot_neutral),
        srational(50730, &[spec.baseline_exposure]),
        short(50778, &[21]), // CalibrationIlluminant1 = D65
    ];
    if spec.kind == FixtureKind::Bayer {
        e.push(short(33421, &[2, 2]));
        e.push(bytes(33422, &[0, 1, 1, 2]));
        e.push(bytes(50710, &[0, 1, 2]));
        e.push(short(50711, &[1]));
    }
    build_tiff(e, &image)
}

/// The standard test scene: smooth gradients, colour patches and a bright
/// marker in the stored top-left corner (useful for orientation checks).
pub fn test_scene(width: usize, height: usize) -> impl Fn(usize, usize) -> [f64; 3] {
    move |x, y| {
        let fx = x as f64 / (width.max(2) - 1) as f64;
        let fy = y as f64 / (height.max(2) - 1) as f64;
        if x < width / 10 && y < height / 10 {
            return [0.9, 0.05, 0.05]; // red marker, stored top-left
        }
        // Four colour quadrants over a horizontal brightness ramp.
        let base = 0.02 + 0.6 * fx;
        let tint = match (fx > 0.5, fy > 0.5) {
            (false, false) => [1.0, 1.0, 1.0],
            (true, false) => [1.0, 0.8, 0.6],
            (false, true) => [0.6, 0.9, 0.7],
            (true, true) => [0.7, 0.75, 1.0],
        };
        [base * tint[0], base * tint[1], base * tint[2]]
    }
}
