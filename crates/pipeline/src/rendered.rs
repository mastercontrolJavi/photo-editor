//! Opening photos that a camera or another program has already processed:
//! JPEG, PNG and HEIC.
//!
//! These differ from RAW files in two ways that matter for editing:
//!
//! * The picture is finished: white balance, contrast and sharpening are
//!   already applied, and anything brighter than white or darker than black
//!   was thrown away when the file was made.
//! * Most hold 8 bits per colour (256 levels), so large changes show
//!   steps ("banding") sooner than with RAW.
//!
//! They are converted to the same working space as RAW photos (linear light,
//! sRGB primaries) so every slider works on them the same way.

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, ImageDecoder};

use crate::color::{self, Mat3};
use crate::decode::{DecodedImage, SourceInfo, SourceKind};
use crate::error::{PipelineError, Result};
use crate::image::LinearImage;
use crate::local::LocalMaps;
use crate::probe::TagReport;

/// Refuse anything bigger than this many pixels (~2.4 GB of working memory).
const MAX_PIXELS: usize = 200_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Heic,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Heic => "HEIC",
        }
    }
}

/// Decoded pixels before any colour conversion.
pub(crate) struct Pixels {
    pub width: usize,
    pub height: usize,
    /// Red, green, blue, opacity for each pixel, 0…65535, sRGB-encoded.
    pub rgba: Vec<u16>,
    /// Bits per colour in the file (8, 10 or 16).
    pub bits: u8,
    /// The file's ICC colour profile, if it has one.
    pub icc: Option<Vec<u8>>,
    /// EXIF orientation still to be applied (1 = none).
    pub orientation: u16,
}

/// Which colours the numbers in the file stand for.
#[derive(Debug, Clone, PartialEq)]
enum ColourSpace {
    Srgb,
    /// The wider-gamut space iPhones use for photos.
    DisplayP3,
    /// Something else; treated as sRGB, with a note.
    Other,
}

/// Linear Display P3 → linear sRGB (both D65).
const P3_TO_SRGB: Mat3 = [
    [1.224_940_2, -0.224_940_2, 0.0],
    [-0.042_056_955, 1.042_056_9, 0.0],
    [-0.019_637_555, -0.078_636_05, 1.098_273_6],
];

fn contains_text(bytes: &[u8], text: &str) -> bool {
    let ascii = text.as_bytes();
    // ICC version 4 stores names as UTF-16 (big-endian).
    let wide: Vec<u8> = text.encode_utf16().flat_map(|c| c.to_be_bytes()).collect();
    bytes.windows(ascii.len()).any(|w| w == ascii) || bytes.windows(wide.len()).any(|w| w == wide.as_slice())
}

fn colour_space(icc: Option<&[u8]>) -> ColourSpace {
    match icc {
        None => ColourSpace::Srgb,
        Some(p) if contains_text(p, "Display P3") || contains_text(p, "P3 D65") => ColourSpace::DisplayP3,
        Some(p) if contains_text(p, "sRGB") => ColourSpace::Srgb,
        Some(_) => ColourSpace::Other,
    }
}

fn unreadable(format: Format, detail: impl std::fmt::Display) -> PipelineError {
    PipelineError::Unreadable { detail: format!("this {} file can't be read: {detail}", format.name()) }
}

fn with_image_crate(format: Format, mut decoder: impl ImageDecoder) -> Result<Pixels> {
    let (w, h) = decoder.dimensions();
    if (w as usize).saturating_mul(h as usize) > MAX_PIXELS {
        return Err(PipelineError::Unsupported { detail: format!("{w}×{h} is larger than the {} MP limit", MAX_PIXELS / 1_000_000) });
    }
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().map(|o| o.to_exif() as u16).unwrap_or(1);
    let bits = (decoder.color_type().bits_per_pixel() / decoder.color_type().channel_count() as u16) as u8;
    let img = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(format, e))?;
    Ok(Pixels { width: w as usize, height: h as usize, rgba: img.into_rgba16().into_raw(), bits, icc, orientation })
}

