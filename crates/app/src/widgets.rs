//! Small reusable pieces of the side panel: sliders, section headers, the curve editor.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Pos2, RichText, Sense, Stroke, Vec2};
use pipeline::adjust::curve_table;

use crate::theme;

/// Start with every group open (set by the `--expand` option, for screenshots).
pub static EXPAND_ALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Open only these sections at start-up (set by the `--only` option, for screenshots).
pub static ONLY_SECTIONS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// What to draw under a slider to show which way it pushes.
pub enum Track {
    None,
    /// A two-colour strip, left to right.
    Blend(Color32, Color32),
    /// The full colour wheel (for hue sliders).
    Rainbow,
}

pub struct SliderSpec<'a> {
    pub label: &'a str,
    pub range: (f32, f32),
    /// Value the slider returns to on a double-click.
    pub default: f32,
    pub step: f64,
    pub decimals: usize,
    pub suffix: &'a str,
    pub track: Track,
    /// Colour dot before the label (Color Mixer bands).
    pub dot: Option<Color32>,
}

impl<'a> SliderSpec<'a> {
    /// A −100 … +100 slider that rests at 0.
    pub fn percent(label: &'a str) -> Self {
        Self { label, range: (-100.0, 100.0), default: 0.0, step: 1.0, decimals: 0, suffix: "", track: Track::None, dot: None }
    }

    /// A 0 … 100 slider.
    pub fn amount(label: &'a str, default: f32) -> Self {
        Self { range: (0.0, 100.0), default, ..Self::percent(label) }
    }

    pub fn track(mut self, track: Track) -> Self {
        self.track = track;
        self
    }

    pub fn dot(mut self, colour: Color32) -> Self {
        self.dot = Some(colour);
        self
    }
}

fn format_value(v: f32, spec: &SliderSpec) -> String {
    let signed = spec.range.0 < 0.0;
    let d = spec.decimals;
    let v = if v.abs() < 0.5 * 10f32.powi(-(d as i32)) { 0.0 } else { v };
    if signed {
        format!("{v:+.d$}{}", spec.suffix)
    } else {
        format!("{v:.d$}{}", spec.suffix)
    }
}

/// Label + value on one line, a full-width slider underneath. Double-click
/// resets. Returns true if the value changed.
pub fn slider(ui: &mut egui::Ui, value: &mut f32, spec: SliderSpec) -> bool {
    let mut changed = false;
    let (lo, hi) = spec.range;
    ui.horizontal(|ui| {
        if let Some(colour) = spec.dot {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.5, colour);
        }
        ui.label(spec.label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let suffix = spec.suffix.trim().to_string();
            let r = ui.add(
                egui::DragValue::new(value)
                    .range(lo..=hi)
                    .speed(spec.step)
                    .custom_formatter(|v, _| format_value(v as f32, &spec))
                    .custom_parser(move |s| s.trim().trim_end_matches(suffix.as_str()).trim().parse::<f64>().ok()),
            );
            changed |= r.changed();
        });
    });
    let width = ui.available_width();
    ui.spacing_mut().slider_width = width;
    let r = ui.add(egui::Slider::new(value, lo..=hi).step_by(spec.step).show_value(false));
    if r.double_clicked() {
        *value = spec.default;
        changed = true;
    }
    changed |= r.changed();
    match spec.track {
        Track::None => {}
        Track::Blend(left, right) => strip(ui, width, &[left, right]),
        Track::Rainbow => {
            let stops: Vec<Color32> = (0..=12).map(|i| hue_colour(i as f32 * 30.0)).collect();
            strip(ui, width, &stops);
        }
    }
    ui.add_space(4.0);
    changed
}

/// A fully saturated colour for a hue in degrees.
pub fn hue_colour(hue: f32) -> Color32 {
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    Color32::from_rgb((r * 235.0) as u8, (g * 235.0) as u8, (b * 235.0) as u8)
}

/// A colour to show for a Point Color pick: hue in degrees, saturation and
/// brightness 0 … 1.
pub fn swatch_colour(hue: f32, saturation: f32, brightness: f32) -> Color32 {
    let pure = hue_colour(hue);
    let s = saturation.clamp(0.0, 1.0);
    // Mix the pure hue towards white by the saturation, then scale to the brightness.
    let rgb = [pure.r(), pure.g(), pure.b()].map(|c| 1.0 + (c as f32 / 235.0 - 1.0) * s);
    let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let k = (brightness.clamp(0.02, 1.0) / luma.max(1e-3)).min(1.0 / rgb[0].max(rgb[1]).max(rgb[2]));
    let [r, g, b] = rgb.map(|c| (c * k * 255.0).round().clamp(0.0, 255.0) as u8);
    Color32::from_rgb(r, g, b)
}

