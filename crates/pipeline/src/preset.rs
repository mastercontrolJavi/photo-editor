//! Presets: named sets of slider values.
//!
//! Three sources:
//!
//! * **Starter presets** built into the app ([`builtin`]).
//! * **Imported** Lightroom presets (`.xmp`, or the older `.lrtemplate`) and
//!   `.cube` look-up tables. Importing converts them to this app's own small
//!   JSON format and keeps a copy in the app's presets folder.
//! * **Yours**: saved from the current edit.
//!
//! A Lightroom preset is a list of Lightroom's slider values. The sliders this
//! app has are mapped across by name; anything it can't apply yet is listed in
//! [`Preset::unsupported`] so the import can say so plainly instead of
//! silently looking different.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::adjust::{Adjustments, BAND_NAMES};
use crate::lut::Lut3D;

pub const SCHEMA: &str = "photo-editor/preset";
pub const FILE_SUFFIX: &str = ".preset.json";
pub const GROUP_YOURS: &str = "Yours";
pub const GROUP_LUTS: &str = "LUTs";

fn schema() -> String {
    SCHEMA.into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default = "schema")]
    pub schema: String,
    pub name: String,
    pub group: String,
    /// The settings this preset changes, laid out like the "adjustments" of an
    /// edit file. Settings that aren't listed are left as they are.
    pub settings: Value,
    /// Start from an unedited photo before applying `settings` (instead of
    /// layering them over the current edit).
    #[serde(default)]
    pub replace: bool,
    /// Things the original preset asked for that this app can't apply.
    #[serde(default)]
    pub unsupported: Vec<String>,
    /// Where this preset is stored (not part of the file).
    #[serde(skip)]
    pub file: Option<PathBuf>,
}

