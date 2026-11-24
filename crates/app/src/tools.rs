//! Tools that work directly on the photo: masks (gradients and the brush) and
//! healing spots. Each one draws its handles over the picture and turns mouse
//! input into changes to the edit.
//!
//! They are plain functions, not methods of the app, so the tests can drive
//! them with pretend mouse input.

use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Response, Shape, Stroke, Vec2};
use pipeline::adjust::{BrushStroke, LocalAdjust, Mask, MaskShape, Spot, SpotMode, MAX_BRUSH_POINTS, MAX_MASKS, MAX_SPOTS, SPOT_RADIUS};
use pipeline::geometry::Affine;
use pipeline::perspective::Warp;
use pipeline::Adjustments;

/// How close (in screen points) the pointer must be to a handle to grab it.
const GRAB: f32 = 11.0;
const LINE: Color32 = Color32::WHITE;
const SHADOW: Color32 = Color32::from_black_alpha(150);

/// Where the photo is on screen: turns screen positions into places in the
/// photo (as fractions, 0 … 1 across and down) and back, through the zoom,
/// the crop and the Geometry warp.
pub(crate) struct PhotoMap {
    /// Screen position of `centre`.
    origin: Pos2,
    /// Screen points per photo pixel.
    scale: f32,
    /// The point of the finished picture (0 … 1) shown at `origin`.
    centre: (f32, f32),
    /// Size of the finished picture in photo pixels.
    out: (f32, f32),
    /// Finished picture (0 … 1) → the warped photo, in pixels, and back.
    to_frame: Affine,
    from_frame: Affine,
    warp: Option<Warp>,
    size: (f64, f64),
}

impl PhotoMap {
    pub fn new(origin: Pos2, scale: f32, centre: (f32, f32), adj: &Adjustments, full_size: (usize, usize)) -> Self {
        let size = (full_size.0 as f64, full_size.1 as f64);
        let to_frame = adj.crop.unit_to_source(size.0, size.1);
        let (ow, oh) = adj.crop.output_size_f(size.0, size.1);
        PhotoMap {
            origin,
            scale,
            centre,
            out: (ow as f32, oh as f32),
            to_frame,
            from_frame: to_frame.inverse().unwrap_or(Affine::IDENTITY),
            warp: Warp::new(&adj.geometry, full_size.0, full_size.1),
            size,
        }
    }

    /// Screen position of a point of the finished picture (0 … 1).
    pub fn place(&self, u: f32, v: f32) -> Pos2 {
        self.origin + Vec2::new((u - self.centre.0) * self.out.0, (v - self.centre.1) * self.out.1) * self.scale
    }

    /// The point of the finished picture (0 … 1) at a screen position.
    pub fn unplace(&self, pos: Pos2) -> (f32, f32) {
        let d = (pos - self.origin) / self.scale;
        (self.centre.0 + d.x / self.out.0, self.centre.1 + d.y / self.out.1)
    }

    /// The place in the photo under a screen position.
    pub fn to_photo(&self, pos: Pos2) -> [f32; 2] {
        let (u, v) = self.unplace(pos);
        let (x, y) = self.to_frame.apply(u as f64, v as f64);
        let (x, y) = match &self.warp {
            Some(w) => w.to_source_unbounded(x, y).unwrap_or((x, y)),
            None => (x, y),
        };
        [(x / self.size.0) as f32, (y / self.size.1) as f32]
    }

    /// The screen position of a place in the photo.
    pub fn to_screen(&self, p: [f32; 2]) -> Pos2 {
        let (x, y) = (p[0] as f64 * self.size.0, p[1] as f64 * self.size.1);
        let (x, y) = match &self.warp {
            Some(w) => w.from_source(x, y),
            None => (x, y),
        };
        let (u, v) = self.from_frame.apply(x, y);
        self.place(u as f32, v as f32)
    }

    fn long(&self) -> f32 {
        self.size.0.max(self.size.1) as f32
    }

    /// On-screen length of a size given as a fraction of the photo's longer side.
    pub fn length(&self, fraction: f32) -> f32 {
        fraction * self.long() * self.scale
    }

