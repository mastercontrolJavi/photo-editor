//! Background work. The UI thread never decodes, renders or encodes: it hands
//! jobs to threads here and picks up the results as messages each frame.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use pipeline::{
    decode_file, export_jpeg, render, render_preview, sidecar, Adjustments, DecodedImage, ExportOptions, LinearImage, Look,
    Lut3D, RenderedImage, Scene, View, PREVIEW_LONG_EDGE,
};

/// Preview sizes kept for each photo (long edge, largest first). The app
/// renders the smallest one that still fills the space the photo is shown in,
/// which keeps sliders quick in a normal-sized window.
pub const PREVIEW_EDGES: [usize; 3] = [PREVIEW_LONG_EDGE, 1600, 1000];

pub enum Msg {
    Loaded(Box<Loaded>),
    LoadFailed { generation: u64, path: PathBuf, error: String },
    Rendered { generation: u64, revision: u64, image: RenderedImage, region: Option<Region>, ms: f64 },
    Exported { result: Result<PathBuf, String>, ms: f64 },
}

pub struct Loaded {
    pub generation: u64,
    pub path: PathBuf,
    pub decoded: DecodedImage,
    /// Downscaled copies of the photo, matching [`PREVIEW_EDGES`].
    pub previews: Vec<Arc<LinearImage>>,
    /// The photo rendered with no edits, for the Before view.
    pub before: RenderedImage,
    /// The saved edit, if any (or why it couldn't be read).
    pub saved: Result<Option<Adjustments>, String>,
    pub decode_ms: f64,
}

/// Part of the finished picture to render from the full-size photo (when zoomed in).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// `[left, top, right, bottom]`, each 0…1 of the finished picture.
    pub unit: [f64; 4],
    pub out_w: usize,
    pub out_h: usize,
}

pub struct RenderRequest {
    pub generation: u64,
    pub revision: u64,
    /// Render this part from the full-size photo instead of the whole preview.
    pub region: Option<(Region, Arc<LinearImage>)>,
    pub preview: Arc<LinearImage>,
    /// Size of the full photo, so the crop and grain scale correctly.
    pub full_size: (usize, usize),
    pub adjustments: Adjustments,
    pub look: Look,
    pub lut: Option<Arc<Lut3D>>,
}

struct Shared {
    /// Only the newest preview request is kept: dragging a slider fast skips
    /// the in-between frames instead of queueing them.
    slot: Mutex<Option<RenderRequest>>,
    wake: Condvar,
    quit: AtomicBool,
}

pub struct Engine {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    shared: Arc<Shared>,
    ctx: eframe::egui::Context,
}

impl Engine {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared { slot: Mutex::new(None), wake: Condvar::new(), quit: AtomicBool::new(false) });
        {
            let shared = shared.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            thread::Builder::new()
                .name("preview-render".into())
                .spawn(move || render_loop(shared, tx, ctx))
                .expect("start render thread");
        }
        Self { tx, rx, shared, ctx }
    }

    pub fn poll(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }

    /// Decode a file, build its previews and read its saved edit, off the UI thread.
    pub fn open(&self, generation: u64, path: PathBuf) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let msg = match decode_file(&path) {
                Err(e) => Msg::LoadFailed { generation, path, error: e.to_string() },
                Ok(decoded) => {
                    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let mut previews: Vec<Arc<LinearImage>> = Vec::new();
                    for edge in PREVIEW_EDGES {
                        // Each smaller copy is made from the one before it.
                        let from = previews.last().map(|p| p.as_ref()).unwrap_or(decoded.image.as_ref());
                        previews.push(Arc::new(from.downscale_to_fit(edge)));
                    }
                    let full = (decoded.image.width, decoded.image.height);
                    let before = render_preview(&previews[0], &decoded.info.starting_adjustments(), &decoded.info.look(), None, full);
                    let saved = sidecar::load(&path).map_err(|e| e.to_string());
                    Msg::Loaded(Box::new(Loaded { generation, path, decoded, previews, before, saved, decode_ms }))
                }
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub fn request_render(&self, request: RenderRequest) {
        *self.shared.slot.lock().unwrap() = Some(request);
        self.shared.wake.notify_one();
    }

    /// Full-resolution export on its own thread so the preview stays responsive.
    pub fn export(&self, source: &Path, decoded: DecodedImage, adjustments: Adjustments, opts: ExportOptions) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let source = source.to_owned();
        thread::spawn(move || {
            let started = Instant::now();
            let result = export_jpeg(&source, &decoded, &adjustments, &opts).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Exported { result, ms: started.elapsed().as_secs_f64() * 1000.0 });
            ctx.request_repaint();
        });
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

fn render_loop(shared: Arc<Shared>, tx: Sender<Msg>, ctx: eframe::egui::Context) {
    loop {
        let job = {
            let mut slot = shared.slot.lock().unwrap();
            loop {
                if shared.quit.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = slot.take() {
                    break job;
                }
                slot = shared.wake.wait(slot).unwrap();
            }
        };
        let started = Instant::now();
        let (image, region) = match &job.region {
            Some((region, full)) => {
                let scene = Scene { image: full, look: &job.look, lut: job.lut.as_deref() };
                let view = View::region(&job.adjustments.crop, full.width, full.height, region.unit, region.out_w, region.out_h);
                (render(&scene, &job.adjustments, &view, region.out_w, region.out_h), Some(*region))
            }
            None => {
                let (w, h) = (job.preview.width, job.preview.height);
                let scene = Scene { image: &job.preview, look: &job.look, lut: job.lut.as_deref() };
                // The whole photo is rendered; the crop is applied when it is drawn.
                let view = View::preview(&job.adjustments.crop, w, h, job.full_size.0, job.full_size.1);
                (render(&scene, &job.adjustments, &view, w, h), None)
            }
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if tx.send(Msg::Rendered { generation: job.generation, revision: job.revision, image, region, ms }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}
