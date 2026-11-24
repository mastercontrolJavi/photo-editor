# Photo Editor

An offline photo editor for Mac and Windows, written in Rust. It was built for iPhone ProRAW and also opens JPEG, PNG and HEIC.
It needs no account, network connection or paid service.

You open a photo, edit it with Lightroom-style controls, crop it, and export a full-size sRGB JPEG. Edits are saved beside the photo and never change it.

## Download and install

Get the newest version from the [Releases page](../../releases/latest).

| Your computer | File to download |
|---|---|
| Mac (Apple Silicon or Intel, macOS 11 or newer) | `PhotoEditor-mac.dmg` |
| Windows 10 or 11 | `PhotoEditor-windows.zip` |

### Mac

1. Open `PhotoEditor-mac.dmg` and drag **Photo Editor** onto **Applications**.
2. Open **Photo Editor** from Applications. macOS says it can't check the app for malicious software. Click **Done**.
3. Open **System Settings → Privacy & Security**, scroll down to the message about Photo Editor, and click **Open Anyway**. Confirm with your password or Touch ID.

You do steps 2 and 3 only the first time. The warning appears because the app isn't signed with a paid Apple developer certificate. On macOS 14 or older you can instead right-click the app and choose **Open**.

If you prefer the Terminal, this does the same as step 3:

```bash
xattr -dr com.apple.quarantine "/Applications/Photo Editor.app"
```

### Windows

1. Unzip `PhotoEditor-windows.zip` and double-click **Photo Editor.exe**. You can move it anywhere and pin it to Start or the taskbar.
2. If Windows shows "Windows protected your PC", click **More info**, then **Run anyway**. This appears because the app isn't signed with a paid certificate.

### Then

Click **Open…** or drop a photo onto the window. The sections below explain the controls.

**About the Mac version:** it is built and tested automatically on GitHub's Mac machines, but it hasn't yet been used by a person on a real Mac. If something looks wrong, please say so.

### File types

The app works out the type from the file's contents, not its name.