    /// A place in the photo, moved by a distance given in "longer sides".
    pub fn moved(&self, p: [f32; 2], dx: f32, dy: f32) -> [f32; 2] {
        [p[0] + dx * self.long() / self.size.0 as f32, p[1] + dy * self.long() / self.size.1 as f32]
    }

    /// From `a` to `b` (places in the photo), in "longer sides".
    pub fn between(&self, a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
        ((b[0] - a[0]) * self.size.0 as f32 / self.long(), (b[1] - a[1]) * self.size.1 as f32 / self.long())
    }
}

/// A line that shows on any picture: white over a dark shadow.
fn line(painter: &egui::Painter, points: [Pos2; 2], width: f32) {
    painter.line_segment(points, Stroke::new(width + 2.0, SHADOW));
    painter.line_segment(points, Stroke::new(width, LINE));
}

fn ring(painter: &egui::Painter, centre: Pos2, radius: f32, width: f32) {
    painter.circle_stroke(centre, radius, Stroke::new(width + 2.0, SHADOW));
    painter.circle_stroke(centre, radius, Stroke::new(width, LINE));
}

fn handle(painter: &egui::Painter, at: Pos2, active: bool) {
    painter.circle_filled(at, 6.5, SHADOW);
    painter.circle_filled(at, 5.0, if active { crate::theme::ACCENT } else { LINE });
}

fn outline(painter: &egui::Painter, points: Vec<Pos2>, width: f32) {
    painter.add(Shape::closed_line(points.clone(), Stroke::new(width + 2.0, SHADOW)));
    painter.add(Shape::closed_line(points, Stroke::new(width, LINE)));
}

/// Where the mouse button went down for the drag that is starting.
fn press_position(ui: &egui::Ui, resp: &Response) -> Option<Pos2> {
    ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
}

// ─────────────────────────────────── masks ───────────────────────────────────

/// A gradient waiting to be dragged out on the photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewMask {
    Linear,
    Radial,
}

/// Which part of a mask the mouse has hold of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MaskDrag {
    LinearFrom,
    LinearTo,
    /// Both ends together; holds the pointer's last place in the photo.
    LinearMove([f32; 2]),
    RadialMove([f32; 2]),
    /// Dragging the ellipse wider (`true`) or taller (`false`).
    RadialEdge(bool),
    /// Dragging out a new ellipse from its centre.
    RadialNew,
    Brush,
}

pub(crate) struct MaskUi {
    pub selected: Option<usize>,
    pub adding: Option<NewMask>,
    /// Brush size and softness for the next stroke, as slider values (1 … 100 and 0 … 100).
    pub brush_size: f32,
    pub brush_feather: f32,
    pub erase: bool,
    pub show_overlay: bool,
    pub drag: Option<MaskDrag>,
}

impl Default for MaskUi {
    fn default() -> Self {
        MaskUi { selected: None, adding: None, brush_size: 35.0, brush_feather: 60.0, erase: false, show_overlay: true, drag: None }
    }
}

/// Brush slider value (1 … 100) → radius as a fraction of the photo's longer side.
pub(crate) fn brush_radius(size: f32) -> f32 {
    0.003 + (size / 100.0).powi(2) * 0.147
}

/// Points around an ellipse, as places in the photo.
fn ellipse(map: &PhotoMap, centre: [f32; 2], radius: [f32; 2], angle: f32, scale: f32) -> Vec<Pos2> {
    let (sin, cos) = angle.to_radians().sin_cos();
    (0..72)
        .map(|i| {
            let t = i as f32 / 72.0 * std::f32::consts::TAU;
            let (x, y) = (radius[0] * scale * t.cos(), radius[1] * scale * t.sin());
            map.to_screen(map.moved(centre, cos * x - sin * y, sin * x + cos * y))
        })
        .collect()
}

/// The four points on an ellipse's own axes: right, left, bottom, top.
fn ellipse_ends(map: &PhotoMap, centre: [f32; 2], radius: [f32; 2], angle: f32) -> [Pos2; 4] {
    let (sin, cos) = angle.to_radians().sin_cos();
    [(radius[0], 0.0), (-radius[0], 0.0), (0.0, radius[1]), (0.0, -radius[1])]
        .map(|(x, y)| map.to_screen(map.moved(centre, cos * x - sin * y, sin * x + cos * y)))
}

