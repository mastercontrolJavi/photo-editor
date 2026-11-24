//! The picture area: the photo itself, and the crop overlay with its handles.

use eframe::egui::{self, Color32, CornerRadius, CursorIcon, Key, PointerButton, Pos2, Rect, Response, Sense, Stroke, TextureId, Vec2};
use pipeline::adjust::{PointColor, MAX_POINT_COLORS};
use pipeline::Crop;

use crate::app::{file_name, EditorApp, Tab, TEX_OPTIONS};
use crate::engine::{Region, PREVIEW_EDGES};
use crate::theme;
use crate::tools::{heal_tool, mask_tool, PhotoMap};

/// A crop edit in progress.
pub(crate) struct CropDrag {
    /// Which sides are being dragged: −1 = left/top, +1 = right/bottom, 0 = neither.
    /// `(0, 0)` moves the whole crop.
    handle: (i8, i8),
    /// The crop when the drag started.
    start: Crop,
    origin: Pos2,
}

/// Draw a textured four-cornered shape. Corners and UVs go top-left,
/// top-right, bottom-right, bottom-left.
fn draw_quad(painter: &egui::Painter, texture: TextureId, corners: [Pos2; 4], uvs: [[f32; 2]; 4]) {
    let mut mesh = egui::Mesh::with_texture(texture);
    for (pos, uv) in corners.iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos: *pos, uv: Pos2::new(uv[0], uv[1]), color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

impl EditorApp {
    pub(crate) fn canvas(&mut self, ui: &mut egui::Ui) {
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
            if self.doc.is_none() {
                self.empty_state(ui, area);
                return;
            }
            let (shown_long_edge, region) = if self.tab == Tab::Crop && !self.show_before {
                (self.crop_canvas(ui, area), None)
            } else {
                self.photo_canvas(ui, area)
            };
            self.pick_preview_size(shown_long_edge * ui.ctx().pixels_per_point(), region);
        });
    }

    /// Render the smallest preview that still fills the space the photo is
    /// shown in, or, when zoomed in past the largest preview, the visible part
    /// from the full-size photo.
    fn pick_preview_size(&mut self, shown_long_edge_px: f32, region: Option<Region>) {
        let Some(doc) = self.doc.as_mut() else { return };
        if region != doc.region {
            doc.region = region;
            if region.is_none() {
                doc.region_tex = None;
            }
            self.adjustments_changed();
        }
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.region.is_some() {
            return;
        }
        // A little slack, so a preview a few pixels short of the space isn't swapped for one twice the size.
        let level = (0..doc.previews.len())
            .rev()
            .find(|&i| PREVIEW_EDGES[i] as f32 >= shown_long_edge_px * 0.95)
            .unwrap_or(0);
        if level != doc.level {
            doc.level = level;
            self.adjustments_changed();
        }
    }

    /// The finished (cropped) picture: fitted to the area, or zoomed in and
    /// pannable. Returns the long edge, in screen points, that the *whole*
    /// photo has at this scale, and the part to render sharp if it is zoomed
    /// in further than the largest preview can show.
    fn photo_canvas(&mut self, ui: &mut egui::Ui, area: Rect) -> (f32, Option<Region>) {
        let ppp = ui.ctx().pixels_per_point();
        let doc = self.doc.as_ref().expect("checked by the caller");
        let full = doc.full_size();
        let (sw, sh) = (full.0 as f64, full.1 as f64);
        let crop = doc.adjustments.crop;
        let (ow, oh) = crop.output_size_f(sw, sh);
        let (ow, oh) = (ow as f32, oh as f32);
        let inner = area.shrink(20.0);
        // Scales are in screen points per photo pixel.
        let fit = (inner.width() / ow).min(inner.height() / oh);
        let most = (8.0 / ppp).max(fit);
        let mut scale = self.zoom.scale.map(|s| s / ppp).unwrap_or(fit).clamp(fit, most);
        let mut centre = self.zoom.centre;
        // Where a point of the picture (0…1) is on screen, and back.
        let place = |scale: f32, centre: (f32, f32), u: f32, v: f32| {
            inner.center() + Vec2::new((u - centre.0) * ow, (v - centre.1) * oh) * scale
        };
        let unplace = |scale: f32, centre: (f32, f32), pos: Pos2| {
            let d = (pos - inner.center()) / scale;
            (centre.0 + d.x / ow, centre.1 + d.y / oh)
        };
        // Change the zoom, keeping the picture under `pos` where it is.
        let zoom_at = |scale: &mut f32, centre: &mut (f32, f32), pos: Pos2, new: f32| {
            let under = unplace(*scale, *centre, pos);
            *scale = new.clamp(fit, most);
            let d = (pos - inner.center()) / *scale;
            *centre = (under.0 - d.x / ow, under.1 - d.y / oh);
        };

        // A tool that works on the photo takes the mouse; holding Space gives it back for moving around.
        let tool_active = !self.show_before && (self.picking_colour || matches!(self.tab, Tab::Mask | Tab::Heal));
        let space = ui.input(|i| i.key_down(Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        let resp = ui.interact(area, ui.scope_id().with("photo"), Sense::click_and_drag());
        if let Some(pos) = resp.hover_pos() {
            // A trackpad scrolls in screen points and a mouse wheel in lines: that is how the two are told apart.
            let (pinch, scroll, trackpad) = ui.input(|i| {
                let mut trackpad = None;
                for event in &i.events {
                    if let egui::Event::MouseWheel { unit, source, .. } = event {
                        let fingers = matches!(source, egui::MouseWheelSource::Trackpad | egui::MouseWheelSource::Momentum);
                        trackpad = Some(fingers || *unit == egui::MouseWheelUnit::Point);
                    }
                }
                (i.zoom_delta(), i.smooth_scroll_delta, trackpad)
            });
            if let Some(trackpad) = trackpad {
                self.zoom.trackpad = trackpad;
            }
            // Pinching (or Ctrl + scrolling) zooms.
            if pinch != 1.0 {
                let new = scale * pinch;
                zoom_at(&mut scale, &mut centre, pos, new);
            }
            if scroll != Vec2::ZERO {
                if self.zoom.trackpad {
                    // Two fingers on a trackpad slide the picture around.
                    centre = (centre.0 - scroll.x / (scale * ow), centre.1 - scroll.y / (scale * oh));
                } else {
                    let new = scale * (scroll.y * 0.004).exp();
                    zoom_at(&mut scale, &mut centre, pos, new);
                }
            }
        }
        if resp.double_clicked() && !tool_active {
            let pos = resp.interact_pointer_pos().unwrap_or(inner.center());
            let new = if scale > fit * 1.01 { fit } else { 1.0 / ppp };
            zoom_at(&mut scale, &mut centre, pos, new);
        }
        let zoomed = scale > fit * 1.001;
        let hand = !tool_active || space;
        if zoomed {
            if (hand && resp.dragged_by(PointerButton::Primary)) || resp.dragged_by(PointerButton::Middle) {
                let d = resp.drag_delta() / scale;
                centre = (centre.0 - d.x / ow, centre.1 - d.y / oh);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else if hand && resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        }
        // Keep the picture over the area: centred if it is smaller, never dragged off if larger.
        let half = (inner.width() / (2.0 * ow * scale), inner.height() / (2.0 * oh * scale));
        centre.0 = if half.0 >= 0.5 { 0.5 } else { centre.0.clamp(half.0, 1.0 - half.0) };
        centre.1 = if half.1 >= 0.5 { 0.5 } else { centre.1.clamp(half.1, 1.0 - half.1) };
        self.zoom.scale = zoomed.then_some(scale * ppp);
        self.zoom.centre = centre;
        self.zoom.fit = fit * ppp;
        self.zoom.most = most * ppp;

        let rect = Rect::from_min_max(place(scale, centre, 0.0, 0.0), place(scale, centre, 1.0, 1.0));
        let painter = ui.painter_at(area);
        let overlay = self.mask_overlay_texture(ui.ctx());
        let doc = self.doc.as_ref().expect("checked by the caller");
        // The stored Before picture has no Geometry in it; with a warp the Before view is rendered like any other.
        let stored_before = self.show_before && doc.adjustments.geometry.is_identity();
        let tex = if stored_before { &doc.before_tex } else { &doc.after_tex };
        let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let uvs = doc.adjustments.crop.corners_uv(sw, sh);
        draw_quad(&painter, tex.id(), corners, uvs);

        // Zoomed in past what the largest preview can show sharply: ask for the
        // visible part from the full-size photo, and draw the last one received on top.
        let whole_long_edge = (sw.max(sh) as f32) * scale;
        let mut region = None;
        if whole_long_edge * ppp > PREVIEW_EDGES[0] as f32 * 1.05 {
            let visible = rect.intersect(area);
            if visible.width() > 1.0 && visible.height() > 1.0 {
                let (u0, v0) = unplace(scale, centre, visible.min);
                let (u1, v1) = unplace(scale, centre, visible.max);
                // Rounded a little, so tiny pointer jitter doesn't ask for a new render every frame.
                let round = |v: f32| (v as f64 * 4096.0).round() / 4096.0;
                region = Some(Region {
                    unit: [round(u0).max(0.0), round(v0).max(0.0), round(u1).min(1.0), round(v1).min(1.0)],
                    out_w: ((visible.width() * ppp).round() as usize).max(1),
                    out_h: ((visible.height() * ppp).round() as usize).max(1),
                });
            }
            if let Some((sharp, part)) = &doc.region_tex {
                let at = Rect::from_min_max(
                    place(scale, centre, part.unit[0] as f32, part.unit[1] as f32),
                    place(scale, centre, part.unit[2] as f32, part.unit[3] as f32),
                );
                painter.image(sharp.id(), at, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
        if let Some(overlay) = overlay {
            // Where the selected mask applies, in red.
            draw_quad(&painter.with_clip_rect(rect.intersect(area)), overlay, corners, uvs);
        }
        if self.show_before {
            let badge = Rect::from_min_size(rect.min.max(area.min) + Vec2::new(10.0, 10.0), Vec2::new(66.0, 22.0));
            painter.rect_filled(badge, CornerRadius::same(4), Color32::from_black_alpha(170));
            painter.text(badge.center(), egui::Align2::CENTER_CENTER, "BEFORE", egui::FontId::proportional(12.0), Color32::WHITE);
        }
        if tool_active && !space {
            let map = PhotoMap::new(inner.center(), scale, centre, &doc.adjustments, full);
            self.photo_tools(ui, &resp, &painter.with_clip_rect(area), &map);
        }
        (whole_long_edge, region)
    }

    /// The red picture of where the selected mask applies, remade when the mask changes.
    fn mask_overlay_texture(&mut self, ctx: &egui::Context) -> Option<TextureId> {
        if self.tab != Tab::Mask || !self.mask_ui.show_overlay || self.show_before {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let mask = doc.adjustments.masks.get(self.mask_ui.selected?)?;
        let key = (mask.shape.clone(), mask.invert, doc.adjustments.geometry);
        if self.mask_overlay.as_ref().map(|(k, _)| k) != Some(&key) {
            let (w, h, strength) = pipeline::mask::overlay(mask, &doc.adjustments.geometry, doc.full_size(), 384);
            let mut rgba = Vec::with_capacity(strength.len() * 4);
            for s in strength {
                rgba.extend_from_slice(&[235, 45, 45, (s as u16 * 96 / 255) as u8]);
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let texture = ctx.load_texture("mask-overlay", image, TEX_OPTIONS);
            self.mask_overlay = Some((key, texture));
        }
        self.mask_overlay.as_ref().map(|(_, t)| t.id())
    }

    /// Whatever tool is working on the photo: the colour picker, a mask, or healing.
    fn photo_tools(&mut self, ui: &egui::Ui, resp: &Response, painter: &egui::Painter, map: &PhotoMap) {
        let Some(doc) = self.doc.as_mut() else { return };
        let mut changed = false;
        if self.picking_colour {
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            if let (true, Some(pos)) = (resp.clicked_by(PointerButton::Primary), resp.interact_pointer_pos()) {
                let p = map.to_photo(pos);
                let inside = (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]);
                if inside && doc.adjustments.point_colors.len() < MAX_POINT_COLORS {
                    let look = doc.decoded.info.look();
                    let [hue, saturation, luminance] =
                        pipeline::render::pick_colour(&doc.previews[0], &doc.adjustments, &look, doc.full_size(), p[0], p[1]);
                    doc.adjustments.point_colors.push(PointColor { hue, saturation, luminance, ..PointColor::default() });
                    self.point_selected = doc.adjustments.point_colors.len() - 1;
                    self.picking_colour = false;
                    changed = true;
                }
            }
        } else if self.tab == Tab::Mask {
            changed = mask_tool(ui, resp, painter, map, &mut doc.adjustments.masks, &mut self.mask_ui);
        } else if self.tab == Tab::Heal {
            // The source is chosen on the largest preview: quick, and plenty for judging surroundings.
            let preview = doc.previews[0].clone();
            changed = heal_tool(ui, resp, painter, map, &mut doc.adjustments.spots, &mut self.heal_ui, |at, radius| {
                pipeline::heal::suggest_source(&preview, at, radius)
            });
        }
        if changed {
            self.adjustments_changed();
        }
    }

    /// Crop mode: the whole photo with the crop rectangle on top.
    fn crop_canvas(&mut self, ui: &mut egui::Ui, area: Rect) -> f32 {
        let locked = self.locked_ratio();
        let doc = self.doc.as_mut().expect("checked by the caller");
        let (sw, sh) = doc.full_size();
        let texture = doc.after_tex.id();
        let (changed, long_edge) =
            crop_overlay(ui, area, texture, &mut doc.adjustments.crop, &mut self.crop_ui.drag, locked, (sw as f64, sh as f64));
        if changed {
            self.adjustments_changed();
        }
        long_edge
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, area: Rect) {
        let box_rect = Rect::from_center_size(area.center(), Vec2::new(360.0, 150.0));
        ui.painter().rect_stroke(box_rect, CornerRadius::same(10), Stroke::new(1.0, theme::PANEL_EDGE), egui::StrokeKind::Inside);
        ui.scope_builder(egui::UiBuilder::new().max_rect(box_rect.shrink(16.0)), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("No photo open").size(18.0));
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Open a RAW, JPEG, PNG or HEIC photo, or drop one here.").color(theme::MUTED));
                ui.add_space(12.0);
                if ui.button("Open photo…").clicked() {
                    self.request_open_dialog();
                }
            });
        });
    }
}

/// Draw the whole photo with the crop rectangle on top and let the mouse
/// change it. The photo turns under an upright rectangle as it is
/// straightened. Returns whether the crop changed, and the long edge of the
/// whole photo on screen (in points).
fn crop_overlay(
    ui: &mut egui::Ui,
    area: Rect,
    texture: TextureId,
    crop_slot: &mut Crop,
    drag_slot: &mut Option<CropDrag>,
    locked: Option<f64>,
    (sw, sh): (f64, f64),
) -> (bool, f32) {
    let crop = *crop_slot;
    let (fw, fh) = crop.frame_size(sw, sh);
    let (sin, cos) = (crop.angle as f64).to_radians().sin_cos();
    // Fit the turned photo's bounding box into the area.
    let inner = area.shrink(36.0);
    let (bw, bh) = (cos.abs() * fw + sin.abs() * fh, sin.abs() * fw + cos.abs() * fh);
    let scale = (inner.width() as f64 / bw).min(inner.height() as f64 / bh);
    let centre = inner.center();
    let to_screen = |fx: f64, fy: f64| -> Pos2 {
        let (dx, dy) = (fx - fw / 2.0, fy - fh / 2.0);
        Pos2::new(centre.x + (scale * (cos * dx - sin * dy)) as f32, centre.y + (scale * (sin * dx + cos * dy)) as f32)
    };

    let painter = ui.painter_at(area);
    let frame_corners = [(0.0, 0.0), (fw, 0.0), (fw, fh), (0.0, fh)];
    let corners = frame_corners.map(|(x, y)| to_screen(x, y));
    let uvs = frame_corners.map(|(x, y)| {
        let (sx, sy) = crop.frame_to_source(x, y, sw, sh);
        [(sx / sw) as f32, (sy / sh) as f32]
    });
    draw_quad(&painter, texture, corners, uvs);

    // The crop rectangle, upright on screen.
    let (ow, oh) = crop.output_size_f(sw, sh);
    let rect = Rect::from_center_size(
        to_screen(crop.cx as f64 * fw, crop.cy as f64 * fh),
        Vec2::new((ow * scale) as f32, (oh * scale) as f32),
    );
    let shade = Color32::from_black_alpha(150);
    for dim in [
        Rect::from_min_max(area.min, Pos2::new(area.max.x, rect.min.y)),
        Rect::from_min_max(Pos2::new(area.min.x, rect.max.y), area.max),
        Rect::from_min_max(Pos2::new(area.min.x, rect.min.y), Pos2::new(rect.min.x, rect.max.y)),
        Rect::from_min_max(Pos2::new(rect.max.x, rect.min.y), Pos2::new(area.max.x, rect.max.y)),
    ] {
        painter.rect_filled(dim, CornerRadius::ZERO, shade);
    }
    let thin = Stroke::new(1.0, Color32::from_white_alpha(90));
    for i in 1..3 {
        let t = i as f32 / 3.0;
        let (x, y) = (rect.left() + rect.width() * t, rect.top() + rect.height() * t);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], thin);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], thin);
    }
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
    let bar = Stroke::new(3.0, Color32::WHITE);
    let len = 18.0f32.min(rect.width() / 3.0).min(rect.height() / 3.0);
    for (corner, sx, sy) in [
        (rect.left_top(), 1.0, 1.0),
        (rect.right_top(), -1.0, 1.0),
        (rect.right_bottom(), -1.0, -1.0),
        (rect.left_bottom(), 1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + Vec2::new(len * sx, 0.0)], bar);
        painter.line_segment([corner, corner + Vec2::new(0.0, len * sy)], bar);
    }

    // Which part of the rectangle is under the pointer?
    let grab = 12.0;
    let handle_at = |pos: Pos2| -> Option<(i8, i8)> {
        if !rect.expand(grab).contains(pos) {
            return None;
        }
        let hx = if (pos.x - rect.left()).abs() <= grab {
            -1
        } else if (pos.x - rect.right()).abs() <= grab {
            1
        } else {
            0
        };
        let hy = if (pos.y - rect.top()).abs() <= grab {
            -1
        } else if (pos.y - rect.bottom()).abs() <= grab {
            1
        } else {
            0
        };
        Some((hx, hy))
    };
    let resp = ui.interact(area, ui.scope_id().with("crop-canvas"), Sense::click_and_drag());
    let hover = drag_slot.as_ref().map(|d| d.handle).or_else(|| resp.hover_pos().and_then(handle_at));
    if let Some(handle) = hover {
        ui.ctx().set_cursor_icon(match handle {
            (0, 0) => CursorIcon::Move,
            (_, 0) => CursorIcon::ResizeHorizontal,
            (0, _) => CursorIcon::ResizeVertical,
            (a, b) if a == b => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        });
    }
    if resp.drag_started() {
        // A drag only "starts" once the mouse has moved a little, so use
        // the place where the button went down to decide what was grabbed.
        let pressed_at = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        if let Some(pos) = pressed_at {
            *drag_slot = handle_at(pos).map(|handle| CropDrag { handle, start: crop, origin: pos });
        }
    }
    let mut changed = false;
    if let (Some(drag), Some(pos)) = (drag_slot.as_ref(), resp.interact_pointer_pos()) {
        if resp.dragged() {
            // Pointer movement in photo pixels, along the crop's own axes.
            let delta = ((pos.x - drag.origin.x) as f64 / scale, (pos.y - drag.origin.y) as f64 / scale);
            let attempt = |t: f64| moved_crop(&drag.start, drag.handle, (delta.0 * t, delta.1 * t), locked, sw, sh);
            // Go as far towards the pointer as stays inside the photo.
            let mut best = attempt(1.0).filter(|c| c.is_inside(sw, sh));
            if best.is_none() {
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..12 {
                    let mid = (lo + hi) / 2.0;
                    match attempt(mid).filter(|c| c.is_inside(sw, sh)) {
                        Some(c) => {
                            best = Some(c);
                            lo = mid;
                        }
                        None => hi = mid,
                    }
                }
            }
            if let Some(new) = best {
                if new != *crop_slot {
                    *crop_slot = new;
                    changed = true;
                }
            }
        }
    }
    if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
        *drag_slot = None;
    }
    (changed, (fw.max(fh) * scale) as f32)
}

