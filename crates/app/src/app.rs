use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, ColorImage, CornerRadius, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, TextureFilter, TextureHandle, TextureOptions, Vec2,
};
use pipeline::adjust::{EXPOSURE_RANGE, TEMPERATURE_RANGE, TINT_RANGE};
use pipeline::decode::SourceKind;
use pipeline::{sidecar, Adjustments, DecodedImage, ExportOptions, LinearImage, RenderedImage};

use crate::engine::{Engine, Loaded, Msg};
use crate::theme;

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const EXPORT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);

const TEX_OPTIONS: TextureOptions = TextureOptions {
    magnification: TextureFilter::Linear,
    minification: TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(TextureFilter::Linear),
};

/// One open photo.
struct Doc {
    path: PathBuf,
    decoded: DecodedImage,
    preview: Arc<LinearImage>,
    adjustments: Adjustments,
    /// What's on disk in the edit file (or defaults if there is none).
    saved: Adjustments,
    before_tex: TextureHandle,
    after_tex: TextureHandle,
    requested_revision: u64,
    shown_revision: u64,
    render_ms: Option<f64>,
    decode_ms: f64,
    export_dir: PathBuf,
}

impl Doc {
    fn dirty(&self) -> bool {
        self.adjustments != self.saved
    }
    fn file_name(&self) -> String {
        file_name(&self.path)
    }
}

/// What to do after the "unsaved changes" question is answered.
enum Pending {
    Open(PathBuf),
    PickAndOpen,
    Close,
}

struct Notice {
    title: String,
    body: String,
}

pub struct EditorApp {
    engine: Engine,
    doc: Option<Doc>,
    generation: u64,
    loading: Option<PathBuf>,
    show_before: bool,
    exporting: bool,
    status: String,
    status_is_error: bool,
    last_export: Option<PathBuf>,
    sidecar_warning: Option<String>,
    notice: Option<Notice>,
    pending: Option<Pending>,
    allow_close: bool,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn to_color_image(img: &RenderedImage) -> ColorImage {
    ColorImage::from_rgb([img.width, img.height], &img.rgb)
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self {
            engine: Engine::new(cc.egui_ctx.clone()),
            doc: None,
            generation: 0,
            loading: None,
            show_before: false,
            exporting: false,
            status: "Ready. Open a DNG to start (Ctrl+O), or drop one onto the window.".into(),
            status_is_error: false,
            last_export: None,
            sidecar_warning: None,
            notice: None,
            pending: None,
            allow_close: false,
        };
        if let Some(p) = initial {
            app.start_open(p);
        }
        app
    }

    fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = msg.into();
        self.status_is_error = is_error;
    }

    // ------------------------------------------------------------ actions

    fn dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(Doc::dirty)
    }

    fn request_open_dialog(&mut self) {
        if self.dirty() {
            self.pending = Some(Pending::PickAndOpen);
        } else {
            self.pick_and_open();
        }
    }

    fn request_open(&mut self, path: PathBuf) {
        if self.dirty() {
            self.pending = Some(Pending::Open(path));
        } else {
            self.start_open(path);
        }
    }

    fn pick_and_open(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open photo")
            .add_filter("RAW photos", &["dng", "DNG", "cr2", "cr3", "nef", "arw", "raf", "rw2", "orf"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self.doc.as_ref().and_then(|d| d.path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.start_open(path);
        }
    }

    fn start_open(&mut self, path: PathBuf) {
        self.generation += 1;
        self.set_status(format!("Opening {}…", file_name(&path)), false);
        self.loading = Some(path.clone());
        self.engine.open(self.generation, path);
    }

    fn save_edit(&mut self) -> bool {
        let Some(doc) = self.doc.as_mut() else { return false };
        match sidecar::save(&doc.path, &doc.adjustments) {
            Ok(p) => {
                doc.saved = doc.adjustments;
                self.sidecar_warning = None;
                self.set_status(format!("Edit saved to {}", file_name(&p)), false);
                true
            }
            Err(e) => {
                self.notice = Some(Notice { title: "Couldn't save the edit".into(), body: e.to_string() });
                self.set_status("Edit not saved.", true);
                false
            }
        }
    }

    fn start_export(&mut self) {
        if self.exporting || self.doc.is_none() {
            return;
        }
        // The export always matches a saved edit, so save first.
        if self.dirty() && !self.save_edit() {
            return;
        }
        let doc = self.doc.as_ref().expect("checked above");
        let opts = ExportOptions { out_dir: doc.export_dir.clone(), quality: pipeline::export::DEFAULT_QUALITY };
        self.engine.export(&doc.path, doc.decoded.clone(), doc.adjustments, opts);
        self.exporting = true;
        let (w, h) = (doc.decoded.info.width, doc.decoded.info.height);
        self.set_status(format!("Exporting full-size JPEG ({w} × {h})…"), false);
    }

    fn adjustments_changed(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.requested_revision += 1;
        self.engine.request_render(
            self.generation,
            doc.requested_revision,
            doc.preview.clone(),
            doc.adjustments,
            doc.decoded.info.look(),
        );
    }

    // ------------------------------------------------------------ messages

    fn handle_messages(&mut self, ctx: &egui::Context) {
        for msg in self.engine.poll() {
            match msg {
                Msg::Loaded(loaded) if loaded.generation == self.generation => self.on_loaded(ctx, *loaded),
                Msg::LoadFailed { generation, path, error } if generation == self.generation => {
                    self.loading = None;
                    self.notice = Some(Notice { title: format!("Couldn't open {}", file_name(&path)), body: error });
                    self.set_status(format!("{} could not be opened.", file_name(&path)), true);
                }
                Msg::Rendered { generation, revision, image, ms } if generation == self.generation => {
                    if let Some(doc) = self.doc.as_mut() {
                        if revision > doc.shown_revision {
                            doc.after_tex.set(to_color_image(&image), TEX_OPTIONS);
                            doc.shown_revision = revision;
                            doc.render_ms = Some(ms);
                        }
                    }
                }
                Msg::Exported { result, ms } => {
                    self.exporting = false;
                    match result {
                        Ok(path) => {
                            self.set_status(format!("Exported {} in {:.1} s", path.display(), ms / 1000.0), false);
                            self.last_export = Some(path);
                        }
                        Err(e) => {
                            self.notice = Some(Notice { title: "Export failed".into(), body: e });
                            self.set_status("Export failed. Nothing was overwritten.", true);
                        }
                    }
                }
                _ => {} // result for a photo that is no longer open
            }
        }
    }

    fn on_loaded(&mut self, ctx: &egui::Context, l: Loaded) {
        self.loading = None;
        self.show_before = false;
        self.last_export = None;
        let (adjustments, warning) = match l.saved {
            Ok(Some(a)) => (a, None),
            Ok(None) => (Adjustments::default(), None),
            Err(e) => (Adjustments::default(), Some(format!("{e} Starting from the original; saving will replace it."))),
        };
        self.sidecar_warning = warning;
        let before_tex = ctx.load_texture("before", to_color_image(&l.before), TEX_OPTIONS);
        // Until the first edited render arrives, show the unedited image.
        let after_tex = ctx.load_texture("after", to_color_image(&l.before), TEX_OPTIONS);
        let export_dir = ExportOptions::next_to(&l.path).out_dir;
        let restored = !adjustments.is_default();
        let info = &l.decoded.info;
        self.set_status(
            format!(
                "Opened {} ({} × {}) in {:.1} s{}",
                file_name(&l.path),
                info.width,
                info.height,
                l.decode_ms / 1000.0,
                if restored { " · saved edit restored" } else { "" }
            ),
            false,
        );
        self.doc = Some(Doc {
            path: l.path,
            decoded: l.decoded,
            preview: l.preview,
            saved: adjustments,
            adjustments,
            before_tex,
            after_tex,
            requested_revision: 0,
            shown_revision: 0,
            render_ms: None,
            decode_ms: l.decode_ms,
            export_dir,
        });
        self.adjustments_changed();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{} — Photo Editor",
            self.doc.as_ref().map(Doc::file_name).unwrap_or_default()
        )));
    }

    // ------------------------------------------------------------ input

    fn handle_input(&mut self, ctx: &egui::Context) {
        let any_modal = self.notice.is_some() || self.pending.is_some();
        if !any_modal {
            if ctx.input_mut(|i| i.consume_shortcut(&OPEN)) {
                self.request_open_dialog();
            }
            if ctx.input_mut(|i| i.consume_shortcut(&SAVE)) && self.doc.is_some() {
                self.save_edit();
            }
            if ctx.input_mut(|i| i.consume_shortcut(&EXPORT)) {
                self.start_export();
            }
            let typing = ctx.egui_wants_keyboard_input();
            if !typing && self.doc.is_some() && ctx.input(|i| i.key_pressed(Key::Backslash)) {
                self.show_before = !self.show_before;
            }
            let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
                .filter(|p| !p.as_os_str().is_empty());
            if let Some(path) = dropped {
                self.request_open(path);
            }
        }

        // Closing the window with unsaved edits asks first.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
    }

    // ------------------------------------------------------------ layout

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top")
            .exact_size(46.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("Open…").on_hover_text("Open a photo (Ctrl+O)").clicked() {
                        self.request_open_dialog();
                    }
                    ui.add_space(8.0);
                    if let Some(doc) = &self.doc {
                        ui.label(RichText::new(doc.file_name()).strong());
                        if doc.dirty() {
                            ui.label(RichText::new("● unsaved edit").color(theme::WARN).small());
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let has_doc = self.doc.is_some();
                        let export_label = if self.exporting { "Exporting…" } else { "Export JPEG" };
                        let export = egui::Button::new(RichText::new(export_label).color(Color32::WHITE).strong())
                            .fill(theme::ACCENT);
                        if ui
                            .add_enabled(has_doc && !self.exporting, export)
                            .on_hover_text("Full-size sRGB JPEG (Ctrl+E). Also saves the edit.")
                            .clicked()
                        {
                            self.start_export();
                        }
                        ui.add_space(12.0);
                        ui.add_enabled_ui(has_doc, |ui| {
                            ui.selectable_value(&mut self.show_before, false, "After")
                                .on_hover_text("Your edit (press \\ to toggle)");
                            ui.selectable_value(&mut self.show_before, true, "Before")
                                .on_hover_text("The photo as shot (press \\ to toggle)");
                        });
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .exact_size(28.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let color = if self.status_is_error { theme::ERROR } else { theme::MUTED };
                    ui.label(RichText::new(&self.status).color(color).small());
                    if let Some(path) = self.last_export.clone() {
                        if ui.small_button("Show in folder").clicked() {
                            let _ = std::process::Command::new("explorer").arg("/select,").arg(&path).spawn();
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(doc) = &self.doc {
                            if let Some(ms) = doc.render_ms {
                                ui.label(
                                    RichText::new(format!(
                                        "Preview {} × {} · render {:.0} ms",
                                        doc.preview.width, doc.preview.height, ms
                                    ))
                                    .color(theme::MUTED)
                                    .small(),
                                );
                            }
                        }
                    });
                });
            });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("adjust")
            .exact_size(300.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if self.doc.is_none() {
                        ui.label(RichText::new("Adjustments appear here once a photo is open.").color(theme::MUTED));
                        return;
                    }
                    let mut changed = false;
                    {
                        let doc = self.doc.as_mut().unwrap();
                        let a = &mut doc.adjustments;
                        section(ui, "LIGHT");
                        changed |= slider_row(ui, "Exposure", &mut a.exposure, EXPOSURE_RANGE, 0.01, |v| {
                            format!("{v:+.2} EV")
                        }, None);
                        ui.add_space(10.0);
                        section(ui, "WHITE BALANCE");
                        changed |= slider_row(ui, "Temperature", &mut a.temperature, TEMPERATURE_RANGE, 1.0, |v| {
                            format!("{v:+.0}")
                        }, Some((Color32::from_rgb(70, 120, 220), Color32::from_rgb(230, 190, 60))));
                        changed |= slider_row(ui, "Tint", &mut a.tint, TINT_RANGE, 1.0, |v| format!("{v:+.0}"),
                            Some((Color32::from_rgb(80, 180, 90), Color32::from_rgb(200, 80, 200))));
                        ui.label(RichText::new("Double-click a slider to reset it.").color(theme::MUTED).small());
                        ui.add_space(10.0);
                        if ui.add_enabled(!a.is_default(), egui::Button::new("Reset all")).clicked() {
                            *a = Adjustments::default();
                            changed = true;
                        }
                    }
                    if changed {
                        self.adjustments_changed();
                    }
                    let dirty = self.dirty();
                    ui.horizontal(|ui| {
                        let label = if dirty { "Save edit" } else { "Edit saved" };
                        if ui.add_enabled(dirty, egui::Button::new(label)).on_hover_text("Ctrl+S").clicked() {
                            self.save_edit();
                        }
                    });
                    if let Some(w) = &self.sidecar_warning {
                        ui.label(RichText::new(w).color(theme::WARN).small());
                    }

                    ui.add_space(14.0);
                    section(ui, "EXPORT");
                    let doc = self.doc.as_mut().unwrap();
                    ui.label(
                        RichText::new(format!(
                            "JPEG · sRGB · quality {} · {} × {}",
                            pipeline::export::DEFAULT_QUALITY,
                            doc.decoded.info.width,
                            doc.decoded.info.height
                        ))
                        .small(),
                    );
                    let dir_text = doc.export_dir.display().to_string();
                    ui.add(egui::Label::new(RichText::new(format!("To: {dir_text}")).small()).truncate())
                        .on_hover_text(dir_text);
                    if ui.small_button("Change folder…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().set_title("Export to folder").pick_folder() {
                            doc.export_dir = dir;
                        }
                    }
                    ui.label(
                        RichText::new("Existing files are never replaced: a number is added to the name instead.")
                            .color(theme::MUTED)
                            .small(),
                    );

                    ui.add_space(14.0);
                    section(ui, "PHOTO");
                    let info = &doc.decoded.info;
                    let kind = match info.kind {
                        SourceKind::LinearRaw => "Linear DNG (ProRAW type)",
                        SourceKind::BayerCfa => "Bayer RAW",
                    };
                    let camera = format!("{} {}", info.make, info.model);
                    info_row(ui, "Camera", camera.trim());
                    info_row(ui, "Type", kind);
                    info_row(ui, "Size", &format!("{} × {}", info.width, info.height));
                    info_row(ui, "Compression", info.tags.compression_name());
                    info_row(ui, "Baseline exp.", &format!("{:+.2} EV", info.baseline_exposure));
                    info_row(ui, "Opened in", &format!("{:.0} ms", doc.decode_ms));
                    for note in &info.notes {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("• {note}")).color(theme::MUTED).small());
                    }
                });
            });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            let area = ui.max_rect();
            if let Some(path) = &self.loading {
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spinner();
                            ui.label(format!("Opening {}…", file_name(path)));
                        });
                    });
                });
                if self.doc.is_none() {
                    return;
                }
            }
            let Some(doc) = &self.doc else {
                self.empty_state(ui, area);
                return;
            };
            let tex = if self.show_before { &doc.before_tex } else { &doc.after_tex };
            let size = tex.size_vec2();
            let inner = area.shrink(20.0);
            let scale = (inner.width() / size.x).min(inner.height() / size.y);
            let rect = Rect::from_center_size(inner.center(), size * scale);
            let painter = ui.painter_at(area);
            painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            if self.show_before {
                let badge = Rect::from_min_size(rect.min + Vec2::new(10.0, 10.0), Vec2::new(66.0, 22.0));
                painter.rect_filled(badge, CornerRadius::same(4), Color32::from_black_alpha(170));
                painter.text(badge.center(), egui::Align2::CENTER_CENTER, "BEFORE", egui::FontId::proportional(12.0), Color32::WHITE);
            }
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, area: Rect) {
        let box_rect = Rect::from_center_size(area.center(), Vec2::new(360.0, 150.0));
        ui.painter().rect_stroke(box_rect, CornerRadius::same(10), Stroke::new(1.0, theme::PANEL_EDGE), egui::StrokeKind::Inside);
        ui.scope_builder(egui::UiBuilder::new().max_rect(box_rect.shrink(16.0)), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(RichText::new("No photo open").size(18.0));
                ui.add_space(6.0);
                ui.label(RichText::new("Open an iPhone ProRAW DNG, or drop one here.").color(theme::MUTED));
                ui.add_space(12.0);
                if ui.button("Open photo…").clicked() {
                    self.request_open_dialog();
                }
            });
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        if let Some(notice) = &self.notice {
            let mut close = false;
            egui::Modal::new(Id::unique("notice")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(RichText::new(&notice.title).strong().size(16.0));
                ui.add_space(6.0);
                ui.label(&notice.body);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            });
            if close {
                self.notice = None;
            }
            return;
        }
        if self.pending.is_some() {
            let name = self.doc.as_ref().map(Doc::file_name).unwrap_or_default();
            let mut choice = None;
            egui::Modal::new(Id::unique("unsaved")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(RichText::new(format!("Save your edit to {name}?")).strong().size(16.0));
                ui.add_space(6.0);
                ui.label("If you don't save, the changes since your last save are lost. The photo itself is never changed.");
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Don't save").clicked() {
                        choice = Some(false);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        choice = None;
                        self.pending = None;
                    }
                });
            });
            if let Some(save) = choice {
                if save && !self.save_edit() {
                    self.pending = None;
                    return;
                }
                if let Some(doc) = self.doc.as_mut() {
                    doc.saved = doc.adjustments; // discard counts as "dealt with"
                }
                match self.pending.take() {
                    Some(Pending::Open(p)) => self.start_open(p),
                    Some(Pending::PickAndOpen) => self.pick_and_open(),
                    Some(Pending::Close) => {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    None => {}
                }
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).color(theme::MUTED).small().strong());
    ui.add_space(2.0);
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 16.0), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Small.resolve(ui.style()),
            theme::MUTED,
        );
        ui.add(egui::Label::new(RichText::new(value).small()).truncate());
    });
}

