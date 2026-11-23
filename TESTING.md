# Milestone 1: test results and verification gaps

Recorded on 2026-11-22. The test machine had:

* Windows 11 Home 10.0.26300
* Intel Core Ultra 7 155H (16 cores / 22 threads), 15.6 GB RAM
* Rust 1.100.0 (x86_64-pc-windows-msvc)

## Summary

| Area | Status |
|---|---|
| Build and launch on Windows | **Passed.** Release build, launched 5 times, opened files, closed cleanly. |
| Pipeline: decode, adjust, preview, export, edit files | **Passed.** 38 automated tests on generated DNGs and synthetic gain maps / tone curves. |
| Preview vs export agreement | **Passed.** Within the tolerances documented below. |
| Real iPhone ProRAW files (iPhone 15 Pro, 7 files) | **Opens, exports, orientation and source-safety passed.** Default rendering now matches Apple's within 0.3 EV on all 7 (fixed 2026-11-23, see below). |
| Comparing against Apple's own rendering | **Passed numerically** against the rendering embedded in each DNG. Javier's own reference edits are still pending. |
| Clicking through the app by hand | **Pending.** Javier to run the checklist at the end of this file. |

The automated tests use generic DNGs built by the test code. Real-file results are in the next section.

## Real iPhone ProRAW results (2026-11-22)

Seven ProRAW DNGs from an **iPhone 15 Pro** were tested, plus one JPEG and one HEIC from the same phone. Six DNGs are 48 MP (8064×6048) and one is 12 MP (4032×3024).
All are DNG 1.6, lossless JPEG, LinearRaw, with a ProfileGainTableMap, ProfileToneCurve and a sky mask. None use JPEG XL.

| Check | Result |
|---|---|
| Decode | **7/7 opened.** 0.9–1.1 s for 48 MP and 0.25 s for 12 MP, with the file already cached. The first read from disk took 1.1–2.1 s. |
| Orientation | **7/7 correct.** Apple's embedded rendering was matched against ours under all 8 rotations, and the best match equals the file's tag every time (correlation 0.93–0.997). |
| Export | **7/7 at full size, upright, with the sRGB profile embedded.** Encode takes 0.8–1.05 s at 48 MP and 0.37 s at 12 MP. |
| Slider speed | Median 10–11.5 ms and worst 12.6 ms per re-render on the 1920×2560 preview, with the tone curve applied (was 6–8 ms before it). Still under one 60 Hz frame. |
| Memory | About 1.5 GB peak in `pe bench` at 48 MP. About 1.0–1.1 GB in the app with a 48 MP photo open. |
| Originals untouched | All 9 files have identical SHA-256, size and modified time before and after every test. No edit or export files were written next to them. |
| JPEG / HEIC | Refused with a clear message: "this is a JPEG/HEIC photo, not a RAW file…". The earlier message wrongly suggested the file might be damaged. That was fixed after this run. |

### Brightness fix (2026-11-23)

The pipeline now applies the file's **ProfileGainTableMap** (Apple's local tone map) and **ProfileToneCurve**:

* The gain map is applied once when the photo opens, before rotation. Its table is looked up with BaselineExposure already applied, as the DNG spec describes. Looking it up without BaselineExposure left the darkest files 1–2 EV too dark, so the comparison settled it.
* The tone curve is applied on every render after Exposure and white balance. The Exposure slider therefore moves the image "under" the curve, as in Lightroom.
* Neither changes how long a photo takes to open (about 0.9 s at 48 MP). Rendering got about 4 ms slower per slider move.

After the fix, the same comparison shows:

| Photo | Our mean brightness | Apple's | Mid-tone gap | Contrast (ours / Apple) | Mean difference |
|---|---|---|---|---|---|
| IMG_6984 (48 MP) | 94 | 88 | −0.16 EV | 64 / 63 | 7.4 levels |
| IMG_7040 (48 MP) | 81 | 84 | +0.27 EV | 54 / 57 | 7.9 levels |
| IMG_7235 (48 MP) | 91 | 91 | −0.07 EV | 49 / 53 | 5.0 levels |
| IMG_7292 (12 MP) | 73 | 73 | −0.19 EV | 59 / 65 | 6.7 levels |
| IMG_7298 (48 MP) | 110 | 109 | +0.08 EV | 67 / 67 | 4.3 levels |
| IMG_7303 (48 MP) | 111 | 112 | +0.05 EV | 56 / 57 | 1.6 levels |
| IMG_7304 (48 MP) | 108 | 103 | −0.15 EV | 62 / 62 | 5.2 levels |

**Every mid-tone gap is now under the 0.3 EV target**; before the fix it was up to 2.7 EV. Contrast and saturation are close to Apple's.

The small differences that remain:

* Apple clips slightly more highlights (up to 0.5 % of pixels against our 0–0.1 %).
* We don't apply the sky mask or any other semantic mask.
* We use one colour matrix rather than interpolating between two.

