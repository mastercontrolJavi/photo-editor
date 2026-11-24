//! End-to-end checks on generated DNG fixtures: decode → preview → export.
//! Run with `cargo test -p pipeline -- --nocapture` to see the measured numbers.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegDecoder;
use image::{DynamicImage, ImageDecoder};

use pipeline::color::{srgb_decode, srgb_encode};
use pipeline::decode::{decode_bytes, SourceKind};
use pipeline::fixtures::{make_dng, test_scene, FixtureKind, FixtureSpec};
use pipeline::{decode_file, export_jpeg, icc, render_preview, sidecar, Adjustments, ExportOptions, LinearImage, PipelineError};

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
    let adj = Adjustments { exposure: 0.5, temperature: 20.0, tint: -10.0, ..Default::default() };
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

#[test]
fn an_already_processed_photo_goes_through_the_whole_loop() {
    use pipeline::decode::SourceKind;
    let dir = tempfile::tempdir().unwrap();
    // A PNG saved under a misleading name: the type comes from the contents.
    let picture = image::RgbImage::from_fn(320, 200, |x, y| image::Rgb([(x / 2) as u8, (y) as u8, 90]));
    let src = dir.path().join("holiday.dng");
    picture.save_with_format(&src, image::ImageFormat::Png).unwrap();
    let before = fnv1a(&std::fs::read(&src).unwrap());

    let d = decode_file(&src).unwrap();
    assert_eq!((d.info.kind, d.info.format.as_str()), (SourceKind::Rendered, "PNG"));
    assert_eq!((d.info.width, d.info.height), (320, 200));
    // It starts with no sharpening, unlike a RAW photo.
    let fresh = d.info.starting_adjustments();
    assert_eq!(fresh.sharpen.amount, 0.0);
    assert!(Adjustments::default().sharpen.amount > 0.0);

    // Unedited, the export shows the same picture (within JPEG's own error).
    let out = export_jpeg(&src, &d, &fresh, &ExportOptions::next_to(&src)).unwrap();
    let (jpg, icc_bytes) = read_jpeg(&out);
    assert_eq!(jpg.dimensions(), (320, 200));
    assert_eq!(icc_bytes.as_deref(), Some(icc::srgb_profile()));
    let worst = jpg.pixels().zip(picture.pixels()).flat_map(|(a, b)| (0..3).map(move |c| (a[c] as i32 - b[c] as i32).abs())).max().unwrap();
    assert!(worst <= 12, "unedited export differs from the original by up to {worst} levels");

    // An edit with a profile saves, reloads and changes the export.
    let mut adj = Adjustments { exposure: 0.5, ..fresh };
    adj.profile.name = "mono".into();
    sidecar::save(&src, &adj).unwrap();
    assert_eq!(sidecar::load(&src).unwrap(), Some(adj.clone()));
    let out = export_jpeg(&src, &d, &adj, &ExportOptions::next_to(&src)).unwrap();
    let (mono, _) = read_jpeg(&out);
    let p = mono.get_pixel(200, 100);
    assert!((p[0] as i32 - p[2] as i32).abs() <= 3, "monochrome profile: {p:?}");
    assert_eq!(fnv1a(&std::fs::read(&src).unwrap()), before, "the original is never written to");
}

