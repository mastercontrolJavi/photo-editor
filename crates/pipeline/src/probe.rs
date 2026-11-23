//! Reports which DNG rendering metadata a file carries and whether this
//! editor uses it. This is how we document ProRAW support honestly: run
//! `pe probe IMG_0001.DNG` on a real file and read the table.

use rawler::formats::tiff::IFD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// The pipeline reads and applies this tag.
    Used,
    /// Present in the file but not applied yet; output may differ from Apple/Adobe.
    Ignored,
}

/// (tag id, name, how we treat it, what it does). Tag ids from the DNG 1.7 spec.
pub const KNOWN_TAGS: &[(u16, &str, Use, &str)] = &[
    (50706, "DNGVersion", Use::Used, "format version"),
    (50721, "ColorMatrix1", Use::Used, "camera colour (D65 one preferred)"),
    (50722, "ColorMatrix2", Use::Used, "camera colour (D65 one preferred)"),
    (50778, "CalibrationIlluminant1", Use::Used, "which light ColorMatrix1 is for"),
    (50779, "CalibrationIlluminant2", Use::Used, "which light ColorMatrix2 is for"),
    (50728, "AsShotNeutral", Use::Used, "as-shot white balance"),
    (50729, "AsShotWhiteXY", Use::Used, "as-shot white balance"),
    (50730, "BaselineExposure", Use::Used, "default brightness offset"),
    (50829, "ActiveArea", Use::Used, "usable sensor area"),
    (50719, "DefaultCropOrigin", Use::Used, "default crop"),
    (50720, "DefaultCropSize", Use::Used, "default crop"),
    (50964, "ForwardMatrix1", Use::Ignored, "preferred camera→XYZ path"),
    (50965, "ForwardMatrix2", Use::Ignored, "preferred camera→XYZ path"),
    (50723, "CameraCalibration1", Use::Ignored, "per-unit colour calibration"),
    (50724, "CameraCalibration2", Use::Ignored, "per-unit colour calibration"),
    (50727, "AnalogBalance", Use::Ignored, "analog gain balance"),
    (50940, "ProfileToneCurve", Use::Used, "camera's default tone curve"),
    (50937, "ProfileHueSatMapDims", Use::Ignored, "camera look (hue/sat map)"),
    (50938, "ProfileHueSatMapData1", Use::Ignored, "camera look (hue/sat map)"),
    (50939, "ProfileHueSatMapData2", Use::Ignored, "camera look (hue/sat map)"),
    (50982, "ProfileLookTableData", Use::Ignored, "camera look table"),
    (51008, "OpcodeList1", Use::Ignored, "raw corrections"),
    (51009, "OpcodeList2", Use::Ignored, "lens/vignette corrections"),
    (51022, "OpcodeList3", Use::Ignored, "lens distortion corrections"),
    (51041, "NoiseProfile", Use::Ignored, "noise model"),
    (51109, "BaselineExposureOffset", Use::Ignored, "profile exposure offset"),
    (51110, "DefaultBlackRender", Use::Ignored, "black rendering hint"),
    (51125, "DefaultUserCrop", Use::Ignored, "suggested crop"),
    (52525, "ProfileGainTableMap", Use::Used, "Apple local tone mapping"),
    (52544, "ProfileGainTableMap2", Use::Ignored, "local tone mapping (DNG 1.7)"),
    (52526, "SemanticName", Use::Ignored, "semantic mask (sky, skin, …)"),
    (52528, "SemanticInstanceID", Use::Ignored, "semantic mask instance"),
    (52536, "MaskSubArea", Use::Ignored, "semantic mask placement"),
    (52543, "RGBTables", Use::Ignored, "colour look tables"),
    (52529, "CalibrationIlluminant3", Use::Ignored, "third colour calibration"),
    (52531, "ColorMatrix3", Use::Ignored, "third colour calibration"),
];

#[derive(Debug, Clone, Default)]
pub struct TagReport {
    /// (tag id, count of IFDs it appears in) for every tag found, sorted.
    pub present: Vec<(u16, usize)>,
    pub dng_version: Option<[u8; 4]>,
    /// Compression code of the main image (1 none, 7 lossless JPEG, 8 deflate, 52546 JPEG XL).
    pub compression: Option<u32>,
    pub baseline_exposure: Option<f32>,
    /// IFDs that look like semantic masks (have a SemanticName).
    pub semantic_masks: usize,
}

impl TagReport {
    pub fn has(&self, tag: u16) -> bool {
        self.present.iter().any(|(t, _)| *t == tag)
    }

