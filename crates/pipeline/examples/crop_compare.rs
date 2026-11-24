//! Compare fine detail at 100 % with the rendering the camera embedded in the DNG.
//!
//! ```text
//! cargo run --release -p pipeline --example crop_compare -- <out.jpg> <photo.dng> [x y]
//! ```
//!
//! Writes one row of 100 % crops of the same spot: ours with sharpening 0, 40
//! and 80, then the camera's own. `x y` (0…1) picks the spot; the default is
//! the centre. Also prints how much fine detail each crop has.

use std::path::Path;
use std::sync::Arc;

use pipeline::color::srgb_decode;
use pipeline::geometry::Affine;
use pipeline::{decode_file, render, Adjustments, LinearImage, RenderedImage, Scene, View};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

const SIZE: usize = 640;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: crop_compare <out.jpg> <photo.dng> [x y]");
        std::process::exit(2);
    }
    let spot = (
        args.get(2).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.5),
        args.get(3).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.5),
    );
    let path = Path::new(&args[1]);
    let decoded = decode_file(path).expect("decode");
    let (fw, fh) = (decoded.image.width, decoded.image.height);
    let x0 = ((fw as f64 * spot.0) as usize).saturating_sub(SIZE / 2).min(fw - SIZE);
    let y0 = ((fh as f64 * spot.1) as usize).saturating_sub(SIZE / 2).min(fh - SIZE);
    let look = decoded.info.look();
    let scene = Scene { image: &decoded.image, look: &look, lut: None };
    let view = View {
        to_source: Some(Affine { a: 1.0, b: 0.0, c: x0 as f64, d: 0.0, e: 1.0, f: y0 as f64 }),
        ..View::whole(fw, fh)
    };
    let mut crops: Vec<(String, RenderedImage)> = Vec::new();
    for amount in [0.0, 40.0, 80.0] {
        let mut adj = Adjustments::default();
        adj.sharpen.amount = amount;
        crops.push((format!("ours, sharpening {amount:.0}"), render(&scene, &adj, &view, SIZE, SIZE)));
    }

    let camera = embedded(path).oriented(decoded.info.orientation);
    if (camera.width, camera.height) == (fw, fh) {
        let mut rgb = Vec::with_capacity(SIZE * SIZE * 3);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = camera.get(x0 + x, y0 + y);
                rgb.extend(p.map(|v| (v * 255.0).round() as u8));
            }
        }
        crops.push(("camera".into(), RenderedImage { width: SIZE, height: SIZE, rgb }));
    } else {
        println!("The embedded rendering is {} x {}, not full size; skipped.", camera.width, camera.height);
    }

    let gap = 8;
    let sheet_w = crops.len() * (SIZE + gap) + gap;
    let sheet_h = SIZE + 2 * gap;
    let mut sheet = vec![40u8; sheet_w * sheet_h * 3];
    for (i, (label, crop)) in crops.iter().enumerate() {
        println!("{:<24} fine detail {:.2}", label, fine_detail(crop));
        let ox = gap + i * (SIZE + gap);
        for y in 0..SIZE {
            let dst = ((gap + y) * sheet_w + ox) * 3;
            sheet[dst..dst + SIZE * 3].copy_from_slice(&crop.rgb[y * SIZE * 3..(y + 1) * SIZE * 3]);
        }
    }
    let jpeg = pipeline::export::encode_jpeg(&RenderedImage { width: sheet_w, height: sheet_h, rgb: sheet }, 95).expect("encode");
    std::fs::write(&args[0], jpeg).expect("write");
    println!("Wrote {} (left to right as listed)", args[0]);
}

/// Average difference between neighbouring pixels, in 8-bit levels: a simple measure of sharpness.
fn fine_detail(img: &RenderedImage) -> f64 {
    let luma = |x: usize, y: usize| {
        let i = (y * img.width + x) * 3;
        0.2126 * img.rgb[i] as f64 + 0.7152 * img.rgb[i + 1] as f64 + 0.0722 * img.rgb[i + 2] as f64
    };
    let mut total = 0.0;
    for y in 0..img.height - 1 {
        for x in 0..img.width - 1 {
            total += (luma(x + 1, y) - luma(x, y)).abs() + (luma(x, y + 1) - luma(x, y)).abs();
        }
    }
    total / (2.0 * ((img.width - 1) * (img.height - 1)) as f64)
}

/// The camera's embedded rendering, as sRGB values in 0…1 (not linear).
fn embedded(path: &Path) -> LinearImage {
    let bytes = std::fs::read(path).expect("read");
    let src = RawSource::new_from_shared_vec(Arc::new(bytes));
    let dec = rawler::get_decoder(&src).expect("decoder");
    let params = RawDecodeParams::default();
    let img = match dec.preview_image(&src, &params).expect("preview") {
        Some(i) => i,
        None => dec.thumbnail_image(&src, &params).expect("thumbnail").expect("no embedded rendering"),
    };
    let rgb = img.to_rgb8();
    let pixels = rgb.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    let _ = srgb_decode(0.0);
    LinearImage::new(rgb.width() as usize, rgb.height() as usize, pixels)
}