The full output is in `Downloads\ProRAW test results\compare-after.txt`, with new side-by-sides in `compare\`. Full-size exports made after the fix are in `exports-after-fix\`. The app screenshot is `app-screenshots\IMG_7303-after-brightness-fix.png`.

### Compared with Apple's rendering (before the fix, 2026-11-22)

The comparison program is `cargo run --release -p pipeline --example compare_apple -- <results folder> <dngs…>`. It compares at 1200 px. Apple's full-size rendering is stored in IFD0 of each DNG.

| Photo | Our mean brightness | Apple's | Mid-tone gap | Contrast after matching exposure (ours / Apple) |
|---|---|---|---|---|
| IMG_6984 (48 MP) | 38 | 88 | +2.06 EV | 45 / 63 |
| IMG_7040 (48 MP) | 34 | 84 | +2.65 EV | 51 / 57 |
| IMG_7235 (48 MP) | 48 | 91 | +1.75 EV | 41 / 53 |
| IMG_7292 (12 MP) | 61 | 73 | +0.37 EV | 54 / 65 |
| IMG_7298 (48 MP) | 47 | 109 | +2.60 EV | 64 / 67 |
| IMG_7303 (48 MP) | 47 | 112 | +2.74 EV | 50 / 57 |
| IMG_7304 (48 MP) | 45 | 103 | +2.60 EV | 53 / 62 |

What this shows:

* **Default renders are too dark**, by about 2 EV on the 48 MP files. Apple brightens with its local gain map, which reaches up to about 9× in places in these files, and with its tone curve. We apply neither. This is a bigger gap than the earlier "a bit flat" estimate.
* **Colour matches well.** Average R/G/B ratios agree with Apple's within a few percent, and our saturation is 10–30 % lower.
* **No highlights are lost.** Our renders clip 0 % of pixels, so all the detail is there. Raising exposure by about +2 EV gets mid-tones close to Apple's, but contrast stays lower than Apple's.

The side-by-side images, exports and screenshots are in `Downloads\ProRAW test results`.

## Automated tests

Run: `cargo test --workspace --release -- --nocapture`. Result: **38 passed, 0 failed.**

What the tests cover:

* **Decoding**
  * Linear DNG pixels come back as the values written. Bayer DNGs are demosaiced.
  * A grey card shot under coloured light comes out neutral. This exercises the as-shot white balance and the colour matrix.
  * BaselineExposure is read from the file.
* **Orientation**
  * All 8 TIFF/EXIF orientation codes are covered.
  * A "rotate 90°" file becomes portrait. The marker in the top-left corner of the stored image ends up top-right in both the preview and the exported JPEG.
* **Camera look**
  * A synthetic gain map brightens by position and by brightness as specified.
  * Truncated, garbage or NaN gain maps are rejected.
  * Tone curves keep greys grey and keep the order of colour channels.
  * Malformed curves are rejected.
  * Exposure is applied before the curve.
* **Adjustments**
  * Sliders at zero change nothing.
  * +1 EV doubles the light.
  * Warmer adds red and cooler adds blue. +Tint adds magenta and −Tint adds green.
  * The white-balance colour-temperature maths matches published reference points.
* **Export**
  * Output is full resolution with the same aspect ratio.
  * The embedded profile is our sRGB ICC profile, read back from the JPEG.
  * Name collisions produce `-1`, `-2`, … and nothing is overwritten.
  * An unwritable destination gives a clear error.
* **Source file**
  * Its bytes and modified time are unchanged after saving an edit and exporting twice.
* **Saved edits**
  * Round trip: save, reopen, same values.
  * A save interrupted by a crash leaves the previous edit intact (simulated by a half-written temp file).
  * Corrupt, foreign or newer-version edit files are rejected with a clear message.
  * Out-of-range values are clamped.
* **Bad input**
  * Missing, empty, random-bytes and fake-header files give clear errors.
  * Files truncated at 5 different points fail cleanly. None of these crash the app.
  * Decoder panics are caught and reported as errors.

### Preview vs export agreement

The method:

1. Render the preview from the shrunk image.
2. Export the full-size JPEG.
3. Shrink the JPEG to preview size, averaging in linear light (the same way the preview is made).
4. Compare the two in sRGB, on a scale of 0–255 levels.

**Documented tolerance:** mean difference ≤ 1.0 level and 99th percentile ≤ 3.0 levels.

| Edit | Mean | 99th pct | Max |
|---|---|---|---|
| none | 0.31 | 1.33 | 1.67 |
| +1.2 EV, temp +35, tint +15 | 0.38 | 1.34 | 9.17 |
| −0.8 EV, temp −60, tint −25 | 0.35 | 1.33 | 1.72 |

The occasional larger maximum comes from pixels that clip to pure white. The preview averages before clipping and the export clips before averaging.
The remaining difference is JPEG compression.

## Windows launch validation (this PC)

I built `photo-editor.exe` in release mode and launched it 5 times from PowerShell. Each time I took a window screenshot, then closed the window normally.
Every run exited cleanly with code 0.

1. **No file:** the empty state appears.
2. **Rotated linear DNG:** shows upright.
3. **Bayer DNG:** opens correctly.
4. **Corrupt DNG:** an error dialog reads "Couldn't open corrupt.dng".
5. **DNG with a saved edit:** the sliders restore to +1.00 EV / +40 / −20 and the status bar says "saved edit restored".

A 48 MP DNG also opens in about 1.0 s, with the app using about 1 GB of memory.

**Not covered:** dragging sliders, clicking Export or Save, and the unsaved-changes dialog were not clicked by a person. The code behind them is the same as the tested pipeline functions.
Launching from Explorer was not tried either, because apps started from this session run inside the Claude app's sandbox.

## Speed on this PC (`pe bench`)

These use synthetic linear DNGs, uncompressed and rotated 90°. Real ProRAW is compressed (lossless JPEG or JPEG XL), so opening it will take longer. Re-run `pe bench` on real files.

| File | Open (decode) | Preview re-render per slider move | Full-size export (render + JPEG) | Peak memory |
|---|---|---|---|---|
| 12 MP (4032×3024) | 226 ms | 4.9 ms median, 5.9 ms worst | 9 ms + 197 ms | – |
| 48 MP (8064×6048) | 866 ms | 5.2 ms median, 5.9 ms worst | 37 ms + 607 ms | ~1.6 GB (export) |

Preview speed doesn't depend on the photo size, because the preview is capped at 2560 px on its long edge.

## Decoder evaluation (rawler 0.8.0)

I chose [rawler](https://github.com/dnglab/dnglab), the decoder behind dnglab, over LibRaw.

* It is pure Rust, so Windows builds need no C/C++ toolchain.
* It decodes DNG linear-raw images (the ProRAW layout) and Bayer images.
* It supports these compressions: none, deflate, lossless JPEG, and **JPEG XL** (through jxl-oxide). Newer iPhones may write JPEG XL.
* It is LGPL-2.1, which is fine for a personal app and for linking from Rust.

`pe probe` reports, for any file, which of these tags are present:

| Used | Present but ignored (may make output differ from Apple Photos / Lightroom) |
|---|---|
| ColorMatrix (D65 one preferred), CalibrationIlluminant, AsShotNeutral / AsShotWhiteXY, BaselineExposure, ActiveArea, DefaultCrop, Orientation, **ProfileGainTableMap**, **ProfileToneCurve** | ProfileGainTableMap2 (DNG 1.7), semantic masks (SemanticName, MaskSubArea), hue/sat maps and look tables, ForwardMatrix, CameraCalibration, AnalogBalance, OpcodeList 1–3 (lens and vignette fixes), BaselineExposureOffset, DefaultUserCrop, third-illuminant tags |

Known effects on ProRAW:

* Colour comes from the single D65 matrix. There is no interpolation between the two calibration lights, so tungsten-lit scenes may be slightly off.
* The file's tone curve gives a gentle highlight roll-off, but there are no user tone controls yet. Lowering exposure brings back detail the sensor recorded.
* The preview assumes an sRGB display. On a wide-gamut monitor without colour management, preview colours will look over-saturated. The exported JPEG is tagged correctly either way.

## Still to verify (needs Javier's files and hands)

1. **Other iPhones:** 15 Pro files pass. Still untested: a newer iPhone that may write JPEG XL, and older 12 MP-only models.
2. **Your reference edits:** compare a portrait and your own edits of these shots against our exports, for skin tone, highlights and shadows. No portrait was in this test set.
3. ~~Speed on real files~~: done (see above).
4. **Hands-on check in the app:**
   1. Drag each slider. The preview should follow smoothly.
   2. Press `\` and the Before/After buttons.
   3. Ctrl+S, close the app, reopen the photo. The edit should come back.
   4. Close with unsaved changes. The app should ask first.
   5. Export twice. You should get two files, `-1` added to the second.
   6. Open the JPEG in Photos. It should be upright and the right size.
5. **DJI Pocket 3 stills:** out of scope for this milestone, but `pe probe` will show whether they decode.

## Next release, in priority order

1. ~~**ProRAW rendering fidelity.**~~ Done on 2026-11-23: every mid-tone gap is now within 0.3 EV of Apple's. Still open: ProfileGainTableMap2 for newer files, and semantic masks.
2. **Basic tone controls.** Contrast, highlights, shadows, whites and blacks, plus a soft highlight roll-off.
3. **Crop and rotate.** Includes straighten and aspect presets. Stored in the edit file.
4. **Folder browsing with thumbnails.** Use the DNG's embedded preview for speed.
5. **Copy/paste edits and app-native presets.** Reuse a look across a shoot.
6. **Batch JPEG export.** Uses the existing export path, with a progress bar and cancel.
7. **Curves and per-colour (HSL) adjustments.**
8. **TIFF and HEIC input.** Only after testing real iPhone samples. HEIC decoding needs a codec choice.
9. **DJI Pocket 3 compatibility check.**
