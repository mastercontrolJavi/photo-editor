//! Profiles: the starting look of a photo, chosen from a menu before any
//! slider is touched.
//!
//! * **Camera** (or **As shot** for JPEG, PNG and HEIC) is the photo as the
//!   camera rendered it. For iPhone ProRAW that means Apple's own tone map
//!   and curve from the file.
//! * The others (Color, Vivid, Landscape, Portrait, Flat, Monochrome…) are
//!   this app's own looks, built from its sliders and applied underneath
//!   them. They have the same purpose as Lightroom's Adobe profiles but they
//!   are not Adobe's and will not match them exactly.
//!
//! Each look is a hidden set of slider values. So that it can be mixed in by
//! an Amount and doesn't use up the visible sliders, it is baked once into a
//! colour look-up table.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::adjust::{Adjustments, BAND_NAMES};
use crate::color::srgb_decode;
use crate::decode::SourceKind;
use crate::lut::Lut3D;

/// Points per side of a baked profile table.
const SIZE: usize = 33;

/// One entry of the Profile menu.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileChoice {
    /// Stored in the edit; empty for the camera's own rendering.
    pub id: &'static str,
    pub label: String,
    /// Starts a new group in the menu (colour looks, then black & white).
    pub separator_before: bool,
}

fn mixer(bands: &[(&str, f32, f32, f32)]) -> Value {
    let mut list = vec![json!({}); BAND_NAMES.len()];
    for (name, hue, saturation, luminance) in bands {
        let i = BAND_NAMES.iter().position(|b| b == name).expect("known band");
        list[i] = json!({ "hue": hue, "saturation": saturation, "luminance": luminance });
    }
    Value::Array(list)
}

/// The slider values behind each look.
fn settings(id: &str) -> Option<Value> {
    Some(match id {
        "color" => json!({ "contrast": 12, "vibrance": 10, "saturation": 4 }),
        "vivid" => json!({ "contrast": 25, "blacks": -6, "vibrance": 22, "saturation": 10 }),
        "landscape" => json!({
            "contrast": 10, "vibrance": 18,
            "mixer": mixer(&[("Green", 0.0, 12.0, 0.0), ("Aqua", 0.0, 8.0, 0.0), ("Blue", 0.0, 14.0, -6.0)])
        }),
        "portrait" => json!({
            "contrast": 4, "vibrance": 6,
            "mixer": mixer(&[("Red", 0.0, -4.0, 0.0), ("Orange", 0.0, -6.0, 4.0)])
        }),
        "flat" => json!({ "contrast": -22, "saturation": -6 }),
        "mono" => json!({ "bw": true }),
        "mono-rich" => json!({ "bw": true, "contrast": 25, "blacks": -8, "bw_mix": [8, 10, 0, 0, 0, -20, 0, 0] }),
        _ => return None,
    })
}

/// Whether `id` names a profile (the empty id, the camera's own rendering, counts).
pub fn exists(id: &str) -> bool {
    id.is_empty() || settings(id).is_some()
}

/// The Profile menu for a photo of this kind.
pub fn choices(kind: SourceKind, make: &str) -> Vec<ProfileChoice> {
    let camera = match kind {
        SourceKind::Rendered => "As shot".to_string(),
        SourceKind::LinearRaw if make.eq_ignore_ascii_case("Apple") => "Apple ProRAW".to_string(),
        _ => "Camera".to_string(),
    };
    let entry = |id: &'static str, label: &str, separator_before: bool| ProfileChoice { id, label: label.to_string(), separator_before };
    vec![
        entry("", &camera, false),
        entry("color", "Color", true),
        entry("vivid", "Vivid", false),
        entry("landscape", "Landscape", false),
        entry("portrait", "Portrait", false),
        entry("flat", "Flat", false),
        entry("mono", "Monochrome", true),
        entry("mono-rich", "Monochrome Rich", false),
    ]
}

