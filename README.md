# Photo Editor

An offline Windows photo editor for iPhone ProRAW DNGs, written in Rust.
It needs no account, network connection or paid service.

You open a DNG, edit it with Lightroom-style controls, crop it, and export a full-size sRGB JPEG. Edits are saved beside the photo and never change it.

## Using it

| Action | How |
|---|---|
| Open a photo | **Open…** (Ctrl+O), drop a file on the window, or `photo-editor.exe IMG_0001.DNG` |
| Edit | The **Edit** tab. Double-click any slider to reset it. |
| Crop | The **Crop** tab: drag the corners or edges, pick a shape, straighten, rotate, flip. Press Enter when done. |
| Presets | The **Presets** tab: click one to apply it. |
| Undo / redo | Ctrl+Z / Ctrl+Y, or the buttons at the top |
| Compare | **Before / After** buttons, or press `\` |
| Save the edit | **Save edit** (Ctrl+S). It writes `IMG_0001.DNG.edit.json` next to the photo. |
| Export | **Export JPEG** (Ctrl+E). It writes `Exported\IMG_0001.jpg` next to the photo. Export also saves the edit. |

### What the Edit tab has

| Section | Controls |
|---|---|
| Light | Exposure, Contrast, Highlights, Shadows, Whites, Blacks, point Curve (RGB, Red, Green, Blue) |
| Color | Temp, Tint, Vibrance, Saturation, B&W, Color Mixer (hue / saturation / luminance for 8 colours), B&W Mixer, Color Grading (shadows, midtones, highlights, global, blending, balance) |
| Effects | Clarity, Dehaze, Vignette (midpoint, feather, roundness), Grain (size, roughness) |
| Look | A `.cube` LUT with an Amount slider, once one is applied from Presets |

The slider names and ranges follow Lightroom's, but the maths behind them is this app's own. The same number gives a similar result, not an identical one.

**Not in this version yet:** Texture, Sharpening, Noise Reduction, lens corrections, Defringe, Geometry, Point Color, Auto, masking and healing.
**Not planned as-is:** Lightroom's AI features (Denoise, Lens Blur, Adaptive presets) need trained models that this app doesn't have.

### Presets and LUTs

* The app ships with 12 starter presets (9 colour, 3 black & white).
* **Import presets…** reads Lightroom presets (`.xmp`, and the older `.lrtemplate`) and `.cube` LUTs. You can pick several files at once.
* An imported preset applies every setting this app has. Anything it can't apply is listed when you import, and the preset is marked with `*`.
  The most common gaps are colour profiles / look tables, Calibration, Texture and Sharpening. Presets built mainly on those will look different from Lightroom.
* Presets never change the crop. A Lightroom preset's white balance in Kelvin is skipped, so the photo keeps its as-shot balance.
* **Save the current edit** stores your own preset under "Yours".
* Imported and saved presets live in `%APPDATA%\Photo Editor\presets` (the Info tab has a button to open it).

* The original photo is never written to.
* Reopening a photo restores its saved edit.
* Closing the app or opening another photo with unsaved changes asks you first.
* If an export name is already taken, the file is saved as `IMG_0001-1.jpg`, `-2`, and so on. Nothing is ever overwritten.

## Build and run on Windows

You need the Rust toolchain (stable, MSVC) and Visual Studio Build Tools with the "Desktop development with C++" workload.

Run these in **PowerShell**, not Git Bash. Git Bash's `link` command hides the MSVC linker.

```powershell
cd photo-editor
cargo test --workspace --release
cargo build --release --workspace
.\target\release\photo-editor.exe
```

**Installing:** run `powershell -ExecutionPolicy Bypass -File .\install.ps1` from this folder.

* It builds the app and copies it to `%LOCALAPPDATA%\Programs\Photo Editor`.
* It puts a **Photo Editor** shortcut, with the app's icon, on the Desktop.
* Run it again after changing the code to update the installed copy. Close the app first.

The exe is self-contained, so copying it anywhere by hand works too.

If the build fails with `LNK1104` because the folder path is too long, use a shorter build folder:
`$env:CARGO_TARGET_DIR = "$env:USERPROFILE\.cache\pe-target"`. The exe then ends up in that folder's `release` subfolder.

## Command-line helper `pe`

`pe.exe` is built alongside the app. Use it to check real files:

```powershell
.\target\release\pe.exe probe IMG_0001.DNG    # what the decoder reads and what it ignores
.\target\release\pe.exe bench IMG_0001.DNG    # open / preview / export timings on this PC
.\target\release\pe.exe export IMG_0001.DNG --exposure 0.5
.\target\release\pe.exe export IMG_0001.DNG --preset "Soft Film"      # or a .xmp / .lrtemplate file
.\target\release\pe.exe presets                                        # list the starter presets
.\target\release\pe.exe fixtures .\fixtures   # synthetic test DNGs (add --large for 12/48 MP)
```

## How it's put together

```
crates/pipeline   all image work, no UI (decode, colour, render, crop, presets, export, edit files) + `pe` tool
crates/app        the window (egui/eframe); runs every image job on background threads
```

Inside `crates/pipeline/src`:

| File | What it does |
|---|---|
| `adjust.rs` | Every slider value of an edit, and how it is saved |
| `render.rs` | Turns the photo plus an edit into pixels (preview and export) |
| `local.rs` | Small helper maps so Shadows, Highlights, Clarity and Dehaze can look at a pixel's surroundings |
| `geometry.rs` | Crop, straighten, rotate, flip |
| `preset.rs` | Starter presets, Lightroom preset import, the presets folder |
| `lut.rs` | `.cube` look-up tables |
| `look.rs` | The camera's own gain map and tone curve from the DNG |

* **Decoding:** [rawler](https://crates.io/crates/rawler) 0.8 (pure Rust, LGPL-2.1).
  It handles ProRAW-style linear DNGs, Bayer DNGs, and DNGs compressed with lossless JPEG or JPEG XL.
* **Working space:** 32-bit float, scene-linear, sRGB primaries, D65 white.
  Values brighter than sensor white are kept, so lowering exposure can bring them back.
* **One render function** is used for both the preview and the export.
  * The preview is the photo shrunk to 2560, 1600 and 1000 px on its long edge. The app renders the smallest one that fills the window, and one size smaller while you are dragging a slider.
  * The crop is applied when the preview is drawn, so cropping never re-renders the photo.
  * The export renders again from the full-size image, cropped.
* **Export:** baseline JPEG at quality 92, with an embedded sRGB ICC profile.
  Pixels are written upright, so the file needs no orientation tag.

See [TESTING.md](TESTING.md) for test results, measured speed, decoder support and what is still unverified.
