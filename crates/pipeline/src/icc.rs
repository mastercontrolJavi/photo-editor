//! A small ICC v2 sRGB display profile, built in code so there is no binary
//! blob of unknown licence in the repo. Embedded in every exported JPEG so
//! viewers know the pixels are sRGB instead of guessing.

use std::sync::OnceLock;

use crate::color::srgb_decode;

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    t.extend(s15f16(x));
    t.extend(s15f16(y));
    t.extend(s15f16(z));
    t
}

fn text_tag(s: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend(s.as_bytes());
    t.push(0);
    t
}

fn desc_tag(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend((s.len() as u32 + 1).to_be_bytes());
    t.extend(s.as_bytes());
    t.push(0);
    t.extend([0u8; 4]); // Unicode language code
    t.extend([0u8; 4]); // Unicode count
    t.extend([0u8; 2]); // ScriptCode code
    t.push(0); // ScriptCode count
    t.extend([0u8; 67]); // ScriptCode description
    t
}

fn curve_tag() -> Vec<u8> {
    const N: usize = 1024;
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend((N as u32).to_be_bytes());
    for i in 0..N {
        let v = srgb_decode(i as f32 / (N - 1) as f32) as f64;
        t.extend(((v * 65535.0).round() as u16).to_be_bytes());
    }
    t
}

fn build() -> Vec<u8> {
    // sRGB primaries chromatically adapted to the D50 profile connection
    // space with Bradford, as in the standard sRGB IEC61966-2.1 profile.
    let curve = curve_tag();
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag("sRGB IEC61966-2.1 (Photo Editor)")),
        (b"cprt", text_tag("No copyright, use freely")),
        (b"wtpt", xyz_tag(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz_tag(0.436_074_7, 0.222_504_5, 0.013_932_2)),
        (b"gXYZ", xyz_tag(0.385_064_9, 0.716_878_6, 0.097_104_5)),
        (b"bXYZ", xyz_tag(0.143_080_4, 0.060_616_9, 0.714_173_3)),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];

    let header_len = 128;
    let table_len = 4 + 12 * tags.len();
    let mut data = Vec::new();
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut curve_offset: Option<(u32, u32)> = None;
    for (sig, body) in &tags {
        let is_trc = sig.ends_with(b"TRC");
        // The three tone curves are identical: store one copy, point all three at it.
        let (off, len) = match (is_trc, curve_offset) {
            (true, Some(shared)) => shared,
            _ => {
                while data.len() % 4 != 0 {
                    data.push(0);
                }
                let off = (header_len + table_len + data.len()) as u32;
                data.extend(body);
                let entry = (off, body.len() as u32);
                if is_trc {
                    curve_offset = Some(entry);
                }
                entry
            }
        };
        table.extend(*sig);
        table.extend(off.to_be_bytes());
        table.extend(len.to_be_bytes());
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let total = header_len + table_len + data.len();

    let mut h = Vec::with_capacity(header_len);
    h.extend((total as u32).to_be_bytes()); // profile size
    h.extend([0u8; 4]); // preferred CMM
    h.extend([0x02, 0x10, 0, 0]); // version 2.1
    h.extend(b"mntr"); // display device class
    h.extend(b"RGB "); // data colour space
    h.extend(b"XYZ "); // profile connection space
    h.extend([0x07, 0xEA, 0, 11, 0, 22, 0, 0, 0, 0, 0, 0]); // 2026-11-22 00:00:00
    h.extend(b"acsp");
    h.extend(b"MSFT"); // primary platform
    h.extend([0u8; 4]); // flags
    h.extend([0u8; 4]); // device manufacturer
    h.extend([0u8; 4]); // device model
    h.extend([0u8; 8]); // device attributes
    h.extend([0, 0, 0, 0]); // rendering intent: perceptual
    h.extend(s15f16(0.9642)); // PCS illuminant (D50)
    h.extend(s15f16(1.0));
    h.extend(s15f16(0.8249));
    h.extend([0u8; 4]); // creator
    h.extend([0u8; 44]); // profile id (v4 only) + reserved
    debug_assert_eq!(h.len(), header_len);

    let mut out = h;
    out.extend(table);
    out.extend(data);
    debug_assert_eq!(out.len(), total);
    out
}

/// The sRGB profile bytes (built once).
pub fn srgb_profile() -> &'static [u8] {
    static P: OnceLock<Vec<u8>> = OnceLock::new();
    P.get_or_init(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_tag_table_are_well_formed() {
        let p = srgb_profile();
        let size = u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize;
        assert_eq!(size, p.len());
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[12..16], b"mntr");
        assert_eq!(&p[16..20], b"RGB ");
        let count = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(count, 9);
        for i in 0..count {
            let e = 132 + i * 12;
            let off = u32::from_be_bytes(p[e + 4..e + 8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(p[e + 8..e + 12].try_into().unwrap()) as usize;
            assert_eq!(off % 4, 0, "tag {i} not 4-byte aligned");
            assert!(off + len <= p.len(), "tag {i} runs past the end");
        }
        // Compact enough to embed without fuss.
        assert!(p.len() < 4096, "{} bytes", p.len());
    }
}
