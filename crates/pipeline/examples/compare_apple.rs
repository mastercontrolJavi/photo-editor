//! Compare our default rendering of a DNG with the rendering the camera embedded in it.
//!
//! ```text
//! cargo run --release -p pipeline --example compare_apple -- <results folder> <photo.dng>...
//! ```
//!
//! For each photo this prints brightness / contrast / clipping / saturation for both
//! renderings, works out which rotation of the embedded image lines up with ours (an
//! independent orientation check), checks `<results folder>\exports\<name>.jpg` if it
//! exists, and writes `<results folder>\compare\<name>.jpg` (ours left, camera right).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use image::ImageDecoder;
use pipeline::color::{srgb_decode, srgb_encode};
use pipeline::{decode_file, render_srgb8, Adjustments, LinearImage, Look, RenderedImage};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

/// Both renderings are compared at this long edge.
const COMPARE_EDGE: usize = 1200;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, photos)) = args.split_first() else {
        eprintln!("usage: compare_apple <results folder> <photo.dng>...");
        std::process::exit(2);
    };
    let out = Path::new(out);
    std::fs::create_dir_all(out.join("compare")).expect("create compare folder");
    for p in photos {
        let p = Path::new(p);
        println!("===== {} =====", p.file_name().unwrap().to_string_lossy());
        if let Err(e) = compare(p, out) {
            println!("FAILED: {e}");
        }
        println!();
    }
}

fn compare(path: &Path, out: &Path) -> Result<(), String> {
    let stem = path.file_stem().unwrap().to_string_lossy().to_string();
    let decoded = decode_file(path).map_err(|e| e.to_string())?;
    // Render exactly as the app does (default sliders, the file's look).
    let small = decoded.image.downscale_to_fit(COMPARE_EDGE);
    let look = decoded.info.look();
    println!(
        "Look: baseline {:+.2} EV, tone curve {}, gain map {}",
        look.baseline_exposure,
        if look.tone_curve.is_some() { "applied" } else { "none" },
        if decoded.info.gain_map_applied { "applied" } else { "none" }
    );
    let render = |exposure: f32| to_linear(&render_srgb8(&small, &Adjustments { exposure, ..Default::default() }, &look));
    let ours = render(0.0);
    let (w, h) = (ours.width, ours.height);

    let camera = embedded_preview(path)?;
    println!("Embedded camera rendering: {} x {} (stored)", camera.width, camera.height);

    // Find which rotation of the embedded image matches ours best.
    let ours_l = luma_levels(&ours);
    let mut best: Option<(u16, f64, LinearImage)> = None;
    for code in 1..=8u16 {
        let o = camera.clone().oriented(code);
        if (o.width > o.height) != (w > h) {
            continue;
        }
        let o = o.resize_box(w, h);
        let r = correlation(&ours_l, &luma_levels(&o));
        if best.as_ref().is_none_or(|b| r > b.1) {
            best = Some((code, r, o));
        }
    }
    let (code, r, camera) = best.ok_or("embedded image has a different shape")?;
    let file_code = decoded.info.orientation;
    println!(
        "Orientation: file says {file_code}; best match for the camera image is {code} (correlation {r:.3}) -> {}",
        if code == file_code { "AGREES" } else { "DISAGREES" }
    );

    let a = stats(&ours);
    let b = stats(&camera);
    let gap_ev = (b.median_y / a.median_y.max(1e-6)).log2();
    println!("                       ours   camera");
    println!("Mean brightness (0-255) {:6.1} {:6.1}", a.mean_l, b.mean_l);
    println!("Shadows p5 / p50 / p95  {:3.0}/{:3.0}/{:3.0}  {:3.0}/{:3.0}/{:3.0}", a.p5, a.p50, a.p95, b.p5, b.p50, b.p95);
    println!("Contrast (std of levels) {:5.1} {:6.1}", a.std_l, b.std_l);
    println!("Blown highlights %      {:6.2} {:6.2}", a.clip_pct, b.clip_pct);
    println!("Crushed shadows %       {:6.2} {:6.2}", a.crush_pct, b.crush_pct);
    println!("Saturation (0-1)        {:6.3} {:6.3}", a.sat, b.sat);
    println!("Mean R/G/B              {:3.0}/{:3.0}/{:3.0}  {:3.0}/{:3.0}/{:3.0}", a.rgb[0], a.rgb[1], a.rgb[2], b.rgb[0], b.rgb[1], b.rgb[2]);
    println!("Mid-tone exposure gap: camera is {gap_ev:+.2} EV vs ours");

    let diff = mean_abs_diff(&ours, &camera);
    println!("Mean difference from camera: {diff:.1} levels");
    println!(
        "SUMMARY {stem} mean {:.0}/{:.0} gap {gap_ev:+.2} contrast {:.0}/{:.0} sat {:.3}/{:.3} blown {:.2}/{:.2} diff {diff:.1}",
        a.mean_l, b.mean_l, a.std_l, b.std_l, a.sat, b.sat, a.clip_pct, b.clip_pct
    );

    // How close do we get if only exposure is matched? (Remaining gap = tone shape.)
    let matched = render(gap_ev as f32);
    let m = stats(&matched);
    println!(
        "After matching exposure: contrast {:.1} vs {:.1}, blown {:.2}% vs {:.2}%, mean diff {:.1} levels",
        m.std_l,
        b.std_l,
        m.clip_pct,
        b.clip_pct,
        mean_abs_diff(&matched, &camera)
    );

    check_export(&out.join("exports").join(format!("{stem}.jpg")), &decoded)?;

    // Side-by-side: ours | camera.
    let gap = 16;
    let mut both = LinearImage::new(w * 2 + gap, h, vec![[0.08; 3]; (w * 2 + gap) * h]);
    for y in 0..h {
        for x in 0..w {
            both.pixels[y * both.width + x] = ours.get(x, y);
            both.pixels[y * both.width + w + gap + x] = camera.get(x, y);
        }
    }
    let rendered = render_srgb8(&both, &Adjustments::default(), &Look::neutral());
    let jpeg = pipeline::export::encode_jpeg(&rendered, 90).map_err(|e| e.to_string())?;
    let dest = out.join("compare").join(format!("{stem}.jpg"));
    std::fs::write(&dest, jpeg).map_err(|e| e.to_string())?;
    println!("Side-by-side (ours left, camera right): {}", dest.display());
    Ok(())
}