/// Draw the selected mask's handles and let the mouse place, move and paint
/// masks. Returns true if the edit changed.
pub(crate) fn mask_tool(ui: &egui::Ui, resp: &Response, painter: &egui::Painter, map: &PhotoMap, masks: &mut Vec<Mask>, state: &mut MaskUi) -> bool {
    let mut changed = false;
    state.selected = state.selected.filter(|i| *i < masks.len());
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());

    // ---- A new gradient is dragged out on the photo.
    if let Some(kind) = state.adding {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if resp.drag_started_by(PointerButton::Primary) && masks.len() < MAX_MASKS {
            if let Some(pos) = press_position(ui, resp) {
                let at = map.to_photo(pos);
                let (shape, drag) = match kind {
                    NewMask::Linear => (MaskShape::Linear { from: at, to: at }, MaskDrag::LinearTo),
                    NewMask::Radial => (MaskShape::Radial { centre: at, radius: [0.01, 0.01], angle: 0.0, feather: 50.0 }, MaskDrag::RadialNew),
                };
                masks.push(Mask { shape, invert: false, adjust: LocalAdjust::default() });
                state.selected = Some(masks.len() - 1);
                state.adding = None;
                state.drag = Some(drag);
                changed = true;
            }
        }
    }

    let Some(index) = state.selected else {
        if !ui.input(|i| i.pointer.any_down()) {
            state.drag = None;
        }
        return changed;
    };
    let mask = &mut masks[index];

    // ---- Start a drag on the selected mask.
    if state.drag.is_none() && resp.drag_started_by(PointerButton::Primary) {
        if let Some(pos) = press_position(ui, resp) {
            let near = |p: Pos2| p.distance(pos) <= GRAB;
            state.drag = match &mut mask.shape {
                MaskShape::Linear { from, to } => {
                    let (a, b) = (map.to_screen(*from), map.to_screen(*to));
                    if near(b) {
                        Some(MaskDrag::LinearTo)
                    } else if near(a) {
                        Some(MaskDrag::LinearFrom)
                    } else if near(a + (b - a) * 0.5) {
                        Some(MaskDrag::LinearMove(map.to_photo(pos)))
                    } else {
                        None
                    }
                }
                MaskShape::Radial { centre, radius, angle, .. } => {
                    let ends = ellipse_ends(map, *centre, *radius, *angle);
                    if near(ends[0]) || near(ends[1]) {
                        Some(MaskDrag::RadialEdge(true))
                    } else if near(ends[2]) || near(ends[3]) {
                        Some(MaskDrag::RadialEdge(false))
                    } else if near(map.to_screen(*centre)) {
                        Some(MaskDrag::RadialMove(map.to_photo(pos)))
                    } else {
                        None
                    }
                }
                MaskShape::Brush { strokes } => {
                    let used: usize = strokes.iter().map(|s| s.points.len()).sum();
                    if used < MAX_BRUSH_POINTS {
                        strokes.push(BrushStroke {
                            points: vec![map.to_photo(pos)],
                            size: brush_radius(state.brush_size),
                            feather: state.brush_feather,
                            erase: state.erase,
                        });
                        changed = true;
                        Some(MaskDrag::Brush)
                    } else {
                        None
                    }
                }
            };
        }
    }

    // ---- A click with the brush leaves a single dab.
    if resp.clicked_by(PointerButton::Primary) && state.drag.is_none() {
        if let (MaskShape::Brush { strokes }, Some(pos)) = (&mut mask.shape, resp.interact_pointer_pos()) {
            let used: usize = strokes.iter().map(|s| s.points.len()).sum();
            if used < MAX_BRUSH_POINTS {
                strokes.push(BrushStroke {
                    points: vec![map.to_photo(pos)],
                    size: brush_radius(state.brush_size),
                    feather: state.brush_feather,
                    erase: state.erase,
                });
                changed = true;
            }
        }
    }

    // ---- Carry on with the drag.
    if let (Some(drag), Some(pos)) = (state.drag, resp.interact_pointer_pos()) {
        if resp.dragged_by(PointerButton::Primary) {
            let at = map.to_photo(pos);
            let before = mask.shape.clone();
            match (&mut mask.shape, drag) {
                (MaskShape::Linear { from, .. }, MaskDrag::LinearFrom) => *from = at,
                (MaskShape::Linear { to, .. }, MaskDrag::LinearTo) => *to = at,
                (MaskShape::Linear { from, to }, MaskDrag::LinearMove(last)) => {
                    let d = [at[0] - last[0], at[1] - last[1]];
                    *from = [from[0] + d[0], from[1] + d[1]];
                    *to = [to[0] + d[0], to[1] + d[1]];
                    state.drag = Some(MaskDrag::LinearMove(at));
                }
                (MaskShape::Radial { centre, .. }, MaskDrag::RadialMove(last)) => {
                    *centre = [centre[0] + at[0] - last[0], centre[1] + at[1] - last[1]];
                    state.drag = Some(MaskDrag::RadialMove(at));
                }
                (MaskShape::Radial { centre, radius, angle, .. }, MaskDrag::RadialEdge(wide)) => {
                    // How far the pointer is from the centre along the ellipse's own axis.
                    let (dx, dy) = map.between(*centre, at);
                    let (sin, cos) = angle.to_radians().sin_cos();
                    if wide {
                        radius[0] = (cos * dx + sin * dy).abs().max(0.005);
                    } else {
                        radius[1] = (cos * dy - sin * dx).abs().max(0.005);
                    }
                }
                (MaskShape::Radial { centre, radius, .. }, MaskDrag::RadialNew) => {
                    let (dx, dy) = map.between(*centre, at);
                    *radius = [dx.abs().max(0.005), dy.abs().max(0.005)];
                }
                (MaskShape::Brush { strokes }, MaskDrag::Brush) => {
                    let used: usize = strokes.iter().map(|s| s.points.len()).sum();
                    if let Some(stroke) = strokes.last_mut() {
                        // A new point once the pointer has moved a fair part of the brush's width.
                        let far_enough = stroke.points.last().is_none_or(|last| {
                            let (dx, dy) = map.between(*last, at);
                            dx.hypot(dy) >= stroke.size * 0.2
                        });
                        if far_enough && used < MAX_BRUSH_POINTS {
                            stroke.points.push(at);
                        }
                    }
                }
                _ => {}
            }
            changed |= mask.shape != before;
        }
    }
    if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
        state.drag = None;
    }

    // ---- Draw the selected mask's guides.
    let over = |p: Pos2| pointer.is_some_and(|q| q.distance(p) <= GRAB);
    match &mask.shape {
        MaskShape::Linear { from, to } => {
            let (a, b) = (map.to_screen(*from), map.to_screen(*to));
            // A line across the picture at each end, at right angles to the gradient.
            let (dx, dy) = map.between(*from, *to);
            let len = dx.hypot(dy);
            if len > 1e-5 {
                let (px, py) = (-dy / len * 3.0, dx / len * 3.0);
                for (end, width) in [(from, 1.5), (to, 1.0)] {
                    line(painter, [map.to_screen(map.moved(*end, -px, -py)), map.to_screen(map.moved(*end, px, py))], width);
                }
            }
            painter.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(140)));
            let mid = a + (b - a) * 0.5;
            handle(painter, a, state.drag == Some(MaskDrag::LinearFrom) || over(a));
            handle(painter, b, state.drag == Some(MaskDrag::LinearTo) || over(b));
            handle(painter, mid, matches!(state.drag, Some(MaskDrag::LinearMove(_))) || over(mid));
            if over(a) || over(b) || over(mid) {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        }
        MaskShape::Radial { centre, radius, angle, feather } => {
            outline(painter, ellipse(map, *centre, *radius, *angle, 1.0), 1.5);
            let inner = 1.0 - feather / 100.0;
            if inner > 0.03 && inner < 0.97 {
                painter.add(Shape::closed_line(ellipse(map, *centre, *radius, *angle, inner), Stroke::new(1.0, Color32::from_white_alpha(110))));
            }
            let c = map.to_screen(*centre);
            let ends = ellipse_ends(map, *centre, *radius, *angle);
            handle(painter, c, matches!(state.drag, Some(MaskDrag::RadialMove(_))) || over(c));
            for (i, end) in ends.iter().enumerate() {
                handle(painter, *end, state.drag == Some(MaskDrag::RadialEdge(i < 2)) || over(*end));
            }
            if over(c) || ends.iter().any(|e| over(*e)) {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        }
        MaskShape::Brush { .. } => {
            if let Some(pos) = resp.hover_pos() {
                let r = map.length(brush_radius(state.brush_size));
                ring(painter, pos, r, 1.0);
                let hard = r * (1.0 - state.brush_feather / 100.0);
                if hard > 2.0 && hard < r - 2.0 {
                    painter.circle_stroke(pos, hard, Stroke::new(1.0, Color32::from_white_alpha(110)));
                }
                if state.erase {
                    painter.line_segment([pos - Vec2::new(5.0, 0.0), pos + Vec2::new(5.0, 0.0)], Stroke::new(1.5, LINE));
                }
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
        }
    }
    changed
}

