// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod theme;

use std::path::PathBuf;

fn main() -> eframe::Result {
    // `photo-editor.exe IMG_0001.DNG` (or "Open with…" in Explorer) opens that file.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
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
    eframe::run_native("Photo Editor", options, Box::new(|cc| Ok(Box::new(app::EditorApp::new(cc, initial)))))
}
