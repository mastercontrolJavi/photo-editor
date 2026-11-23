//! `pe` — command-line companion to the photo editor, for checking real files.
//!
//!   pe probe  <photo>              what the decoder found and what it ignores
//!   pe bench  <photo>              decode / preview / export timings on this PC
//!   pe export <photo> [options]    render a JPEG without opening the app
//!       --out <folder>   (default: <photo folder>\Exported)
//!       --exposure <EV> --temperature <n> --tint <n>   (default: saved edit)
//!   pe fixtures <folder> [--large] write synthetic test DNGs (--large: 12 MP + 48 MP)

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use pipeline::fixtures::{make_dng, test_scene, FixtureKind, FixtureSpec};
use pipeline::{decode_file, export_jpeg, render_srgb8, sidecar, Adjustments, ExportOptions, PREVIEW_LONG_EDGE};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("probe") => with_path(&args, |p| {
            print!("{}", pipeline::probe::report(p));
            Ok(())
        }),
        Some("bench") => with_path(&args, bench),
        Some("export") => with_path(&args, |p| export(p, &args[2..])),
        Some("fixtures") => with_path(&args, |p| fixtures(p, args.iter().any(|a| a == "--large"))),
        _ => Err(include_str!("pe.rs").lines().skip(1).take(8).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn with_path(args: &[String], f: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let p = args.get(1).ok_or("missing path argument")?;
    f(Path::new(p))
}

fn bench(path: &Path) -> Result<(), String> {
    let t = Instant::now();
    let d = decode_file(path).map_err(|e| e.to_string())?;
    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let preview = d.image.downscale_to_fit(PREVIEW_LONG_EDGE);
    let scale_ms = t.elapsed().as_secs_f64() * 1000.0;
    let adj = Adjustments { exposure: 0.3, temperature: 10.0, tint: -5.0 };
    let runs = 20;
    let mut times: Vec<f64> = (0..runs)
        .map(|i| {
            let a = Adjustments { exposure: adj.exposure + i as f32 * 0.01, ..adj };
            let t = Instant::now();
            let _ = render_srgb8(&preview, &a, &d.info.look());
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let t = Instant::now();
    let full = render_srgb8(&d.image, &adj, &d.info.look());
    let render_full_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let jpeg = pipeline::export::encode_jpeg(&full, pipeline::export::DEFAULT_QUALITY).map_err(|e| e.to_string())?;
    let encode_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("File: {}", path.display());
    println!("Threads: {}", rayon::current_num_threads());
    println!("Full size: {} x {}  |  preview: {} x {}", d.image.width, d.image.height, preview.width, preview.height);
    println!("Decode (open file):           {decode_ms:8.1} ms");
    println!("Build preview (once per file): {scale_ms:8.1} ms");
    println!(
        "Preview re-render per slider move: median {:.1} ms, worst {:.1} ms ({runs} runs)",
        times[runs / 2],
        times[runs - 1]
    );
    println!("Export render (full size):    {render_full_ms:8.1} ms");
    println!("Export JPEG encode:           {encode_ms:8.1} ms ({:.1} MB)", jpeg.len() as f64 / 1e6);
    Ok(())
}

fn export(path: &Path, rest: &[String]) -> Result<(), String> {
    let mut adj = sidecar::load(path).map_err(|e| e.to_string())?.unwrap_or_default();
    let mut opts = ExportOptions::next_to(path);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => opts.out_dir = PathBuf::from(val()?),
            "--exposure" => adj.exposure = val()?.parse().map_err(|_| "bad --exposure")?,
            "--temperature" => adj.temperature = val()?.parse().map_err(|_| "bad --temperature")?,
            "--tint" => adj.tint = val()?.parse().map_err(|_| "bad --tint")?,
            other => return Err(format!("unknown option {other}")),
        }
    }
    let d = decode_file(path).map_err(|e| e.to_string())?;
    let out = export_jpeg(path, &d, &adj, &opts).map_err(|e| e.to_string())?;
    println!("Wrote {} ({} x {})", out.display(), d.image.width, d.image.height);
    Ok(())
}

fn fixtures(dir: &Path, large: bool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let write = |name: &str, spec: FixtureSpec| -> Result<(), String> {
        let bytes = make_dng(&spec, test_scene(spec.width, spec.height));
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
        println!("Wrote {}", p.display());
        Ok(())
    };
    if large {
        // Same pixel counts as iPhone 12 MP and 48 MP ProRAW, held in portrait.
        write("linear-12mp.dng", FixtureSpec { orientation: 6, ..FixtureSpec::new(4032, 3024, FixtureKind::Linear) })?;
        write("linear-48mp.dng", FixtureSpec { orientation: 6, ..FixtureSpec::new(8064, 6048, FixtureKind::Linear) })?;
        return Ok(());
    }
    write("linear-landscape.dng", FixtureSpec::new(1600, 1200, FixtureKind::Linear))?;
    write("linear-rotated-90cw.dng", FixtureSpec { orientation: 6, ..FixtureSpec::new(1600, 1200, FixtureKind::Linear) })?;
    write("bayer-landscape.dng", FixtureSpec::new(1600, 1200, FixtureKind::Bayer))?;
    let corrupt = dir.join("corrupt.dng");
    std::fs::write(&corrupt, b"II*\0this is not a real photo").map_err(|e| e.to_string())?;
    println!("Wrote {}", corrupt.display());
    Ok(())
}