/// The crop after dragging `handle` by `delta` (photo pixels along the crop's
/// own axes), starting from `start`. `locked` holds the shape to a ratio.
/// Returns `None` if the result would be too small.
fn moved_crop(start: &Crop, handle: (i8, i8), delta: (f64, f64), locked: Option<f64>, sw: f64, sh: f64) -> Option<Crop> {
    let (fw, fh) = start.frame_size(sw, sh);
    let (ow, oh) = start.output_size_f(sw, sh);
    let (hx, hy) = (handle.0 as f64, handle.1 as f64);
    let (new_w, new_h, shift) = if handle == (0, 0) {
        (ow, oh, delta)
    } else {
        let mut w = ow + hx * delta.0;
        let mut h = oh + hy * delta.1;
        if let Some(ratio) = locked {
            if handle.0 != 0 && handle.1 != 0 {
                // A corner: follow whichever direction was dragged further.
                let k = if (w / ow - 1.0).abs() > (h / oh - 1.0).abs() { w / ow } else { h / oh };
                w = ow * k;
                h = w / ratio;
            } else if handle.0 != 0 {
                h = w / ratio;
            } else {
                w = h * ratio;
            }
        }
        // The side opposite the handle stays where it is.
        (w, h, (hx * (w - ow) / 2.0, hy * (h - oh) / 2.0))
    };
    let min_side = 0.03 * fw.min(fh);
    if new_w < min_side || new_h < min_side {
        return None;
    }
    // The crop's axes are turned by −angle relative to the photo's.
    let (sin, cos) = (-(start.angle as f64)).to_radians().sin_cos();
    let cx = start.cx as f64 * fw + cos * shift.0 - sin * shift.1;
    let cy = start.cy as f64 * fh + sin * shift.0 + cos * shift.1;
    Some(Crop { cx: (cx / fw) as f32, cy: (cy / fh) as f32, w: (new_w / fw) as f32, h: (new_h / fh) as f32, ..*start })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f64 = 4000.0;
    const H: f64 = 3000.0;

    fn px(c: &Crop) -> (f64, f64, f64, f64) {
        let (ow, oh) = c.output_size_f(W, H);
        (c.cx as f64 * W, c.cy as f64 * H, ow, oh)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.5
    }

    /// Drive the crop overlay with pretend mouse input: press at `from`, drag
    /// to `to`, release. Both are fractions of the crop rectangle as first drawn.
    fn drag_on_screen(crop: &mut Crop, locked: Option<f64>, from: (f32, f32), to: (f32, f32)) {
        use eframe::egui::{Event, Modifiers, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(872.0, 672.0));
        let mut drag = None;
        let mut time = 0.0;
        let mut frame = |crop: &mut Crop, events: Vec<Event>| {
            time += 0.05;
            let input = RawInput { screen_rect: Some(area), time: Some(time), events, ..Default::default() };
            let _ = ctx.run_ui(input, |ui| {
                crop_overlay(ui, area, TextureId::default(), crop, &mut drag, locked, (W, H));
            });
        };
        // With no straightening the photo is fitted into the area less a
        // 36-point margin: 800 × 600 with its corner at (36, 36).
        let scale = 800.0 / W as f32;
        let (ow, oh) = crop.output_size_f(W, H);
        let rect = Rect::from_center_size(
            Pos2::new(36.0 + crop.cx * 800.0, 36.0 + crop.cy * 600.0),
            Vec2::new(ow as f32 * scale, oh as f32 * scale),
        );
        let at = |(fx, fy): (f32, f32)| Pos2::new(rect.left() + fx * rect.width(), rect.top() + fy * rect.height());
        let (a, b) = (at(from), at(to));
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(crop, vec![]);
        frame(crop, vec![Event::PointerMoved(a)]);
        frame(crop, vec![button(a, true)]);
        frame(crop, vec![Event::PointerMoved(a + (b - a) * 0.5)]);
        frame(crop, vec![Event::PointerMoved(b)]);
        frame(crop, vec![button(b, false)]);
    }

    #[test]
    fn the_mouse_can_resize_and_move_the_crop() {
        // Drag the bottom-right corner to the middle of the photo.
        let mut crop = Crop::default();
        drag_on_screen(&mut crop, None, (1.0, 1.0), (0.5, 0.5));
        let (cx, cy, ow, oh) = px(&crop);
        assert!(close(ow, 2000.0) && close(oh, 1500.0), "{ow} x {oh}");
        assert!(close(cx, 1000.0) && close(cy, 750.0), "top-left corner should stay put: {cx} {cy}");

        // Drag inside that crop to move it right and down by half its size.
        drag_on_screen(&mut crop, None, (0.5, 0.5), (1.0, 1.0));
        let (cx, cy, ow, oh) = px(&crop);
        assert!(close(ow, 2000.0) && close(oh, 1500.0));
        assert!(close(cx, 2000.0) && close(cy, 1500.0), "{cx} {cy}");

        // Drag the left edge with the shape locked to a square.
        let mut crop = Crop::default();
        crop.set_aspect(1.0, W, H);
        drag_on_screen(&mut crop, Some(1.0), (0.0, 0.5), (0.5, 0.5));
        let (_, _, ow, oh) = px(&crop);
        assert!(close(ow, 1500.0) && close(oh, 1500.0), "{ow} x {oh}");
    }

    #[test]
    fn the_crop_stops_at_the_edge_of_the_photo() {
        // Try to drag a half-size crop far out of the photo: it stops at the corner.
        let mut crop = Crop { w: 0.5, h: 0.5, ..Crop::default() };
        drag_on_screen(&mut crop, None, (0.5, 0.5), (3.0, 3.0));
        assert!(crop.is_inside(W, H));
        let (cx, cy, _, _) = px(&crop);
        assert!((cx - 3000.0).abs() < 5.0 && (cy - 2250.0).abs() < 5.0, "{cx} {cy}");
        // Dragging from outside the rectangle does nothing.
        let before = crop;
        drag_on_screen(&mut crop, None, (-0.5, -0.5), (-0.2, -0.2));
        assert_eq!(crop, before);
    }

    #[test]
    fn dragging_inside_moves_the_crop_without_resizing_it() {
        let start = Crop { w: 0.5, h: 0.5, ..Crop::default() };
        let moved = moved_crop(&start, (0, 0), (300.0, -200.0), None, W, H).unwrap();
        let (cx, cy, ow, oh) = px(&moved);
        assert!(close(cx, 2300.0) && close(cy, 1300.0) && close(ow, 2000.0) && close(oh, 1500.0));
    }

    #[test]
    fn dragging_an_edge_keeps_the_opposite_edge_in_place() {
        let start = Crop::default();
        // Pull the right edge 1000 px to the left.
        let c = moved_crop(&start, (1, 0), (-1000.0, 0.0), None, W, H).unwrap();
        let (cx, _, ow, oh) = px(&c);
        assert!(close(ow, 3000.0) && close(oh, 3000.0));
        assert!(close(cx - ow / 2.0, 0.0), "left edge moved");
        // Pull the top edge 600 px down.
        let c = moved_crop(&start, (0, -1), (0.0, 600.0), None, W, H).unwrap();
        let (_, cy, _, oh) = px(&c);
        assert!(close(oh, 2400.0) && close(cy + oh / 2.0, 3000.0), "bottom edge moved");
    }

    #[test]
    fn a_locked_ratio_is_kept_when_resizing() {
        let start = Crop::default(); // 4 × 3
        let c = moved_crop(&start, (1, 1), (-800.0, -100.0), Some(4.0 / 3.0), W, H).unwrap();
        let (cx, cy, ow, oh) = px(&c);
        assert!(close(ow, 3200.0) && close(oh, 2400.0), "{ow} x {oh}");
        // The top-left corner is the anchor.
        assert!(close(cx - ow / 2.0, 0.0) && close(cy - oh / 2.0, 0.0));
        assert!(c.is_inside(W, H));
        // An edge drag changes the other side to match.
        let c = moved_crop(&start, (1, 0), (-400.0, 0.0), Some(4.0 / 3.0), W, H).unwrap();
        let (_, _, ow, oh) = px(&c);
        assert!(close(ow, 3600.0) && close(oh, 2700.0));
    }

    #[test]
    fn a_crop_cannot_be_dragged_down_to_nothing() {
        assert!(moved_crop(&Crop::default(), (1, 0), (-3990.0, 0.0), None, W, H).is_none());
    }

    #[test]
    fn moving_a_straightened_crop_follows_the_pointer_on_screen() {
        // The picture is straightened by 30°. A drag to the right on screen
        // moves the crop along its own x axis, which is tilted in the photo.
        let start = Crop { angle: 30.0, w: 0.3, h: 0.3, ..Crop::default() };
        let c = moved_crop(&start, (0, 0), (100.0, 0.0), None, W, H).unwrap();
        let (cx, cy, _, _) = px(&c);
        let (dx, dy) = (cx - 2000.0, cy - 1500.0);
        assert!(close((dx * dx + dy * dy).sqrt(), 100.0));
        assert!(dx > 80.0 && dy < -40.0, "{dx} {dy}");
    }
}