/// Overlay `patch` on `base`. Objects merge key by key; two lists of objects of
/// the same length merge item by item (the eight Color Mixer bands); anything
/// else is replaced.
pub(crate) fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (Value::Array(b), Value::Array(p)) if b.len() == p.len() && p.iter().all(Value::is_object) => {
            for (bi, pi) in b.iter_mut().zip(p) {
                merge(bi, pi);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// Parts of an edit that belong to one photo and so are never part of a
/// preset: the framing, and work done on particular places in the picture.
const PER_PHOTO: [&str; 4] = ["crop", "geometry", "masks", "spots"];

impl Preset {
    /// The edit you get by applying this preset to `current`. The crop is never changed.
    pub fn apply(&self, current: &Adjustments) -> Adjustments {
        self.apply_from(current, &Adjustments::default())
    }

    /// Like [`Preset::apply`], for a photo whose untouched state is `fresh`
    /// rather than the general default (an already-processed photo starts
    /// with no sharpening, for instance).
    pub fn apply_from(&self, current: &Adjustments, fresh: &Adjustments) -> Adjustments {
        let start = if self.replace { fresh.clone() } else { current.clone() };
        let mut value = serde_json::to_value(&start).expect("adjustments always serialise");
        let mut settings = self.settings.clone();
        if let Some(obj) = settings.as_object_mut() {
            for key in PER_PHOTO {
                obj.remove(key);
            }
        }
        merge(&mut value, &settings);
        match serde_json::from_value::<Adjustments>(value) {
            Ok(a) => a.sanitized().with_crop_of(current).with_local_work_of(current),
            Err(_) => current.clone(),
        }
    }

    /// A preset holding the whole of `adj` except its crop.
    pub fn from_adjustments(name: &str, group: &str, adj: &Adjustments) -> Self {
        let mut settings = serde_json::to_value(adj.clone().sanitized()).expect("adjustments always serialise");
        if let Some(obj) = settings.as_object_mut() {
            for key in PER_PHOTO {
                obj.remove(key);
            }
        }
        Preset { schema: schema(), name: name.into(), group: group.into(), settings, replace: true, unsupported: vec![], file: None }
    }

    pub fn is_builtin(&self) -> bool {
        self.file.is_none()
    }
}

// ───────────────────────────── starter presets ─────────────────────────────

/// Color Mixer settings for a few bands: `(band name, hue, saturation, luminance)`.
fn mixer(bands: &[(&str, f32, f32, f32)]) -> Value {
    let mut list = vec![json!({}); BAND_NAMES.len()];
    for (name, hue, saturation, luminance) in bands {
        let i = BAND_NAMES.iter().position(|b| b == name).expect("known band");
        list[i] = json!({ "hue": hue, "saturation": saturation, "luminance": luminance });
    }
    Value::Array(list)
}

fn wheel(hue: f32, saturation: f32) -> Value {
    json!({ "hue": hue, "saturation": saturation })
}

/// The presets that ship with the app.
pub fn builtin() -> Vec<Preset> {
    const COLOUR: &str = "Starter: Colour";
    const MONO: &str = "Starter: Black & White";
    let make = |group: &str, name: &str, settings: Value| Preset {
        schema: schema(),
        name: name.into(),
        group: group.into(),
        settings,
        replace: true,
        unsupported: vec![],
        file: None,
    };
    vec![
        make(COLOUR, "Clean Boost", json!({
            "contrast": 12, "highlights": -25, "shadows": 20, "whites": 8, "blacks": -8, "vibrance": 18, "clarity": 8
        })),
        make(COLOUR, "Bright & Airy", json!({
            "exposure": 0.3, "contrast": -8, "highlights": -35, "shadows": 40, "blacks": 15, "vibrance": 10,
            "saturation": -5, "temperature": 4,
            "curve": { "rgb": [[0.0, 0.04], [0.5, 0.52], [1.0, 1.0]] }
        })),
        make(COLOUR, "Warm Golden", json!({
            "temperature": 14, "tint": 4, "contrast": 10, "highlights": -20, "shadows": 15, "vibrance": 15,
            "mixer": mixer(&[("Orange", 0.0, 10.0, 0.0), ("Yellow", -10.0, 0.0, 0.0)]),
            "grading": { "highlights": wheel(45.0, 20.0), "shadows": wheel(30.0, 10.0) }
        })),
        make(COLOUR, "Cool Blue Hour", json!({
            "temperature": -14, "contrast": 12, "shadows": 10, "blacks": -10, "vibrance": 10,
            "mixer": mixer(&[("Blue", 0.0, 10.0, -10.0)]),
            "grading": { "shadows": wheel(220.0, 22.0), "highlights": wheel(200.0, 8.0) }
        })),
        make(COLOUR, "Teal & Orange", json!({
            "contrast": 15, "highlights": -20, "shadows": 15, "vibrance": 12,
            "mixer": mixer(&[("Orange", 0.0, 12.0, 0.0), ("Yellow", -10.0, -15.0, 0.0), ("Green", 15.0, -25.0, 0.0),
                             ("Blue", -15.0, -10.0, 0.0)]),
            "grading": { "shadows": wheel(195.0, 30.0), "highlights": wheel(35.0, 25.0), "balance": 10 }
        })),
        make(COLOUR, "Soft Film", json!({
            "contrast": -10, "highlights": -25, "shadows": 25, "saturation": -12, "vibrance": 8,
            "curve": { "rgb": [[0.0, 0.07], [0.25, 0.24], [0.75, 0.78], [1.0, 0.95]] },
            "grading": { "shadows": wheel(200.0, 10.0), "highlights": wheel(50.0, 10.0) },
            "grain": { "amount": 22, "size": 30 }, "vignette": { "amount": -10 }
        })),
        make(COLOUR, "Moody Dark", json!({
            "exposure": -0.25, "contrast": 20, "highlights": -40, "shadows": -10, "blacks": -15, "saturation": -18,
            "vibrance": 5, "clarity": 12,
            "mixer": mixer(&[("Yellow", 0.0, -25.0, 0.0), ("Green", 0.0, -35.0, -15.0)]),
            "grading": { "shadows": wheel(210.0, 14.0) }, "vignette": { "amount": -22 }
        })),
        make(COLOUR, "Vivid Landscape", json!({
            "contrast": 14, "highlights": -35, "shadows": 30, "whites": 10, "blacks": -12, "vibrance": 28,
            "clarity": 18, "dehaze": 12,
            "mixer": mixer(&[("Green", 0.0, 8.0, 0.0), ("Blue", 0.0, 12.0, -12.0)])
        })),
        make(COLOUR, "Portrait Soft", json!({
            "contrast": -5, "highlights": -20, "shadows": 18, "clarity": -12, "vibrance": 8,
            "mixer": mixer(&[("Red", 0.0, -5.0, 0.0), ("Orange", 0.0, -4.0, 8.0)]), "vignette": { "amount": -8 }
        })),
        make(MONO, "B&W Classic", json!({
            "bw": true, "contrast": 18, "highlights": -15, "shadows": 10, "blacks": -10, "clarity": 10
        })),
        make(MONO, "B&W High Contrast", json!({
            "bw": true, "contrast": 45, "whites": 15, "blacks": -25, "clarity": 22,
            "bw_mix": [10, 15, 0, 0, 0, -30, 0, 0], "grain": { "amount": 18 }
        })),
        make(MONO, "B&W Matte", json!({
            "bw": true, "contrast": 8, "curve": { "rgb": [[0.0, 0.09], [0.3, 0.3], [1.0, 0.96]] },
            "grading": { "shadows": wheel(220.0, 6.0) }, "grain": { "amount": 25 }
        })),
    ]
}

// ─────────────────────────── Lightroom import ───────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Num(f64),
    Bool(bool),
    Text(String),
    List(Vec<f64>),
}

fn scalar(s: &str) -> Val {
    let t = s.trim();
    match t {
        "True" | "true" => return Val::Bool(true),
        "False" | "false" => return Val::Bool(false),
        _ => {}
    }
    match t.trim_start_matches('+').parse::<f64>() {
        Ok(n) if n.is_finite() => Val::Num(n),
        _ => Val::Text(t.to_string()),
    }
}

type Values = HashMap<String, Val>;

fn ident_len(s: &str) -> usize {
    s.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count()
}

/// Every `<rdf:li …>text</rdf:li>` inside `inner`.
fn list_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut rest = inner;
    while let Some(i) = rest.find("<rdf:li") {
        let after = &rest[i..];
        let Some(open_end) = after.find('>') else { break };
        if after[..open_end].ends_with('/') {
            rest = &after[open_end + 1..];
            continue;
        }
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</rdf:li>") else { break };
        items.push(&body[..close]);
        rest = &body[close..];
    }
    items
}

