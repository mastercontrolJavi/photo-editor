//! Image pipeline for the photo editor.
//!
//! Everything that touches pixels lives here, with no UI code, so the desktop
//! app and the `pe` command-line tool share exactly the same maths.
//!
//! Flow:
//!
//! ```text
//! file ──decode──► LinearImage (scene-linear f32, sRGB primaries, D65, oriented)
//!                     │
//!         ┌───────────┴────────────┐
//!   downscale once            (full size)
//!         │                        │
//!   render(preview)          render(export) ──► JPEG + sRGB ICC
//! ```
//!
//! Preview and export call the same `render` function; the only difference is
//! the input size.

pub mod adjust;
pub mod color;
pub mod decode;
pub mod error;
pub mod export;
pub mod fixtures;
pub mod icc;
pub mod image;
pub mod look;
pub mod probe;
pub mod render;
pub mod sidecar;

pub use adjust::Adjustments;
pub use decode::{decode_file, DecodedImage, SourceInfo};
pub use error::PipelineError;
pub use export::{export_jpeg, ExportOptions};
pub use image::LinearImage;
pub use look::{GainTableMap, Look, ToneCurve};
pub use render::{render_srgb8, RenderedImage};

/// Longest edge, in pixels, of the cached preview image.
pub const PREVIEW_LONG_EDGE: usize = 2560;
