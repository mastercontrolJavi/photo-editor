//! End-to-end checks on generated DNG fixtures: decode → preview → export.
//! Run with `cargo test -p pipeline -- --nocapture` to see the measured numbers.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegDecoder;
use image::{DynamicImage, ImageDecoder};

use pipeline::color::{srgb_decode, srgb_encode};
use pipeline::decode::{decode_bytes, SourceKind};
use pipeline::fixtures::{make_dng, test_scene, FixtureKind, FixtureSpec};
use pipeline::{decode_file, export_jpeg, icc, render_srgb8, sidecar, Adjustments, ExportOptions, LinearImage, PipelineError};

fn write_fixture(dir: &Path, name: &str, spec: &FixtureSpec) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, make_dng(spec, test_scene(spec.width, spec.height))).unwrap();
    p
}

fn read_jpeg(path: &Path) -> (image::RgbImage, Option<Vec<u8>>) {
    let bytes = std::fs::read(path).unwrap();
    let mut dec = JpegDecoder::new(Cursor::new(bytes)).unwrap();
    let icc = dec.icc_profile().unwrap();
    let img = DynamicImage::from_decoder(dec).unwrap().to_rgb8();
    (img, icc)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[test]
fn linear_dng_decodes_to_the_scene_values() {
    let spec = FixtureSpec::new(200, 100, FixtureKind::Linear);
    let d = decode_bytes(make_dng(&spec, test_scene(200, 100))).unwrap();
    assert_eq!(d.info.kind, SourceKind::LinearRaw);
    assert_eq!((d.image.width, d.image.height), (200, 100));
    let scene = test_scene(200, 100);
    for (x, y) in [(0, 0), (50, 20), (150, 30), (60, 80), (199, 99)] {
        let want = scene(x, y);
        let got = d.image.get(x, y);
        for c in 0..3 {
            assert!((got[c] as f64 - want[c]).abs() < 1e-3, "({x},{y}) ch{c}: {got:?} vs {want:?}");
        }
    }
}

#[test]
fn as_shot_white_balance_and_colour_matrix_make_camera_neutral_gray() {
    // A camera whose native space is nothing like sRGB, and a warm as-shot light.
    let mut spec = FixtureSpec::new(16, 16, FixtureKind::Linear);
    spec.color_matrix = [0.75, -0.2, -0.05, -0.45, 1.3, 0.15, -0.08, 0.2, 0.55];
    spec.as_shot_neutral = [0.6, 1.0, 0.45];
    // The scene is a gray card under that light: the camera records the neutral itself.
    let n = spec.as_shot_neutral;
    let d = decode_bytes(make_dng(&spec, move |_, _| [0.4 * n[0], 0.4 * n[1], 0.4 * n[2]])).unwrap();
    let px = d.image.get(8, 8);
    assert!((px[0] - px[1]).abs() < 2e-3 && (px[2] - px[1]).abs() < 2e-3, "not neutral: {px:?}");
}

#[test]
fn baseline_exposure_is_read_from_the_file() {
    let mut spec = FixtureSpec::new(16, 16, FixtureKind::Linear);
    spec.baseline_exposure = 0.75;
    let d = decode_bytes(make_dng(&spec, |_, _| [0.1; 3])).unwrap();
    assert!((d.info.baseline_exposure - 0.75).abs() < 1e-4, "{}", d.info.baseline_exposure);
}

#[test]
fn bayer_dng_is_demosaiced() {
    let spec = FixtureSpec::new(64, 48, FixtureKind::Bayer);
    let d = decode_bytes(make_dng(&spec, |_, _| [0.5, 0.25, 0.125])).unwrap();
    assert_eq!(d.info.kind, SourceKind::BayerCfa);
    assert_eq!((d.image.width, d.image.height), (64, 48));
    let px = d.image.get(30, 20);
    assert!((px[0] - 0.5).abs() < 0.01 && (px[1] - 0.25).abs() < 0.01 && (px[2] - 0.125).abs() < 0.01, "{px:?}");
}

#[test]
fn orientation_is_applied_to_preview_and_export() {
    let dir = tempfile::tempdir().unwrap();
    // Stored landscape 300×200 with the red marker in the stored top-left;
    // orientation 6 means "rotate 90° clockwise to display".
    let spec = FixtureSpec { orientation: 6, ..FixtureSpec::new(300, 200, FixtureKind::Linear) };
    let src = write_fixture(dir.path(), "rotated.dng", &spec);
    let d = decode_file(&src).unwrap();
    assert_eq!((d.info.width, d.info.height), (200, 300), "portrait after rotation");
    // After a clockwise turn the stored top-left corner is at the top-right.
    let marker = d.image.get(195, 5);
    assert!(marker[0] > 0.8 && marker[1] < 0.1, "marker not top-right: {marker:?}");

    let out = export_jpeg(&src, &d, &Adjustments::default(), &ExportOptions::next_to(&src)).unwrap();
    let (jpg, _) = read_jpeg(&out);
    assert_eq!(jpg.dimensions(), (200, 300));
    let p = jpg.get_pixel(195, 5);
    assert!(p[0] > 200 && p[1] < 90, "exported marker not top-right: {p:?}");
}

#[test]
fn export_is_full_size_srgb_tagged_and_leaves_the_source_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec::new(640, 480, FixtureKind::Linear);
    let src = write_fixture(dir.path(), "IMG_0001.DNG", &spec);
    let before = fnv1a(&std::fs::read(&src).unwrap());
    let modified_before = std::fs::metadata(&src).unwrap().modified().unwrap();

    let d = decode_file(&src).unwrap();
    let adj = Adjustments { exposure: 0.5, temperature: 20.0, tint: -10.0 };
    sidecar::save(&src, &adj).unwrap();
    let opts = ExportOptions::next_to(&src);
    let first = export_jpeg(&src, &d, &adj, &opts).unwrap();
    let second = export_jpeg(&src, &d, &adj, &opts).unwrap();

    assert_eq!(first, dir.path().join("Exported").join("IMG_0001.jpg"));
    assert_eq!(second, dir.path().join("Exported").join("IMG_0001-1.jpg"));
    let (jpg, icc_bytes) = read_jpeg(&first);
    assert_eq!(jpg.dimensions(), (640, 480), "full resolution, same aspect ratio");
    assert_eq!(icc_bytes.as_deref(), Some(icc::srgb_profile()), "sRGB ICC profile embedded");

    assert_eq!(fnv1a(&std::fs::read(&src).unwrap()), before, "source bytes changed");
    assert_eq!(std::fs::metadata(&src).unwrap().modified().unwrap(), modified_before);
    assert_eq!(sidecar::load(&src).unwrap(), Some(adj), "edit restored on reopen");
}

