//! The application: state, actions, messages from the background threads, and
//! the fixed parts of the window (top bar, status bar, dialogs). The side
//! panel lives in `side.rs` and the picture area in `canvas.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, ColorImage, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers, RichText, TextureFilter,
    TextureHandle, TextureOptions,
};
use pipeline::{preset, sidecar, Adjustments, Crop, DecodedImage, ExportOptions, LinearImage, Lut3D, Preset, RenderedImage};

use crate::canvas::CropDrag;
use crate::engine::{Engine, Loaded, Msg, RenderRequest};
use crate::theme;

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const EXPORT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);

/// How many steps Undo remembers.
const HISTORY_LIMIT: usize = 200;

pub(crate) const TEX_OPTIONS: TextureOptions = TextureOptions {
    magnification: TextureFilter::Linear,
    minification: TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(TextureFilter::Linear),
};

/// Which tool the side panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Edit,
    Crop,
    Presets,
    Info,
}

/// Shape the crop is held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Aspect {
    Free,
    Original,
    Ratio(u32, u32),
}

impl Aspect {
    pub(crate) const CHOICES: [Aspect; 7] = [
        Aspect::Free,
        Aspect::Original,
        Aspect::Ratio(1, 1),
        Aspect::Ratio(5, 4),
        Aspect::Ratio(4, 3),
        Aspect::Ratio(3, 2),
        Aspect::Ratio(16, 9),
    ];

    pub(crate) fn label(&self) -> String {
        match self {
            Aspect::Free => "Free".into(),
            Aspect::Original => "Original".into(),
            Aspect::Ratio(a, b) => format!("{a} × {b}"),
        }
    }
}

pub(crate) struct CropUi {
    pub aspect: Aspect,
    /// Use the chosen ratio the tall way round.
    pub portrait: bool,
    pub drag: Option<CropDrag>,
    /// The crop when the Straighten slider was grabbed, and whether it was as
    /// large as it could be (in which case it stays as large as it can be).
    pub straighten_from: Option<(Crop, bool)>,
}

/// One open photo.
pub(crate) struct Doc {
    pub path: PathBuf,
    pub decoded: DecodedImage,
    /// Downscaled copies, largest first (see `engine::PREVIEW_EDGES`).
    pub previews: Vec<Arc<LinearImage>>,
    /// Which of `previews` is being rendered.
    pub level: usize,
    pub adjustments: Adjustments,
    /// What's on disk in the edit file (or defaults if there is none).
    pub saved: Adjustments,
    /// The edit as of the last finished change; Undo steps between these.
    pub committed: Adjustments,
    pub undo: Vec<Adjustments>,
    pub redo: Vec<Adjustments>,
    pub before_tex: TextureHandle,
    pub after_tex: TextureHandle,
    pub requested_revision: u64,
    pub shown_revision: u64,
    pub render_ms: Option<f64>,
    pub shown_size: (usize, usize),
    /// What the last preview request was for (the edit and the preview size),
    /// so a change that can't alter the picture doesn't trigger another render.
    pub rendered: Option<(Adjustments, usize)>,
    pub decode_ms: f64,
    pub export_dir: PathBuf,
}

impl Doc {
    pub fn dirty(&self) -> bool {
        self.adjustments != self.saved
    }
    pub fn file_name(&self) -> String {
        file_name(&self.path)
    }
    /// Size of the full photo in pixels.
    pub fn full_size(&self) -> (usize, usize) {
        (self.decoded.image.width, self.decoded.image.height)
    }
}

/// What to do after the "unsaved changes" question is answered.
pub(crate) enum Pending {
    Open(PathBuf),
    PickAndOpen,
    Close,
}

pub(crate) struct Notice {
    pub title: String,
    pub body: String,
}