| Type | What you get |
|---|---|
| **RAW** (DNG, including iPhone ProRAW; other RAW formats the decoder knows) | The most room to edit: highlights and shadows the camera recorded can be brought back. Starts with Sharpening 40. |
| **JPEG, PNG** | Every slider works. The picture is already processed and usually 8-bit, so blown highlights can't be recovered and big changes show banding sooner. Starts with Sharpening 0. |
| **HEIC** (iPhone's default photo format) | Same as JPEG. The app uses the decoder each system already has. On a Mac that always works. On Windows it needs the "HEIF Image Extensions" and "HEVC Video Extensions" from the Microsoft Store if the PC doesn't have them (many do). |

iPhone JPEG and HEIC photos store their colours as Display P3; the app converts them to sRGB for editing. Other colour profiles (Adobe RGB, ProPhoto) are not converted yet.

The Edit tab shows the same sections for every type. What changes is the **Profile** menu, the starting defaults, and a short note saying what kind of file is open.

### Profiles

The Profile menu sets the photo's starting look, underneath the sliders, with an Amount slider (0–200).

* The first entry is the camera's own rendering: **Apple ProRAW** for iPhone ProRAW, **Camera** for other RAW files, **As shot** for JPEG, PNG and HEIC.
* **Color, Vivid, Landscape, Portrait, Flat, Monochrome, Monochrome Rich** are this app's own looks. They play the same role as Lightroom's Adobe Color, Adobe Vivid and so on, but they are not Adobe's and won't match them exactly.
* An imported preset that names one of Lightroom's Adobe profiles selects the matching one here.

## Using it

| Action | How |
|---|---|
| Open a photo | **Open…** (Ctrl+O), drop a file on the window, or `photo-editor.exe IMG_0001.DNG`. RAW, JPEG, PNG and HEIC all open the same way. |
| Edit | The **Edit** tab. Double-click any slider to reset it. |
| Crop | The **Crop** tab: drag the corners or edges, pick a shape, straighten, rotate, flip. Press Enter when done. |
| Presets | The **Presets** tab: click one to apply it. |
| Zoom | **100%** / **Fit** at the top, press `Z`, double-click the photo, or use the mouse wheel. Drag to move around when zoomed in. |
| Undo / redo | Ctrl+Z / Ctrl+Y, or the buttons at the top |
| Compare | **Before / After** buttons, or press `\` |
| Save the edit | **Save edit** (Ctrl+S). It writes `IMG_0001.DNG.edit.json` next to the photo. |
| Export | **Export JPEG** (Ctrl+E). It writes `Exported\IMG_0001.jpg` next to the photo. Export also saves the edit. |

On a Mac, use **Cmd** wherever this page says Ctrl.

### What the Edit tab has

| Section | Controls |
|---|---|
| Top row | Auto (sets the Light sliders from the photo's brightness), B&W, Reset all, Profile and its Amount |
| Light | Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Curve (point curves for RGB, Red, Green, Blue, plus the four parametric sliders) |
| Color | Temp, Tint, Vibrance, Saturation, B&W, Color Mixer (hue / saturation / luminance for 8 colours), B&W Mixer, Color Grading (shadows, midtones, highlights, global, blending, balance) |
| Effects | Texture, Clarity, Dehaze, Vignette (midpoint, feather, roundness), Grain (size, roughness) |
| Detail | Sharpening (radius, detail, masking), Noise Reduction (luminance, colour) |
| Optics | Remove chromatic aberration, Distortion, Lens vignetting, Defringe (purple, green) |
| Look | A `.cube` LUT with an Amount slider, once one is applied from Presets |

The slider names and ranges follow Lightroom's, but the maths behind them is this app's own. The same number gives a similar result, not an identical one.

* **Sharpening starts at 40 for RAW**, as in Lightroom, because RAW data is slightly soft until sharpened. It starts at 0 for JPEG, PNG and HEIC, which the camera already sharpened.
* **Sharpening and noise reduction only show properly at 100%.** In the fit-to-window view each screen pixel covers several photo pixels, so they fade out, exactly as they do when the exported photo is viewed small.
* **Lens corrections:** iPhone ProRAW files are already corrected by the phone, so there is nothing to switch on. The Optics sliders are for touch-ups. "Remove chromatic aberration" measures the colour misalignment in each photo and does nothing if there is none.
* **Auto** is a simple rule based on the photo's brightness. It is not Lightroom's Auto, which uses a trained model.
* **Calibration** (the camera's colour primaries) has no sliders, but imported presets that use it are applied.

**Not in this version yet:** Geometry (perspective), Point Color, masking and healing.
**Not planned as-is:** Lightroom's AI features (Denoise, Lens Blur, Adaptive presets) need trained models that this app doesn't have.

### Presets and LUTs

* The app ships with 12 starter presets (9 colour, 3 black & white).
* **Import presets…** reads Lightroom presets (`.xmp`, and the older `.lrtemplate`) and `.cube` LUTs. You can pick several files at once.
* An imported preset applies every setting this app has. Anything it can't apply is listed when you import, and the preset is marked with `*`.
  The most common gap is a colour profile or look table embedded in the preset. Presets built mainly on one will look different from Lightroom.
* Presets never change the crop. A Lightroom preset's white balance in Kelvin is skipped, so the photo keeps its as-shot balance.
* **Save the current edit** stores your own preset under "Yours".
* Imported and saved presets live in `%APPDATA%\Photo Editor\presets` on Windows and `~/Library/Application Support/Photo Editor/presets` on a Mac (the Info tab has a button to open it).

* The original photo is never written to.
* Reopening a photo restores its saved edit.
* Closing the app or opening another photo with unsaved changes asks you first.
* If an export name is already taken, the file is saved as `IMG_0001-1.jpg`, `-2`, and so on. Nothing is ever overwritten.

## Building it yourself

Most people should use the downloads above. These notes are for changing the code.

**On a Mac:** install Rust from [rustup.rs](https://rustup.rs), then run `cargo run --release -p photo-editor`. To make the `.dmg`, run `bash packaging/macos/bundle.sh`.

**Publishing a new version:** push a tag such as `v0.2.1`. GitHub then builds the Mac and Windows downloads and attaches them to a release (see `.github/workflows/release.yml`).

### On Windows

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
.\target\release\pe.exe export IMG_0002.HEIC --profile vivid            # JPEG, PNG and HEIC work too
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
| `detail.rs` | Texture, Sharpening, Noise Reduction, Defringe |
| `lens.rs` | Distortion, lens vignetting, measuring and removing chromatic aberration |
| `auto.rs` | The Auto button's rule |
| `rendered.rs`, `wic.rs` | Opening JPEG, PNG and HEIC (HEIC through Windows' own decoder) |
| `profile.rs` | The Profile menu's looks |
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
  * Zoomed in past the largest preview, only the visible part is rendered, from the full-size photo.
  * The export renders again from the full-size image, cropped.
* **Export:** baseline JPEG at quality 92, with an embedded sRGB ICC profile.
  Pixels are written upright, so the file needs no orientation tag.

See [TESTING.md](TESTING.md) for test results, measured speed, decoder support and what is still unverified.