/// Read Lightroom's `crs:` settings out of an XMP preset. Returns the settings
/// and the preset's own name and group, if it states them.
fn read_xmp(text: &str) -> (Values, Option<String>, Option<String>) {
    let mut values = Values::new();
    let (mut name, mut group) = (None, None);
    let mut pos = 0;
    while let Some(i) = text[pos..].find("crs:") {
        let start = pos + i + 4;
        pos = start;
        let rest = &text[start..];
        let n = ident_len(rest);
        if n == 0 {
            continue;
        }
        let key = &rest[..n];
        let after = &rest[n..];
        let is_element = start >= 5 && text.as_bytes()[start - 5] == b'<';
        if let Some(v) = after.strip_prefix("=\"") {
            // Attribute: crs:Key="value"
            if let Some(end) = v.find('"') {
                values.entry(key.to_string()).or_insert_with(|| scalar(&v[..end]));
            }
        } else if is_element && after.starts_with('>') {
            // Element: <crs:Key> … </crs:Key>
            let closing = format!("</crs:{key}>");
            let Some(end) = after.find(&closing) else { continue };
            let inner = &after[1..end];
            let items = list_items(inner);
            if key == "Name" || key == "Group" {
                let text = items.first().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if key == "Name" && name.is_none() {
                    name = text;
                } else if key == "Group" && group.is_none() {
                    group = text;
                }
                continue;
            }
            let val = if items.is_empty() {
                scalar(inner)
            } else {
                let nums: Vec<f64> =
                    items.iter().flat_map(|it| it.split(',')).filter_map(|n| n.trim().parse::<f64>().ok()).collect();
                if nums.is_empty() {
                    Val::Text(items.join(" "))
                } else {
                    Val::List(nums)
                }
            };
            values.entry(key.to_string()).or_insert(val);
        }
    }
    (values, name, group)
}

/// Read the settings out of an old-style `.lrtemplate` preset (a Lua table).
fn read_lrtemplate(text: &str) -> (Values, Option<String>) {
    let mut values = Values::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            // Skip a string so words inside it aren't taken for keys.
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        if !(c.is_ascii_alphabetic() || c == b'_') {
            i += 1;
            continue;
        }
        let n = ident_len(&text[i..]);
        let key = &text[i..i + n];
        i += n;
        let rest = text[i..].trim_start();
        let Some(value) = rest.strip_prefix('=') else { continue };
        let value = value.trim_start();
        let consumed = text.len() - value.len();
        if let Some(body) = value.strip_prefix('"') {
            let end = body.find('"').unwrap_or(body.len());
            values.entry(key.to_string()).or_insert_with(|| Val::Text(body[..end].to_string()));
            i = consumed + 1 + end + 1;
        } else if let Some(body) = value.strip_prefix('{') {
            let end = body.find('}').unwrap_or(body.len());
            let inside = &body[..end];
            if !inside.contains('=') && !inside.contains('{') {
                let nums: Vec<f64> = inside.split(',').filter_map(|n| n.trim().parse::<f64>().ok()).collect();
                values.entry(key.to_string()).or_insert(Val::List(nums));
                i = consumed + 1 + end;
            } else {
                i = consumed + 1; // a nested table: keep reading inside it
            }
        } else {
            let end = value.find([',', '\n', '}']).unwrap_or(value.len());
            values.entry(key.to_string()).or_insert_with(|| scalar(&value[..end]));
            i = consumed + end;
        }
    }
    let title = match values.get("title") {
        Some(Val::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => None,
    };
    (values, title)
}

