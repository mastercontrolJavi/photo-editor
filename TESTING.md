# Test results and verification gaps

First recorded on 2026-11-22; the Lightroom-style controls were added on 2026-11-23, and Geometry, Point Color, masks and healing on 2026-11-24. The test machine had:

* Windows 11 Home 10.0.26300
* Intel Core Ultra 7 155H (16 cores / 22 threads), 15.6 GB RAM
* Rust 1.100.0 (x86_64-pc-windows-msvc)

## Summary

| Area | Status |
|---|---|
| Build and launch on Windows | **Passed.** Release build, launched 5 times, opened files, closed cleanly. |
| Pipeline: decode, adjust, preview, export, edit files | **Passed.** 146 automated tests (see the sections below for what the new ones cover). |
| Geometry, Point Color, masks, healing, zoom controls (added 2026-11-24, fourth batch) | **Built and tested by program**, including pretend mouse input. Checked by eye on contact sheets and screenshots of real photos. Not yet used by a person. Pinch-to-zoom cannot be tested without a trackpad. |
| JPEG, PNG, HEIC and profiles (added 2026-11-23, third batch) | **Built and tested by program.** Your real iPhone JPEG and HEIC open correctly. Not yet used by a person. |
| Detail, optics, zoom, Auto (added 2026-11-23, second batch) | **Built and tested by program.** Checked by eye at 100 % on real photos. Not yet used by a person. |
| Tone, colour, effects, crop, presets (added 2026-11-23) | **Built and tested by program.** Checked by eye on contact sheets of real photos. Not yet used by a person. |
| Preview vs export agreement | **Passed.** Within the tolerances documented below. |
| Real iPhone ProRAW files (iPhone 15 Pro, 7 files) | **Opens, exports, orientation and source-safety passed.** Default rendering now matches Apple's within 0.3 EV on all 7 (fixed 2026-11-23, see below). |
| Comparing against Apple's own rendering | **Passed numerically** against the rendering embedded in each DNG. The owner's own reference edits are still pending. |
| Clicking through the app by hand | **Pending.** The owner runs the checklist at the end of this file. |

The automated tests use generic DNGs built by the test code. Real-file results are in the sections below.

## Geometry, Point Color, masks, healing and viewing (2026-11-24, fourth batch)

Added after feedback from a working photo and video editor who tried version 0.2 on a Mac:

