//! The side panel: Edit, Crop, Presets and Info.

use eframe::egui::{self, Color32, Margin, RichText};
use pipeline::adjust::{Wheel, BAND_HUES, BAND_NAMES, DEFRINGE, EXPOSURE_RANGE, PROFILE_AMOUNT, SHARPEN_AMOUNT, SHARPEN_RADIUS};
use pipeline::decode::SourceKind;
use pipeline::geometry::ANGLE_RANGE;
use pipeline::{preset, Crop};

use crate::app::{Aspect, EditorApp, Notice, Tab};
use crate::theme;
use crate::widgets::{curve_editor, heading, hue_colour, info_row, section, slider, subsection, SliderSpec, Track};

const WARM: Color32 = Color32::from_rgb(230, 190, 60);
const COOL: Color32 = Color32::from_rgb(70, 120, 220);
const GREEN: Color32 = Color32::from_rgb(80, 180, 90);
const MAGENTA: Color32 = Color32::from_rgb(200, 80, 200);

impl EditorApp {
    pub(crate) fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("adjust")
            .exact_size(320.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                if self.doc.is_none() {
                    ui.label(RichText::new("Adjustments appear here once a photo is open.").color(theme::MUTED));
                    return;
                }
                ui.horizontal(|ui| {
                    for (tab, label) in [(Tab::Edit, "Edit"), (Tab::Crop, "Crop"), (Tab::Presets, "Presets"), (Tab::Info, "Info")] {
                        ui.selectable_value(&mut self.tab, tab, label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.tab {
                    Tab::Edit => self.edit_tab(ui),
                    Tab::Crop => self.crop_tab(ui),
                    Tab::Presets => self.presets_tab(ui),
                    Tab::Info => self.info_tab(ui),
                });
            });
    }

    // ───────────────────────────────── Edit ─────────────────────────────────

    fn edit_tab(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let (mut curve_channel, mut mixer_tab) = (self.curve_channel, self.mixer_tab);
        let mut auto = false;
        let doc = self.doc.as_mut().expect("the panel is only shown with a photo open");
        let info = &doc.decoded.info;
        let (chromatic, has_lens_data, kind) = (info.chromatic, info.has_lens_opcodes, info.kind);
        let profiles = pipeline::profile::choices(kind, &info.make);
        // What this kind of file is, and what that means for editing it.
        let about = if kind.is_raw() {
            None
        } else {
            Some(format!(
                "{} · {}-bit · already processed. All the sliders work; RAW files leave more room to recover bright and dark areas.",
                info.format, info.bits
            ))
        };
        let fresh = doc.fresh.clone();
        let a = &mut doc.adjustments;

        ui.horizontal(|ui| {
            auto = ui.button("Auto").on_hover_text("Set the Light sliders from the photo's brightness").clicked();
            changed |= ui.toggle_value(&mut a.bw, "B&W").on_hover_text("Black & white").changed();
            let untouched = a.clone().with_crop_of(&fresh) == fresh;
            if ui.add_enabled(!untouched, egui::Button::new("Reset all")).on_hover_text("Back to the photo as shot. The crop is kept.").clicked() {
                *a = fresh.clone().with_crop_of(a);
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Profile").color(theme::MUTED));
            let current = profiles.iter().find(|p| p.id == a.profile.name).unwrap_or(&profiles[0]).label.clone();
            egui::ComboBox::from_id_salt("profile").selected_text(current).width(200.0).show_ui(ui, |ui| {
                for p in &profiles {
                    if p.separator_before {
                        ui.separator();
                    }
                    if ui.selectable_label(a.profile.name == p.id, &p.label).clicked() && a.profile.name != p.id {
                        a.profile.name = p.id.to_string();
                        changed = true;
                    }
                }
            });
        });
        if !a.profile.name.is_empty() {
            let amount = SliderSpec { range: PROFILE_AMOUNT, ..SliderSpec::amount("Amount", 100.0) };
            changed |= slider(ui, &mut a.profile.amount, amount);
        }
        if let Some(about) = &about {
            ui.label(RichText::new(about).color(theme::MUTED).small());
        }
        ui.label(RichText::new("Double-click a slider to reset it.").color(theme::MUTED).small());
        ui.add_space(4.0);

        section(ui, "Light", true, |ui| {
            let exposure = SliderSpec { range: EXPOSURE_RANGE, step: 0.01, decimals: 2, suffix: " EV", ..SliderSpec::percent("Exposure") };
            changed |= slider(ui, &mut a.exposure, exposure);
            changed |= slider(ui, &mut a.contrast, SliderSpec::percent("Contrast"));
            changed |= slider(ui, &mut a.highlights, SliderSpec::percent("Highlights"));
            changed |= slider(ui, &mut a.shadows, SliderSpec::percent("Shadows"));
            changed |= slider(ui, &mut a.whites, SliderSpec::percent("Whites"));
            changed |= slider(ui, &mut a.blacks, SliderSpec::percent("Blacks"));
            subsection(ui, "Curve", !a.curve.is_identity(), |ui| {
                ui.horizontal(|ui| {
                    for (i, name) in ["RGB", "Red", "Green", "Blue"].iter().enumerate() {
                        ui.selectable_value(&mut curve_channel, i, *name);
                    }
                });
                let (points, colour) = match curve_channel {
                    1 => (&mut a.curve.red, Color32::from_rgb(235, 90, 90)),
                    2 => (&mut a.curve.green, Color32::from_rgb(90, 200, 100)),
                    3 => (&mut a.curve.blue, Color32::from_rgb(100, 140, 245)),
                    _ => (&mut a.curve.rgb, Color32::from_gray(225)),
                };
                changed |= curve_editor(ui, points, colour);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Click to add a point. Double-click a point to remove it.").color(theme::MUTED).small());
                });
                if ui.add_enabled(!points.is_empty(), egui::Button::new("Reset this curve").small()).clicked() {
                    points.clear();
                    changed = true;
                }
                ui.add_space(6.0);
                heading(ui, "PARAMETRIC");
                let p = &mut a.curve.parametric;
                changed |= slider(ui, &mut p.highlights, SliderSpec::percent("Highlights"));
                changed |= slider(ui, &mut p.lights, SliderSpec::percent("Lights"));
                changed |= slider(ui, &mut p.darks, SliderSpec::percent("Darks"));
                changed |= slider(ui, &mut p.shadows, SliderSpec::percent("Shadows"));
            });
        });

        section(ui, "Color", true, |ui| {
            changed |= slider(ui, &mut a.temperature, SliderSpec::percent("Temp").track(Track::Blend(COOL, WARM)));
            changed |= slider(ui, &mut a.tint, SliderSpec::percent("Tint").track(Track::Blend(GREEN, MAGENTA)));
            ui.add_enabled_ui(!a.bw, |ui| {
                changed |= slider(ui, &mut a.vibrance, SliderSpec::percent("Vibrance"));
                changed |= slider(ui, &mut a.saturation, SliderSpec::percent("Saturation"));
            });
            if a.bw {
                let marked = a.bw_mix.iter().any(|v| *v != 0.0);
                subsection(ui, "B&W Mixer", marked, |ui| {
                    ui.label(RichText::new("How bright each colour becomes in black & white.").color(theme::MUTED).small());
                    for (i, name) in BAND_NAMES.iter().enumerate() {
                        changed |= slider(ui, &mut a.bw_mix[i], SliderSpec::percent(name).dot(hue_colour(BAND_HUES[i])));
                    }
                });
            } else {
                let marked = a.mixer.iter().any(|b| *b != Default::default());
                subsection(ui, "Color Mixer", marked, |ui| {
                    ui.horizontal(|ui| {
                        for (i, name) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                            ui.selectable_value(&mut mixer_tab, i, *name);
                        }
                    });
                    for (i, name) in BAND_NAMES.iter().enumerate() {
                        let band = &mut a.mixer[i];
                        let value = match mixer_tab {
                            0 => &mut band.hue,
                            1 => &mut band.saturation,
                            _ => &mut band.luminance,
                        };
                        changed |= slider(ui, value, SliderSpec::percent(name).dot(hue_colour(BAND_HUES[i])));
                    }
                });
            }
            let g = &mut a.grading;
            let marked = [g.shadows, g.midtones, g.highlights, g.global].iter().any(|w| w.saturation != 0.0 || w.luminance != 0.0);
            subsection(ui, "Color Grading", marked, |ui| {
                let wheel = |ui: &mut egui::Ui, title: &str, w: &mut Wheel| {
                    ui.horizontal(|ui| {
                        heading(ui, title);
                        if w.saturation > 0.0 {
                            let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.5, hue_colour(w.hue));
                        }
                    });
                    let hue = SliderSpec { range: (0.0, 360.0), suffix: "°", ..SliderSpec::amount("Hue", 0.0) }.track(Track::Rainbow);
                    let mut c = slider(ui, &mut w.hue, hue);
                    c |= slider(ui, &mut w.saturation, SliderSpec::amount("Saturation", 0.0));
                    c |= slider(ui, &mut w.luminance, SliderSpec::percent("Luminance"));
                    ui.add_space(4.0);
                    c
                };
                changed |= wheel(ui, "SHADOWS", &mut g.shadows);
                changed |= wheel(ui, "MIDTONES", &mut g.midtones);
                changed |= wheel(ui, "HIGHLIGHTS", &mut g.highlights);
                changed |= wheel(ui, "GLOBAL", &mut g.global);
                changed |= slider(ui, &mut g.blending, SliderSpec::amount("Blending", 50.0));
                changed |= slider(ui, &mut g.balance, SliderSpec::percent("Balance"));
            });
        });

        section(ui, "Effects", true, |ui| {
            changed |= slider(ui, &mut a.texture, SliderSpec::percent("Texture"));
            changed |= slider(ui, &mut a.clarity, SliderSpec::percent("Clarity"));
            changed |= slider(ui, &mut a.dehaze, SliderSpec::percent("Dehaze"));
            changed |= slider(ui, &mut a.vignette.amount, SliderSpec::percent("Vignette"));
            subsection(ui, "Vignette options", false, |ui| {
                changed |= slider(ui, &mut a.vignette.midpoint, SliderSpec::amount("Midpoint", 50.0));
                changed |= slider(ui, &mut a.vignette.feather, SliderSpec::amount("Feather", 50.0));
                changed |= slider(ui, &mut a.vignette.roundness, SliderSpec::percent("Roundness"));
            });
            changed |= slider(ui, &mut a.grain.amount, SliderSpec::amount("Grain", 0.0));
            subsection(ui, "Grain options", false, |ui| {
                changed |= slider(ui, &mut a.grain.size, SliderSpec::amount("Size", 25.0));
                changed |= slider(ui, &mut a.grain.roughness, SliderSpec::amount("Roughness", 50.0));
            });
        });

        section(ui, "Detail", true, |ui| {
            // Double-click returns to this kind of photo's own default (40 for RAW, 0 for processed photos).
            let sharpening = SliderSpec { range: SHARPEN_AMOUNT, ..SliderSpec::amount("Sharpening", fresh.sharpen.amount) };
            changed |= slider(ui, &mut a.sharpen.amount, sharpening);
            let touched = a.sharpen.radius != 1.0 || a.sharpen.detail != 25.0 || a.sharpen.masking != 0.0;
            subsection(ui, "Sharpening options", touched, |ui| {
                let radius = SliderSpec { range: SHARPEN_RADIUS, step: 0.1, decimals: 1, ..SliderSpec::amount("Radius", 1.0) };
                changed |= slider(ui, &mut a.sharpen.radius, radius);
                changed |= slider(ui, &mut a.sharpen.detail, SliderSpec::amount("Detail", 25.0));
                changed |= slider(ui, &mut a.sharpen.masking, SliderSpec::amount("Masking", 0.0));
            });
            ui.add_space(4.0);
            heading(ui, "NOISE REDUCTION");
            changed |= slider(ui, &mut a.noise.luminance, SliderSpec::amount("Luminance", 0.0));
            changed |= slider(ui, &mut a.noise.color, SliderSpec::amount("Color", 0.0));
            ui.label(
                RichText::new("Sharpening and noise only show properly at 100%. Press Z or double-click the photo to zoom in.")
                    .color(theme::MUTED)
                    .small(),
            );
        });

        section(ui, "Optics", false, |ui| {
            let note = if has_lens_data {
                "This file carries its own lens-correction data, which this app doesn't apply yet."
            } else if kind.is_raw() {
                "The camera already corrected this photo for its lens before saving it. These controls are for touch-ups."
            } else {
                "This photo is already processed, so any lens corrections were made by the camera. These controls are for touch-ups."
            };
            ui.label(RichText::new(note).color(theme::MUTED).small());
            ui.add_space(4.0);
            changed |= ui.checkbox(&mut a.optics.remove_ca, "Remove chromatic aberration").changed();
            if a.optics.remove_ca && chromatic == [0.0; 2] {
                ui.label(
                    RichText::new("No colour misalignment was measured in this photo, so there is nothing to remove.")
                        .color(theme::MUTED)
                        .small(),
                );
            }
            ui.add_space(4.0);
            changed |= slider(ui, &mut a.optics.distortion, SliderSpec::percent("Distortion"));
            changed |= slider(ui, &mut a.optics.vignetting, SliderSpec::percent("Lens vignetting"));
            let fringes = a.optics.defringe_purple != 0.0 || a.optics.defringe_green != 0.0;
            subsection(ui, "Defringe", fringes, |ui| {
                let purple = SliderSpec { range: DEFRINGE, ..SliderSpec::amount("Purple amount", 0.0) }.dot(Color32::from_rgb(170, 90, 220));
                let green = SliderSpec { range: DEFRINGE, ..SliderSpec::amount("Green amount", 0.0) }.dot(Color32::from_rgb(90, 190, 100));
                changed |= slider(ui, &mut a.optics.defringe_purple, purple);
                changed |= slider(ui, &mut a.optics.defringe_green, green);
            });
        });

        if a.lut.is_some() {
            section(ui, "Look (LUT)", true, |ui| {
                let mut remove = false;
                if let Some(l) = &mut a.lut {
                    ui.add(egui::Label::new(RichText::new(l.name.trim_end_matches(".cube")).strong()).truncate());
                    changed |= slider(ui, &mut l.amount, SliderSpec::amount("Amount", 100.0));
                    remove = ui.button("Remove look").clicked();
                }
                if remove {
                    a.lut = None;
                    changed = true;
                }
            });
        }

        ui.add_space(6.0);
        ui.label(
            RichText::new("Not in this version yet: Geometry, Point Color, masking and healing.").color(theme::MUTED).small(),
        );
        if let Some(w) = &self.sidecar_warning {
            ui.label(RichText::new(w).color(theme::WARN).small());
        }
        if auto {
            // Judge the photo as shot, from the smallest preview (it only needs the overall brightness).
            if let Some(small) = doc.previews.last() {
                let unedited =
                    pipeline::render_preview(small, &doc.fresh, &doc.decoded.info.look(), None, doc.full_size());
                doc.adjustments = pipeline::auto::auto_tone(&unedited, &doc.adjustments);
                changed = true;
            }
        }
        self.curve_channel = curve_channel;
        self.mixer_tab = mixer_tab;
        if changed {
            self.adjustments_changed();
        }
    }

    // ───────────────────────────────── Crop ─────────────────────────────────

    /// Width ÷ height the crop is held to, if any.
    pub(crate) fn locked_ratio(&self) -> Option<f64> {
        let doc = self.doc.as_ref()?;
        let (sw, sh) = doc.full_size();
        let (fw, fh) = doc.adjustments.crop.frame_size(sw as f64, sh as f64);
        let ratio = match self.crop_ui.aspect {
            Aspect::Free => return None,
            Aspect::Original => return Some(fw / fh),
            Aspect::Ratio(a, b) => a as f64 / b as f64,
        };
        Some(if self.crop_ui.portrait { 1.0 / ratio } else { ratio })
    }

    fn crop_tab(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let (sw, sh) = self.doc.as_ref().map(|d| d.full_size()).unwrap_or((1, 1));
        let (sw, sh) = (sw as f64, sh as f64);

        heading(ui, "ASPECT");
        let mut aspect_changed = false;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("aspect").selected_text(self.crop_ui.aspect.label()).width(140.0).show_ui(ui, |ui| {
                for choice in Aspect::CHOICES {
                    aspect_changed |= ui.selectable_value(&mut self.crop_ui.aspect, choice, choice.label()).changed();
                }
            });
            let can_turn = matches!(self.crop_ui.aspect, Aspect::Ratio(a, b) if a != b);
            if ui.add_enabled(can_turn, egui::Button::new("Swap tall / wide")).on_hover_text("Use the ratio the other way round").clicked() {
                self.crop_ui.portrait = !self.crop_ui.portrait;
                aspect_changed = true;
            }
        });
        if aspect_changed {
            if let Some(ratio) = self.locked_ratio() {
                let crop = &mut self.doc.as_mut().unwrap().adjustments.crop;
                crop.set_aspect(ratio, sw, sh);
                changed = true;
            }
        }
        ui.add_space(8.0);

        heading(ui, "STRAIGHTEN");
        {
            let ratio = self.locked_ratio();
            let doc = self.doc.as_mut().unwrap();
            let crop = &mut doc.adjustments.crop;
            let before = *crop;
            let mut angle = crop.angle;
            let spec = SliderSpec { range: ANGLE_RANGE, step: 0.1, decimals: 1, suffix: "°", ..SliderSpec::percent("Angle") };
            if slider(ui, &mut angle, spec) {
                // Remember the crop as it was when the slider was grabbed, so
                // sliding back and forth doesn't shrink it further each time.
                let (base, was_largest) = *self.crop_ui.straighten_from.get_or_insert_with(|| {
                    let mut largest = before;
                    largest.set_aspect(before.aspect(sw, sh), sw, sh);
                    (before, before.w >= largest.w * 0.98 && before.h >= largest.h * 0.98)
                });
                *crop = Crop { angle, ..base };
                if was_largest {
                    crop.set_aspect(ratio.unwrap_or(base.aspect(sw, sh)), sw, sh);
                } else {
                    crop.fit(sw, sh);
                }
                changed = true;
            }
            if !ui.input(|i| i.pointer.any_down()) {
                self.crop_ui.straighten_from = None;
            }
        }
        ui.add_space(8.0);

        heading(ui, "ROTATE & FLIP");
        {
            let crop = &mut self.doc.as_mut().unwrap().adjustments.crop;
            ui.horizontal(|ui| {
                if ui.button("Rotate left").clicked() {
                    crop.rotate_ccw();
                    changed = true;
                }
                if ui.button("Rotate right").clicked() {
                    crop.rotate_cw();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Flip horizontal").clicked() {
                    crop.flip_horizontal();
                    changed = true;
                }
                if ui.button("Flip vertical").clicked() {
                    crop.flip_vertical();
                    changed = true;
                }
            });
        }
        ui.add_space(12.0);

        let doc = self.doc.as_mut().unwrap();
        let (w, h) = doc.adjustments.crop.output_size(sw as usize, sh as usize);
        ui.label(RichText::new(format!("Result: {w} × {h} pixels ({:.1} MP)", (w * h) as f64 / 1e6)).small());
        ui.label(RichText::new("Drag the corners or edges on the photo. Drag inside to move the crop.").color(theme::MUTED).small());
        ui.add_space(8.0);
        let mut done = false;
        ui.horizontal(|ui| {
            if ui.add_enabled(!doc.adjustments.crop.is_identity(), egui::Button::new("Reset crop")).clicked() {
                doc.adjustments.crop = Crop::default();
                changed = true;
            }
            done = ui.button("Done").on_hover_text("Enter").clicked();
        });
        if done {
            self.tab = Tab::Edit;
        }
        if changed {
            self.adjustments_changed();
        }
    }

    // ──────────────────────────────── Presets ────────────────────────────────

    fn presets_tab(&mut self, ui: &mut egui::Ui) {
        if ui.button("Import presets…").on_hover_text("Lightroom presets (.xmp, .lrtemplate) and LUTs (.cube)").clicked() {
            self.import_presets();
        }
        ui.add_space(6.0);
        heading(ui, "SAVE THE CURRENT EDIT");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(190.0));
            if ui.add_enabled(!self.preset_name.trim().is_empty(), egui::Button::new("Save")).clicked() {
                let adj = self.doc.as_ref().map(|d| d.adjustments.clone()).unwrap_or_default();
                match preset::save_user_preset(&preset::library_dir(), &self.preset_name, &adj) {
                    Ok(p) => {
                        self.set_status(format!("Saved preset \"{}\"", p.name), false);
                        self.preset_name.clear();
                        self.reload_presets();
                    }
                    Err(e) => self.notice = Some(Notice { title: "Couldn't save the preset".into(), body: e }),
                }
            }
        });
        ui.separator();

        // Groups in the order they first appear: starter presets, then the library.
        let mut groups: Vec<String> = Vec::new();
        for p in &self.presets {
            if !groups.contains(&p.group) {
                groups.push(p.group.clone());
            }
        }
        let mut apply: Option<usize> = None;
        let mut delete: Option<usize> = None;
        for group in &groups {
            egui::CollapsingHeader::new(RichText::new(group).strong())
                .id_salt(("preset-group", group))
                .default_open(true)
                .show_unindented(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for (i, p) in self.presets.iter().enumerate().filter(|(_, p)| &p.group == group) {
                        let label = if p.unsupported.is_empty() { p.name.clone() } else { format!("{}  *", p.name) };
                        let button = egui::Button::new(label).frame(false).wrap_mode(egui::TextWrapMode::Truncate);
                        let mut r = ui.add(button);
                        if !p.unsupported.is_empty() {
                            r = r.on_hover_text(format!("Not applied, because this app doesn't have it yet:\n• {}", p.unsupported.join("\n• ")));
                        }
                        if r.clicked() {
                            apply = Some(i);
                        }
                        if !p.is_builtin() {
                            r.context_menu(|ui| {
                                if ui.button("Delete preset").clicked() {
                                    delete = Some(i);
                                    ui.close();
                                }
                            });
                        }
                    }
                    });
                });
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("Click a preset to apply it. Ctrl+Z undoes it. Right-click an imported preset to delete it. A * means part of the preset can't be applied yet; hover to see what.")
                .color(theme::MUTED)
                .small(),
        );

        if let Some(i) = apply {
            let preset = self.presets[i].clone();
            if let Some(doc) = self.doc.as_mut() {
                doc.adjustments = preset.apply_from(&doc.adjustments, &doc.fresh);
            }
            self.set_status(format!("Applied \"{}\"", preset.name), false);
            self.adjustments_changed();
        }
        if let Some(i) = delete {
            let preset = self.presets[i].clone();
            match preset::delete(&preset, &preset::library_dir()) {
                Ok(()) => self.set_status(format!("Deleted \"{}\"", preset.name), false),
                Err(e) => self.notice = Some(Notice { title: "Couldn't delete the preset".into(), body: e }),
            }
            self.luts.clear();
            self.reload_presets();
        }
    }

    fn import_presets(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .set_title("Import presets")
            .add_filter("Presets and LUTs", &["xmp", "XMP", "lrtemplate", "cube", "CUBE"])
            .pick_files()
        else {
            return;
        };
        let dir = preset::library_dir();
        let (mut ok, mut partial, mut failed) = (0usize, Vec::new(), Vec::new());
        for file in &files {
            match preset::import_file(file, &dir) {
                Ok(p) => {
                    ok += 1;
                    if !p.unsupported.is_empty() {
                        partial.push(format!("{}: {}", p.name, p.unsupported.join(", ")));
                    }
                }
                Err(e) => failed.push(format!("{}: {e}", crate::app::file_name(file))),
            }
        }
        self.luts.clear();
        self.reload_presets();
        self.set_status(format!("Imported {ok} of {} file(s)", files.len()), !failed.is_empty());
        if !partial.is_empty() || !failed.is_empty() {
            let mut body = format!("Imported {ok} of {} file(s).", files.len());
            if !partial.is_empty() {
                body += "\n\nThese presets use things this app doesn't have yet, so they will look different from Lightroom:\n• ";
                body += &partial.join("\n• ");
            }
            if !failed.is_empty() {
                body += "\n\nNot imported:\n• ";
                body += &failed.join("\n• ");
            }
            self.notice = Some(Notice { title: "Preset import".into(), body });
        }
    }

    // ───────────────────────────────── Info ─────────────────────────────────

    fn info_tab(&mut self, ui: &mut egui::Ui) {
        let doc = self.doc.as_mut().expect("the panel is only shown with a photo open");
        heading(ui, "EXPORT");
        let (fw, fh) = doc.full_size();
        let (w, h) = doc.adjustments.crop.output_size(fw, fh);
        ui.label(RichText::new(format!("JPEG · sRGB · quality {} · {w} × {h}", pipeline::export::DEFAULT_QUALITY)).small());
        let dir_text = doc.export_dir.display().to_string();
        ui.add(egui::Label::new(RichText::new(format!("To: {dir_text}")).small()).truncate()).on_hover_text(dir_text);
        if ui.small_button("Change folder…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().set_title("Export to folder").pick_folder() {
                doc.export_dir = dir;
            }
        }
        ui.label(
            RichText::new("Existing files are never replaced: a number is added to the name instead.").color(theme::MUTED).small(),
        );

        ui.add_space(14.0);
        heading(ui, "PHOTO");
        let info = &doc.decoded.info;
        let kind = match info.kind {
            SourceKind::LinearRaw => "RAW · linear DNG (ProRAW type)".to_string(),
            SourceKind::BayerCfa => "RAW · Bayer".to_string(),
            SourceKind::Rendered => format!("{} · {}-bit, already processed", info.format, info.bits),
        };
        let camera = format!("{} {}", info.make, info.model);
        if !camera.trim().is_empty() {
            info_row(ui, "Camera", camera.trim());
        }
        info_row(ui, "Type", &kind);
        info_row(ui, "Size", &format!("{} × {}", info.width, info.height));
        if info.kind.is_raw() {
            info_row(ui, "Compression", info.tags.compression_name());
            info_row(ui, "Baseline exp.", &format!("{:+.2} EV", info.baseline_exposure));
        }
        info_row(ui, "Opened in", &format!("{:.0} ms", doc.decode_ms));
        for note in &info.notes {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("• {note}")).color(theme::MUTED).small());
        }

        ui.add_space(14.0);
        heading(ui, "PRESETS FOLDER");
        let dir = preset::library_dir();
        ui.add(egui::Label::new(RichText::new(dir.display().to_string()).small()).truncate()).on_hover_text(dir.display().to_string());
        if ui.small_button("Open folder").clicked() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        }
    }
}
