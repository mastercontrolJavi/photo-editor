//! RAW/DNG decoding via the `rawler` crate (pure Rust, LGPL-2.1).
//!
//! rawler gives us black/white-level scaling, demosaicing (for Bayer files) and
//! the DNG crop. We do the colour conversion ourselves so values above sensor
//! white are kept unclipped for later exposure changes.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use rawler::decoders::{Orientation, RawDecodeParams, WellKnownIFD};
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;

use crate::color::{self, Mat3};
use crate::error::{PipelineError, Result};
use crate::image::LinearImage;
use crate::local::LocalMaps;
use crate::look::{GainTableMap, Look, ToneCurve};
use crate::probe::{self, TagReport};

/// Refuse anything bigger than this many pixels (~2.4 GB of working memory).
const MAX_PIXELS: usize = 200_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Already-demosaiced linear DNG. iPhone ProRAW is this kind.
    LinearRaw,
    /// Classic Bayer-mosaic RAW (most cameras, DJI Pocket 3 DNGs).
    BayerCfa,
}

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub make: String,
    pub model: String,
    pub kind: SourceKind,
    /// Final, upright size in pixels (after crop and orientation).
    pub width: usize,
    pub height: usize,
    /// EXIF/TIFF orientation code that was applied (1 = none).
    pub orientation: u16,
    /// DNG BaselineExposure in stops; the renderer adds it to the user's exposure.
    pub baseline_exposure: f32,
    /// The file's default tone curve (ProfileToneCurve), applied at render time.
    pub tone_curve: Option<Arc<ToneCurve>>,
    /// Whether the file's local tone map (ProfileGainTableMap) was applied at decode.
    pub gain_map_applied: bool,
    /// Helper maps for the local sliders, worked out once from the decoded photo.
    pub local: Arc<LocalMaps>,
    pub tags: TagReport,
    /// Plain-language limitations for this particular file, shown in the app.
    pub notes: Vec<String>,
}

impl SourceInfo {
    /// Everything the renderer needs from the file.
    pub fn look(&self) -> Look {
        Look { baseline_exposure: self.baseline_exposure, tone_curve: self.tone_curve.clone(), local: Some(self.local.clone()) }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub image: Arc<LinearImage>,
    pub info: SourceInfo,
}

pub fn decode_file(path: &Path) -> Result<DecodedImage> {
    let bytes = std::fs::read(path).map_err(|source| PipelineError::Read { path: path.to_owned(), source })?;
    if bytes.is_empty() {
        return Err(PipelineError::EmptyFile { path: path.to_owned() });
    }
    decode_bytes(bytes)
}

/// Decode from memory. The source file is only ever read, never opened for writing.
pub fn decode_bytes(bytes: Vec<u8>) -> Result<DecodedImage> {
    if let Some(kind) = non_raw_kind(&bytes) {
        return Err(PipelineError::Unsupported {
            detail: format!("this is a {kind}, not a RAW file. The editor opens RAW DNG files, such as iPhone ProRAW."),
        });
    }
    let src = RawSource::new_from_shared_vec(Arc::new(bytes));
    // rawler has `todo!()`/`panic!` paths for unusual files; turn those into
    // ordinary errors so a bad file can never crash the app.
    match catch_unwind(AssertUnwindSafe(|| decode_source(&src))) {
        Ok(result) => result,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "decoder stopped unexpectedly".into());
            Err(PipelineError::Unreadable { detail: format!("decoder failure: {msg}") })
        }
    }
}

/// Recognise common non-RAW photo formats by their first bytes, so people get a
/// clear "not supported" message instead of "damaged".
fn non_raw_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("JPEG photo");
    }
    if bytes.starts_with(b"\x89PNG") {
        return Some("PNG image");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some(match &bytes[8..12] {
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => "HEIC photo",
            b"avif" | b"avis" => "AVIF image",
            _ => "video or HEIF file",
        });
    }
    None
}