fn to_linear(r: &RenderedImage) -> LinearImage {
    let lut: Vec<f32> = (0..256).map(|i| srgb_decode(i as f32 / 255.0)).collect();
    let pixels = r.rgb.as_chunks::<3>().0.iter().map(|p| [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]]).collect();
    LinearImage::new(r.width, r.height, pixels)
}

fn embedded_preview(path: &Path) -> Result<LinearImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let src = RawSource::new_from_shared_vec(Arc::new(bytes));
    let dec = rawler::get_decoder(&src).map_err(|e| e.to_string())?;
    // Apple ProRAW stores its full-size rendering in IFD0 (NewSubFileType 1), which
    // rawler exposes as the "thumbnail"; other DNGs keep it in a SubIFD ("preview").
    let params = RawDecodeParams::default();
    let img = match dec.preview_image(&src, &params).map_err(|e| e.to_string())? {
        Some(i) => i,
        None => dec.thumbnail_image(&src, &params).map_err(|e| e.to_string())?.ok_or("no embedded rendering in this file")?,
    };
    let rgb = img.to_rgb8();
    let lut: Vec<f32> = (0..256).map(|i| srgb_decode(i as f32 / 255.0)).collect();
    let pixels = rgb.pixels().map(|p| [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]]).collect();
    Ok(LinearImage::new(rgb.width() as usize, rgb.height() as usize, pixels))
}

fn check_export(jpg: &Path, decoded: &pipeline::DecodedImage) -> Result<(), String> {
    if !jpg.exists() {
        println!("Export check: {} not found, skipped", jpg.display());
        return Ok(());
    }
    let file = File::open(jpg).map_err(|e| e.to_string())?;
    let mut dec = image::codecs::jpeg::JpegDecoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    let (ew, eh) = dec.dimensions();
    let icc = dec.icc_profile().map_err(|e| e.to_string())?;
    let size_ok = ew as usize == decoded.image.width && eh as usize == decoded.image.height;
    let icc_ok = icc.as_deref() == Some(pipeline::icc::srgb_profile());
    println!(
        "Export check: {ew} x {eh} ({}), sRGB profile {}",
        if size_ok { "full size, upright" } else { "SIZE MISMATCH" },
        if icc_ok { "embedded" } else { "MISSING" }
    );
    Ok(())
}

struct Stats {
    mean_l: f64,
    p5: f64,
    p50: f64,
    p95: f64,
    std_l: f64,
    clip_pct: f64,
    crush_pct: f64,
    sat: f64,
    rgb: [f64; 3],
    median_y: f64,
}

fn luma_y(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn luma_levels(img: &LinearImage) -> Vec<f64> {
    img.pixels.iter().map(|&p| srgb_encode(luma_y(p).clamp(0.0, 1.0)) as f64 * 255.0).collect()
}

fn stats(img: &LinearImage) -> Stats {
    let n = img.pixels.len() as f64;
    let mut l = luma_levels(img);
    let mean_l = l.iter().sum::<f64>() / n;
    let std_l = (l.iter().map(|v| (v - mean_l).powi(2)).sum::<f64>() / n).sqrt();
    let mut ys: Vec<f64> = img.pixels.iter().map(|&p| luma_y(p) as f64).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    l.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f64| l[((l.len() - 1) as f64 * q) as usize];
    let mut clip = 0usize;
    let mut crush = 0usize;
    let mut sat = 0f64;
    let mut sat_n = 0usize;
    let mut rgb = [0f64; 3];
    for &p in &img.pixels {
        let e = p.map(|v| srgb_encode(v.clamp(0.0, 1.0)));
        let mx = e[0].max(e[1]).max(e[2]);
        let mn = e[0].min(e[1]).min(e[2]);
        if mx >= 254.0 / 255.0 {
            clip += 1;
        }
        if mx <= 3.0 / 255.0 {
            crush += 1;
        }
        if mx > 0.05 {
            sat += ((mx - mn) / mx) as f64;
            sat_n += 1;
        }
        for c in 0..3 {
            rgb[c] += e[c] as f64 * 255.0;
        }
    }
    Stats {
        mean_l,
        p5: pct(0.05),
        p50: pct(0.5),
        p95: pct(0.95),
        std_l,
        clip_pct: clip as f64 / n * 100.0,
        crush_pct: crush as f64 / n * 100.0,
        sat: sat / sat_n.max(1) as f64,
        rgb: rgb.map(|v| v / n),
        median_y: ys[ys.len() / 2],
    }
}

fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma).powi(2);
        sbb += (y - mb).powi(2);
    }
    sab / (saa * sbb).sqrt().max(1e-12)
}

fn mean_abs_diff(a: &LinearImage, b: &LinearImage) -> f64 {
    let (la, lb) = (luma_levels(a), luma_levels(b));
    la.iter().zip(&lb).map(|(x, y)| (x - y).abs()).sum::<f64>() / la.len() as f64
}