#[test]
fn export_applies_the_crop_and_turns() {
    let dir = tempfile::tempdir().unwrap();
    let src = write_fixture(dir.path(), "crop.dng", &FixtureSpec::new(600, 400, FixtureKind::Linear));
    let d = decode_file(&src).unwrap();
    let mut adj = Adjustments::default();
    adj.crop.set_aspect(1.0, 600.0, 400.0);
    adj.crop.rotate_cw();
    let out = export_jpeg(&src, &d, &adj, &ExportOptions::next_to(&src)).unwrap();
    assert_eq!(read_jpeg(&out).0.dimensions(), (400, 400));
    // Straightening keeps the shape and trims the corners.
    let mut adj = Adjustments::default();
    adj.crop.angle = 8.0;
    adj.crop.fit(600.0, 400.0);
    let out = export_jpeg(&src, &d, &adj, &ExportOptions::next_to(&src)).unwrap();
    let (w, h) = read_jpeg(&out).0.dimensions();
    assert!(w < 600 && h < 400 && ((w as f64 / h as f64) - 1.5).abs() < 0.01, "{w} x {h}");
    // An edit that names a look-up table that isn't installed fails clearly.
    let lut = pipeline::adjust::LutUse { name: "not-installed-5c1e.cube".into(), amount: 100.0 };
    let adj = Adjustments { lut: Some(lut), ..Default::default() };
    let err = export_jpeg(&src, &d, &adj, &ExportOptions::next_to(&src)).unwrap_err();
    assert!(matches!(err, PipelineError::Look(_)), "{err}");
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

/// Most sliders in use together (no grain: it is random by design).
fn everything_at_once() -> Adjustments {
    let mut a = Adjustments {
        exposure: 0.3,
        contrast: 25.0,
        highlights: -40.0,
        shadows: 35.0,
        whites: 10.0,
        blacks: -10.0,
        vibrance: 20.0,
        saturation: -8.0,
        clarity: 30.0,
        dehaze: 15.0,
        ..Default::default()
    };
    a.curve.rgb = vec![[0.0, 0.05], [0.5, 0.52], [1.0, 0.97]];
    a.mixer[5].saturation = 30.0;
    a.mixer[1].luminance = 10.0;
    a.grading.shadows = pipeline::adjust::Wheel { hue: 210.0, saturation: 30.0, luminance: 0.0 };
    a.vignette.amount = -30.0;
    a
}

/// Masks of each kind, a picked colour, a healing spot and a perspective warp.
fn local_work() -> Adjustments {
    use pipeline::adjust::{BrushStroke, Geometry, LocalAdjust, Mask, MaskShape, PointColor, Spot, SpotMode};
    let mut a = Adjustments { exposure: 0.2, ..Default::default() };
    let darker = LocalAdjust { exposure: -1.0, highlights: -30.0, saturation: 20.0, ..Default::default() };
    let warmer = LocalAdjust { exposure: 0.6, temperature: 30.0, shadows: 25.0, ..Default::default() };
    a.masks.push(Mask { shape: MaskShape::Linear { from: [0.5, 0.1], to: [0.45, 0.5] }, invert: false, adjust: darker });
    a.masks.push(Mask { shape: MaskShape::Radial { centre: [0.6, 0.55], radius: [0.25, 0.15], angle: 20.0, feather: 60.0 }, invert: true, adjust: warmer });
    let stroke = BrushStroke { points: vec![[0.1, 0.8], [0.4, 0.7], [0.7, 0.85]], size: 0.06, feather: 70.0, erase: false };
    a.masks.push(Mask { shape: MaskShape::Brush { strokes: vec![stroke] }, invert: false, adjust: warmer });
    a.point_colors.push(PointColor { hue: 30.0, saturation: 0.6, luminance: 0.4, hue_shift: 40.0, sat_shift: -30.0, lum_shift: 20.0, range: 60.0 });
    a.spots.push(Spot { at: [0.3, 0.4], source: [0.45, 0.42], radius: 0.05, feather: 50.0, opacity: 100.0, mode: SpotMode::Heal });
    a.spots.push(Spot { at: [0.7, 0.3], source: [0.8, 0.35], radius: 0.03, feather: 30.0, opacity: 80.0, mode: SpotMode::Clone });
    a.geometry = Geometry { vertical: -20.0, horizontal: 10.0, rotate: 2.0, constrain: true, ..Default::default() };
    a
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
        Adjustments { exposure: 1.2, temperature: 35.0, tint: 15.0, ..Default::default() },
        Adjustments { exposure: -0.8, temperature: -60.0, tint: -25.0, ..Default::default() },
        everything_at_once(),
        local_work(),
    ] {
        let preview_src = d.image.downscale_to_fit(400);
        let preview = render_preview(&preview_src, &adj, &d.info.look(), None, (d.image.width, d.image.height));
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
        println!("preview vs export (exposure {}, {} masks): mean {mean:.3}, p99 {p99:.2}, max {max:.2} levels", adj.exposure, adj.masks.len());
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

    // A damaged JPEG or HEIC is named as such in the message.
    let jpeg = decode_bytes(b"\xFF\xD8\xFF\xE1rest of a jpeg".to_vec()).unwrap_err();
    assert!(jpeg.to_string().contains("JPEG"), "{jpeg}");
    let heic = decode_bytes(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec()).unwrap_err();
    assert!(heic.to_string().contains("HEIC"), "{heic}");
    // Files the editor doesn't open say what they are and what it does open.
    let video = decode_bytes(b"\0\0\0\x18ftypmp42\0\0\0\0isommp42".to_vec()).unwrap_err();
    assert!(matches!(video, PipelineError::Unsupported { .. }) && video.to_string().contains("JPEG, PNG and HEIC"), "{video}");
    for (bytes, what) in [
        (&b"GIF89a  "[..], "GIF"),
        (&b"RIFF$   WEBPVP8 "[..], "WebP"),
        (&b"   ftypavif    avifmif1"[..], "AVIF"),
    ] {
        let e = decode_bytes(bytes.to_vec()).unwrap_err();
        assert!(matches!(e, PipelineError::Unsupported { .. }) && e.to_string().contains(what), "{what}: {e}");
    }

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
