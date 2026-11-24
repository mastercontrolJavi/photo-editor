//! Time each slider on its own, to see what a preview redraw costs.
//!
//! ```text
//! cargo run --release -p pipeline --example step_cost -- <photo.dng> [long edge]
//! ```

use std::path::Path;
use std::time::Instant;

use pipeline::adjust::Wheel;
use pipeline::{decode_file, render, Adjustments, Scene, View};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let photo = args.first().expect("usage: step_cost <photo.dng> [long edge]");
    let edge: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(pipeline::PREVIEW_LONG_EDGE);
    let decoded = decode_file(Path::new(photo)).expect("decode");
    let preview = decoded.image.downscale_to_fit(edge);
    let look = decoded.info.look();
    let d = Adjustments::default;
    let mut cases: Vec<(&str, Adjustments)> = vec![
        ("nothing", d()),
        ("contrast", Adjustments { contrast: 30.0, ..d() }),
        ("whites+blacks", Adjustments { whites: 30.0, blacks: -20.0, ..d() }),
        ("shadows+highlights", Adjustments { shadows: 30.0, highlights: -30.0, ..d() }),
        ("clarity", Adjustments { clarity: 30.0, ..d() }),
        ("dehaze", Adjustments { dehaze: 30.0, ..d() }),
        ("saturation", Adjustments { saturation: 20.0, ..d() }),
        ("vibrance", Adjustments { vibrance: 20.0, ..d() }),
        ("bw", Adjustments { bw: true, ..d() }),
    ];
    let mut a = d();
    a.curve.rgb = vec![[0.0, 0.05], [1.0, 0.95]];
    cases.push(("curve", a));
    let mut a = d();
    a.mixer[5].saturation = 20.0;
    cases.push(("mixer", a));
    let mut a = d();
    a.grading.shadows = Wheel { hue: 200.0, saturation: 30.0, luminance: 0.0 };
    cases.push(("grading", a));
    let mut a = d();
    a.vignette.amount = -30.0;
    cases.push(("vignette", a));
    let mut a = d();
    a.grain.amount = 30.0;
    cases.push(("grain", a));
    {
        use pipeline::adjust::{BrushStroke, Geometry, LocalAdjust, Mask, MaskShape, PointColor, Spot};
        let darker = LocalAdjust { exposure: -1.0, ..Default::default() };
        let toned = LocalAdjust { exposure: -1.0, highlights: -40.0, shadows: 20.0, ..Default::default() };
        let mut a = d();
        a.masks.push(Mask { shape: MaskShape::Linear { from: [0.5, 0.1], to: [0.5, 0.5] }, invert: false, adjust: darker });
        cases.push(("linear mask", a));
        let mut a = d();
        a.masks.push(Mask { shape: MaskShape::Radial { centre: [0.5, 0.5], radius: [0.3, 0.2], angle: 0.0, feather: 50.0 }, invert: true, adjust: toned });
        cases.push(("radial mask + tone", a));
        let mut a = d();
        let stroke = BrushStroke { points: (0..200).map(|i| [0.1 + 0.004 * i as f32, 0.5 + 0.2 * (i as f32 * 0.1).sin()]).collect(), size: 0.04, feather: 60.0, erase: false };
        a.masks.push(Mask { shape: MaskShape::Brush { strokes: vec![stroke] }, invert: false, adjust: darker });
        cases.push(("brush mask", a));
        let mut a = d();
        a.point_colors.push(PointColor { hue: 50.0, saturation: 0.8, luminance: 0.4, hue_shift: 30.0, ..Default::default() });
        cases.push(("point color", a));
        let mut a = d();
        a.geometry = Geometry { vertical: -30.0, rotate: 2.0, constrain: true, ..Default::default() };
        cases.push(("geometry", a));
        let mut a = d();
        a.spots = (0..20).map(|i| Spot { at: [0.1 + 0.04 * i as f32, 0.5], source: [0.1 + 0.04 * i as f32, 0.6], radius: 0.01, ..Spot::default() }).collect();
        cases.push(("20 heal spots", a));
    }
    println!("{} x {}", preview.width, preview.height);
    for (name, adj) in &cases {
        let scene = Scene { image: &preview, look: &look, lut: None };
        let view = View::preview(&adj.crop, preview.width, preview.height, decoded.image.width, decoded.image.height);
        let mut times: Vec<f64> = (0..9)
            .map(|_| {
                let t = Instant::now();
                let _ = render(&scene, adj, &view, preview.width, preview.height);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{name:<20} {:6.1} ms", times[4]);
    }
}
