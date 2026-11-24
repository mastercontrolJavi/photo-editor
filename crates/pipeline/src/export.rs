//! Full-resolution JPEG export.
//!
//! Rendering always starts again from the full-size linear image (never from
//! the preview), using the same `render` the preview uses. The crop, turns and
//! straightening are applied here, so the JPEG is the finished picture.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::adjust::Adjustments;
use crate::decode::DecodedImage;
use crate::error::{PipelineError, Result};
use crate::icc;
use crate::lut::Lut3D;
use crate::preset;
use crate::render::{render, RenderedImage, Scene, View};

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub out_dir: PathBuf,
    /// JPEG quality 1–100.
    pub quality: u8,
}

pub const DEFAULT_QUALITY: u8 = 92;

impl ExportOptions {
    /// Default destination: an `Exported` folder next to the source photo.
    pub fn next_to(source: &Path) -> Self {
        let dir = source.parent().map(Path::to_path_buf).unwrap_or_default();
        Self { out_dir: dir.join("Exported"), quality: DEFAULT_QUALITY }
    }
}

/// Encode 8-bit sRGB pixels as a baseline JPEG with an embedded sRGB ICC profile.
/// Pixels are written upright, so no EXIF orientation tag is needed.
pub fn encode_jpeg(img: &RenderedImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(img.rgb.len() / 4);
    let mut enc = JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
    enc.set_icc_profile(icc::srgb_profile().to_vec()).map_err(|e| PipelineError::Encode(e.to_string()))?;
    enc.write_image(&img.rgb, img.width as u32, img.height as u32, ExtendedColorType::Rgb8)
        .map_err(|e| PipelineError::Encode(e.to_string()))?;
    Ok(buf)
}

/// Render at full resolution and write `<stem>.jpg` into `opts.out_dir`.
/// If that name is taken, writes `<stem>-1.jpg`, `<stem>-2.jpg`, … — existing
/// files (including the source) are never overwritten. Returns the path written.
pub fn export_jpeg(source: &Path, decoded: &DecodedImage, adj: &Adjustments, opts: &ExportOptions) -> Result<PathBuf> {
    let lut = match &adj.lut {
        Some(l) => Some(preset::load_lut(&preset::library_dir(), &l.name).map_err(PipelineError::Look)?),
        None => None,
    };
    let rendered = render_full(decoded, adj, lut.as_ref());
    let bytes = encode_jpeg(&rendered, opts.quality)?;
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    write_unique(&opts.out_dir, &stem, "jpg", &bytes)
}

/// Render the finished picture at full resolution: the whole edit, cropped.
pub fn render_full(decoded: &DecodedImage, adj: &Adjustments, lut: Option<&Lut3D>) -> RenderedImage {
    let look = decoded.info.look();
    let scene = Scene { image: &decoded.image, look: &look, lut };
    let (view, width, height) = View::export(&adj.crop, decoded.image.width, decoded.image.height);
    render(&scene, adj, &view, width, height)
}

/// Write `bytes` to the first free `<stem>[-n].<ext>` in `dir`, reserving the
/// name atomically (create-new) so two exports can't collide. A partly written
/// file is removed if the write fails.
pub fn write_unique(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|source| PipelineError::Write { path: dir.to_owned(), source })?;
    for n in 0..100_000u32 {
        let name = if n == 0 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") };
        let path = dir.join(name);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(PipelineError::Write { path, source }),
        };
        let written = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        return match written {
            Ok(()) => Ok(path),
            Err(source) => {
                let _ = fs::remove_file(&path);
                Err(PipelineError::Write { path, source })
            }
        };
    }
    Err(PipelineError::Write {
        path: dir.join(format!("{stem}.{ext}")),
        source: std::io::Error::other("too many files with this name already"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collisions_get_numbered_names_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IMG_1.jpg"), b"keep me").unwrap();
        let a = write_unique(dir.path(), "IMG_1", "jpg", b"one").unwrap();
        let b = write_unique(dir.path(), "IMG_1", "jpg", b"two").unwrap();
        assert_eq!(a.file_name().unwrap(), "IMG_1-1.jpg");
        assert_eq!(b.file_name().unwrap(), "IMG_1-2.jpg");
        assert_eq!(std::fs::read(dir.path().join("IMG_1.jpg")).unwrap(), b"keep me");
    }

    #[test]
    fn unwritable_destination_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        // A *file* where the output folder should be makes create_dir_all fail.
        let blocker = dir.path().join("Exported");
        std::fs::write(&blocker, b"x").unwrap();
        let err = write_unique(&blocker, "IMG", "jpg", b"data").unwrap_err();
        assert!(matches!(err, PipelineError::Write { .. }), "{err}");
    }
}