/// Turn Lightroom's setting names into this app's settings.
fn map_lightroom(v: &Values) -> (Map<String, Value>, Vec<String>) {
    let num = |k: &str| match v.get(k) {
        Some(Val::Num(n)) => Some(*n),
        _ => None,
    };
    let nonzero = |keys: &[&str]| keys.iter().any(|k| num(k).is_some_and(|n| n != 0.0));
    let mut out = Map::new();
    let put = |obj: &mut Map<String, Value>, ours: &str, theirs: &str| {
        if let Some(n) = num(theirs) {
            obj.insert(ours.into(), json!(n));
        }
    };

    for (theirs, ours) in [
        ("Exposure2012", "exposure"),
        ("Contrast2012", "contrast"),
        ("Highlights2012", "highlights"),
        ("Shadows2012", "shadows"),
        ("Whites2012", "whites"),
        ("Blacks2012", "blacks"),
        ("Clarity2012", "clarity"),
        ("Texture", "texture"),
        ("Dehaze", "dehaze"),
        ("Vibrance", "vibrance"),
        ("Saturation", "saturation"),
        ("IncrementalTemperature", "temperature"),
        ("IncrementalTint", "tint"),
    ] {
        put(&mut out, ours, theirs);
    }

    // Lightroom's own profiles have counterparts here; anything else is a custom look.
    let adobe_profile = match (v.get("Name"), v.get("CameraProfile")) {
        (Some(Val::Text(name)), _) if v.contains_key("Look") => Some(name.as_str()),
        (_, Some(Val::Text(name))) => Some(name.as_str()),
        _ => None,
    };
    let matched_profile = adobe_profile.and_then(crate::profile::for_adobe_name);
    if let Some(id) = matched_profile {
        out.insert("profile".into(), json!({ "name": id }));
    }

    // Color Mixer.
    let mut bands = Vec::new();
    let mut any_band = false;
    for band in BAND_NAMES {
        let mut b = Map::new();
        put(&mut b, "hue", &format!("HueAdjustment{band}"));
        put(&mut b, "saturation", &format!("SaturationAdjustment{band}"));
        put(&mut b, "luminance", &format!("LuminanceAdjustment{band}"));
        any_band |= !b.is_empty();
        bands.push(Value::Object(b));
    }
    if any_band {
        out.insert("mixer".into(), Value::Array(bands));
    }

    // Black & white.
    let treatment_bw = matches!(v.get("Treatment"), Some(Val::Text(t)) if t.contains("Black"));
    match v.get("ConvertToGrayscale") {
        Some(Val::Bool(b)) => {
            out.insert("bw".into(), json!(*b || treatment_bw));
        }
        _ if treatment_bw => {
            out.insert("bw".into(), json!(true));
        }
        _ => {}
    }
    if BAND_NAMES.iter().any(|b| num(&format!("GrayMixer{b}")).is_some()) {
        let mix: Vec<f64> = BAND_NAMES.iter().map(|b| num(&format!("GrayMixer{b}")).unwrap_or(0.0)).collect();
        out.insert("bw_mix".into(), json!(mix));
    }

    // Color Grading (older presets call the two-way version "split toning").
    let mut grading = Map::new();
    for (ours, hue, sat, lum) in [
        ("shadows", "SplitToningShadowHue", "SplitToningShadowSaturation", "ColorGradeShadowLum"),
        ("highlights", "SplitToningHighlightHue", "SplitToningHighlightSaturation", "ColorGradeHighlightLum"),
        ("midtones", "ColorGradeMidtoneHue", "ColorGradeMidtoneSat", "ColorGradeMidtoneLum"),
        ("global", "ColorGradeGlobalHue", "ColorGradeGlobalSat", "ColorGradeGlobalLum"),
    ] {
        let mut w = Map::new();
        put(&mut w, "hue", hue);
        put(&mut w, "saturation", sat);
        put(&mut w, "luminance", lum);
        if !w.is_empty() {
            grading.insert(ours.into(), Value::Object(w));
        }
    }
    put(&mut grading, "blending", "ColorGradeBlending");
    put(&mut grading, "balance", "SplitToningBalance");
    if !grading.is_empty() {
        out.insert("grading".into(), Value::Object(grading));
    }

    // Point curves: Lightroom lists them as x, y, x, y… on a 0–255 scale.
    let mut curve = Map::new();
    for (ours, theirs) in [
        ("rgb", "ToneCurvePV2012"),
        ("red", "ToneCurvePV2012Red"),
        ("green", "ToneCurvePV2012Green"),
        ("blue", "ToneCurvePV2012Blue"),
    ] {
        if let Some(Val::List(list)) = v.get(theirs) {
            let points: Vec<[f64; 2]> = list.as_chunks::<2>().0.iter().map(|p| [p[0] / 255.0, p[1] / 255.0]).collect();
            curve.insert(ours.into(), json!(points));
        }
    }
    if !curve.is_empty() {
        out.insert("curve".into(), Value::Object(curve));
    }

    // The parametric curve.
    let mut parametric = Map::new();
    put(&mut parametric, "shadows", "ParametricShadows");
    put(&mut parametric, "darks", "ParametricDarks");
    put(&mut parametric, "lights", "ParametricLights");
    put(&mut parametric, "highlights", "ParametricHighlights");
    if !parametric.is_empty() {
        let curve = out.entry("curve").or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = curve.as_object_mut() {
            obj.insert("parametric".into(), Value::Object(parametric));
        }
    }

    let mut calibration = Map::new();
    for (ours, theirs) in [
        ("red_hue", "RedHue"),
        ("red_saturation", "RedSaturation"),
        ("green_hue", "GreenHue"),
        ("green_saturation", "GreenSaturation"),
        ("blue_hue", "BlueHue"),
        ("blue_saturation", "BlueSaturation"),
    ] {
        put(&mut calibration, ours, theirs);
    }
    if !calibration.is_empty() {
        out.insert("calibration".into(), Value::Object(calibration));
    }

    let mut sharpen = Map::new();
    put(&mut sharpen, "amount", "Sharpness");
    put(&mut sharpen, "radius", "SharpenRadius");
    put(&mut sharpen, "detail", "SharpenDetail");
    put(&mut sharpen, "masking", "SharpenEdgeMasking");
    if !sharpen.is_empty() {
        out.insert("sharpen".into(), Value::Object(sharpen));
    }
    let mut noise = Map::new();
    put(&mut noise, "luminance", "LuminanceSmoothing");
    put(&mut noise, "color", "ColorNoiseReduction");
    if !noise.is_empty() {
        out.insert("noise".into(), Value::Object(noise));
    }
    let mut optics = Map::new();
    put(&mut optics, "defringe_purple", "DefringePurpleAmount");
    put(&mut optics, "defringe_green", "DefringeGreenAmount");
    put(&mut optics, "distortion", "LensManualDistortionAmount");
    if let Some(Val::Num(n)) = v.get("AutoLateralCA") {
        optics.insert("remove_ca".into(), json!(*n != 0.0));
    }
    if !optics.is_empty() {
        out.insert("optics".into(), Value::Object(optics));
    }

    let mut vignette = Map::new();
    put(&mut vignette, "amount", "PostCropVignetteAmount");
    put(&mut vignette, "midpoint", "PostCropVignetteMidpoint");
    put(&mut vignette, "feather", "PostCropVignetteFeather");
    put(&mut vignette, "roundness", "PostCropVignetteRoundness");
    if !vignette.is_empty() {
        out.insert("vignette".into(), Value::Object(vignette));
    }
    let mut grain = Map::new();
    put(&mut grain, "amount", "GrainAmount");
    put(&mut grain, "size", "GrainSize");
    put(&mut grain, "roughness", "GrainFrequency");
    if !grain.is_empty() {
        out.insert("grain".into(), Value::Object(grain));
    }

    // What the preset wanted that we can't do.
    let mut missing = Vec::new();
    let mut note = |cond: bool, what: &str| {
        if cond {
            missing.push(what.to_string());
        }
    };
    let custom_look = v.contains_key("LookTable") || v.contains_key("RGBTable") || (v.contains_key("Look") && matched_profile.is_none());
    note(custom_look, "Colour profile / look table");
    note(
        ["MaskGroupBasedCorrections", "GradientBasedCorrections", "CircularGradientBasedCorrections", "PaintBasedCorrections"]
            .iter()
            .any(|k| v.contains_key(*k)),
        "Masks (local adjustments)",
    );
    note(nonzero(&["ShadowTint"]), "Calibration shadow tint");
    let custom_wb = matches!(v.get("WhiteBalance"), Some(Val::Text(t)) if t != "As Shot");
    note(custom_wb || num("Temperature").is_some(), "White balance in Kelvin (kept as shot)");
    note(nonzero(&["LensProfileEnable"]), "Lens profile corrections");
    note(
        nonzero(&["PerspectiveVertical", "PerspectiveHorizontal", "PerspectiveRotate", "PerspectiveScale", "PerspectiveAspect"]),
        "Geometry",
    );
    note(v.contains_key("PointColors"), "Point colour");
    note(nonzero(&["LensBlurActive"]) || matches!(v.get("LensBlurActive"), Some(Val::Bool(true))), "Lens blur");
    let old_process = out.is_empty() && ["Exposure", "Contrast", "Brightness", "FillLight"].iter().any(|k| num(k).is_some());
    note(old_process, "Tone settings from an old Lightroom version");
    (out, missing)
}