fn unreadable(e: impl std::fmt::Display) -> PipelineError {
    PipelineError::Unreadable { detail: e.to_string() }
}

fn decode_source(src: &RawSource) -> Result<DecodedImage> {
    let decoder = rawler::get_decoder(src)
        .map_err(|_| PipelineError::Unreadable { detail: "the file type wasn't recognised as a RAW photo".into() })?;
    let raw = decoder.raw_image(src, &RawDecodeParams::default(), false).map_err(|e| match e {
        rawler::RawlerError::Unsupported { what, .. } => PipelineError::Unsupported { detail: what },
        other => unreadable(other),
    })?;
    let root = decoder.ifd(WellKnownIFD::Root).ok().flatten();
    let raw_ifd = decoder.ifd(WellKnownIFD::Raw).ok().flatten();
    let tags = probe::scan_tags(root.as_deref(), raw_ifd.as_deref());
    let gain_map = probe::find_entry(root.as_deref(), raw_ifd.as_deref(), 52525).and_then(|e| match &e.value {
        rawler::formats::tiff::Value::Undefined(b) | rawler::formats::tiff::Value::Byte(b) => GainTableMap::parse(b),
        _ => None,
    });
    let tone_curve = probe::find_entry(root.as_deref(), raw_ifd.as_deref(), 50940).and_then(|e| match &e.value {
        rawler::formats::tiff::Value::Float(v) if v.len() % 2 == 0 => {
            let pts: Vec<(f32, f32)> = v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect();
            ToneCurve::from_points(&pts).map(Arc::new)
        }
        _ => None,
    });

    if raw.width.saturating_mul(raw.height) > MAX_PIXELS {
        return Err(PipelineError::Unsupported {
            detail: format!("{}×{} is larger than the {} MP limit", raw.width, raw.height, MAX_PIXELS / 1_000_000),
        });
    }

    let kind = match (&raw.photometric, raw.cpp) {
        (RawPhotometricInterpretation::LinearRaw, 3) => SourceKind::LinearRaw,
        (RawPhotometricInterpretation::Cfa(cfg), 1) if cfg.cfa.is_rgb() => SourceKind::BayerCfa,
        (RawPhotometricInterpretation::Cfa(_), _) => {
            return Err(PipelineError::Unsupported { detail: "four-colour or unusual sensor mosaic".into() })
        }
        (RawPhotometricInterpretation::LinearRaw, n) => {
            return Err(PipelineError::Unsupported { detail: format!("linear DNG with {n} channels") })
        }
        (RawPhotometricInterpretation::BlackIsZero, _) => {
            return Err(PipelineError::Unsupported { detail: "monochrome image".into() })
        }
    };

    let mut notes = Vec::new();

    // Camera → linear sRGB matrix, white balance folded in.
    let wb = as_shot_white_balance(&raw.wb_coeffs, &mut notes);
    let cam_to_srgb = camera_matrix(&raw, &mut notes);
    let mut m: Mat3 = cam_to_srgb;
    for row in m.iter_mut() {
        for (c, v) in row.iter_mut().enumerate() {
            *v *= wb[c];
        }
    }

    let orientation = orientation_code(raw.orientation);
    let make = raw.make.trim().to_string();
    let model = raw.model.trim().to_string();

    let develop = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::CropActiveArea,
        ProcessingStep::CropDefault,
    ]);
    let intermediate = develop.develop_intermediate(&raw).map_err(unreadable)?;
    drop(raw);
    let mut rgb = match intermediate {
        Intermediate::ThreeColor(p) => p,
        Intermediate::FourColor(_) => {
            return Err(PipelineError::Unsupported { detail: "four-colour sensor".into() })
        }
        Intermediate::Monochrome(_) => return Err(PipelineError::Unsupported { detail: "monochrome image".into() }),
    };
    rgb.data.par_iter_mut().for_each(|px| *px = color::apply(&m, *px));
    let mut image = LinearImage::new(rgb.width, rgb.height, std::mem::take(&mut rgb.data));

    let baseline_exposure = tags.baseline_exposure.unwrap_or(0.0);
    let baseline_exposure = if baseline_exposure.is_finite() { baseline_exposure.clamp(-10.0, 10.0) } else { 0.0 };
    // The gain map is positional, so it goes on before rotating. Per the DNG
    // spec its table is looked up with BaselineExposure already applied; the
    // gain itself multiplies the stored values, and the renderer still adds
    // BaselineExposure afterwards. Checked against Apple's own rendering on
    // seven iPhone 15 Pro files (mid-tones within 0.3 EV).
    let gain_map_applied = match &gain_map {
        Some(map) => {
            map.apply(&mut image, 2f32.powf(baseline_exposure));
            true
        }
        None => false,
    };
    if tags.has(52525) && gain_map.is_none() {
        notes.push("The file's local tone map (ProfileGainTableMap) couldn't be read, so shadows may look darker than in the Photos app.".into());
    }
    if tags.has(50940) && tone_curve.is_none() {
        notes.push("The file's tone curve couldn't be read; using a plain linear rendering.".into());
    }
    let image = image.oriented(orientation);
    let local = Arc::new(LocalMaps::compute(&image));

    notes.extend(tags.limitations());
    let info = SourceInfo {
        make,
        model,
        kind,
        width: image.width,
        height: image.height,
        orientation,
        baseline_exposure,
        tone_curve,
        gain_map_applied,
        local,
        tags,
        notes,
    };
    Ok(DecodedImage { image: Arc::new(image), info })
}

