# Photo Editor (milestone 1)

An offline Windows photo editor for iPhone ProRAW DNGs, written in Rust.
It needs no account, network connection or paid service.

Milestone 1 does one complete loop. You open a DNG, adjust exposure and white balance, and compare before and after. Then you save the edit and export a full-size sRGB JPEG.

## Using it

| Action | How |
|---|---|
| Open a photo | **Open…** (Ctrl+O), drop a file on the window, or `photo-editor.exe IMG_0001.DNG` |
| Adjust | Exposure (−5 to +5 EV), Temperature, Tint. Double-click a slider to reset it. |
| Compare | **Before / After** buttons, or press `\` |
| Save the edit | **Save edit** (Ctrl+S). It writes `IMG_0001.DNG.edit.json` next to the photo. |
| Export | **Export JPEG** (Ctrl+E). It writes `Exported\IMG_0001.jpg` next to the photo. Export also saves the edit. |

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
.\target\release\pe.exe fixtures .\fixtures   # synthetic test DNGs (add --large for 12/48 MP)
```

## How it's put together

```
crates/pipeline   all image work, no UI (decode, colour, render, export, edit files) + `pe` tool
crates/app        the window (egui/eframe); runs every image job on background threads
```

* **Decoding:** [rawler](https://crates.io/crates/rawler) 0.8 (pure Rust, LGPL-2.1).
  It handles ProRAW-style linear DNGs, Bayer DNGs, and DNGs compressed with lossless JPEG or JPEG XL.
* **Working space:** 32-bit float, scene-linear, sRGB primaries, D65 white.
  Values brighter than sensor white are kept, so lowering exposure can bring them back.
* **One render function** is used for both the preview and the export.
  * The preview is the photo shrunk once to at most 2560 px on its long edge.
  * The export renders again from the full-size image.
* **Export:** baseline JPEG at quality 92, with an embedded sRGB ICC profile.
  Pixels are written upright, so the file needs no orientation tag.

See [TESTING.md](TESTING.md) for test results, measured speed, decoder support and what is still unverified.