fn strip(ui: &mut egui::Ui, width: f32, colours: &[Color32]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 3.0), Sense::hover());
    let mut mesh = egui::Mesh::default();
    let n = colours.len() - 1;
    for (i, c) in colours.iter().enumerate() {
        let x = rect.left() + rect.width() * i as f32 / n as f32;
        mesh.colored_vertex(Pos2::new(x, rect.top()), *c);
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), *c);
    }
    for i in 0..n as u32 {
        mesh.add_triangle(2 * i, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(2 * i, 2 * i + 3, 2 * i + 1);
    }
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// A collapsible top-level section ("Light", "Color", …).
pub fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    let open = ONLY_SECTIONS.get().map(|only| only.iter().any(|name| name.eq_ignore_ascii_case(title))).unwrap_or(open);
    ui.add_space(2.0);
    egui::CollapsingHeader::new(RichText::new(title).strong().size(14.0))
        .id_salt(("section", title))
        .default_open(open)
        .show_unindented(ui, |ui| {
            ui.add_space(2.0);
            body(ui);
        });
    ui.add_space(2.0);
    ui.separator();
}

/// A collapsible group inside a section ("Curve", "Color Mixer", …). `marked`
/// shows a dot when the group holds a change, so it is visible while closed.
pub fn subsection(ui: &mut egui::Ui, title: &str, marked: bool, body: impl FnOnce(&mut egui::Ui)) {
    let text = if marked { format!("{title}  •") } else { title.to_string() };
    egui::CollapsingHeader::new(RichText::new(text).color(theme::TEXT))
        .id_salt(("sub", title))
        .default_open(match ONLY_SECTIONS.get() {
            // `--only` can name groups as well as sections.
            Some(only) => only.iter().any(|name| name.eq_ignore_ascii_case(title)),
            None => EXPAND_ALL.load(std::sync::atomic::Ordering::Relaxed),
        })
        .show_unindented(ui, |ui| {
            ui.add_space(2.0);
            body(ui);
        });
}

pub fn heading(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).color(theme::MUTED).small().strong());
    ui.add_space(2.0);
}

pub fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
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

