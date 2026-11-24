//! Render one photo many ways side by side, to check sliders and presets by eye.
//!
//! ```text
//! cargo run --release -p pipeline --example contact_sheet -- <presets|sliders> <out.jpg> <photo.dng>
//! ```
//!
//! Cells are filled left to right, top to bottom, in the order printed.

use std::path::Path;

use pipeline::{decode_file, render, Adjustments, LinearImage, RenderedImage, Scene, View};

const CELL_EDGE: usize = 560;
const COLUMNS: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, out, photo] = args.as_slice() else {
        eprintln!("usage: contact_sheet <presets|sliders> <out.jpg> <photo.dng>");
        std::process::exit(2);
    };
    let cells: Vec<(String, Adjustments)> = match mode.as_str() {
        "presets" => std::iter::once(("Original".to_string(), Adjustments::default()))
            .chain(pipeline::preset::builtin().into_iter().map(|p| (p.name.clone(), p.apply(&Adjustments::default()))))
            .collect(),
        _ => slider_cells(),
    };
    let decoded = decode_file(Path::new(photo)).expect("decode");
    let small = decoded.image.downscale_to_fit(CELL_EDGE);
    let look = decoded.info.look();
    let (w, h) = (small.width, small.height);
    let gap = 6;
    let rows = cells.len().div_ceil(COLUMNS);
    let (sheet_w, sheet_h) = (COLUMNS * (w + gap) + gap, rows * (h + gap) + gap);
    let mut sheet = vec![40u8; sheet_w * sheet_h * 3];
    for (i, (label, adj)) in cells.iter().enumerate() {
        println!("{:>2}  row {} col {}  {label}", i + 1, i / COLUMNS + 1, i % COLUMNS + 1);
        let scene = Scene { image: &small, look: &look, lut: None };
        let view = View::preview(&adj.crop, w, h, decoded.image.width, decoded.image.height);
        let r = render(&scene, adj, &view, w, h);
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
