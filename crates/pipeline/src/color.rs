//! Colour maths shared by decode, preview and export.
//!
//! Working space: scene-linear RGB with sRGB/Rec.709 primaries and a D65 white,
//! stored as unclipped f32. Values above 1.0 (brighter than sensor white after
//! white balance) are kept so negative exposure can bring them back.

pub type Mat3 = [[f32; 3]; 3];

/// Linear sRGB (D65) → CIE XYZ (D65). IEC 61966-2-1.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192, 0.950_304_1],
];

/// Bradford cone-response matrix (XYZ → sharpened LMS).
const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

#[inline]
pub fn apply(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// 3×3 inverse. Returns `None` for a singular matrix.
pub fn invert(m: &Mat3) -> Option<Mat3> {
    // Work in f64: camera matrices can be badly conditioned.
    let a = |i: usize, j: usize| m[i][j] as f64;
    let c00 = a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1);
    let c01 = a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2);
    let c02 = a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0);
    let det = a(0, 0) * c00 + a(0, 1) * c01 + a(0, 2) * c02;
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let inv = [
        [c00, a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2), a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)],
        [c01, a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0), a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)],
        [c02, a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1), a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)],
    ];
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (inv[i][j] / det) as f32;
        }
    }
    Some(out)
}

pub fn xyz_to_srgb() -> Mat3 {
    invert(&SRGB_TO_XYZ).expect("sRGB matrix is invertible")
}

/// Camera RGB (already white balanced) → linear sRGB, from a DNG-style
/// XYZ(D65)→camera colour matrix. Rows of `xyz_to_cam · srgb_to_xyz` are
/// normalised so a white-balanced neutral maps to neutral (the dcraw/rawler
/// convention).
pub fn camera_to_srgb(xyz_to_cam: &Mat3) -> Option<Mat3> {
    let mut rgb_to_cam = mul(xyz_to_cam, &SRGB_TO_XYZ);
    for row in rgb_to_cam.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() < 1e-9 || !sum.is_finite() {
            return None;
        }
        row.iter_mut().for_each(|v| *v /= sum);
    }
    invert(&rgb_to_cam)
}

// ---------------------------------------------------------------- white balance

/// CIE 1931 xy chromaticity of a Planckian (black-body) radiator, using the
/// Kim et al. (2002) cubic-spline fit. Valid 1667 K – 25000 K.
pub fn planckian_xy(cct: f64) -> (f64, f64) {
    let t = cct.clamp(1667.0, 25000.0);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    (x, y)
}

fn xy_to_uv(x: f64, y: f64) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy(u: f64, v: f64) -> (f64, f64) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

fn xy_to_xyz(x: f64, y: f64) -> [f32; 3] {
    [(x / y) as f32, 1.0, ((1.0 - x - y) / y) as f32]
}

/// Reference colour temperature that "0" on the temperature slider means.
pub const NEUTRAL_CCT: f64 = 6504.0;
/// How far the tint slider moves the assumed illuminant off the Planckian
/// locus at ±100, in CIE 1960 Δuv.
pub const TINT_DUV_AT_100: f64 = 0.03;

/// Assumed scene-illuminant chromaticity for a slider position.
/// Temperature +100 means the light was bluer than neutral (≈13 000 K), so
/// correcting for it warms the picture; −100 ≈ 3250 K cools it.
/// Tint +100 means the light was greener, so correcting adds magenta.
fn illuminant_xy(temperature: f32, tint: f32) -> (f64, f64) {
    let cct = NEUTRAL_CCT * 2f64.powf(temperature as f64 / 100.0);
    let (x, y) = planckian_xy(cct);
    let (u, v) = xy_to_uv(x, y);
    // Unit normal to the locus, pointing to +v (the green side).
    let (x2, y2) = planckian_xy(cct + 10.0);
    let (u2, v2) = xy_to_uv(x2, y2);
    let (tu, tv) = (u2 - u, v2 - v);
    let len = (tu * tu + tv * tv).sqrt().max(1e-12);
    let (mut nu, mut nv) = (-tv / len, tu / len);
    if nv < 0.0 {
        nu = -nu;
        nv = -nv;
    }
    let duv = tint as f64 / 100.0 * TINT_DUV_AT_100;
    uv_to_xy(u + nu * duv, v + nv * duv)
}

/// Linear-sRGB → linear-sRGB matrix implementing the temperature/tint sliders
/// as a Bradford chromatic adaptation from the assumed illuminant to neutral.
/// Sliders at 0/0 give the identity exactly.
pub fn white_balance_matrix(temperature: f32, tint: f32) -> Mat3 {
    if temperature == 0.0 && tint == 0.0 {
        return IDENTITY;
    }
    let (sx, sy) = illuminant_xy(temperature, tint);
    let (dx, dy) = planckian_xy(NEUTRAL_CCT);
    let src = apply(&BRADFORD, xy_to_xyz(sx, sy));
    let dst = apply(&BRADFORD, xy_to_xyz(dx, dy));
    let scale = [
        [dst[0] / src[0], 0.0, 0.0],
        [0.0, dst[1] / src[1], 0.0],
        [0.0, 0.0, dst[2] / src[2]],
    ];
    let brad_inv = invert(&BRADFORD).expect("Bradford is invertible");
    let adapt_xyz = mul(&brad_inv, &mul(&scale, &BRADFORD));
    mul(&xyz_to_srgb(), &mul(&adapt_xyz, &SRGB_TO_XYZ))
}

// ---------------------------------------------------------------- fast maths