/// Convert the text of a Lightroom `.xmp` preset.
pub fn from_xmp(text: &str, fallback_name: &str) -> Result<Preset, String> {
    let (values, name, group) = read_xmp(text);
    finish(values, name, group, fallback_name)
}

/// Convert the text of an old-style Lightroom `.lrtemplate` preset.
pub fn from_lrtemplate(text: &str, fallback_name: &str) -> Result<Preset, String> {
    let (values, title) = read_lrtemplate(text);
    finish(values, title, None, fallback_name)
}

fn finish(values: Values, name: Option<String>, group: Option<String>, fallback_name: &str) -> Result<Preset, String> {
    let (settings, unsupported) = map_lightroom(&values);
    if settings.is_empty() {
        return Err(if unsupported.is_empty() {
            "no Lightroom settings were found in this file".into()
        } else {
            format!("it only uses things this app can't apply yet ({})", unsupported.join(", "))
        });
    }
    Ok(Preset {
        schema: schema(),
        name: name.unwrap_or_else(|| fallback_name.to_string()),
        group: group.unwrap_or_else(|| "Imported".to_string()),
        settings: Value::Object(settings),
        replace: false,
        unsupported,
        file: None,
    })
}

// ─────────────────────────────── the library ───────────────────────────────

/// Where imported and saved presets live:
/// `%APPDATA%\Photo Editor\presets` on Windows,
/// `~/Library/Application Support/Photo Editor/presets` on a Mac.
pub fn library_dir() -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("Photo Editor").join("presets")
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_alphanumeric() || " -_.&+()".contains(c) { c } else { '_' }).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "preset".into()
    } else {
        cleaned.chars().take(80).collect()
    }
}