/// Label + value on one line, a full-width slider underneath, optional colour
/// strip showing which way the slider pushes. Returns true if the value changed.
fn slider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    (lo, hi): (f32, f32),
    step: f64,
    fmt: impl Fn(f32) -> String,
    gradient: Option<(Color32, Color32)>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui.add(
                egui::DragValue::new(value)
                    .range(lo..=hi)
                    .speed(step)
                    .custom_formatter(|v, _| fmt(v as f32))
                    .custom_parser(|s| s.trim().trim_end_matches("EV").trim().parse::<f64>().ok()),
            );
            changed |= r.changed();
        });
    });
    let width = ui.available_width();
    ui.spacing_mut().slider_width = width;
    let r = ui.add(egui::Slider::new(value, lo..=hi).step_by(step).show_value(false));
    if r.double_clicked() {
        *value = 0.0;
        changed = true;
    }
    changed |= r.changed();
    if let Some((left, right)) = gradient {
        let (strip, _) = ui.allocate_exact_size(Vec2::new(width, 3.0), Sense::hover());
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(strip.left_top(), left);
        mesh.colored_vertex(strip.right_top(), right);
        mesh.colored_vertex(strip.right_bottom(), right);
        mesh.colored_vertex(strip.left_bottom(), left);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
    }
    ui.add_space(6.0);
    changed
}

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages(&ctx);
        self.handle_input(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        self.side_panel(ui);
        self.canvas(ui);
        self.modals(&ctx);
    }
}