/// log2 for positive, finite numbers, accurate to about 1e-6. Several times
/// faster than the system maths library, which matters when it runs for every
/// pixel on every slider move.
#[inline(always)]
pub fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000); // 1 ≤ m < 2
    // Fold the top half of the range down to √½…√2, without a branch (this
    // test goes either way about equally often, so a branch would be slow).
    let upper = (m > std::f32::consts::SQRT_2) as i32;
    let m = m * (1.0 - 0.5 * upper as f32);
    let e = e + upper;
    // log2(m) = 2/ln2 · atanh(s), s = (m−1)/(m+1), |s| ≤ 0.172
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    e as f32 + s * (2.885_39 + s2 * (0.961_796_7 + s2 * (0.577_078 + s2 * 0.412_198_6)))
}

/// 2^x, accurate to about 2e-5 (relative) and continuous. See [`fast_log2`].
#[inline(always)]
pub fn fast_exp2(x: f32) -> f32 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(-100.0, 100.0) };
    let whole = x as i32 - (x < 0.0) as i32;
    let f = x - whole as f32; // 0 ≤ f ≤ 1
    let p = 1.0
        + f * (std::f32::consts::LN_2
            + f * (0.240_226_5 + f * (0.055_504_11 + f * (0.009_618_13 + f * (0.001_333_36 + f * 0.000_170_7)))));
    f32::from_bits(((whole + 127) as u32) << 23) * p
}

/// base^exponent for a positive base.
#[inline(always)]
pub fn fast_pow(base: f32, exponent: f32) -> f32 {
    fast_exp2(exponent * fast_log2(base))
}

// ---------------------------------------------------------------- sRGB transfer

/// sRGB opto-electronic transfer function (linear → encoded), IEC 61966-2-1.
#[inline]
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Inverse of [`srgb_encode`].
#[inline]
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Mat3, b: Mat3, tol: f32) -> bool {
        a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < tol)
    }

    #[test]
    fn fast_maths_matches_the_exact_functions() {
        let mut x = 1e-6f32;
        while x < 1e4 {
            assert!((fast_log2(x) - x.log2()).abs() < 2e-5, "log2({x}): {} vs {}", fast_log2(x), x.log2());
            x *= 1.013;
        }
        let mut e = -20.0f32;
        while e < 20.0 {
            let (fast, exact) = (fast_exp2(e), e.exp2());
            assert!((fast / exact - 1.0).abs() < 5e-5, "exp2({e}): {fast} vs {exact}");
            e += 0.0137;
        }
        assert_eq!(fast_exp2(0.0), 1.0);
        assert_eq!(fast_exp2(f32::NAN), 1.0);
        assert!((fast_pow(0.5, 2.2) - 0.5f32.powf(2.2)).abs() < 1e-5);
    }

    #[test]
    fn inverse_round_trips() {
        let inv = invert(&SRGB_TO_XYZ).unwrap();
        assert!(close(mul(&inv, &SRGB_TO_XYZ), IDENTITY, 1e-5));
        assert!(invert(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }

    #[test]
    fn srgb_white_is_d65() {
        let w = apply(&SRGB_TO_XYZ, [1.0, 1.0, 1.0]);
        assert!((w[0] - 0.9505).abs() < 1e-3 && (w[1] - 1.0).abs() < 1e-3 && (w[2] - 1.089).abs() < 1e-3);
    }

    #[test]
    fn camera_matrix_keeps_neutral_neutral() {
        // An arbitrary but plausible XYZ→camera matrix.
        let xyz_to_cam = [[0.7, -0.1, -0.05], [-0.4, 1.2, 0.2], [-0.05, 0.15, 0.6]];
        let m = camera_to_srgb(&xyz_to_cam).unwrap();
        let out = apply(&m, [0.5, 0.5, 0.5]);
        for c in out {
            assert!((c - 0.5).abs() < 1e-4, "{out:?}");
        }
    }

    #[test]
    fn zero_sliders_are_identity() {
        assert_eq!(white_balance_matrix(0.0, 0.0), IDENTITY);
        // Tiny non-zero values must be close to identity (continuity at 0).
        assert!(close(white_balance_matrix(0.01, 0.0), IDENTITY, 1e-3));
        assert!(close(white_balance_matrix(0.0, 0.01), IDENTITY, 1e-3));
    }

    #[test]
    fn temperature_and_tint_directions() {
        let gray = [0.18, 0.18, 0.18];
        let warm = apply(&white_balance_matrix(50.0, 0.0), gray);
        assert!(warm[0] > warm[2], "warmer must have more red than blue: {warm:?}");
        let cool = apply(&white_balance_matrix(-50.0, 0.0), gray);
        assert!(cool[2] > cool[0], "cooler must have more blue than red: {cool:?}");
        let magenta = apply(&white_balance_matrix(0.0, 50.0), gray);
        assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "{magenta:?}");
        let green = apply(&white_balance_matrix(0.0, -50.0), gray);
        assert!(green[1] > green[0] && green[1] > green[2], "{green:?}");
    }

    #[test]
    fn planckian_matches_known_points() {
        // Published locus values: 2856 K ≈ (0.4476, 0.4074) (CIE A), 6500 K ≈ (0.3135, 0.3237).
        let (x, y) = planckian_xy(2856.0);
        assert!((x - 0.4476).abs() < 2e-3 && (y - 0.4074).abs() < 2e-3, "{x} {y}");
        let (x, y) = planckian_xy(6500.0);
        assert!((x - 0.3135).abs() < 2e-3 && (y - 0.3237).abs() < 2e-3, "{x} {y}");
    }

    #[test]
    fn srgb_transfer_round_trips() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((srgb_decode(srgb_encode(v)) - v).abs() < 1e-5);
        }
        assert!((srgb_encode(0.18) - 0.4614).abs() < 1e-3);
    }
}
