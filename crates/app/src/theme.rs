//! Neutral dark theme. Photo editors stay colourless on purpose: a tinted
//! interface around a picture shifts how you judge its colour.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle};

pub const CANVAS: Color32 = Color32::from_gray(22);
pub const PANEL: Color32 = Color32::from_gray(32);
pub const PANEL_EDGE: Color32 = Color32::from_gray(44);
pub const TEXT: Color32 = Color32::from_gray(218);
pub const MUTED: Color32 = Color32::from_gray(140);
pub const ACCENT: Color32 = Color32::from_rgb(86, 140, 230);
pub const WARN: Color32 = Color32::from_rgb(226, 178, 92);
pub const ERROR: Color32 = Color32::from_rgb(232, 110, 100);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::dark();
    let v = &mut style.visuals;
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_gray(38);
    v.extreme_bg_color = Color32::from_gray(20);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    // Sliders are centred on 0, so a fill from the left end would be misleading.
    v.slider_trailing_fill = false;
    v.window_corner_radius = CornerRadius::same(8);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = CornerRadius::same(5);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_gray(48);
    v.widgets.hovered.weak_bg_fill = Color32::from_gray(60);
    v.widgets.active.weak_bg_fill = Color32::from_gray(70);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_EDGE);

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.slider_width = 200.0;
    style.spacing.interact_size.y = 24.0;
    style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
    style.text_styles.insert(TextStyle::Heading, FontId::proportional(13.0));
    ctx.set_global_style(style);
}
