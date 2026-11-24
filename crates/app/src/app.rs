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
use pipeline::adjust::{Geometry, MaskShape};
use pipeline::{preset, sidecar, Adjustments, Crop, DecodedImage, ExportOptions, LinearImage, Lut3D, Preset, RenderedImage};

use crate::canvas::CropDrag;
use crate::engine::{Engine, Loaded, Msg, Region, RenderRequest};
use crate::theme;
use crate::tools::{HealUi, MaskUi};

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const EXPORT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);

/// The name of the shortcut key on this system, for hints: "Ctrl" or "Cmd".
pub(crate) const MOD: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };

/// Show a file in the system's file manager (Explorer or Finder), selected.
pub(crate) fn reveal(path: &Path) {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg("/select,").arg(path);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    let _ = command.spawn();
}

/// Open a folder in the system's file manager.
pub(crate) fn open_folder(dir: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

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
    Heal,
    Mask,
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

/// How far the photo is zoomed in, and which part is in the middle of the screen.
pub(crate) struct Zoom {
    /// Screen pixels per photo pixel; `None` fits the whole picture in the window.
    pub scale: Option<f32>,
    /// The point of the finished picture at the centre of the screen (0…1).
    pub centre: (f32, f32),
    /// The scale that fits the whole picture, and the furthest zoom allowed
    /// (both noted by the picture area each time it is drawn).
    pub fit: f32,
    pub most: f32,
    /// The last scrolling came from a trackpad (which slides the picture) and
    /// not from a mouse wheel (which zooms).
    pub trackpad: bool,
}

impl Zoom {
    pub const FIT: Zoom = Zoom { scale: None, centre: (0.5, 0.5), fit: 0.25, most: 8.0, trackpad: false };

    /// The zoom in use, as screen pixels per photo pixel.
    pub fn current(&self) -> f32 {
        self.scale.unwrap_or(self.fit)
    }

    /// Zoom to `scale` (at or below the fitted size means "fit").
    pub fn set(&mut self, scale: f32) {
        self.scale = (scale > self.fit * 1.002).then_some(scale.min(self.most));
    }
}

/// What a preview request was for.
#[derive(Clone, PartialEq)]
pub(crate) struct RenderKey {
    adjustments: Adjustments,
    level: usize,
    region: Option<Region>,
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
    /// The edit this photo starts with and returns to on "Reset all". It
    /// depends on the kind of file (see `SourceInfo::starting_adjustments`).
    pub fresh: Adjustments,
    /// What's on disk in the edit file (or the fresh edit if there is none).
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
    /// What the last preview request was for, so a change that can't alter
    /// the picture doesn't trigger another render.
    pub rendered: Option<RenderKey>,
    /// When zoomed in: the part of the picture that should be rendered sharp.
    pub region: Option<Region>,
    /// The sharp rendering of a zoomed-in part, and which part it shows.
    pub region_tex: Option<(TextureHandle, Region)>,
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
    pub(crate) zoom: Zoom,
    pub(crate) curve_channel: usize,
    pub(crate) mixer_tab: usize,
    pub(crate) preset_name: String,
    /// Point Color: the next click on the photo picks a colour.
    pub(crate) picking_colour: bool,
    pub(crate) point_selected: usize,
    pub(crate) mask_ui: MaskUi,
    pub(crate) heal_ui: HealUi,
    /// The red picture of the selected mask, and what it was made from.
    pub(crate) mask_overlay: Option<((MaskShape, bool, Geometry), TextureHandle)>,
    /// Select the first mask and healing spot of the first photo opened (the
    /// `--select` option, for checking the app from a script).
    select_first: bool,
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn to_color_image(img: &RenderedImage) -> ColorImage {
    ColorImage::from_rgb([img.width, img.height], &img.rgb)
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>, tab: Tab, start_zoomed: bool, select_first: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self {
            engine: Engine::new(cc.egui_ctx.clone()),
            doc: None,
            generation: 0,
            loading: None,
            show_before: false,
            exporting: false,
            status: format!("Ready. Open a photo to start ({MOD}+O), or drop one onto the window."),
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
            zoom: if start_zoomed { Zoom { scale: Some(1.0), ..Zoom::FIT } } else { Zoom::FIT },
            curve_channel: 0,
            mixer_tab: 1,
            preset_name: String::new(),
            picking_colour: false,
            point_selected: 0,
            mask_ui: MaskUi::default(),
            heal_ui: HealUi::default(),
            mask_overlay: None,
            select_first,
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
            .add_filter("Photos", &["dng", "jpg", "jpeg", "png", "heic", "heif", "cr2", "cr3", "nef", "arw", "raf", "rw2", "orf"])
            .add_filter("RAW photos", &["dng", "cr2", "cr3", "nef", "arw", "raf", "rw2", "orf"])
            .add_filter("JPEG, PNG, HEIC", &["jpg", "jpeg", "png", "heic", "heif"])
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
        // "Before" is the photo with no edits, framed the same way.
        let adjustments = if self.show_before { doc.fresh.clone().with_crop_of(&doc.adjustments) } else { doc.adjustments.clone() };
        // Zoomed in, only the visible part is rendered, from the full-size photo
        // (at half size while dragging).
        let region = doc.region.map(|r| {
            let shrink = if dragging { 2 } else { 1 };
            Region { out_w: (r.out_w / shrink).max(1), out_h: (r.out_h / shrink).max(1), ..r }
        });
        if self.show_before && region.is_none() && doc.adjustments.geometry.is_identity() {
            // The whole-photo "Before" picture was made when the photo opened.
            // (With a Geometry warp it has to be rendered, to be framed the same way.)
            return;
        }
        let mut key = RenderKey { adjustments: adjustments.clone(), level, region };
        // The whole-photo preview doesn't depend on the crop, except for the
        // effects that are placed relative to it.
        if region.is_none() && key.adjustments.vignette.amount == 0.0 && key.adjustments.grain.amount == 0.0 {
            key.adjustments.crop = Crop::default();
        }
        if doc.rendered.as_ref() == Some(&key) {
            return;
        }
        doc.rendered = Some(key);
        doc.requested_revision += 1;
        self.engine.request_render(RenderRequest {
            generation: self.generation,
            revision: doc.requested_revision,
            region: region.map(|r| (r, doc.decoded.image.clone())),
            preview: doc.previews[level].clone(),
            full_size: doc.full_size(),
            adjustments,
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

    /// Delete key: remove the selected healing spot or mask (in their own tabs).
    pub(crate) fn delete_selected(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        let a = &mut doc.adjustments;
        match self.tab {
            Tab::Heal => {
                if let Some(i) = self.heal_ui.selected.filter(|i| *i < a.spots.len()) {
                    a.spots.remove(i);
                    self.heal_ui.selected = None;
                    self.adjustments_changed();
                }
            }
            Tab::Mask => {
                if let Some(i) = self.mask_ui.selected.filter(|i| *i < a.masks.len()) {
                    a.masks.remove(i);
                    self.mask_ui.selected = None;
                    self.adjustments_changed();
                }
            }
            _ => {}
        }
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
                Msg::Rendered { generation, revision, image, region, ms } if generation == self.generation => {
                    if let Some(doc) = self.doc.as_mut() {
                        if revision > doc.shown_revision {
                            match region {
                                Some(region) => {
                                    let tex = ctx.load_texture("region", to_color_image(&image), TEX_OPTIONS);
                                    doc.region_tex = Some((tex, region));
                                }
                                None => {
                                    doc.after_tex.set(to_color_image(&image), TEX_OPTIONS);
                                    doc.region_tex = None;
                                }
                            }
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
        self.picking_colour = false;
        self.point_selected = 0;
        self.mask_ui = MaskUi { brush_size: self.mask_ui.brush_size, brush_feather: self.mask_ui.brush_feather, ..MaskUi::default() };
        self.heal_ui = HealUi { selected: None, drag: None, ..std::mem::take(&mut self.heal_ui) };
        self.mask_overlay = None;
        // The first photo keeps a zoom asked for on the command line; later ones start fitted.
        if self.doc.is_some() {
            self.zoom = Zoom::FIT;
        }
        let fresh = l.decoded.info.starting_adjustments();
        let (adjustments, warning) = match l.saved {
            Ok(Some(a)) => (a, None),
            Ok(None) => (fresh.clone(), None),
            Err(e) => (fresh.clone(), Some(format!("{e} Starting from the original; saving will replace it."))),
        };
        self.sidecar_warning = warning;
        let before_tex = ctx.load_texture("before", to_color_image(&l.before), TEX_OPTIONS);
        // Until the first edited render arrives, show the unedited image.
        let after_tex = ctx.load_texture("after", to_color_image(&l.before), TEX_OPTIONS);
        let export_dir = ExportOptions::next_to(&l.path).out_dir;
        let restored = adjustments != fresh;
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
            fresh,
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
            region: None,
            region_tex: None,
            render_ms: None,
            decode_ms: l.decode_ms,
            export_dir,
        });
        if std::mem::take(&mut self.select_first) {
            let a = &self.doc.as_ref().expect("just set").adjustments;
            self.mask_ui.selected = (!a.masks.is_empty()).then_some(0);
            self.heal_ui.selected = (!a.spots.is_empty()).then_some(0);
        }
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
                    self.adjustments_changed();
                }
                if self.doc.is_some() && self.tab != Tab::Crop && ctx.input(|i| i.key_pressed(Key::Z) && i.modifiers.is_none()) {
                    self.zoom.scale = if self.zoom.scale.is_some() { None } else { Some(1.0) };
                }
                if self.tab == Tab::Crop && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                    self.tab = Tab::Edit;
                }
                if ctx.input(|i| i.key_pressed(Key::Escape)) {
                    // Put down whatever was waiting for a click on the photo.
                    self.picking_colour = false;
                    self.mask_ui.adding = None;
                }
                if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
                    self.delete_selected();
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
                    if ui.button("Open…").on_hover_text(format!("Open a photo ({MOD}+O)")).clicked() {
                        self.request_open_dialog();
                    }
                    ui.add_space(8.0);
                    let (can_undo, can_redo) =
                        self.doc.as_ref().map(|d| (!d.undo.is_empty(), !d.redo.is_empty())).unwrap_or_default();
                    if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text(format!("{MOD}+Z")).clicked() {
                        self.undo();
                    }
                    if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text(format!("{MOD}+Y")).clicked() {
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
                            .on_hover_text(format!("Full-size sRGB JPEG ({MOD}+E). Also saves the edit."))
                            .clicked()
                        {
                            self.start_export();
                        }
                        let dirty = self.dirty();
                        let save_label = if dirty { "Save edit" } else { "Edit saved" };
                        if ui.add_enabled(dirty, egui::Button::new(save_label)).on_hover_text(format!("{MOD}+S")).clicked() {
                            self.save_edit();
                        }
                    });
                });
            });
    }

    /// The strip under the photo: zoom on the left, Before/After on the right.
    fn view_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("view")
            .exact_size(38.0)
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(12, 6)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let has_doc = self.doc.is_some();
                    ui.add_enabled_ui(has_doc && (self.tab != Tab::Crop || self.show_before), |ui| {
                        let zoom = &mut self.zoom;
                        if ui.selectable_label(zoom.scale.is_none(), "Fit").on_hover_text("Whole picture (Z)").clicked() {
                            zoom.scale = None;
                        }
                        let actual = zoom.scale.is_some_and(|s| (s - 1.0).abs() < 0.005);
                        if ui.selectable_label(actual, "100%").on_hover_text("Actual pixels (Z, or double-click the photo)").clicked() {
                            zoom.set(1.0);
                        }
                        ui.add_space(6.0);
                        if ui.button("−").on_hover_text("Zoom out").clicked() {
                            zoom.set(zoom.current() / 1.25);
                        }
                        // The slider runs evenly through the zoom steps: each bit of travel multiplies the size.
                        let span = (zoom.most / zoom.fit).max(1.001).ln();
                        let mut t = ((zoom.current() / zoom.fit).max(1.0).ln() / span).clamp(0.0, 1.0);
                        ui.spacing_mut().slider_width = 150.0;
                        if ui.add(egui::Slider::new(&mut t, 0.0..=1.0).show_value(false)).on_hover_text("Zoom. Pinch or scroll over the photo does the same.").changed() {
                            zoom.set(zoom.fit * (t * span).exp());
                        }
                        if ui.button("+").on_hover_text("Zoom in").clicked() {
                            zoom.set((zoom.current() * 1.25).max(zoom.fit * 1.05));
                        }
                        ui.label(RichText::new(format!("{:.0}%", zoom.current() * 100.0)).color(theme::MUTED).small());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(has_doc, |ui| {
                            let label = if self.show_before { "Showing: Before" } else { "Before / After" };
                            if ui
                                .toggle_value(&mut self.show_before, label)
                                .on_hover_text("Compare with the photo as shot. Shortcut: the \\ key.")
                                .changed()
                            {
                                self.adjustments_changed();
                            }
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
                            reveal(&path);
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
        self.view_bar(ui);
        self.canvas(ui);
        self.modals(&ctx);
        self.commit_history(&ctx);
    }
}
