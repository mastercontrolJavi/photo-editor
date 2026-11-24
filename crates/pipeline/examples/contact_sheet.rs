//! Render one photo many ways side by side, to check sliders and presets by eye.
//!
//! ```text
//! cargo run --release -p pipeline --example contact_sheet -- <presets|profiles|sliders|optics|detail> <out.jpg> <photo> [x y]
//! ```
//!
//! Cells are filled left to right, top to bottom, in the order printed.
//! `detail` shows a 100 % crop around the point `x y` (0…1, default the centre);
//! the other modes show the whole photo.

use std::path::Path;

use pipeline::{decode_file, render, Adjustments, LinearImage, RenderedImage, Scene, View};

const CELL_EDGE: usize = 560;
const COLUMNS: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, out, photo, ..] = args.as_slice() else {
        eprintln!("usage: contact_sheet <presets|profiles|sliders|optics|detail> <out.jpg> <photo> [x y]");
        std::process::exit(2);
    };
    let decoded = decode_file(Path::new(photo)).expect("decode");
    let fresh = decoded.info.starting_adjustments();
    let cells: Vec<(String, Adjustments)> = match mode.as_str() {
        "presets" => std::iter::once(("Original".to_string(), fresh.clone()))
            .chain(pipeline::preset::builtin().into_iter().map(|p| (p.name.clone(), p.apply_from(&fresh, &fresh))))
            .collect(),
        "profiles" => pipeline::profile::choices(decoded.info.kind, &decoded.info.make)
            .into_iter()
            .map(|p| {
                let mut a = fresh.clone();
                a.profile.name = p.id.to_string();
                (p.label, a)
            })
            .collect(),
        "optics" => optics_cells(),
        "detail" => detail_cells(),
        _ => slider_cells(),
    };
    let spot: Vec<f64> = args[3..].iter().filter_map(|v| v.parse().ok()).collect();
    let spot = (spot.first().copied().unwrap_or(0.5), spot.get(1).copied().unwrap_or(0.5));
    let small = decoded.image.downscale_to_fit(CELL_EDGE);
    let look = decoded.info.look();
    let (mut w, mut h) = (small.width, small.height);
    if mode == "detail" {
        (w, h) = (CELL_EDGE, CELL_EDGE * 3 / 4);
    }
    let (fw, fh) = (decoded.image.width, decoded.image.height);
    // In detail mode: the part of the photo shown, one output pixel per photo pixel.
    let part = {
        let (pw, ph) = (w as f64 / fw as f64, h as f64 / fh as f64);
        let (u0, v0) = ((spot.0 - pw / 2.0).clamp(0.0, 1.0 - pw), (spot.1 - ph / 2.0).clamp(0.0, 1.0 - ph));
        [u0, v0, u0 + pw, v0 + ph]
    };
    let gap = 6;
    let rows = cells.len().div_ceil(COLUMNS);
    let (sheet_w, sheet_h) = (COLUMNS * (w + gap) + gap, rows * (h + gap) + gap);
    let mut sheet = vec![40u8; sheet_w * sheet_h * 3];
    for (i, (label, adj)) in cells.iter().enumerate() {
        println!("{:>2}  row {} col {}  {label}", i + 1, i / COLUMNS + 1, i % COLUMNS + 1);
        let r = if mode == "detail" {
            let scene = Scene { image: &decoded.image, look: &look, lut: None };
            render(&scene, adj, &View::region(&adj.crop, fw, fh, part, w, h), w, h)
        } else {
            let scene = Scene { image: &small, look: &look, lut: None };
            render(&scene, adj, &View::preview(&adj.crop, w, h, fw, fh), w, h)
        };
        let (ox, oy) = (gap + (i % COLUMNS) * (w + gap), gap + (i / COLUMNS) * (h + gap));
        for y in 0..h {
            let dst = ((oy + y) * sheet_w + ox) * 3;
            sheet[dst..dst + w * 3].copy_from_slice(&r.rgb[y * w * 3..(y + 1) * w * 3]);
        }
    }
    let jpeg = pipeline::export::encode_jpeg(&RenderedImage { width: sheet_w, height: sheet_h, rgb: sheet }, 88).expect("encode");
    std::fs::write(out, jpeg).expect("write sheet");
    println!("Wrote {out} ({sheet_w} x {sheet_h})");
    let _ = LinearImage::new(0, 0, vec![]);
}