/// The profile this app offers in place of one of Lightroom's Adobe
/// profiles, for imported presets. `None` if there is no counterpart.
pub fn for_adobe_name(name: &str) -> Option<&'static str> {
    Some(match name.trim() {
        "Adobe Standard" | "Camera Standard" | "Apple ProRaw" | "Apple ProRAW" | "Embedded" => "",
        "Adobe Color" => "color",
        "Adobe Vivid" => "vivid",
        "Adobe Landscape" => "landscape",
        "Adobe Portrait" => "portrait",
        "Adobe Neutral" => "flat",
        "Adobe Monochrome" | "Apple ProRaw Monochrome" | "Camera Monochrome" => "mono",
        _ => return None,
    })
}

/// The baked colour table for a profile. `None` for the camera's own
/// rendering (nothing to apply) or an unknown id.
pub fn table(id: &str) -> Option<Arc<Lut3D>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Lut3D>>>> = OnceLock::new();
    let settings = settings(id)?;
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = cache.get(id) {
        return Some(found.clone());
    }
    // The look's slider values, on an otherwise untouched edit.
    let mut value = serde_json::to_value(Adjustments::default()).expect("adjustments always serialise");
    crate::preset::merge(&mut value, &settings);
    let mut look: Adjustments = serde_json::from_value(value).expect("profile settings are valid");
    look.sharpen.amount = 0.0;
    // Run every grid colour through the normal slider maths.
    let n = (SIZE - 1) as f32;
    let mut colours = Vec::with_capacity(SIZE * SIZE * SIZE);
    for b in 0..SIZE {
        for g in 0..SIZE {
            for r in 0..SIZE {
                colours.push([r as f32 / n, g as f32 / n, b as f32 / n].map(srgb_decode));
            }
        }
    }
    let shaded = crate::render::shade_colours(&look, &colours);
    let baked = Arc::new(Lut3D::from_grid(id, SIZE, shaded));
    cache.insert(id.to_string(), baked.clone());
    Some(baked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_names_the_camera_rendering_by_what_the_photo_is() {
        assert_eq!(choices(SourceKind::LinearRaw, "Apple")[0].label, "Apple ProRAW");
        assert_eq!(choices(SourceKind::BayerCfa, "DJI")[0].label, "Camera");
        assert_eq!(choices(SourceKind::Rendered, "")[0].label, "As shot");
        // Every entry can be applied.
        for c in choices(SourceKind::Rendered, "") {
            assert!(exists(c.id), "{}", c.id);
            assert_eq!(table(c.id).is_some(), !c.id.is_empty(), "{}", c.id);
        }
        assert!(!exists("no-such-profile") && table("no-such-profile").is_none());
    }

    #[test]
    fn looks_do_what_their_names_say() {
        let orange = [0.8, 0.5, 0.3];
        let spread = |c: [f32; 3]| c.iter().cloned().fold(0.0, f32::max) - c.iter().cloned().fold(1.0, f32::min);
        let through = |id: &str, c: [f32; 3]| table(id).unwrap().sample(c);
        assert!(spread(through("vivid", orange)) > spread(through("color", orange)));
        assert!(spread(through("color", orange)) > spread(orange));
        assert!(spread(through("flat", orange)) < spread(orange));
        assert!(spread(through("mono", orange)) < 0.01);
        // Greys stay grey, black stays black and white stays white in every colour look.
        for id in ["color", "vivid", "landscape", "portrait", "flat"] {
            let grey = through(id, [0.5; 3]);
            assert!(spread(grey) < 0.01, "{id}: {grey:?}");
            assert!(through(id, [0.0; 3])[0] < 0.02 && through(id, [1.0; 3])[0] > 0.98, "{id}");
        }
        // Landscape boosts blue more than it boosts orange.
        let blue = [0.3, 0.45, 0.8];
        let gain = |id: &str, c: [f32; 3]| spread(through(id, c)) / spread(c);
        assert!(gain("landscape", blue) > gain("landscape", orange));
    }

    #[test]
    fn adobe_profile_names_map_to_their_counterparts() {
        assert_eq!(for_adobe_name("Adobe Color"), Some("color"));
        assert_eq!(for_adobe_name("Adobe Standard"), Some(""));
        assert_eq!(for_adobe_name("Adobe Monochrome"), Some("mono"));
        assert_eq!(for_adobe_name("Somebody's Film Pack 03"), None);
    }
}