/// Point-curve editor. Click to add a point, drag to move it, double-click or
/// right-click a point to remove it. An empty list is a straight line.
/// Returns true if the curve changed.
pub fn curve_editor(ui: &mut egui::Ui, points: &mut Vec<[f32; 2]>, colour: Color32) -> bool {
    let width = ui.available_width();
    let size = Vec2::new(width, width.min(230.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), Color32::from_gray(22));
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let grid = Stroke::new(1.0, Color32::from_gray(44));
        let x = rect.left() + rect.width() * t;
        let y = rect.top() + rect.height() * t;
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_gray(60)));

    let mut pts: Vec<[f32; 2]> = if points.len() >= 2 { points.clone() } else { vec![[0.0, 0.0], [1.0, 1.0]] };
    let to_screen = |p: [f32; 2]| Pos2::new(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height());
    let from_screen = |pos: Pos2| -> [f32; 2] {
        [((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0)]
    };
    let nearest = |pts: &[[f32; 2]], pos: Pos2| -> Option<usize> {
        let (i, d) = pts
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_screen(*p).distance(pos)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
        (d <= 12.0).then_some(i)
    };

    let drag_id = resp.id.with("dragged-point");
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let is_end = |i: usize, n: usize| i == 0 || i + 1 == n;
        if resp.double_clicked() || resp.secondary_clicked() {
            if let Some(i) = nearest(&pts, pos) {
                if !is_end(i, pts.len()) {
                    pts.remove(i);
                    changed = true;
                }
            }
            ui.data_mut(|d| d.remove::<usize>(drag_id));
        } else if resp.drag_started() {
            // A drag only "starts" once the mouse has moved a little, so look
            // at where the button went down, not where the pointer is now.
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let index = match nearest(&pts, origin) {
                Some(i) => i,
                None => {
                    let p = from_screen(origin);
                    let at = pts.partition_point(|q| q[0] < p[0]).clamp(1, pts.len() - 1);
                    pts.insert(at, p);
                    changed = true;
                    at
                }
            };
            ui.data_mut(|d| d.insert_temp(drag_id, index));
        } else if resp.dragged() {
            if let Some(i) = ui.data(|d| d.get_temp::<usize>(drag_id)).filter(|i| *i < pts.len()) {
                let mut p = from_screen(pos);
                // End points slide up and down their edge; the others stay between their neighbours.
                if i == 0 {
                    p[0] = 0.0;
                } else if i + 1 == pts.len() {
                    p[0] = 1.0;
                } else {
                    p[0] = p[0].clamp(pts[i - 1][0] + 0.01, pts[i + 1][0] - 0.01);
                }
                if pts[i] != p {
                    pts[i] = p;
                    changed = true;
                }
            }
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }

    if let Some(table) = curve_table(&pts, 96) {
        let line: Vec<Pos2> = table.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 95.0, *y])).collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.6, colour)));
    }
    for p in &pts {
        painter.circle_filled(to_screen(*p), 4.0, colour);
        painter.circle_stroke(to_screen(*p), 4.0, Stroke::new(1.0, Color32::from_gray(15)));
    }
    if changed {
        let straight = pts.len() == 2 && pts[0] == [0.0, 0.0] && pts[1] == [1.0, 1.0];
        *points = if straight { Vec::new() } else { pts };
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, RawInput, Rect};

    /// Drives the curve editor with pretend mouse input, one frame per call.
    struct Rig {
        ctx: egui::Context,
        time: f64,
        points: Vec<[f32; 2]>,
        rect: Rect,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Rig { ctx: egui::Context::default(), time: 0.0, points: Vec::new(), rect: Rect::NOTHING };
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<Event>) -> bool {
            self.time += 0.05;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut changed = false;
            let (points, rect) = (&mut self.points, &mut self.rect);
            let _ = self.ctx.run_ui(input, |ui| {
                changed = curve_editor(ui, points, Color32::WHITE);
                *rect = ui.min_rect();
            });
            changed
        }

        /// Screen position of a curve coordinate.
        fn at(&self, x: f32, y: f32) -> Pos2 {
            Pos2::new(self.rect.left() + x * self.rect.width(), self.rect.bottom() - y * self.rect.height())
        }

        fn button(&mut self, pos: Pos2, pressed: bool) -> bool {
            self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }])
        }
    }

    #[test]
    fn dragging_adds_and_moves_a_point_and_a_double_click_removes_it() {
        let mut rig = Rig::new();
        assert!(rig.rect.width() > 100.0 && rig.rect.height() > 100.0, "{:?}", rig.rect);

        // Press well away from the line and drag: a new point follows the mouse.
        let (start, end) = (rig.at(0.5, 0.2), rig.at(0.6, 0.35));
        rig.frame(vec![Event::PointerMoved(start)]);
        rig.button(start, true);
        rig.frame(vec![Event::PointerMoved(rig.at(0.55, 0.3))]);
        rig.frame(vec![Event::PointerMoved(end)]);
        rig.button(end, false);
        assert_eq!(rig.points.len(), 3, "{:?}", rig.points);
        let p = rig.points[1];
        assert!((p[0] - 0.6).abs() < 0.02 && (p[1] - 0.35).abs() < 0.02, "{p:?}");
        assert_eq!((rig.points[0], rig.points[2]), ([0.0, 0.0], [1.0, 1.0]));

        // Grab the same point again and move it: no extra point appears.
        let again = rig.at(0.3, 0.5);
        rig.button(end, true);
        rig.frame(vec![Event::PointerMoved(rig.at(0.45, 0.42))]);
        rig.frame(vec![Event::PointerMoved(again)]);
        rig.button(again, false);
        assert_eq!(rig.points.len(), 3);
        assert!((rig.points[1][0] - 0.3).abs() < 0.02 && (rig.points[1][1] - 0.5).abs() < 0.02, "{:?}", rig.points);

        // Double-click the point: it goes, and the curve is a straight line again.
        for _ in 0..2 {
            rig.button(again, true);
            rig.button(again, false);
        }
        assert!(rig.points.is_empty(), "{:?}", rig.points);
    }

    #[test]
    fn end_points_only_slide_along_their_edge() {
        let mut rig = Rig::new();
        let (corner, target) = (rig.at(0.0, 0.0), rig.at(0.3, 0.25));
        rig.frame(vec![Event::PointerMoved(corner)]);
        rig.button(corner, true);
        rig.frame(vec![Event::PointerMoved(rig.at(0.1, 0.1))]);
        rig.frame(vec![Event::PointerMoved(target)]);
        rig.button(target, false);
        assert_eq!(rig.points.len(), 2, "{:?}", rig.points);
        assert_eq!(rig.points[0][0], 0.0);
        assert!((rig.points[0][1] - 0.25).abs() < 0.02, "{:?}", rig.points);
    }

    #[test]
    fn slider_values_are_formatted_like_lightroom() {
        assert_eq!(format_value(12.0, &SliderSpec::percent("x")), "+12");
        assert_eq!(format_value(-0.2, &SliderSpec::percent("x")), "+0");
        assert_eq!(format_value(25.0, &SliderSpec::amount("x", 0.0)), "25");
        let ev = SliderSpec { decimals: 2, suffix: " EV", ..SliderSpec::percent("x") };
        assert_eq!(format_value(-1.5, &ev), "-1.50 EV");
    }
}