fn store(dir: &Path, mut preset: Preset) -> Result<Preset, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}{FILE_SUFFIX}", safe_file_name(&format!("{} - {}", preset.group, preset.name))));
    let json = serde_json::to_vec_pretty(&preset).expect("presets always serialise");
    std::fs::write(&path, json).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    preset.file = Some(path);
    Ok(preset)
}

/// Import a `.xmp`, `.lrtemplate` or `.cube` file into the library at `dir`.
pub fn import_file(path: &Path, dir: &Path) -> Result<Preset, String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Preset".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let read = || std::fs::read(path).map_err(|e| format!("couldn't read the file: {e}"));
    let preset = match ext.as_str() {
        "xmp" => from_xmp(&String::from_utf8_lossy(&read()?), &stem)?,
        "lrtemplate" => from_lrtemplate(&String::from_utf8_lossy(&read()?), &stem)?,
        "cube" => {
            let bytes = read()?;
            Lut3D::parse(&String::from_utf8_lossy(&bytes))?;
            let luts = dir.join("luts");
            std::fs::create_dir_all(&luts).map_err(|e| format!("couldn't create {}: {e}", luts.display()))?;
            let file_name = format!("{}.cube", safe_file_name(&stem));
            std::fs::write(luts.join(&file_name), &bytes).map_err(|e| format!("couldn't copy the LUT: {e}"))?;
            Preset {
                schema: schema(),
                name: stem,
                group: GROUP_LUTS.into(),
                settings: json!({ "lut": { "name": file_name, "amount": 100 } }),
                replace: false,
                unsupported: vec![],
                file: None,
            }
        }
        _ => return Err("only .xmp, .lrtemplate and .cube files can be imported".into()),
    };
    store(dir, preset)
}

/// Save the current edit as one of "Yours".
pub fn save_user_preset(dir: &Path, name: &str, adj: &Adjustments) -> Result<Preset, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("give the preset a name first".into());
    }
    store(dir, Preset::from_adjustments(name, GROUP_YOURS, adj))
}

/// Every stored preset in `dir`, sorted by group then name. Files that can't
/// be read are skipped.
pub fn load_library(dir: &Path) -> Vec<Preset> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(FILE_SUFFIX)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(mut preset) = serde_json::from_slice::<Preset>(&bytes) else { continue };
        if preset.schema != SCHEMA || !preset.settings.is_object() {
            continue;
        }
        preset.file = Some(path);
        out.push(preset);
    }
    out.sort_by_cached_key(|p| (p.group.to_lowercase(), p.name.to_lowercase()));
    out
}

/// Remove a stored preset (and its LUT file, if it has one).
pub fn delete(preset: &Preset, dir: &Path) -> Result<(), String> {
    let Some(file) = &preset.file else { return Err("built-in presets can't be deleted".into()) };
    if let Some(name) = preset.settings.pointer("/lut/name").and_then(Value::as_str) {
        let _ = std::fs::remove_file(dir.join("luts").join(safe_file_name(name)));
    }
    std::fs::remove_file(file).map_err(|e| format!("couldn't delete {}: {e}", file.display()))
}