fn optics_cells() -> Vec<(String, Adjustments)> {
    let with = |f: fn(&mut Adjustments)| {
        let mut a = Adjustments::default();
        f(&mut a);
        a
    };
    vec![
        ("Original".into(), Adjustments::default()),
        ("Distortion +100".into(), with(|a| a.optics.distortion = 100.0)),
        ("Distortion -100".into(), with(|a| a.optics.distortion = -100.0)),
        ("Lens vignetting +100".into(), with(|a| a.optics.vignetting = 100.0)),
        ("Lens vignetting -100".into(), with(|a| a.optics.vignetting = -100.0)),
        ("Texture +100".into(), with(|a| a.texture = 100.0)),
        ("Texture -100".into(), with(|a| a.texture = -100.0)),
        ("Calibration: blue hue -60, blue sat +40".into(), with(|a| {
            a.calibration.blue_hue = -60.0;
            a.calibration.blue_saturation = 40.0;
        })),
    ]
}

fn detail_cells() -> Vec<(String, Adjustments)> {
    let with = |f: fn(&mut Adjustments)| {
        let mut a = Adjustments::default();
        a.sharpen.amount = 0.0;
        f(&mut a);
        a
    };
    vec![
        ("No sharpening".into(), with(|_| {})),
        ("Sharpening 40 (default)".into(), with(|a| a.sharpen.amount = 40.0)),
        ("Sharpening 150".into(), with(|a| a.sharpen.amount = 150.0)),
        ("Sharpening 100, radius 2.5".into(), with(|a| {
            a.sharpen.amount = 100.0;
            a.sharpen.radius = 2.5;
        })),
        ("Sharpening 150, masking 80".into(), with(|a| {
            a.sharpen.amount = 150.0;
            a.sharpen.masking = 80.0;
        })),
        ("Luminance noise 80".into(), with(|a| a.noise.luminance = 80.0)),
        ("Colour noise 100".into(), with(|a| a.noise.color = 100.0)),
        ("Texture +100".into(), with(|a| a.texture = 100.0)),
        ("Texture -100".into(), with(|a| a.texture = -100.0)),
        ("Exposure +2, no noise reduction".into(), with(|a| a.exposure = 2.0)),
        ("Exposure +2, luminance 60, colour 60".into(), with(|a| {
            a.exposure = 2.0;
            a.noise.luminance = 60.0;
            a.noise.color = 60.0;
        })),
        ("Defringe purple 20, green 20".into(), with(|a| {
            a.optics.defringe_purple = 20.0;
            a.optics.defringe_green = 20.0;
        })),
    ]
}

fn slider_cells() -> Vec<(String, Adjustments)> {
    let d = Adjustments::default;
    let mut vignette = d();
    vignette.vignette.amount = -70.0;
    let mut grain = d();
    grain.grain.amount = 80.0;
    let mut blue = d();
    blue.mixer[5].saturation = -100.0;
    let mut grade = d();
    grade.grading.shadows = pipeline::adjust::Wheel { hue: 210.0, saturation: 60.0, luminance: 0.0 };
    grade.grading.highlights = pipeline::adjust::Wheel { hue: 40.0, saturation: 60.0, luminance: 0.0 };
    vec![
        ("Original".into(), d()),
        ("Shadows +100".into(), Adjustments { shadows: 100.0, ..d() }),
        ("Shadows -100".into(), Adjustments { shadows: -100.0, ..d() }),
        ("Highlights -100".into(), Adjustments { highlights: -100.0, ..d() }),
        ("Highlights +100".into(), Adjustments { highlights: 100.0, ..d() }),
        ("Whites +100".into(), Adjustments { whites: 100.0, ..d() }),
        ("Blacks -100".into(), Adjustments { blacks: -100.0, ..d() }),
        ("Blacks +100".into(), Adjustments { blacks: 100.0, ..d() }),
        ("Contrast +100".into(), Adjustments { contrast: 100.0, ..d() }),
        ("Contrast -100".into(), Adjustments { contrast: -100.0, ..d() }),
        ("Clarity +100".into(), Adjustments { clarity: 100.0, ..d() }),
        ("Clarity -100".into(), Adjustments { clarity: -100.0, ..d() }),
        ("Dehaze +100".into(), Adjustments { dehaze: 100.0, ..d() }),
        ("Dehaze -100".into(), Adjustments { dehaze: -100.0, ..d() }),
        ("Vibrance +100".into(), Adjustments { vibrance: 100.0, ..d() }),
        ("Saturation +100".into(), Adjustments { saturation: 100.0, ..d() }),
        ("Vignette -70".into(), vignette),
        ("Grain 80".into(), grain),
        ("Blue saturation -100".into(), blue),
        ("Grading blue/orange 60".into(), grade),
    ]
}
