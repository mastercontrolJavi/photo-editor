//! Render one photo many ways side by side, to check sliders and presets by eye.
//!
//! ```text
//! cargo run --release -p pipeline --example contact_sheet -- <presets|profiles|sliders|optics|detail|local|heal> <out.jpg> <photo> [x y]
//! ```
//!
//! Cells are filled left to right, top to bottom, in the order printed.
//! `detail` and `heal` show a 100 % crop around the point `x y` (0…1, default
//! the centre); the other modes show the whole photo. `local` (masks, Point
//! Color, Geometry) picks its colour at `x y`, and `heal` puts its spot there.

use std::path::Path;

use pipeline::{decode_file, render, Adjustments, LinearImage, RenderedImage, Scene, View};

const CELL_EDGE: usize = 560;
const COLUMNS: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, out, photo, ..] = args.as_slice() else {
        eprintln!("usage: contact_sheet <presets|profiles|sliders|optics|detail|local|heal> <out.jpg> <photo> [x y]");
        std::process::exit(2);
    };
    let decoded = decode_file(Path::new(photo)).expect("decode");
    let fresh = decoded.info.starting_adjustments();
    let spot: Vec<f64> = args[3..].iter().filter_map(|v| v.parse().ok()).collect();
    let spot = (spot.first().copied().unwrap_or(0.5), spot.get(1).copied().unwrap_or(0.5));
    let zoomed = mode == "detail" || mode == "heal";
    let cells: Vec<(String, Adjustments)> = match mode.as_str() {
        "local" => local_cells(&decoded, &fresh, spot),
        "heal" => heal_cells(&decoded, &fresh, spot),
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
    let small = decoded.image.downscale_to_fit(CELL_EDGE);
    let look = decoded.info.look();
    let (mut w, mut h) = (small.width, small.height);
    if zoomed {
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
        let r = if zoomed {
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

/// Masks, Point Color and Geometry, plus the wider Highlights and Whites.
fn local_cells(decoded: &pipeline::DecodedImage, fresh: &Adjustments, spot: (f64, f64)) -> Vec<(String, Adjustments)> {
    use pipeline::adjust::{BrushStroke, Geometry, LocalAdjust, Mask, MaskShape, PointColor};
    let with = |f: &dyn Fn(&mut Adjustments)| {
        let mut a = fresh.clone();
        f(&mut a);
        a
    };
    let mask = |shape: MaskShape, invert: bool, adjust: LocalAdjust| Mask { shape, invert, adjust };
    let sky = MaskShape::Linear { from: [0.5, 0.15], to: [0.5, 0.55] };
    let oval = MaskShape::Radial { centre: [0.5, 0.55], radius: [0.28, 0.2], angle: 0.0, feather: 60.0 };
    let scribble = MaskShape::Brush {
        strokes: vec![
            BrushStroke { points: vec![[0.15, 0.8], [0.4, 0.7], [0.65, 0.82], [0.85, 0.72]], size: 0.07, feather: 70.0, erase: false },
            BrushStroke { points: vec![[0.5, 0.6], [0.5, 0.95]], size: 0.03, feather: 30.0, erase: true },
        ],
    };
    // The colour under the chosen point, picked the way the app does it.
    let small = decoded.image.downscale_to_fit(1000);
    let full = (decoded.image.width, decoded.image.height);
    let [hue, saturation, luminance] = pipeline::render::pick_colour(&small, fresh, &decoded.info.look(), full, spot.0 as f32, spot.1 as f32);
    println!("picked at ({}, {}): hue {hue:.0}, saturation {saturation:.2}, brightness {luminance:.2}", spot.0, spot.1);
    let pick = PointColor { hue, saturation, luminance, ..Default::default() };
    vec![
        ("Original".into(), fresh.clone()),
        ("Highlights -100".into(), with(&|a| a.highlights = -100.0)),
        ("Whites -100".into(), with(&|a| a.whites = -100.0)),
        ("Whites +100".into(), with(&|a| a.whites = 100.0)),
        ("Linear mask from the top: exposure -1.5".into(), with(&|a| {
            a.masks.push(mask(sky.clone(), false, LocalAdjust { exposure: -1.5, ..Default::default() }))
        })),
        ("Linear mask from the top: warmer, more saturated".into(), with(&|a| {
            a.masks.push(mask(sky.clone(), false, LocalAdjust { temperature: 60.0, saturation: 60.0, ..Default::default() }))
        })),
        ("Radial mask: +1 inside".into(), with(&|a| a.masks.push(mask(oval.clone(), false, LocalAdjust { exposure: 1.0, ..Default::default() })))),
        ("Radial mask inverted: -1.5 outside".into(), with(&|a| {
            a.masks.push(mask(oval.clone(), true, LocalAdjust { exposure: -1.5, ..Default::default() }))
        })),
        ("Brush mask (with an erased strip): +1.5, shadows +60".into(), with(&|a| {
            a.masks.push(mask(scribble.clone(), false, LocalAdjust { exposure: 1.5, shadows: 60.0, ..Default::default() }))
        })),
        ("Point Color: picked colour, saturation -100".into(), with(&|a| a.point_colors.push(PointColor { sat_shift: -100.0, ..pick }))),
        ("Point Color: picked colour, hue +100".into(), with(&|a| a.point_colors.push(PointColor { hue_shift: 100.0, ..pick }))),
        ("Point Color: picked colour, luminance -100, range 100".into(), with(&|a| {
            a.point_colors.push(PointColor { lum_shift: -100.0, range: 100.0, ..pick })
        })),
        ("Geometry: vertical -70".into(), with(&|a| a.geometry = Geometry { vertical: -70.0, ..Default::default() })),
        ("Geometry: vertical -70, constrained".into(), with(&|a| a.geometry = Geometry { vertical: -70.0, constrain: true, ..Default::default() })),
        ("Geometry: horizontal +60, rotate +6".into(), with(&|a| a.geometry = Geometry { horizontal: 60.0, rotate: 6.0, ..Default::default() })),
        ("Geometry: aspect +60, scale 80, x +20, y +20".into(), with(&|a| {
            a.geometry = Geometry { aspect: 60.0, scale: 80.0, x_offset: 20.0, y_offset: 20.0, ..Default::default() }
        })),
    ]
}

/// A healing spot at the chosen point, with the source the app would pick.
fn heal_cells(decoded: &pipeline::DecodedImage, fresh: &Adjustments, spot: (f64, f64)) -> Vec<(String, Adjustments)> {
    use pipeline::adjust::{Spot, SpotMode};
    let preview = decoded.image.downscale_to_fit(2560);
    let at = [spot.0 as f32, spot.1 as f32];
    let mut cells = vec![("Original".to_string(), fresh.clone())];
    for (radius, name) in [(0.008f32, "small"), (0.02, "large")] {
        let source = pipeline::heal::suggest_source(&preview, at, radius);
        println!("{name} spot at {at:?}: source suggested at {source:?}");
        for mode in [SpotMode::Heal, SpotMode::Clone] {
            let mut a = fresh.clone();
            a.spots.push(Spot { at, source, radius, feather: 40.0, opacity: 100.0, mode });
            cells.push((format!("{mode:?}, {name} spot"), a));
        }
    }
    cells.truncate(4);
    cells.insert(3, ("Heal, large spot".into(), {
        let radius = 0.02;
        let mut a = fresh.clone();
        a.spots.push(Spot { at, source: pipeline::heal::suggest_source(&preview, at, radius), radius, feather: 40.0, opacity: 100.0, mode: SpotMode::Heal });
        a
    }));
    cells.truncate(4);
    cells
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