fn read_pixels(format: Format, bytes: &[u8]) -> Result<Pixels> {
    match format {
        Format::Jpeg => {
            let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes)).map_err(|e| unreadable(format, e))?;
            with_image_crate(format, decoder)
        }
        Format::Png => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).map_err(|e| unreadable(format, e))?;
            with_image_crate(format, decoder)
        }
        Format::Heic => crate::wic::decode(bytes).map_err(|detail| PipelineError::Unsupported {
            detail: format!(
                "this HEIC photo couldn't be decoded ({detail}). HEIC needs Windows' \"HEIF Image Extensions\" and \"HEVC Video Extensions\" from the Microsoft Store."
            ),
        }),
    }
}

/// sRGB-encoded 16-bit value → linear light.
fn decode_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| (0..=u16::MAX).map(|v| color::srgb_decode(v as f32 / 65535.0)).collect())
}

/// Decode a JPEG, PNG or HEIC file from memory.
pub fn decode(format: Format, bytes: &[u8]) -> Result<DecodedImage> {
    let px = read_pixels(format, bytes)?;
    if px.width == 0 || px.height == 0 || px.rgba.len() != px.width * px.height * 4 {
        return Err(unreadable(format, "it has no picture in it"));
    }
    let space = colour_space(px.icc.as_deref());
    let table = decode_table();
    let pixels: Vec<[f32; 3]> = px
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let mut c = [table[p[0] as usize], table[p[1] as usize], table[p[2] as usize]];
            if p[3] != u16::MAX {
                // See-through pixels are shown over white.
                let a = p[3] as f32 / 65535.0;
                c = c.map(|v| v * a + (1.0 - a));
            }
            if space == ColourSpace::DisplayP3 {
                c = color::apply(&P3_TO_SRGB, c);
            }
            c
        })
        .collect();
    let image = LinearImage::new(px.width, px.height, pixels).oriented(px.orientation);
    let local = Arc::new(LocalMaps::compute(&image));
    let chromatic = crate::lens::chromatic_scales(&image.downscale_to_fit(crate::PREVIEW_LONG_EDGE));

    let mut notes = vec![format!(
        "This is an already-processed {}-bit photo. Every slider works, but there is less room to recover very bright or very dark areas than with a RAW file.",
        px.bits
    )];
    match space {
        ColourSpace::DisplayP3 => notes.push("Its colours are stored as Display P3 and were converted to sRGB for editing.".into()),
        ColourSpace::Other => {
            notes.push("It uses a colour profile this app doesn't convert, so it is treated as sRGB and colours may look slightly off.".into())
        }
        ColourSpace::Srgb => {}
    }
    let info = SourceInfo {
        make: String::new(),
        model: String::new(),
        kind: SourceKind::Rendered,
        format: format.name().to_string(),
        bits: px.bits,
        width: image.width,
        height: image.height,
        orientation: px.orientation,
        baseline_exposure: 0.0,
        tone_curve: None,
        gain_map_applied: false,
        local,
        chromatic,
        has_lens_opcodes: false,
        tags: TagReport::default(),
        notes,
    };
    Ok(DecodedImage { image: Arc::new(image), info })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};

    fn png_bytes(img: &DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_with_encoder(image::codecs::png::PngEncoder::new(&mut out)).unwrap();
        out
    }

    #[test]
    fn a_png_opens_with_its_exact_colours_in_linear_light() {
        let mut img = RgbImage::new(4, 3);
        img.put_pixel(0, 0, Rgb([255, 255, 255]));
        img.put_pixel(1, 0, Rgb([188, 188, 188])); // sRGB 188 is linear 0.5
        img.put_pixel(2, 0, Rgb([255, 0, 0]));
        let d = decode(Format::Png, &png_bytes(&DynamicImage::ImageRgb8(img))).unwrap();
        assert_eq!((d.info.kind, d.info.format.as_str(), d.info.bits), (SourceKind::Rendered, "PNG", 8));
        assert_eq!((d.image.width, d.image.height), (4, 3));
        assert_eq!(d.image.get(0, 0), [1.0; 3]);
        assert!((d.image.get(1, 0)[0] - 0.5).abs() < 0.01);
        assert_eq!(d.image.get(2, 0), [1.0, 0.0, 0.0]);
        assert_eq!(d.image.get(3, 2), [0.0; 3]);
        // An unedited photo comes back out as it went in.
        let r = crate::render_srgb8(&d.image, &d.info.starting_adjustments(), &d.info.look());
        assert_eq!(&r.rgb[0..9], &[255, 255, 255, 188, 188, 188, 255, 0, 0]);
    }

    #[test]
    fn sixteen_bit_and_see_through_pngs_open_too() {
        let img16 = image::ImageBuffer::<Rgb<u16>, _>::from_pixel(2, 2, Rgb([65535, 32768, 0]));
        let d = decode(Format::Png, &png_bytes(&DynamicImage::ImageRgb16(img16))).unwrap();
        assert_eq!(d.info.bits, 16);
        assert!((d.image.get(0, 0)[1] - color::srgb_decode(32768.0 / 65535.0)).abs() < 1e-5);
        // A fully see-through black pixel shows as white.
        let clear = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let d = decode(Format::Png, &png_bytes(&DynamicImage::ImageRgba8(clear))).unwrap();
        assert_eq!(d.image.get(0, 0), [1.0; 3]);
    }

    #[test]
    fn a_jpeg_opens() {
        let img = RgbImage::from_fn(64, 48, |x, _| if x < 32 { Rgb([200, 60, 40]) } else { Rgb([30, 90, 160]) });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .write_image(img.as_raw(), 64, 48, image::ExtendedColorType::Rgb8)
            .unwrap();
        let d = decode(Format::Jpeg, &bytes).unwrap();
        assert_eq!((d.info.format.as_str(), d.image.width, d.image.height), ("JPEG", 64, 48));
        let left = d.image.get(8, 24);
        assert!(left[0] > left[1] * 3.0 && left[0] > left[2] * 3.0, "left side is red: {left:?}");
        assert!(d.info.notes[0].contains("8-bit"));
    }

    #[test]
    fn display_p3_colours_are_converted() {
        // Pure P3 red is more saturated than sRGB can show: red above 1, green below 0.
        let red = color::apply(&P3_TO_SRGB, [1.0, 0.0, 0.0]);
        assert!(red[0] > 1.1 && red[1] < 0.0);
        // White stays white.
        let white = color::apply(&P3_TO_SRGB, [1.0; 3]);
        assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-4));
        assert_eq!(colour_space(Some(b"....desc....Display P3....")), ColourSpace::DisplayP3);
        let wide: Vec<u8> = "Display P3".encode_utf16().flat_map(|c| c.to_be_bytes()).collect();
        assert_eq!(colour_space(Some(&wide)), ColourSpace::DisplayP3);
        assert_eq!(colour_space(Some(b"sRGB IEC61966-2.1")), ColourSpace::Srgb);
        assert_eq!(colour_space(Some(b"Adobe RGB (1998)")), ColourSpace::Other);
        assert_eq!(colour_space(None), ColourSpace::Srgb);
    }

    #[test]
    fn damaged_files_give_a_clear_error() {
        let e = decode(Format::Jpeg, b"\xFF\xD8\xFF\xE1rest of a jpeg").unwrap_err();
        assert!(matches!(e, PipelineError::Unreadable { .. }) && e.to_string().contains("JPEG"), "{e}");
        let e = decode(Format::Png, b"\x89PNG\r\n\x1a\nnot really").unwrap_err();
        assert!(e.to_string().contains("PNG"), "{e}");
        let e = decode(Format::Heic, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap_err();
        assert!(e.to_string().contains("HEIC"), "{e}");
    }
}
