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