/// Downscale an 8-bit sRGB image by area-averaging in linear light (the same
/// thing the preview does before encoding).
fn downscale_srgb(img: &image::RgbImage, w: usize, h: usize) -> Vec<[f32; 3]> {
    let lin = LinearImage::new(
        img.width() as usize,
        img.height() as usize,
        img.pixels().map(|p| [0, 1, 2].map(|c| srgb_decode(p[c] as f32 / 255.0))).collect(),
    );
    lin.resize_box(w, h).pixels.iter().map(|p| p.map(|v| srgb_encode(v) * 255.0)).collect()
}

#[test]
fn preview_agrees_with_export_scaled_to_preview_size() {
    // Documented tolerance (8-bit sRGB levels, 0–255), measured over all
    // channels of every preview pixel:
    const MEAN_TOL: f64 = 1.0;
    const P99_TOL: f64 = 3.0;
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec::new(1200, 800, FixtureKind::Linear);
    let src = write_fixture(dir.path(), "agree.dng", &spec);
    let d = decode_file(&src).unwrap();
    for adj in [
        Adjustments::default(),
        Adjustments { exposure: 1.2, temperature: 35.0, tint: 15.0 },
        Adjustments { exposure: -0.8, temperature: -60.0, tint: -25.0 },
    ] {
        let preview_src = d.image.downscale_to_fit(400);
        let preview = render_srgb8(&preview_src, &adj, &d.info.look());
        let out = export_jpeg(&src, &d, &adj, &ExportOptions::next_to(&src)).unwrap();
        let (jpg, _) = read_jpeg(&out);
        let scaled = downscale_srgb(&jpg, preview.width, preview.height);
        let mut diffs: Vec<f64> = preview
            .rgb
            .chunks_exact(3)
            .zip(&scaled)
            .flat_map(|(p, s)| (0..3).map(move |c| (p[c] as f64 - s[c] as f64).abs()))
            .collect();
        diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        let p99 = diffs[diffs.len() * 99 / 100];
        let max = *diffs.last().unwrap();
        println!("preview vs export {adj:?}: mean {mean:.3}, p99 {p99:.2}, max {max:.2} levels");
        assert!(mean <= MEAN_TOL && p99 <= P99_TOL, "mean {mean:.3} p99 {p99:.2}");
    }
}

#[test]
fn bad_files_give_clear_errors_not_crashes() {
    let dir = tempfile::tempdir().unwrap();

    let missing = decode_file(&dir.path().join("nope.dng")).unwrap_err();
    assert!(matches!(missing, PipelineError::Read { .. }), "{missing}");

    let empty = dir.path().join("empty.dng");
    std::fs::write(&empty, b"").unwrap();
    assert!(matches!(decode_file(&empty).unwrap_err(), PipelineError::EmptyFile { .. }));

    let garbage: Vec<u8> = (0..10_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let e = decode_bytes(garbage).unwrap_err();
    assert!(matches!(e, PipelineError::Unreadable { .. } | PipelineError::Unsupported { .. }), "{e}");

    // Ordinary iPhone JPEG / HEIC files are named as such, not called "damaged".
    let jpeg = decode_bytes(b"\xFF\xD8\xFF\xE1rest of a jpeg".to_vec()).unwrap_err();
    assert!(matches!(jpeg, PipelineError::Unsupported { .. }) && jpeg.to_string().contains("JPEG"), "{jpeg}");
    let heic = decode_bytes(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec()).unwrap_err();
    assert!(matches!(heic, PipelineError::Unsupported { .. }) && heic.to_string().contains("HEIC"), "{heic}");

    let text = decode_bytes(b"II*\0this is not a real photo".to_vec()).unwrap_err();
    println!("tiny TIFF header: {text}");

    // A real fixture cut short at several points (as if a copy was interrupted).
    let full = make_dng(&FixtureSpec::new(300, 200, FixtureKind::Linear), test_scene(300, 200));
    for cut in [16, 200, 600, full.len() / 2, full.len() - 10] {
        let r = decode_bytes(full[..cut].to_vec());
        assert!(r.is_err(), "truncated at {cut} bytes should fail, not decode");
        println!("truncated at {cut}: {}", r.unwrap_err());
    }
}