fn as_shot_white_balance(coeffs: &[f32; 4], notes: &mut Vec<String>) -> [f32; 3] {
    let wb = [coeffs[0], coeffs[1], coeffs[2]];
    if wb.iter().all(|v| v.is_finite() && *v > 0.0) {
        [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
    } else {
        notes.push("The file has no as-shot white balance; starting from a neutral (uncorrected) balance.".into());
        [1.0, 1.0, 1.0]
    }
}

fn camera_matrix(raw: &rawler::RawImage, notes: &mut Vec<String>) -> Mat3 {
    let found = raw.color_matrix_find_first([
        Illuminant::D65,
        Illuminant::D55,
        Illuminant::D75,
        Illuminant::D50,
        Illuminant::Daylight,
        Illuminant::FineWeather,
        Illuminant::Flash,
        Illuminant::A,
        Illuminant::Tungsten,
    ]);
    let Some((illuminant, flat)) = found else {
        notes.push("No colour matrix in the file; colours are the camera's raw primaries.".into());
        return color::IDENTITY;
    };
    if flat.len() != 9 {
        notes.push("Unusual colour matrix size; colours are the camera's raw primaries.".into());
        return color::IDENTITY;
    }
    if illuminant != Illuminant::D65 {
        notes.push(format!("Using the {illuminant:?} colour matrix (no D65 matrix in the file); colours may be slightly off."));
    }
    let xyz_to_cam = [[flat[0], flat[1], flat[2]], [flat[3], flat[4], flat[5]], [flat[6], flat[7], flat[8]]];
    color::camera_to_srgb(&xyz_to_cam).unwrap_or_else(|| {
        notes.push("The file's colour matrix is invalid; colours are the camera's raw primaries.".into());
        color::IDENTITY
    })
}

fn orientation_code(o: Orientation) -> u16 {
    match o {
        Orientation::Normal | Orientation::Unknown => 1,
        Orientation::HorizontalFlip => 2,
        Orientation::Rotate180 => 3,
        Orientation::VerticalFlip => 4,
        Orientation::Transpose => 5,
        Orientation::Rotate90 => 6,
        Orientation::Transverse => 7,
        Orientation::Rotate270 => 8,
    }
}