// ────────────────────────────────── healing ──────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum HealDrag {
    /// Moving the spot (its source keeps its distance); holds the pointer's last place.
    Spot([f32; 2]),
    Source([f32; 2]),
}

pub(crate) struct HealUi {
    pub selected: Option<usize>,
    /// Settings for the next spot, as slider values.
    pub size: f32,
    pub feather: f32,
    pub opacity: f32,
    pub mode: SpotMode,
    pub drag: Option<HealDrag>,
}

impl Default for HealUi {
    fn default() -> Self {
        HealUi { selected: None, size: 20.0, feather: 40.0, opacity: 100.0, mode: SpotMode::Heal, drag: None }
    }
}

/// Size slider value (1 … 100) → radius as a fraction of the photo's longer side.
pub(crate) fn spot_radius(size: f32) -> f32 {
    (SPOT_RADIUS.0 + (size / 100.0).powi(2) * 0.078).clamp(SPOT_RADIUS.0, SPOT_RADIUS.1)
}

pub(crate) fn spot_size(radius: f32) -> f32 {
    (((radius - SPOT_RADIUS.0) / 0.078).max(0.0).sqrt() * 100.0).clamp(1.0, 100.0)
}

/// Draw the healing spots and let the mouse add, select and move them.
/// `suggest` picks a source for a new spot (its centre and radius are given).
/// Returns true if the edit changed.
pub(crate) fn heal_tool(
    ui: &egui::Ui,
    resp: &Response,
    painter: &egui::Painter,
    map: &PhotoMap,
    spots: &mut Vec<Spot>,
    state: &mut HealUi,
    suggest: impl Fn([f32; 2], f32) -> [f32; 2],
) -> bool {
    let mut changed = false;
    state.selected = state.selected.filter(|i| *i < spots.len());
    // Which spot (and which of its two circles) is under a screen position.
    let hit = |spots: &[Spot], selected: Option<usize>, pos: Pos2| -> Option<(usize, bool)> {
        if let Some(i) = selected {
            let s = &spots[i];
            if map.to_screen(s.source).distance(pos) <= map.length(s.radius).max(GRAB) {
                return Some((i, true));
            }
        }
        (0..spots.len()).rev().find(|&i| map.to_screen(spots[i].at).distance(pos) <= map.length(spots[i].radius).max(GRAB)).map(|i| (i, false))
    };

    if resp.drag_started_by(PointerButton::Primary) {
        if let Some(pos) = press_position(ui, resp) {
            if let Some((i, source)) = hit(spots, state.selected, pos) {
                state.selected = Some(i);
                let at = map.to_photo(pos);
                state.drag = Some(if source { HealDrag::Source(at) } else { HealDrag::Spot(at) });
            }
        }
    }
    if let (Some(drag), Some(i), Some(pos)) = (state.drag, state.selected, resp.interact_pointer_pos()) {
        if resp.dragged_by(PointerButton::Primary) {
            let at = map.to_photo(pos);
            let s = &mut spots[i];
            let before = *s;
            match drag {
                HealDrag::Spot(last) => {
                    let d = [at[0] - last[0], at[1] - last[1]];
                    s.at = [s.at[0] + d[0], s.at[1] + d[1]];
                    s.source = [s.source[0] + d[0], s.source[1] + d[1]];
                    state.drag = Some(HealDrag::Spot(at));
                }
                HealDrag::Source(last) => {
                    s.source = [s.source[0] + at[0] - last[0], s.source[1] + at[1] - last[1]];
                    state.drag = Some(HealDrag::Source(at));
                }
            }
            changed |= *s != before;
        }
    }
    if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
        state.drag = None;
    }
    if resp.clicked_by(PointerButton::Primary) {
        if let Some(pos) = resp.interact_pointer_pos() {
            match hit(spots, state.selected, pos) {
                Some((i, _)) => state.selected = Some(i),
                None if spots.len() < MAX_SPOTS => {
                    let at = map.to_photo(pos);
                    let inside = (0.0..=1.0).contains(&at[0]) && (0.0..=1.0).contains(&at[1]);
                    if inside {
                        let radius = spot_radius(state.size);
                        spots.push(Spot { at, source: suggest(at, radius), radius, feather: state.feather, opacity: state.opacity, mode: state.mode });
                        state.selected = Some(spots.len() - 1);
                        changed = true;
                    }
                }
                None => {}
            }
        }
    }

    // ---- Draw.
    for (i, s) in spots.iter().enumerate() {
        let (c, r) = (map.to_screen(s.at), map.length(s.radius).max(3.0));
        if state.selected == Some(i) {
            let from = map.to_screen(s.source);
            let d = c - from;
            if d.length() > 2.0 * r {
                // An arrow from the source to the spot, between the two circles.
                let dir = d / d.length();
                let (start, end) = (from + dir * r, c - dir * r);
                line(painter, [start, end], 1.0);
                let side = Vec2::new(-dir.y, dir.x) * 4.0;
                line(painter, [end, end - dir * 8.0 + side], 1.0);
                line(painter, [end, end - dir * 8.0 - side], 1.0);
            }
            ring(painter, c, r, 2.0);
            painter.add(Shape::dashed_line(
                &(0..=48).map(|k| from + Vec2::angled(k as f32 / 48.0 * std::f32::consts::TAU) * r).collect::<Vec<_>>(),
                Stroke::new(1.5, LINE),
                5.0,
                4.0,
            ));
        } else {
            ring(painter, c, r, 1.0);
        }
    }
    if let Some(pos) = resp.hover_pos() {
        match hit(spots, state.selected, pos) {
            Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
            None if state.drag.is_none() => {
                painter.circle_stroke(pos, map.length(spot_radius(state.size)), Stroke::new(1.0, Color32::from_white_alpha(170)));
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            None => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, RawInput, Rect, Sense};
    use pipeline::adjust::Geometry;
    use pipeline::Crop;

    const FULL: (usize, usize) = (4000, 3000);

    /// A 800 × 600 picture of the whole photo with its corner at (100, 50).
    fn simple_map(adj: &Adjustments) -> PhotoMap {
        PhotoMap::new(Pos2::new(500.0, 350.0), 0.2, (0.5, 0.5), adj, FULL)
    }

    /// Screen position of a place in the uncropped, unwarped photo.
    fn at(u: f32, v: f32) -> Pos2 {
        Pos2::new(100.0 + u * 800.0, 50.0 + v * 600.0)
    }

    struct Rig {
        ctx: egui::Context,
        time: f64,
        adj: Adjustments,
        masks: MaskUi,
        heal: HealUi,
        heal_mode: bool,
    }

    impl Rig {
        fn new(heal_mode: bool) -> Self {
            let mut rig = Rig { ctx: egui::Context::default(), time: 0.0, adj: Adjustments::default(), masks: MaskUi::default(), heal: HealUi::default(), heal_mode };
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<Event>) -> bool {
            self.time += 0.05;
            let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
            let input = RawInput { screen_rect: Some(area), time: Some(self.time), events, ..Default::default() };
            let mut changed = false;
            let (adj, masks, heal, heal_mode) = (&mut self.adj, &mut self.masks, &mut self.heal, self.heal_mode);
            let _ = self.ctx.run_ui(input, |ui| {
                let resp = ui.interact(area, ui.scope_id().with("photo"), Sense::click_and_drag());
                let painter = ui.painter_at(area);
                let map = simple_map(adj);
                changed = if heal_mode {
                    heal_tool(ui, &resp, &painter, &map, &mut adj.spots, heal, |at, radius| [at[0] + 3.0 * radius, at[1]])
                } else {
                    mask_tool(ui, &resp, &painter, &map, &mut adj.masks, masks)
                };
            });
            changed
        }

        fn button(&mut self, pos: Pos2, pressed: bool) -> bool {
            self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }])
        }

        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.frame(vec![Event::PointerMoved(from)]);
            self.button(from, true);
            for k in 1..=4 {
                self.frame(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 4.0))]);
            }
            self.button(to, false);
            self.frame(vec![]);
        }

        fn click(&mut self, pos: Pos2) {
            self.frame(vec![Event::PointerMoved(pos)]);
            self.button(pos, true);
            self.button(pos, false);
            self.frame(vec![]);
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.004 && (a[1] - b[1]).abs() < 0.004
    }

    #[test]
    fn screen_and_photo_positions_agree_through_crop_and_warp() {
        let mut adj = Adjustments::default();
        let map = simple_map(&adj);
        assert!(close(map.to_photo(at(0.25, 0.75)), [0.25, 0.75]));
        assert!(map.to_screen([0.25, 0.75]).distance(at(0.25, 0.75)) < 0.01);
        assert!((map.length(0.1) - 80.0).abs() < 0.01);
        // Turned, flipped, cropped, straightened and warped: there and back still agrees.
        adj.crop = Crop { quarter_turns: 1, flip: true, angle: 7.0, cx: 0.45, cy: 0.55, w: 0.6, h: 0.5 };
        adj.geometry = Geometry { vertical: -35.0, horizontal: 20.0, rotate: 4.0, scale: 115.0, ..Default::default() };
        let map = PhotoMap::new(Pos2::new(500.0, 350.0), 0.31, (0.4, 0.6), &adj, FULL);
        for p in [[0.5, 0.5], [0.3, 0.62], [0.71, 0.4]] {
            let back = map.to_photo(map.to_screen(p));
            assert!((back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4, "{p:?} → {back:?}");
        }
    }

    #[test]
    fn a_linear_gradient_is_dragged_out_and_its_ends_can_be_moved() {
        let mut rig = Rig::new(false);
        rig.masks.adding = Some(NewMask::Linear);
        rig.drag(at(0.5, 0.2), at(0.5, 0.5));
        assert_eq!(rig.adj.masks.len(), 1);
        assert_eq!((rig.masks.selected, rig.masks.adding), (Some(0), None));
        let MaskShape::Linear { from, to } = rig.adj.masks[0].shape.clone() else { panic!("not a linear gradient") };
        assert!(close(from, [0.5, 0.2]) && close(to, [0.5, 0.5]), "{from:?} {to:?}");
        // Grab the far end and pull it down.
        rig.drag(at(0.5, 0.5), at(0.6, 0.7));
        let MaskShape::Linear { from, to } = rig.adj.masks[0].shape.clone() else { panic!() };
        assert!(close(from, [0.5, 0.2]) && close(to, [0.6, 0.7]), "{from:?} {to:?}");
        // The middle handle moves both ends together.
        rig.drag(at(0.55, 0.45), at(0.45, 0.35));
        let MaskShape::Linear { from, to } = rig.adj.masks[0].shape.clone() else { panic!() };
        assert!(close(from, [0.4, 0.1]) && close(to, [0.5, 0.6]), "{from:?} {to:?}");
        // Dragging on empty photo does nothing.
        let before = rig.adj.masks.clone();
        rig.drag(at(0.1, 0.9), at(0.2, 0.8));
        assert_eq!(rig.adj.masks, before);
    }

    #[test]
    fn a_radial_gradient_is_dragged_out_resized_and_moved() {
        let mut rig = Rig::new(false);
        rig.masks.adding = Some(NewMask::Radial);
        // 0.2 of the width across and 0.2 of the height down: 0.2 and 0.15 of the longer side.
        rig.drag(at(0.5, 0.5), at(0.7, 0.7));
        let MaskShape::Radial { centre, radius, .. } = rig.adj.masks[0].shape.clone() else { panic!("not a radial gradient") };
        assert!(close(centre, [0.5, 0.5]) && close(radius, [0.2, 0.15]), "{centre:?} {radius:?}");
        // Pull the right-hand edge outwards.
        rig.drag(at(0.7, 0.5), at(0.8, 0.5));
        let MaskShape::Radial { radius, .. } = rig.adj.masks[0].shape.clone() else { panic!() };
        assert!(close(radius, [0.3, 0.15]), "{radius:?}");
        // Move it by its centre.
        rig.drag(at(0.5, 0.5), at(0.4, 0.6));
        let MaskShape::Radial { centre, radius, .. } = rig.adj.masks[0].shape.clone() else { panic!() };
        assert!(close(centre, [0.4, 0.6]) && close(radius, [0.3, 0.15]), "{centre:?}");
    }

    #[test]
    fn the_brush_paints_strokes_and_single_dabs() {
        let mut rig = Rig::new(false);
        rig.adj.masks.push(Mask { shape: MaskShape::Brush { strokes: vec![] }, invert: false, adjust: LocalAdjust::default() });
        rig.masks.selected = Some(0);
        rig.drag(at(0.2, 0.3), at(0.6, 0.3));
        rig.masks.erase = true;
        rig.click(at(0.4, 0.3));
        let MaskShape::Brush { strokes } = rig.adj.masks[0].shape.clone() else { panic!() };
        assert_eq!(strokes.len(), 2, "{strokes:?}");
        assert!(strokes[0].points.len() >= 3 && !strokes[0].erase);
        assert!(close(strokes[0].points[0], [0.2, 0.3]) && close(*strokes[0].points.last().unwrap(), [0.6, 0.3]));
        assert!((strokes[0].size - brush_radius(35.0)).abs() < 1e-6);
        assert!(strokes[1].erase && strokes[1].points.len() == 1 && close(strokes[1].points[0], [0.4, 0.3]));
    }

    #[test]
    fn clicking_adds_a_healing_spot_and_dragging_moves_it_or_its_source() {
        let mut rig = Rig::new(true);
        rig.click(at(0.3, 0.4));
        assert_eq!(rig.adj.spots.len(), 1);
        let s = rig.adj.spots[0];
        let r = spot_radius(20.0);
        assert!(close(s.at, [0.3, 0.4]) && close(s.source, [0.3 + 3.0 * r, 0.4]) && (s.radius - r).abs() < 1e-6);
        assert_eq!(rig.heal.selected, Some(0));
        // Clicking it again doesn't add another.
        rig.click(at(0.3, 0.4));
        assert_eq!(rig.adj.spots.len(), 1);
        // Drag the source somewhere else.
        rig.drag(at(0.3 + 3.0 * r, 0.4), at(0.5, 0.6));
        let s = rig.adj.spots[0];
        assert!(close(s.at, [0.3, 0.4]) && close(s.source, [0.5, 0.6]), "{s:?}");
        // Drag the spot: its source comes along.
        rig.drag(at(0.3, 0.4), at(0.35, 0.3));
        let s = rig.adj.spots[0];
        assert!(close(s.at, [0.35, 0.3]) && close(s.source, [0.55, 0.5]), "{s:?}");
        // A second spot elsewhere becomes the selected one.
        rig.click(at(0.8, 0.8));
        assert_eq!((rig.adj.spots.len(), rig.heal.selected), (2, Some(1)));
        // Outside the photo nothing is added.
        rig.click(Pos2::new(40.0, 20.0));
        assert_eq!(rig.adj.spots.len(), 2);
    }

    #[test]
    fn slider_values_and_sizes_convert_both_ways() {
        for size in [1.0, 20.0, 55.0, 100.0] {
            assert!((spot_size(spot_radius(size)) - size).abs() < 0.6, "{size}");
        }
        assert!(brush_radius(1.0) > 0.002 && brush_radius(100.0) <= 0.15);
    }
}