| What was asked for | What was done |
|---|---|
| More range in Highlights and Whites | Both now squeeze or stretch the bright tones instead of sliding them by a fixed amount (see below). |
| A low / mid / high range control | Already there as Color Grading (the tester found it). Nothing changed. |
| An easier way to see before and after | The toggle moved from the top bar to the bottom right of the photo, labelled **Before / After**. The `\` key still works. |
| Masks, for gradients | A Mask tab with linear gradient, radial gradient and brush masks. |
| Zoom that is easy to adjust; pinch on a Mac | A strip under the photo with Fit, 100%, −, +, a slider and the current percentage. Pinch now zooms. Two fingers on a trackpad slide the picture. |
| Picking a colour from the photo | Point Color, in the Color section. |

Also added: Geometry (perspective sliders) and a Heal tab.

### How Highlights and Whites changed

Worked out from the formulas for a plain grey area, in stops (one stop is a halving or doubling of light), at the slider's far end:

| Tone | Highlights −100 before | Highlights −100 now | Whites −100 before | Whites −100 now |
|---|---|---|---|---|
| Middle grey | −0.13 | −0.11 | −0.18 | 0 |
| 1.5 stops above middle grey | −1.28 | −1.28 | −0.73 | −0.60 |
| White (2.5 stops above) | −1.60 | −2.10 | −0.90 | −1.38 |
| 1 stop brighter than white (RAW only) | −1.60 | −2.95 | −0.90 | −2.18 |

So the mid-tones behave as before and the top end moves about a third further (Highlights) or half as far again (Whites), and further still for detail brighter than white in a RAW file. The positive direction changed the same way.

**This changes the look of edits saved with version 0.2 that used Highlights or Whites**, and of any preset that uses them: bright areas come out somewhat darker (or brighter) than before. The test "brighter in never comes out darker" still passes with every tone slider at its extreme.

### What was verified

* **By program (31 new tests, 146 in all)**
  * Geometry: the sliders at rest do nothing; there-and-back mapping is exact; Vertical −60 widens the top and makes leaning verticals more upright; Rotate, Scale, Aspect and the offsets move the picture the way their labels say; empty corners are found, and Constrain zooms in just far enough to remove them (the nearest corner ends within 3 px of the photo's edge); the same settings give the same picture at any size; preview path and export path give identical pixels.
  * Masks: a linear gradient is full on one side, half-way in the middle, nothing on the other, and only ever falls; a radial gradient is a true circle on a wide photo, follows its angle, and inverts; brush strokes paint and erase; continuing a stroke point by point gives exactly the mask that painting it in one go gives.
  * Mask sliders: Exposure, Temp/Tint and the tone sliders give exactly the same pixels as the main sliders when the mask covers everything; a mask with no slider moved changes nothing; a mask stays on the same part of the photo when the picture is warped or cropped.
  * Point Color: picking the orange stripe of a test image and setting Saturation −100 turns it grey and leaves the blue and grey stripes bit-for-bit unchanged; hue and luminance shifts likewise; a grey pick is matched by brightness.
  * Healing: on a textured wall that gets lighter towards one side, Heal removes a dark blemish and leaves the patch within 8 % of the clean wall, where Clone from the same source is more than 8 % too light; pixels outside every spot are untouched; opacity and feather blend as expected; the suggested source avoids a dark line next to the spot and stays inside the photo.
  * In the app, driven with pretend mouse input: dragging out a linear gradient, moving each of its three handles; dragging out a radial gradient, resizing and moving it; painting a stroke and a single erasing dab; adding a healing spot with a click, moving its source, moving the spot, adding a second, and clicking outside the photo; screen positions map to photo positions and back through a turned, flipped, straightened, cropped and warped picture.
  * Edit files: masks of all three kinds, spots, picked colours and Geometry survive saving and loading. Presets never carry or remove a photo's masks, spots, crop or Geometry.
  * Preview against export with three masks, a picked colour, two spots and a constrained warp at once: mean 0.46 levels, 99th percentile 1.9 (limits 1.0 and 3.0).
* **By eye, on real photos**
  * `batch4\local-7235.jpg` and `local-7235-leaves.jpg` (IMG_7235, 48 MP ProRAW): Highlights −100, Whites ±100, a linear mask darkening the top, one warming it, a radial mask and its inverse, a brush mask with an erased strip, Point Color on the yellow leaves (to grey, to green, darker), and four Geometry settings. All behave as labelled. With the leaves picked, the trunks, sky and buildings are left alone.
  * `batch4\heal-7235.jpg`: 100 % crops of a small and a large healing spot on fallen leaves. No visible seam.
  * `batch4\local-0637-jpeg.jpg`: the same sheet on an iPhone JPEG.
  * App screenshots of the Mask tab (red overlay, handles), the Heal tab (fitted and at 100 %), and the Point Color panel, on a copy of IMG_7235 with an edit file using every new feature. A full-size export of that edit took 3.7 s.
* **A bug found and fixed while testing:** sharpening drew a line along the edge of a warped picture; and selecting a healing spot marked the edit as changed.
* The owner's 9 originals were checked unchanged (SHA-256) after this batch, and the 2 edit files beside them still carry their dates from 2026-11-23. All testing of the new tools used a copy elsewhere.

### Speed (IMG_7303, 48 MP, largest preview 1920 × 2560)

| Edit | Per preview |
|---|---|
| Nothing | 19 ms |
| One linear mask (exposure) | 32 ms |
| One radial mask with Highlights and Shadows | 71 ms |
| One brush mask | 29 ms |
| Point Color, one colour | 40 ms |
| Geometry | 38 ms |
| 20 healing spots | 36 ms |
| Everything at once on IMG_7235 (3 masks, warp, 2 spots, a picked colour) | 118–128 ms in the app |

While a slider or handle is dragged the app renders one size smaller, which takes well under half of these.

### Not verified

* **No person has used any of this yet.** Screenshots show the tools drawn correctly; nobody has painted a mask or healed a real blemish by hand.
* **Pinch-to-zoom and two-finger panning** were written for the Mac trackpad and cannot be tested on this PC or on GitHub's Mac machines. They rely on the window library reporting pinches and trackpad scrolling the documented way.
* **The slider strengths were not compared with Lightroom.** The new Highlights and Whites are stronger than before; whether they now match what the tester expects is for him to say.
* **Geometry's direction and strength** follow Lightroom's labels (negative Vertical for buildings leaning back) but were not compared side by side with it.
* **Healing** was checked on leaf litter and on synthetic walls, not on skin or a sky with a real dust spot.
* **Point Color** matching was checked on one ProRAW and one JPEG. Lightroom also has separate hue, saturation and luminance range sliders; here one Range slider drives all three.

## JPEG, PNG, HEIC and profiles (2026-11-23, third batch)

Added: opening JPEG, PNG and HEIC with the type identified from the file's contents; a Profile menu with an Amount slider; starting defaults and notes that depend on the file type.

### What was verified

| Check | Result |
|---|---|
| Automated tests | **115 passed, 0 failed** (95 pipeline unit, 10 end-to-end, 10 app). |
| PNG | Tested: 8-bit, 16-bit and see-through PNGs open; colours arrive exactly; an unedited PNG renders back to the same pixel values. |
| JPEG | Tested: a JPEG made by the test opens with the right size and colours. |
| Whole loop for a processed photo | Tested end to end: a PNG saved under a `.dng` name is still recognised as PNG, starts with Sharpening 0, exports unedited to within JPEG's own error, saves and reloads an edit with a profile, and the original file is unchanged. |
| Your real iPhone JPEG (`IMG_0637.JPEG`, 48 MP) | Opens in about 1.0 s, upright, recognised as Display P3 and converted. |
| Your real iPhone HEIC (`IMG_6975.HEIC`, 12 MP) | Opens in about 0.7 s through Windows' HEIF decoder, upright, recognised as Display P3 and converted. |
| Profiles | Tested: Vivid is more saturated than Color, which is more than the original; Flat is less; Monochrome is grey; greys, black and white are unchanged by every colour profile; Amount scales the effect and 0 is exactly the original; the sliders still work on top. |
| Preset import | Tested: a preset naming "Adobe Vivid" selects Vivid; a third-party look is still reported as unsupported. |
| Starting defaults | Tested: a starter preset applied to a processed photo keeps Sharpening at 0. |
| Damaged or unsupported files | Tested: a damaged JPEG, PNG or HEIC names its type in the message; video, GIF, WebP and AVIF are refused with a list of what does open. |
| By eye | Contact sheets of all 8 profiles on the HEIC and on IMG_7235 show sensible, distinct looks. Screenshots of both real files open in the app are in `Downloads\ProRAW test results\app-screenshots`. |

### Not verified

* **HEIC on a PC without Microsoft's HEIF/HEVC extensions.** This PC has them. On one that doesn't, the app shows a message naming the extensions; that message was only tested with a damaged file.
* **Colour accuracy of the Display P3 conversion** was checked by eye and by the conversion maths, not against a colour-managed reference.
* **JPEGs with Adobe RGB or ProPhoto profiles** are treated as sRGB (with a note in the Info tab), so their colours will be slightly off.
* **JPEGs from cameras that store the picture sideways** rely on the rotation tag being read. The two real files were stored upright, so that path was only covered by the decoder library's own handling.
* **Nobody has used the Profile menu by hand yet.**

## Detail, optics and zoom (2026-11-23, second batch)

Added: Texture, Sharpening (radius, detail, masking), Noise Reduction (luminance, colour), Defringe, Remove chromatic aberration, Distortion, Lens vignetting, the parametric curve, Calibration (through presets), Auto, and zoom to 100 % with panning.

### What was verified

| Check | Result |
|---|---|
| Automated tests | 103 passed at the time (84 pipeline unit, 9 end-to-end, 10 app). Now 115 with the third batch. |
| Sharpening | Tested: steepens an edge, leaves flat areas and grey balance alone, and Masking keeps it off fine grain while still sharpening edges. |
| Noise reduction | Tested: luminance NR halves noise in flat areas and keeps a hard edge; colour NR removes colour speckle without changing brightness or the area's real colour. |
| Texture, Defringe | Tested: Texture changes medium-sized detail in both directions; Defringe removes purple only beside strong edges and leaves a purple object elsewhere alone. |
| Chromatic aberration | Tested on a synthetic image with a known red/blue misalignment: the measurement finds its size and direction within 0.0002, the correction lines the colours up, and a clean image measures as exactly zero. |
| Distortion, lens vignetting | Tested: the centre never moves, no pixel is ever read from outside the photo at any setting, and vignetting changes only the corners. |
| Zoomed view | Tested: a zoomed-in region rendered from the full photo matches the same part of the full export to within 1 level. |
| Parametric curve, Auto, Calibration | Tested: each parametric slider bends only its region and the curve never dips; Auto brightens dark photos, darkens bright ones, and leaves a well-exposed one and all non-Light settings alone. |
| Preset import | The hand-written Lightroom preset now imports with nothing reported as unsupported (Texture, Sharpening and Calibration map across). |
| Default rendering | `compare_apple` gives the same numbers as before, so default sharpening does not change brightness, contrast or colour. |
| By eye at 100 % | A contact sheet of 12 detail settings on IMG_7292 shows the expected effect for each and no artefacts at default settings. Texture ±100 gave halos and was toned down. |

### Fine detail compared with Apple's rendering

`crop_compare` measures fine detail (average difference between neighbouring pixels) in a 100 % crop.

| Photo | No sharpening | Sharpening 40 (default) | Sharpening 80 | Apple's |
|---|---|---|---|---|
| IMG_7292 (12 MP) | 6.2 | 8.6 | 10.4 | 9.9 |
| IMG_7235 (48 MP) | 1.4 | 1.5 | 1.7 | 2.4 |
| IMG_7303 (48 MP) | 0.6 | 0.7 | 0.8 | 1.0 |

* On the 12 MP photo, sharpening between 40 and 80 matches Apple's level of detail.
* On the 48 MP photos Apple's rendering is clearly crisper than ours even at 80. The 48 MP RAW data is softer per pixel, and Apple sharpens it harder. Raising Sharpening and Radius closes some of the gap.

### What the lens corrections do for these photos

None of the 7 ProRAW files carries lens-correction data (`pe probe` shows no OpcodeList tags). The phone corrects them before saving. The chromatic-aberration measurement is exactly zero on all 7 (shown by `pe probe`), which is the expected result for corrected files. So on these photos "Remove chromatic aberration" does nothing, and the app says so.

### Speed (IMG_7303, 48 MP)

| | Time |
|---|---|
| Open | about 0.9 s (60–100 ms more than before, for the chromatic-aberration measurement) |
| Preview, light edit, 2560 px | 15 ms |
| Preview, heavy edit, 2560 px | about 120 ms |
| Zoomed to 100 %, default sharpening, 1680 × 1239 visible | 34 ms |
| Full-size export render, heavy edit plus sharpening | 1.4 s, plus 0.85 s to encode |

### Not verified

* **Nobody has used these controls by hand yet.** Zooming, panning and the new sliders were checked by screenshot and by program.
* **The filters are not tuned against Lightroom.** Sharpening 40 here is not guaranteed to look like Sharpening 40 there.
* **Chromatic aberration removal has only been proven on a synthetic image.** No uncorrected real photo was available.
* **Noise reduction has not been tried on a genuinely noisy photo** (a dark indoor or night shot).

## Lightroom-style controls (2026-11-23)

Added: Contrast, Highlights, Shadows, Whites, Blacks, point curves, Vibrance, Saturation, B&W, Color Mixer, B&W Mixer, Color Grading, Clarity, Dehaze, Vignette, Grain, `.cube` LUTs, crop / straighten / rotate / flip, presets (12 starter presets, Lightroom `.xmp` / `.lrtemplate` import, saving your own), and undo / redo.

**These sliders are modelled on Lightroom's, not copied from it.** Adobe's maths is not public. Names, ranges and direction match; the exact result for a given number does not.

### What was verified

| Check | Result |
|---|---|
| Automated tests | 83 passed at the time (64 pipeline unit, 9 end-to-end, 10 app). Now 103 with the second batch. |
| Each slider does what its name says | Tested: Contrast pivots on middle grey; Shadows / Highlights / Whites / Blacks act on their own range and never reverse the order of tones; Highlights recovers detail above sensor white; Saturation −100 and B&W give pure grey; the Color Mixer changes only its own colour; curves pass through their points without overshoot; Color Grading tints only the chosen range; Vignette darkens corners and not the centre; Grain keeps the average brightness. |
| Clarity and Dehaze | Tested on synthetic stripes: both add local contrast when positive and remove it when negative. The edge-aware helper map keeps a 5-stop edge without a halo. |
| Crop geometry | Tested: quarter turns and flips compose correctly, a crop follows the picture through a turn, straightening keeps the crop inside the photo, aspect presets give the largest crop of that shape. |
| Crop in the export | Tested: the exported JPEG has the cropped size, pixels are exact for turns and plain crops, and the vignette sits in the same place in preview and export. |
| Crop handles | Tested with simulated mouse drags: resizing from a corner, moving, ratio lock, stopping at the photo's edge, ignoring drags outside the rectangle. |
| Curve editor | Tested with simulated mouse input: add a point, move it, remove it with a double-click; end points only slide along their edge. This found and fixed a bug where a quick grab added a second point. |
| Preset import | Tested on a hand-written `.xmp` and `.lrtemplate` in Lightroom's format: every supported setting maps across, unsupported ones are reported, presets layer over the current edit and never change the crop. |
| LUTs | Tested: `.cube` parsing, blending by Amount, clear errors for bad files, and an export that names a missing LUT fails with a message instead of silently ignoring it. |
| Old edit files | Version 1 edit files (exposure, temperature, tint only) still load. |
| Default rendering unchanged | `compare_apple` gives the same numbers as before on IMG_7303 and IMG_7040, so the new code does not alter a photo with no edits. |
| By eye | Contact sheets of all 20 slider extremes and all 12 starter presets on IMG_7235 show no halos, banding or colour breaks at screen size. Screenshots of the Edit, Crop and Presets tabs are in `Downloads\ProRAW test results\app-screenshots`. |

### Preview vs export with many sliders

The same agreement test as below, with Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Vibrance, Saturation, Clarity, Dehaze, a curve, the Color Mixer, Color Grading and a vignette all active, stays within the documented tolerance (mean ≤ 1.0 level, 99th percentile ≤ 3.0 levels).

### Speed with the new sliders (IMG_7303, 48 MP)

| Preview size | No edit | One tone slider | Heavy edit (about 10 sliders, curve, grading, grain) |
|---|---|---|---|
| 2560 px (zoomed-in crop, or a very large window) | 15 ms | 47 ms | about 125 ms |
| 1600 px (normal window, final picture) | 6 ms | 19 ms | about 50 ms |
| 1000 px (while dragging a slider) | 2 ms | 7 ms | about 18 ms |

* While a slider is being dragged the app renders one size smaller, then the full-size preview when the button is released.
* The heavy-edit figures for 1600 and 1000 px are scaled from the measured 2560 px figure.
* Full-size export of a heavy edit: about 1.1 s to render plus 0.85 s to encode at 48 MP.
* This is slower than milestone 1's 6–11 ms. It is usable, but a heavy edit on a cropped photo will feel less than instant. Speeding up the tone step is on the list below.

### Not verified

* **Nobody has used the new controls by hand yet.** Slider dragging, the crop handles, preset clicks and the import dialog were exercised by program, not by a person.
* **No real downloaded preset pack has been imported.** The importer was tested against Lightroom's file format as written by hand. Try a pack and check the import message.
* **No real `.cube` file from the web has been tried.**
* **How close each slider is to Lightroom** has not been measured. There is no Lightroom on this PC to compare with.

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

Run: `cargo test --workspace --release -- --nocapture`. Result: **146 passed, 0 failed.**

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

## Still to verify (needs the owner's files and hands)

1. **Other iPhones:** 15 Pro files pass. Still untested: a newer iPhone that may write JPEG XL, and older 12 MP-only models.
2. **Your reference edits:** compare a portrait and your own edits of these shots against our exports, for skin tone, highlights and shadows. No portrait was in this test set.
3. ~~Speed on real files~~: done (see above).
4. **Hands-on check in the app:**
   0. New on 2026-11-24:
      * Pull Highlights and Whites to −100 on a photo with a bright sky. Is the range enough now?
      * Mask tab: drag out a Linear gradient over a sky and lower its Exposure. Add a Radial one, tick Invert. Paint with the Brush, then Erase part of it.
      * Heal tab: zoom in, click a blemish, drag the dashed circle to another patch. Press Delete.
      * Edit → Color → Point Color: pick a colour, move Hue shift.
      * Edit → Geometry: Vertical −30 on a building, then tick Constrain crop.
      * Under the photo: the zoom slider, − and +, and Before / After. On a Mac: pinch to zoom, two fingers to slide.
   0. From 2026-11-23: try the Edit sliders, the curve, the Crop tab (drag corners, straighten, rotate), click a few starter presets, and import a downloaded Lightroom preset. Zoom to 100 % (press Z), drag to pan, and try Sharpening and Noise Reduction there. Try Auto.
   1. Drag each slider. The preview should follow smoothly.
   2. Press `\` and the Before/After buttons.
   3. Ctrl+S, close the app, reopen the photo. The edit should come back.
   4. Close with unsaved changes. The app should ask first.
   5. Export twice. You should get two files, `-1` added to the second.
   6. Open the JPEG in Photos. It should be upright and the right size.
5. **DJI Pocket 3 stills:** out of scope for this milestone, but `pe probe` will show whether they decode.

## Next release, in priority order

1. ~~**ProRAW rendering fidelity.**~~ Done on 2026-11-23: every mid-tone gap is now within 0.3 EV of Apple's. Still open: ProfileGainTableMap2 for newer files, and semantic masks.
2. ~~**Basic tone controls.**~~ Done on 2026-11-23, with curves, Color Mixer, Color Grading, Clarity, Dehaze, Vignette and Grain.
3. ~~**Crop and rotate.**~~ Done on 2026-11-23.
4. ~~**Detail and optics.**~~ Done on 2026-11-23: Texture, Sharpening, Noise Reduction, Defringe, chromatic aberration, Distortion, lens vignetting. Still open: lens-correction data stored in non-iPhone DNGs (no sample file to test with).
5. ~~**Closer preset matching.**~~ Calibration and the parametric curve are done. Still open: Lightroom colour profiles / look tables embedded in presets.
6. **Faster tone step.** Replace the per-pixel logarithms with look-up tables, aiming for a heavy edit under 50 ms at 2560 px.
7. ~~**Geometry (perspective), Point Color, masks, healing.**~~ Done on 2026-11-24. Still open: automatic Upright; masks that select a subject, sky, colour or brightness range; healing along a painted line; separate hue / saturation / luminance ranges for Point Color.
8. **Folder browsing with thumbnails.** Use the DNG's embedded preview for speed.
9. **Copy/paste edits and batch JPEG export.**
10. ~~**HEIC input.**~~ Done on 2026-11-23, with JPEG and PNG, using Windows' own decoder. Still open: TIFF, and converting Adobe RGB / ProPhoto colour profiles.
11. **DJI Pocket 3 compatibility check.**

Not planned as-is: Lightroom's AI Denoise, Lens Blur and Adaptive presets. They rely on trained models. The DNG's own sky mask could support a simple "sky" adjustment later.