    pub fn compression_name(&self) -> &'static str {
        match self.compression {
            Some(1) => "uncompressed",
            Some(7) => "lossless JPEG",
            Some(8) => "deflate",
            Some(34892) => "lossy JPEG",
            Some(52546) => "JPEG XL",
            Some(_) => "other",
            None => "unknown",
        }
    }

    /// Short, plain-language warnings about metadata we don't apply yet.
    pub fn limitations(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.has(52544) && !self.has(52525) {
            out.push("This file's newer local tone map (ProfileGainTableMap2) isn't applied yet, so shadows and highlights may look flatter than in the Photos app.".into());
        }
        if self.has(50982) || self.has(50938) || self.has(50939) {
            out.push("The camera's colour tables (hue/sat maps) aren't applied, so colours may differ slightly from the camera's.".into());
        }
        if self.has(51009) || self.has(51022) {
            out.push("Lens corrections stored in the file (DNG opcodes) aren't applied.".into());
        }
        out
    }
}

fn walk<'a>(ifd: &'a IFD, out: &mut Vec<&'a IFD>, depth: usize) {
    if depth > 8 {
        return;
    }
    out.push(ifd);
    for subs in ifd.sub_ifds().values() {
        for s in subs {
            walk(s, out, depth + 1);
        }
    }
}

/// Find a tag anywhere in the root IFD tree or the main raw IFD tree.
pub fn find_entry<'a>(root: Option<&'a IFD>, raw: Option<&'a IFD>, tag: u16) -> Option<&'a rawler::formats::tiff::Entry> {
    let mut ifds = Vec::new();
    if let Some(r) = raw {
        walk(r, &mut ifds, 0);
    }
    if let Some(r) = root {
        walk(r, &mut ifds, 0);
    }
    ifds.into_iter().find_map(|i| i.entries().get(&tag))
}

/// Collect tag presence from the root IFD tree (and the main raw IFD, which
/// may live outside it).
pub fn scan_tags(root: Option<&IFD>, raw: Option<&IFD>) -> TagReport {
    let mut ifds = Vec::new();
    if let Some(r) = root {
        walk(r, &mut ifds, 0);
    }
    if let Some(r) = raw {
        walk(r, &mut ifds, 0);
    }
    let mut counts = std::collections::BTreeMap::<u16, usize>::new();
    let mut semantic = 0;
    for ifd in &ifds {
        for tag in ifd.entries().keys() {
            *counts.entry(*tag).or_default() += 1;
        }
        if ifd.entries().contains_key(&52526) {
            semantic += 1;
        }
    }
    let find = |tag: u16| ifds.iter().find_map(|i| i.entries().get(&tag));
    TagReport {
        present: counts.into_iter().collect(),
        dng_version: find(50706).map(|e| {
            let v = &e.value;
            [v.force_u32(0) as u8, v.force_u32(1) as u8, v.force_u32(2) as u8, v.force_u32(3) as u8]
        }),
        compression: raw.and_then(|r| r.entries().get(&259)).map(|e| e.value.force_u32(0)),
        baseline_exposure: find(50730).map(|e| e.value.force_f32(0)),
        semantic_masks: semantic,
    }
}

/// Human-readable decoder report for one file, used by `pe probe`.
pub fn report(path: &std::path::Path) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "File: {}", path.display());
    let started = std::time::Instant::now();
    match crate::decode::decode_file(path) {
        Err(e) => {
            let _ = writeln!(s, "Decode: FAILED — {e}");
        }
        Ok(d) => {
            let i = &d.info;
            let t = &i.tags;
            let _ = writeln!(s, "Decode: OK in {} ms", started.elapsed().as_millis());
            let _ = writeln!(s, "Camera: {} {}", i.make, i.model);
            let _ = writeln!(s, "Kind: {:?}", i.kind);
            let _ = writeln!(s, "Output size (upright): {} × {}", i.width, i.height);
            let _ = writeln!(s, "Orientation code applied: {}", i.orientation);
            let _ = writeln!(s, "DNG version: {:?}", t.dng_version);
            let _ = writeln!(s, "Compression: {} ({:?})", t.compression_name(), t.compression);
            let _ = writeln!(s, "BaselineExposure: {:+.2} EV", i.baseline_exposure);
            let _ = writeln!(s, "Semantic mask IFDs: {}", t.semantic_masks);
            let _ = writeln!(s, "\nRendering metadata:");
            for (tag, name, use_, what) in KNOWN_TAGS {
                if t.has(*tag) {
                    let _ = writeln!(s, "  {:<24} {:>5}  {:<8} {}", name, tag, format!("{use_:?}"), what);
                }
            }
            let unknown_new: Vec<u16> = t
                .present
                .iter()
                .map(|(tag, _)| *tag)
                .filter(|tag| *tag >= 52500 && !KNOWN_TAGS.iter().any(|k| k.0 == *tag))
                .collect();
            if !unknown_new.is_empty() {
                let _ = writeln!(s, "  Other DNG 1.6+ tags (not used): {unknown_new:?}");
            }
            if !i.notes.is_empty() {
                let _ = writeln!(s, "\nLimitations for this file:");
                for n in &i.notes {
                    let _ = writeln!(s, "  - {n}");
                }
            }
        }
    }
    s
}
