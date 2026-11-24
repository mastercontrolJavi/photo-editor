// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod engine;
mod side;
mod theme;
mod widgets;

use std::path::PathBuf;

fn main() -> eframe::Result {
    // `photo-editor.exe IMG_0001.DNG` (or "Open with…" in Explorer) opens that file.
    // Options for checking the app from a script:
    //   --tab <edit|crop|presets|info>   start on that tab
    //   --expand                         start with every group in the Edit tab open
    let mut initial = None;
    let mut start_tab = app::Tab::Edit;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--tab") => {
                start_tab = match args.next().and_then(|t| t.to_str().map(str::to_lowercase)).as_deref() {
                    Some("crop") => app::Tab::Crop,
                    Some("presets") => app::Tab::Presets,
                    Some("info") => app::Tab::Info,
                    _ => app::Tab::Edit,
                };
            }
            Some("--expand") => widgets::EXPAND_ALL.store(true, std::sync::atomic::Ordering::Relaxed),
            _ if initial.is_none() => initial = Some(PathBuf::from(arg)),
            _ => {}
        }
    }
    let mut viewport = eframe::egui::ViewportBuilder::default();
    // Same picture as the desktop shortcut (assets/icon.ico), for the title bar and taskbar.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title("Photo Editor")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Photo Editor", options, Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, initial, start_tab)))))
}