pub struct EditorApp {
    pub(crate) engine: Engine,
    pub(crate) doc: Option<Doc>,
    pub(crate) generation: u64,
    pub(crate) loading: Option<PathBuf>,
    pub(crate) show_before: bool,
    pub(crate) exporting: bool,
    pub(crate) status: String,
    pub(crate) status_is_error: bool,
    pub(crate) last_export: Option<PathBuf>,
    pub(crate) sidecar_warning: Option<String>,
    pub(crate) notice: Option<Notice>,
    pub(crate) pending: Option<Pending>,
    pub(crate) allow_close: bool,
    pub(crate) tab: Tab,
    pub(crate) crop_ui: CropUi,
    /// Starter presets, then everything in the presets folder.
    pub(crate) presets: Vec<Preset>,
    /// Look-up tables already read from disk (`None` = couldn't be read).
    pub(crate) luts: HashMap<String, Option<Arc<Lut3D>>>,
    /// Whether a mouse button is held (a slider or crop handle is being dragged).
    pub(crate) pointer_down: bool,
    pub(crate) curve_channel: usize,
    pub(crate) mixer_tab: usize,
    pub(crate) preset_name: String,
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn to_color_image(img: &RenderedImage) -> ColorImage {
    ColorImage::from_rgb([img.width, img.height], &img.rgb)
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>, tab: Tab) -> Self {
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
            tab,
            crop_ui: CropUi { aspect: Aspect::Original, portrait: false, drag: None, straighten_from: None },
            presets: Vec::new(),
            luts: HashMap::new(),
            pointer_down: false,
            curve_channel: 0,
            mixer_tab: 1,
            preset_name: String::new(),
        };
        app.reload_presets();
        if let Some(p) = initial {
            app.start_open(p);
        }
        app
    }

    pub(crate) fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = msg.into();
        self.status_is_error = is_error;
    }

    pub(crate) fn reload_presets(&mut self) {
        self.presets = preset::builtin();
        self.presets.extend(preset::load_library(&preset::library_dir()));
    }

    // ------------------------------------------------------------ actions

    pub(crate) fn dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(Doc::dirty)
    }

    pub(crate) fn request_open_dialog(&mut self) {
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

    pub(crate) fn save_edit(&mut self) -> bool {
        let Some(doc) = self.doc.as_mut() else { return false };
        match sidecar::save(&doc.path, &doc.adjustments) {
            Ok(p) => {
                doc.saved = doc.adjustments.clone();
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

    pub(crate) fn start_export(&mut self) {
        if self.exporting || self.doc.is_none() {
            return;
        }
        // The export always matches a saved edit, so save first.
        if self.dirty() && !self.save_edit() {
            return;
        }
        let doc = self.doc.as_ref().expect("checked above");
        let opts = ExportOptions { out_dir: doc.export_dir.clone(), quality: pipeline::export::DEFAULT_QUALITY };
        self.engine.export(&doc.path, doc.decoded.clone(), doc.adjustments.clone(), opts);
        self.exporting = true;
        let (fw, fh) = doc.full_size();
        let (w, h) = doc.adjustments.crop.output_size(fw, fh);
        self.set_status(format!("Exporting full-size JPEG ({w} × {h})…"), false);
    }

    /// The look-up table the current edit uses, read from disk the first time.
    fn current_lut(&mut self) -> Option<Arc<Lut3D>> {
        let name = self.doc.as_ref()?.adjustments.lut.as_ref()?.name.clone();
        if !self.luts.contains_key(&name) {
            let loaded = match preset::load_lut(&preset::library_dir(), &name) {
                Ok(lut) => Some(Arc::new(lut)),
                Err(e) => {
                    self.set_status(e, true);
                    None
                }
            };
            self.luts.insert(name.clone(), loaded);
        }
        self.luts.get(&name).cloned().flatten()
    }

    /// Ask for a new preview after any change to the edit.
    pub(crate) fn adjustments_changed(&mut self) {
        let lut = self.current_lut();
        let dragging = self.pointer_down;
        let Some(doc) = self.doc.as_mut() else { return };
        // While something is being dragged, render one size smaller so the
        // picture keeps up with the mouse; the full-quality preview follows
        // as soon as the button is released.
        let level = if dragging { (doc.level + 1).min(doc.previews.len() - 1) } else { doc.level };
        // The preview shows the whole photo, so the crop only matters to the
        // effects that are placed relative to it.
        let mut key = doc.adjustments.clone();
        if key.vignette.amount == 0.0 && key.grain.amount == 0.0 {
            key.crop = Crop::default();
        }
        if doc.rendered.as_ref().is_some_and(|(k, l)| *k == key && *l == level) {
            return;
        }
        doc.rendered = Some((key, level));
        doc.requested_revision += 1;
        self.engine.request_render(RenderRequest {
            generation: self.generation,
            revision: doc.requested_revision,
            preview: doc.previews[level].clone(),
            full_size: doc.full_size(),
            adjustments: doc.adjustments.clone(),
            look: doc.decoded.info.look(),
            lut,
        });
    }

    /// Record a finished change so Undo can go back to it. A slider drag
    /// counts as one change, recorded when the mouse button is released.
    fn commit_history(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.adjustments == doc.committed || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let previous = std::mem::replace(&mut doc.committed, doc.adjustments.clone());
        doc.undo.push(previous);
        if doc.undo.len() > HISTORY_LIMIT {
            doc.undo.remove(0);
        }
        doc.redo.clear();
    }

    pub(crate) fn undo(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(previous) = doc.undo.pop() else { return };
        doc.redo.push(std::mem::replace(&mut doc.adjustments, previous));
        doc.committed = doc.adjustments.clone();
        self.adjustments_changed();
    }

    pub(crate) fn redo(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(next) = doc.redo.pop() else { return };
        doc.undo.push(std::mem::replace(&mut doc.adjustments, next));
        doc.committed = doc.adjustments.clone();
        self.adjustments_changed();
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
                            doc.shown_size = (image.width, image.height);
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
        self.crop_ui.drag = None;
        self.crop_ui.straighten_from = None;
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
            shown_size: (l.before.width, l.before.height),
            previews: l.previews,
            level: 0,
            saved: adjustments.clone(),
            committed: adjustments.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            adjustments,
            before_tex,
            after_tex,
            requested_revision: 0,
            shown_revision: 0,
            rendered: None,
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
        let down = ctx.input(|i| i.pointer.any_down());
        if self.pointer_down && !down {
            // A drag just ended: replace the draft preview with the full-quality one.
            self.pointer_down = false;
            self.adjustments_changed();
        }
        self.pointer_down = down;
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
            if !typing {
                if ctx.input_mut(|i| i.consume_shortcut(&REDO_ALT) || i.consume_shortcut(&REDO)) {
                    self.redo();
                } else if ctx.input_mut(|i| i.consume_shortcut(&UNDO)) {
                    self.undo();
                }
                if self.doc.is_some() && ctx.input(|i| i.key_pressed(Key::Backslash)) {
                    self.show_before = !self.show_before;
                }
                if self.tab == Tab::Crop && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                    self.tab = Tab::Edit;
                }
            }
            let dropped = ctx
                .input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
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
                    let (can_undo, can_redo) =
                        self.doc.as_ref().map(|d| (!d.undo.is_empty(), !d.redo.is_empty())).unwrap_or_default();
                    if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                        self.undo();
                    }
                    if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
                        self.redo();
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
                        let dirty = self.dirty();
                        let save_label = if dirty { "Save edit" } else { "Edit saved" };
                        if ui.add_enabled(dirty, egui::Button::new(save_label)).on_hover_text("Ctrl+S").clicked() {
                            self.save_edit();
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
                                        doc.shown_size.0, doc.shown_size.1, ms
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

    fn modals(&mut self, ctx: &egui::Context) {
        if let Some(notice) = &self.notice {
            let mut close = false;
            egui::Modal::new(Id::unique("notice")).show(ctx, |ui| {
                ui.set_max_width(460.0);
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
                    doc.saved = doc.adjustments.clone(); // discard counts as "dealt with"
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
        self.commit_history(&ctx);
    }
}