/// Load the look-up table called `name` from the library at `dir`.
pub fn load_lut(dir: &Path, name: &str) -> Result<Lut3D, String> {
    // Only a plain file name is accepted, never a path.
    if name.contains(['/', '\\']) || name.contains("..") {
        return Err(format!("\"{name}\" is not a valid LUT name"));
    }
    Lut3D::load(&dir.join("luts").join(name)).map_err(|e| format!("the look-up table \"{name}\" can't be used: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal" crs:ProcessVersion="11.0"
   crs:Exposure2012="+0.35" crs:Contrast2012="+20" crs:Highlights2012="-40" crs:Shadows2012="+30"
   crs:Whites2012="0" crs:Blacks2012="-12" crs:Texture="+15" crs:Clarity2012="+10" crs:Dehaze="+5"
   crs:Vibrance="+18" crs:Saturation="-6" crs:Sharpness="40" crs:ConvertToGrayscale="False"
   crs:HueAdjustmentOrange="-8" crs:SaturationAdjustmentGreen="-30" crs:LuminanceAdjustmentBlue="-15"
   crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="18"
   crs:SplitToningHighlightHue="45" crs:SplitToningHighlightSaturation="12" crs:SplitToningBalance="+10"
   crs:ColorGradeBlending="60" crs:GrainAmount="20" crs:PostCropVignetteAmount="-15"
   crs:RedHue="+10" crs:WhiteBalance="As Shot">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Moody Forest</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Free Pack</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 20</rdf:li>
     <rdf:li>128, 130</rdf:li>
     <rdf:li>255, 240</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012Red>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn imports_a_lightroom_xmp_preset() {
        let p = from_xmp(XMP, "file name").unwrap();
        assert_eq!((p.name.as_str(), p.group.as_str()), ("Moody Forest", "Free Pack"));
        let a = p.apply(&Adjustments::default());
        assert_eq!((a.exposure, a.contrast, a.highlights, a.shadows, a.blacks), (0.35, 20.0, -40.0, 30.0, -12.0));
        assert_eq!((a.clarity, a.dehaze, a.vibrance, a.saturation), (10.0, 5.0, 18.0, -6.0));
        assert_eq!((a.mixer[1].hue, a.mixer[3].saturation, a.mixer[5].luminance), (-8.0, -30.0, -15.0));
        assert_eq!((a.grading.shadows.hue, a.grading.shadows.saturation), (210.0, 18.0));
        assert_eq!((a.grading.highlights.hue, a.grading.balance, a.grading.blending), (45.0, 10.0, 60.0));
        assert_eq!(a.curve.rgb.len(), 3);
        assert!((a.curve.rgb[0][1] - 20.0 / 255.0).abs() < 1e-6);
        assert!(a.curve.red.is_empty(), "a straight channel curve is no curve");
        assert_eq!((a.grain.amount, a.vignette.amount, a.vignette.midpoint), (20.0, -15.0, 50.0));
        assert!(!a.bw);
        assert_eq!((a.texture, a.sharpen.amount, a.sharpen.radius, a.calibration.red_hue), (15.0, 40.0, 1.0, 10.0));
        assert!(p.unsupported.is_empty(), "everything in this preset is supported now: {:?}", p.unsupported);
    }

    #[test]
    fn a_preset_layers_over_the_current_edit_and_never_touches_the_crop() {
        let p = from_xmp(XMP, "x").unwrap();
        let mut current = Adjustments { temperature: 25.0, exposure: -1.0, ..Default::default() };
        current.mixer[0].saturation = 40.0;
        current.crop.rotate_cw();
        let a = p.apply(&current);
        assert_eq!(a.temperature, 25.0, "settings the preset doesn't mention stay");
        assert_eq!(a.exposure, 0.35);
        assert_eq!(a.mixer[0].saturation, 40.0, "bands the preset doesn't mention stay");
        assert_eq!(a.crop, current.crop);
    }

    #[test]
    fn presets_leave_masks_healing_and_geometry_alone_and_never_carry_them() {
        use crate::adjust::{Geometry, LocalAdjust, Mask, MaskShape, PointColor, Spot};
        let mut current = Adjustments { exposure: 0.3, ..Default::default() };
        current.masks.push(Mask {
            shape: MaskShape::Linear { from: [0.5, 0.1], to: [0.5, 0.4] },
            invert: false,
            adjust: LocalAdjust { exposure: -0.5, ..Default::default() },
        });
        current.spots.push(Spot::default());
        current.geometry = Geometry { vertical: -20.0, ..Default::default() };
        current.point_colors.push(PointColor { hue: 200.0, sat_shift: 30.0, ..Default::default() });
        // A whole-look preset replaces the look but not the work done on this photo.
        let saved = Preset::from_adjustments("Mine", "User", &current);
        let settings = saved.settings.as_object().unwrap();
        for key in ["crop", "geometry", "masks", "spots"] {
            assert!(!settings.contains_key(key), "{key} must not be saved in a preset");
        }
        assert!(settings.contains_key("point_colors"), "a picked colour is part of the look");
        let other = builtin().into_iter().find(|p| p.replace).expect("a whole-look starter preset");
        let after = other.apply(&current);
        assert_eq!((&after.masks, &after.spots, after.geometry), (&current.masks, &current.spots, current.geometry));
        // And applying the saved preset to another photo brings none of them along.
        let fresh = saved.apply(&Adjustments::default());
        assert!(fresh.masks.is_empty() && fresh.spots.is_empty() && fresh.geometry == Geometry::default());
        assert_eq!((fresh.exposure, fresh.point_colors.len()), (0.3, 1));
    }

    #[test]
    fn imports_an_old_lrtemplate_preset() {
        let text = r#"s = {
	id = "0A1B",
	internalName = "Faded Summer",
	title = "Faded Summer",
	type = "Develop",
	value = {
		settings = {
			Blacks2012 = 25,
			Contrast2012 = -10,
			ConvertToGrayscale = false,
			Exposure2012 = 0.2,
			LuminanceAdjustmentOrange = 12,
			ToneCurveName2012 = "Custom",
			ToneCurvePV2012 = {
				0,
				30,
				255,
				255,
			},
			Vibrance = 15,
		},
		uuid = "1234",
	},
	version = 0,
}"#;
        let p = from_lrtemplate(text, "fallback").unwrap();
        assert_eq!(p.name, "Faded Summer");
        let a = p.apply(&Adjustments::default());
        assert_eq!((a.blacks, a.contrast, a.exposure, a.vibrance, a.mixer[1].luminance), (25.0, -10.0, 0.2, 15.0, 12.0));
        assert_eq!(a.curve.rgb, vec![[0.0, 30.0 / 255.0], [1.0, 1.0]]);
    }

    #[test]
    fn lightrooms_own_profiles_map_to_this_apps_profiles() {
        let xmp = r#"<rdf:Description crs:Version="15" crs:Contrast2012="+5">
   <crs:Look><rdf:Description crs:Name="Adobe Vivid" crs:Amount="1"/></crs:Look></rdf:Description>"#;
        let p = from_xmp(xmp, "x").unwrap();
        assert_eq!(p.apply(&Adjustments::default()).profile.name, "vivid");
        assert!(p.unsupported.is_empty(), "{:?}", p.unsupported);
        // A third-party look is reported, and the profile is left alone.
        let custom = xmp.replace("Adobe Vivid", "Kodak-ish 400");
        let p = from_xmp(&custom, "x").unwrap();
        assert!(p.unsupported.iter().any(|u| u.contains("look table")));
        assert_eq!(p.apply(&Adjustments::default()).profile.name, "");
    }

    #[test]
    fn a_whole_look_preset_starts_from_the_photos_own_fresh_state() {
        // An already-processed photo starts with no sharpening; a starter preset keeps it that way.
        let mut fresh = Adjustments::default();
        fresh.sharpen.amount = 0.0;
        let starter = builtin().into_iter().next().unwrap();
        let current = Adjustments { exposure: 2.0, ..fresh.clone() };
        let a = starter.apply_from(&current, &fresh);
        assert_eq!(a.sharpen.amount, 0.0);
        assert_ne!(a.exposure, 2.0);
    }

    #[test]
    fn files_without_usable_settings_are_refused() {
        assert!(from_xmp("<x:xmpmeta></x:xmpmeta>", "x").unwrap_err().contains("no Lightroom settings"));
        let only_profile = r#"<rdf:Description crs:Version="15"><crs:Look><rdf:Description crs:Name="Film 01"/></crs:Look></rdf:Description>"#;
        let err = from_xmp(only_profile, "x").unwrap_err();
        assert!(err.contains("look table"), "{err}");
    }

    #[test]
    fn every_starter_preset_is_valid_and_changes_something() {
        let names: Vec<String> = builtin().iter().map(|p| p.name.clone()).collect();
        assert!(names.len() >= 10);
        for p in builtin() {
            // Applying it must give exactly what its settings say (nothing dropped by a typo).
            let a = p.apply(&Adjustments { exposure: 3.0, ..Default::default() });
            assert!(!a.is_default(), "{} does nothing", p.name);
            let written = serde_json::to_value(&a).unwrap();
            fn check(name: &str, settings: &Value, written: &Value, path: &str) {
                match (settings, written) {
                    (Value::Object(s), Value::Object(w)) => {
                        for (k, v) in s {
                            let inner = w.get(k).unwrap_or_else(|| panic!("{name}: unknown setting {path}/{k}"));
                            check(name, v, inner, &format!("{path}/{k}"));
                        }
                    }
                    (Value::Array(s), Value::Array(w)) => {
                        assert_eq!(s.len(), w.len(), "{name}: {path}");
                        for (i, (sv, wv)) in s.iter().zip(w).enumerate() {
                            check(name, sv, wv, &format!("{path}/{i}"));
                        }
                    }
                    (s, w) => {
                        let same = match (s.as_f64(), w.as_f64()) {
                            (Some(a), Some(b)) => (a - b).abs() < 1e-4,
                            _ => s == w,
                        };
                        assert!(same, "{name}: {path} is {w}, preset says {s}");
                    }
                }
            }
            check(&p.name, &p.settings, &written, "");
            assert_ne!(a.exposure, 3.0, "{} should start from an unedited photo", p.name);
        }
    }

    #[test]
    fn library_round_trip_with_a_lut() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("presets");
        let src = dir.path().join("downloads");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("Moody.xmp"), XMP).unwrap();
        let mut cube = String::from("TITLE \"Warm\"\nLUT_3D_SIZE 2\n");
        for i in 0..8 {
            cube += &format!("{} {} {}\n", i & 1, (i >> 1) & 1, (i >> 2) & 1);
        }
        std::fs::write(src.join("Warm Look.cube"), &cube).unwrap();
        std::fs::write(src.join("notes.txt"), "hello").unwrap();

        let xmp = import_file(&src.join("Moody.xmp"), &lib).unwrap();
        let lut = import_file(&src.join("Warm Look.cube"), &lib).unwrap();
        assert!(import_file(&src.join("notes.txt"), &lib).is_err());
        let mine = save_user_preset(&lib, "My look", &Adjustments { contrast: 33.0, ..Default::default() }).unwrap();
        assert!(save_user_preset(&lib, "  ", &Adjustments::default()).is_err());

        let all = load_library(&lib);
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|p| p.name == "Moody Forest" && p.group == "Free Pack"));
        let applied = lut.apply(&Adjustments::default());
        let used = applied.lut.expect("the LUT preset selects its table");
        assert_eq!(used.amount, 100.0);
        assert_eq!(load_lut(&lib, &used.name).unwrap().size, 2);
        assert!(load_lut(&lib, "..\\secret.cube").is_err());
        assert_eq!(mine.apply(&Adjustments { exposure: 2.0, ..Default::default() }).contrast, 33.0);

        delete(&xmp, &lib).unwrap();
        delete(&lut, &lib).unwrap();
        assert_eq!(load_library(&lib).len(), 1);
        assert!(load_lut(&lib, &used.name).is_err(), "deleting a LUT preset removes its table");
    }
}
