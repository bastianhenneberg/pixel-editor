//! Side panels: tools + palette (left), properties of layer, frame/cel,
//! tag, regions and the edit history (right).

use eframe::egui::{self, Color32, RichText, Sense, Stroke, StrokeKind, Vec2};
use pixel_core::document::Direction;
use pixel_core::journal::Journal;
use pixel_core::{Region, Rgba, Tag, TRANSPARENT};

use super::{to_color32, EditorApp, Tool};

#[derive(Default)]
pub struct TagDraft {
    pub(super) tag: Tag,
    /// Which selection (`play_tag`) the draft was loaded from.
    loaded: Option<Option<usize>>,
    /// Arbitrary frame order, e.g. "0 1 0 2".
    pub(super) sequence: String,
}

#[derive(Default)]
pub struct RegionDraft {
    pub(super) name: String,
    this_frame_only: bool,
    /// create a marker (a point at the selection's top-left) instead of an area
    pub(super) marker: bool,
}

impl EditorApp {
    pub(super) fn tools_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Werkzeuge");
        self.toolbar(ui);
        ui.label(RichText::new(format!("{} · {}", self.tool.label(), self.tool.shortcut_label())).strong());
        if matches!(self.tool, Tool::Rect | Tool::Ellipse) {
            ui.checkbox(&mut self.filled, "gefüllt");
        }
        self.dither_panel(ui);
        if matches!(self.tool, Tool::Pencil | Tool::Eraser) {
            ui.checkbox(&mut self.pixel_perfect, "Pixel-Perfect").on_hover_text(
                "Entfernt beim Freihandzeichnen die doppelten Eckpixel (L-Ecken), damit Linien 1 Pixel dünn bleiben",
            );
        }
        if self.tool == Tool::Shade {
            ui.label(
                RichText::new(
                    "Klick/Ziehen = eine Stufe heller entlang der Farbrampe, Rechtsklick = dunkler; \
                     Farben ohne Rampe bleiben, jedes Pixel ändert sich einmal pro Strich",
                )
                .small()
                .weak(),
            );
            if self.doc.ramps.is_empty() {
                ui.label(RichText::new(NO_RAMPS_HELP).small().color(Color32::from_rgb(230, 180, 90)));
            }
        }
        self.symmetry_panel(ui);
        self.tile_panel(ui);
        if self.tool == Tool::Point {
            ui.label(
                RichText::new(
                    "Ziehen = Bereich, Klick = Punkt für die KI markieren; Rechtsklick = Markierung entfernen",
                )
                .small()
                .weak(),
            );
            ui.label(
                RichText::new("In eine Markierung klicken = auswählen, Notiz eintippen; ziehen = verschieben, Ecken/Kanten = Größe")
                    .small()
                    .weak(),
            );
            self.mark_buttons(ui);
        }
        if self.tool.selects() {
            let how = match self.tool {
                Tool::SelectBrush => "Klicken/Ziehen malt Pixel in die Auswahl; Rechtsklick oder Alt nimmt sie heraus",
                Tool::Wand => {
                    "Klick wählt die zusammenhängende Fläche gleicher Farbe; in der Auswahl ziehen = verschieben"
                }
                Tool::Lasso => "Umriss frei ziehen; in der Auswahl ziehen = verschieben",
                _ => "Ziehen = Rechteck auswählen; in der Auswahl ziehen = verschieben",
            };
            ui.label(RichText::new(how).small().weak());
            if self.tool != Tool::SelectBrush {
                ui.label(RichText::new("Umschalt = hinzufügen, Alt/Strg = abziehen").small().weak());
            }
            ui.label(RichText::new("Pfeile verschieben, Strg+C/X/V, Entf, Enter = ablegen, Esc").small().weak());
            if self.tool == Tool::Wand {
                ui.checkbox(&mut self.wand_all, "alle gleichfarbigen (nicht nur zusammenhängend)");
            }
        }
        if self.tool.selects() || self.floating.is_some() {
            ui.horizontal(|ui| {
                ui.label("Ablegen:");
                let before = self.drop_behind;
                ui.selectable_value(&mut self.drop_behind, false, "Vorne")
                    .on_hover_text("Verschobene Pixel überdecken, was dort ist (transparente lassen es stehen)");
                ui.selectable_value(&mut self.drop_behind, true, "Hinten")
                    .on_hover_text("Verschobene Pixel füllen nur leere Stellen: sie landen hinter dem Bestehenden");
                ui.label(RichText::new("Umschalt+H").small().weak());
                if self.drop_behind != before {
                    self.canvas.invalidate();
                }
            });
        }
        if !self.tool.selects() && self.tool != Tool::Shade {
            ui.label(RichText::new("Rechtsklick = transparent, Alt+Klick = Pipette").small().weak());
        }

        ui.separator();
        ui.heading("Palette");
        self.ramps_ui(ui);
        let current = self.color;
        ui.horizontal(|ui| {
            swatch(ui, self.doc.color_of(current), 28.0, true);
            ui.label(self.doc.key_label(current));
        });
        ui.add_space(4.0);
        let (mut picked, mut toggled, mut replace) = (None, None, None);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(3.0);
            let r = swatch(ui, None, 22.0, current == TRANSPARENT).on_hover_text("'.' transparent");
            if r.clicked() {
                picked = Some(TRANSPARENT);
            }
            r.context_menu(|ui| {
                if ui.button("Diese Farbe ersetzen durch …").clicked() {
                    replace = Some(TRANSPARENT);
                    ui.close();
                }
            });
            for p in &self.doc.palette {
                let tip = if p.name.is_empty() {
                    format!("Taste '{}' · {}", p.key as char, p.color)
                } else {
                    format!("Taste '{}' · {} · {}", p.key as char, p.name, p.color)
                };
                let r = swatch(ui, Some(p.color), 22.0, current == p.key).on_hover_text(format!(
                    "{tip}\nStrg+Klick = für eine Rampe markieren\nRechtsklick = Farbe ersetzen"
                ));
                r.context_menu(|ui| {
                    if ui.button("Diese Farbe ersetzen durch …").clicked() {
                        replace = Some(p.key);
                        ui.close();
                    }
                });
                if let Some(n) = self.ramp_pick.iter().position(|k| *k == p.key) {
                    pick_badge(ui, r.rect, n + 1);
                }
                if r.clicked() {
                    if ui.input(|i| i.modifiers.command) {
                        toggled = Some(p.key);
                    } else {
                        picked = Some(p.key);
                    }
                }
            }
        });
        if let Some(k) = picked {
            self.color = k;
        }
        if let Some(k) = replace {
            self.open_replace(k);
        }
        if let Some(k) = toggled {
            match self.ramp_pick.iter().position(|q| *q == k) {
                Some(i) => {
                    self.ramp_pick.remove(i);
                }
                None => self.ramp_pick.push(k),
            }
        }
        self.ramp_pick_ui(ui);
        ui.add_space(4.0);
        if let Some(mut c) = self.doc.color_of(self.color) {
            let key = self.color;
            ui.horizontal(|ui| {
                let lbl = ui.label("Name");
                let mut name = self.doc.palette_entry(key).map(|p| p.name.clone()).unwrap_or_default();
                let r = ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(110.0).hint_text("z. B. crust_dark"))
                    .labelled_by(lbl.id);
                if r.changed() && name.len() != 1 {
                    let desc = format!("Farbe '{}' heißt jetzt {name:?}", key as char);
                    self.edit(desc, |d| {
                        if let Some(p) = d.palette.iter_mut().find(|p| p.key == key) {
                            p.name = name;
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                let lbl = ui.label("Farbe");
                let r = ui.color_edit_button_srgba_unmultiplied(&mut c.0).labelled_by(lbl.id);
                let mut hex = c.to_hex();
                let hr = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(80.0)).labelled_by(lbl.id);
                if hr.changed() {
                    if let Ok(parsed) = Rgba::parse(&hex) {
                        c = parsed;
                    }
                }
                if r.changed() || hr.changed() {
                    self.change_palette_color(key, c, ui.input(|i| i.time));
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button("+ Farbe").clicked() {
                match self.doc.next_free_key() {
                    Some(k) => {
                        let c = self.doc.color_of(self.color).unwrap_or(Rgba([255, 255, 255, 255]));
                        self.edit(format!("Farbe '{}' hinzugefügt", k as char), |d| d.set_palette_color(k, c)).ok();
                        self.color = k;
                    }
                    None => self.status = "Keine freie Palettentaste mehr".into(),
                }
            }
            let k = self.color;
            if ui
                .add_enabled(k != TRANSPARENT, egui::Button::new("Entfernen"))
                .on_hover_text("Pixel mit dieser Farbe werden transparent")
                .clicked()
            {
                let locked = (0..self.doc.layers.len())
                    .flat_map(|l| (0..self.doc.frames.len()).map(move |f| (l, f)))
                    .any(|(l, f)| self.doc.cel(l, f).contains(&k) && self.doc.edit_blocked(l, f).is_some());
                if locked {
                    self.status = "Die Farbe wird in gesperrten Ebenen/Frames benutzt; nicht entfernt".into();
                } else {
                    let desc = format!("Farbe {} entfernt", self.doc.key_label(k));
                    self.edit(desc, |d| d.remove_palette_color(k));
                }
            }
        });
    }

    /// Color edits come in continuously while dragging in the picker;
    /// group them into one undo step per key and pause.
    fn change_palette_color(&mut self, key: u8, c: Rgba, now: f64) {
        let new_step = !matches!(self.palette_edit_at, Some((k, t)) if k == key && now - t < 1.5);
        if new_step {
            let desc = format!("Farbe {} geändert", self.doc.key_label(key));
            self.history.push(self.doc.clone(), desc.clone());
            self.unsaved_steps.push(desc);
        }
        self.doc.set_palette_color(key, c).ok();
        self.palette_edit_at = Some((key, now));
        self.touch();
    }

    /// Right panel: properties of the current layer, frame, cel and tag,
    /// the regions and the history.
    pub(super) fn properties_panel(&mut self, ui: &mut egui::Ui) {
        // while playing, keep showing the frame chosen before Play
        if !self.playing {
            self.panel_frame = self.frame;
        }
        self.panel_frame = self.panel_frame.min(self.doc.frames.len() - 1);
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.layer_props(ui);
            ui.separator();
            self.frame_props(ui);
            ui.separator();
            self.tag_props(ui);
            ui.separator();
            self.region_props(ui);
            ui.separator();
            self.history_props(ui);
        });
    }

    fn layer_props(&mut self, ui: &mut egui::Ui) {
        let l = self.layer;
        ui.heading(format!("Ebene #{l}: {}", self.doc.layers[l].name));
        let mut name = self.doc.layers[l].name.clone();
        ui.horizontal(|ui| {
            let lbl = ui.label("Name");
            if ui.text_edit_singleline(&mut name).labelled_by(lbl.id).changed() && !name.is_empty() {
                let desc = format!("Ebene {l} umbenannt in {name:?}");
                self.edit(desc, |d| d.rename_layer(l, &name));
            }
        });
        ui.horizontal(|ui| {
            let mut visible = self.doc.layers[l].visible;
            if ui.checkbox(&mut visible, "sichtbar").changed() {
                let desc = format!(
                    "Ebene {} {}",
                    self.doc.layers[l].name,
                    if visible { "eingeblendet" } else { "ausgeblendet" }
                );
                self.edit(desc, |d| d.layers[l].visible = visible);
            }
            let mut locked = self.doc.layers[l].locked;
            if ui
                .checkbox(&mut locked, "gesperrt")
                .on_hover_text("Gesperrte Ebenen nehmen keine Pixeländerungen an (GUI und Claude)")
                .changed()
            {
                let desc =
                    format!("Ebene {} {}", self.doc.layers[l].name, if locked { "gesperrt" } else { "entsperrt" });
                self.edit(desc, |d| d.layers[l].locked = locked);
            }
            if ui
                .button("Referenz…")
                .on_hover_text("Bild oder Dokument als halbtransparente Vorlage (wird nie exportiert)")
                .clicked()
            {
                self.canvas.reference.open = true;
            }
        });
        let mut opacity = self.doc.layers[l].opacity;
        ui.horizontal(|ui| {
            let lbl = ui.label("Deckkraft");
            let r = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0)).labelled_by(lbl.id);
            if r.drag_started() || (r.changed() && !r.dragged()) {
                let desc = format!("Deckkraft Ebene {}", self.doc.layers[l].name);
                self.history.push(self.doc.clone(), desc.clone());
                self.unsaved_steps.push(desc);
            }
            if r.changed() {
                self.doc.layers[l].opacity = opacity;
                self.touch();
            }
        });
    }

    fn frame_props(&mut self, ui: &mut egui::Ui) {
        let f = self.panel_frame;
        ui.heading(format!("Frame #{f}: {}", self.doc.frame_label(f)));
        ui.horizontal(|ui| {
            let lbl = ui.label("Name");
            let mut name = self.doc.frames[f].name.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0)).labelled_by(lbl.id);
            let taken = self.doc.frames.iter().enumerate().any(|(i, fr)| i != f && fr.name == name);
            if r.changed() && !name.is_empty() && !name.contains('/') && !taken {
                let desc = format!("Frame {f} umbenannt in {name:?}");
                self.edit(desc, |d| d.frames[f].name = name);
            }
        });
        ui.horizontal(|ui| {
            let lbl = ui.label("Dauer");
            let mut ms = self.doc.frames[f].duration_ms;
            let r = ui.add(egui::DragValue::new(&mut ms).range(1..=10000).suffix(" ms")).labelled_by(lbl.id);
            if r.changed() {
                let desc = format!("Dauer {} = {ms} ms", self.doc.frame_label(f));
                self.edit(desc, |d| d.frames[f].duration_ms = ms);
            }
            if ui.button("für alle").on_hover_text("Diese Dauer für alle Frames übernehmen").clicked() {
                self.edit(format!("Dauer aller Frames = {ms} ms"), |d| {
                    d.frames.iter_mut().for_each(|fr| fr.duration_ms = ms)
                });
            }
            let sel = self.selected_frames();
            if sel.len() > 1
                && ui
                    .button(format!("für Auswahl ({})", sel.len()))
                    .on_hover_text("Diese Dauer für die gewählten Frames übernehmen")
                    .clicked()
            {
                self.set_duration(&sel, ms);
            }
        });
        let mut locked = self.doc.frames[f].locked;
        if ui.checkbox(&mut locked, "Frame gesperrt").changed() {
            let desc = format!("Frame {} {}", self.doc.frame_label(f), if locked { "gesperrt" } else { "entsperrt" });
            self.edit(desc, |d| d.frames[f].locked = locked);
        }
        // cel of the current layer in this frame. The rows are the same for
        // every frame (buttons only get disabled), so the panel below never
        // shifts when the frame changes.
        let l = self.layer;
        let here = format!("{} · {}", self.doc.frame_label(f), self.doc.layers[l].name);
        let linked = self.doc.layers[l].linked_frames(f);
        if linked.len() > 1 {
            ui.label(format!("Cel verknüpft mit Frames {linked:?}"));
        } else if self.doc.cel_is_empty(l, f) {
            ui.label(RichText::new("Cel leer").weak());
        } else {
            ui.label(RichText::new("Cel eigenständig").weak());
        }
        if ui
            .add_enabled(linked.len() > 1, egui::Button::new("Verknüpfung lösen"))
            .on_hover_text("Diese Cel bekommt eine eigene Kopie")
            .clicked()
        {
            self.edit(format!("Verknüpfung gelöst: {here}"), |d| d.layers[l].unlink(f));
        }
        if ui
            .add_enabled(f > 0, egui::Button::new("Mit vorherigem Frame verknüpfen"))
            .on_hover_text("Ersetzt die Pixel dieser Cel")
            .clicked()
        {
            match self.doc.edit_blocked(l, f) {
                Some(why) => self.status = format!("Gesperrt: {why}"),
                None => self.edit(format!("Verknüpft mit Frame {}: {here}", f - 1), |d| d.layers[l].link(f, f - 1)),
            }
        }
    }

    fn tag_props(&mut self, ui: &mut egui::Ui) {
        let n = self.doc.frames.len();
        ui.heading("Tag (Animation)");
        // load the selected tag into the draft whenever the selection changes
        if self.tag_draft.loaded != Some(self.play_tag) {
            let sequence = std::mem::take(&mut self.tag_draft.sequence);
            self.tag_draft = match self.play_tag.and_then(|t| self.doc.tags.get(t)) {
                Some(t) => TagDraft { tag: t.clone(), loaded: Some(self.play_tag), sequence },
                None => TagDraft { tag: Tag::new("", self.frame, self.frame), loaded: Some(None), sequence },
            };
        }
        let editing = self.play_tag.is_some();
        let d = &mut self.tag_draft.tag;
        ui.horizontal(|ui| {
            let lbl = ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(110.0).hint_text("idle")).labelled_by(lbl.id);
        });
        ui.horizontal(|ui| {
            let lbl = ui.label("Frames");
            ui.add(egui::DragValue::new(&mut d.from).range(0..=n - 1)).labelled_by(lbl.id);
            let lbl = ui.label("bis");
            ui.add(egui::DragValue::new(&mut d.to).range(0..=n - 1)).labelled_by(lbl.id);
        });
        ui.horizontal(|ui| {
            let lbl = ui.label("Richtung");
            egui::ComboBox::from_id_salt("tag_dir")
                .selected_text(direction_label(d.direction))
                .show_ui(ui, |ui| {
                    for dir in Direction::ALL {
                        ui.selectable_value(&mut d.direction, dir, direction_label(dir));
                    }
                })
                .response
                .labelled_by(lbl.id);
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.looping, "Loop");
            let mut c = d.color.unwrap_or_else(|| super::timeline::auto_tag_color(0));
            let lbl = ui.label("Farbe");
            if ui.color_edit_button_srgba_unmultiplied(&mut c.0).labelled_by(lbl.id).changed() {
                d.color = Some(c);
            }
        });
        let mut tag = d.clone();
        (tag.from, tag.to) = (tag.from.min(tag.to), tag.from.max(tag.to));
        tag.name = tag.name.trim().to_string();
        let valid = !tag.name.is_empty() && !tag.name.contains('/');
        ui.horizontal(|ui| {
            let label = if editing { "Übernehmen" } else { "Tag anlegen" };
            if ui.add_enabled(valid, egui::Button::new(label)).clicked() {
                let old = self.play_tag.and_then(|t| self.doc.tags.get(t)).map(|t| t.name.clone());
                let name = tag.name.clone();
                let desc = format!("Tag {name:?} = Frames {}..{} {}", tag.from, tag.to, direction_label(tag.direction));
                let r = self.edit(desc, |doc| {
                    // renaming: drop the old entry first
                    if let Some(old) = old.filter(|o| *o != name) {
                        doc.tags.retain(|t| t.name != old);
                    }
                    doc.set_tag(tag)
                });
                match r {
                    Ok(()) => {
                        self.play_tag = self.doc.tags.iter().position(|t| t.name == name);
                        self.tag_draft.loaded = None;
                        self.status = format!("Tag {name:?} gesetzt");
                    }
                    Err(e) => self.status = e,
                }
            }
            if let Some(t) = self.play_tag {
                if ui.button("Löschen").clicked() {
                    let desc = format!("Tag {:?} gelöscht", self.doc.tags[t].name);
                    self.edit(desc, |doc| doc.tags.remove(t));
                    self.play_tag = None;
                }
                if ui.button("Neu").on_hover_text("Neuen Tag ab dem aktuellen Frame anlegen").clicked() {
                    self.play_tag = None;
                }
            }
        });
        // arbitrary frame order (Aseprite tags are ranges: built from linked frames)
        ui.horizontal(|ui| {
            let lbl = ui.label("Folge");
            ui.add(egui::TextEdit::singleline(&mut self.tag_draft.sequence).desired_width(110.0).hint_text("0 1 0 2"))
                .labelled_by(lbl.id)
                .on_hover_text("Beliebige Frame-Reihenfolge (Indizes, Namen oder tag/n). Wird als verknüpfte Frames angehängt, der Tag umfasst sie.");
            let name = self.tag_draft.tag.name.trim().to_string();
            if ui.add_enabled(!name.is_empty() && !name.contains('/'), egui::Button::new("Als Folge anlegen")).clicked() {
                let parsed: Result<Vec<usize>, String> = self
                    .tag_draft
                    .sequence
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .map(|s| self.doc.find_frame(s))
                    .collect();
                match parsed {
                    Ok(frames) => {
                        let desc = format!("Tag {name:?} als Folge {frames:?}");
                        match self.edit(desc, |d| d.sequence_tag(&name, &frames)) {
                            Ok((from, to)) => {
                                self.play_tag = self.doc.tags.iter().position(|t| t.name == name);
                                self.tag_draft.loaded = None;
                                self.status = format!("Tag {name:?} = Frames {from}..{to} (verknüpfte Frames)");
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    Err(e) => self.status = e,
                }
            }
        });
    }

    fn region_props(&mut self, ui: &mut egui::Ui) {
        ui.heading("Regionen");
        ui.checkbox(&mut self.show_regions, "auf der Leinwand anzeigen");
        let f = self.panel_frame;
        let mut action: Option<(usize, &str)> = None;
        for (i, r) in self.doc.regions.iter().enumerate() {
            ui.horizontal(|ui| {
                let c = r.color.map_or(pixel_core::render::auto_color(i), |c| c.0);
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().rect_filled(dot, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
                let rect = r.rect_at(f);
                let text = match rect {
                    Some(rc) if r.is_marker() => format!("+ {} · Marker ({}, {})", r.name, rc.x, rc.y),
                    Some(rc) => format!("{} · {}", r.name, rc.describe()),
                    None => format!("{} · (nicht in diesem Frame)", r.name),
                };
                let mut tip = String::new();
                if let Some(l) = &r.layer {
                    tip += &format!("nur Ebene {l}; ");
                }
                tip += "Klick: als Auswahl übernehmen";
                if ui.selectable_label(false, text).on_hover_text(tip).clicked() {
                    action = Some((i, "select"));
                }
                let mut locked = r.locked;
                let tip = if r.is_marker() {
                    "gesperrt: Position und Name geschützt (Pixel bleiben bearbeitbar)"
                } else {
                    "gesperrt: Pixel darin sind geschützt"
                };
                if ui.checkbox(&mut locked, "").on_hover_text(tip).changed() {
                    action = Some((i, "lock"));
                }
                if ui
                    .add_enabled(!r.locked, egui::Button::new("x"))
                    .on_hover_text("Region löschen (Pixel bleiben)")
                    .clicked()
                {
                    action = Some((i, "delete"));
                }
            });
        }
        match action {
            Some((i, "select")) => {
                self.commit_floating();
                self.selection = self.doc.regions[i].rect_at(f).map(super::Selection::rect);
                self.set_tool(Tool::Select);
            }
            Some((i, "lock")) => {
                let r = &self.doc.regions[i];
                let desc = format!("Region {} {}", r.name, if r.locked { "entsperrt" } else { "gesperrt" });
                self.edit(desc, |d| d.regions[i].locked = !d.regions[i].locked);
            }
            Some((i, _)) => {
                let desc = format!("Region {} gelöscht", self.doc.regions[i].name);
                self.edit(desc, |d| d.regions.remove(i));
            }
            None => {}
        }
        // new region from the selection
        ui.horizontal(|ui| {
            let lbl = ui.label("Neu");
            ui.add(egui::TextEdit::singleline(&mut self.region_draft.name).desired_width(100.0).hint_text("left_shoe"))
                .labelled_by(lbl.id);
            ui.checkbox(&mut self.region_draft.this_frame_only, "nur dieser Frame");
        });
        ui.checkbox(&mut self.region_draft.marker, "als Marker (Punkt oben links der Auswahl)");
        let name = self.region_draft.name.trim().to_string();
        let ok = !name.is_empty() && !name.contains('/') && self.selection.is_some();
        if ui
            .add_enabled(ok, egui::Button::new("Region aus Auswahl"))
            .on_hover_text("Erst mit dem Auswahl-Werkzeug (M) einen Bereich wählen")
            .clicked()
        {
            let marker = self.region_draft.marker;
            let rect = match self.selection.as_ref().unwrap().rect {
                r if marker => pixel_core::Rect::new(r.x, r.y, 1, 1),
                r => r,
            };
            let n = self.doc.frames.len();
            let only = self.region_draft.this_frame_only;
            if let Some(r) = self.doc.regions.iter().find(|r| r.name == name && r.locked) {
                self.status = format!("Region {:?} ist gesperrt", r.name);
                return;
            }
            let desc = format!(
                "Region {name:?} {}{}",
                rect.describe(),
                if only { format!(" (nur {})", self.doc.frame_label(f)) } else { String::new() }
            );
            let exists = self.doc.regions.iter().position(|r| r.name == name);
            self.edit(desc, |d| {
                let i = match exists {
                    Some(i) => i,
                    None => {
                        d.regions.push(Region::new(&name, rect));
                        d.regions.len() - 1
                    }
                };
                d.regions[i].kind = if marker {
                    pixel_core::document::RegionKind::Marker
                } else {
                    pixel_core::document::RegionKind::Area
                };
                if only {
                    if exists.is_none() {
                        d.regions[i].keys = vec![pixel_core::document::RegionKey { frame: 0, rect: None }];
                    }
                    d.regions[i].set_rect_for(f, f, Some(rect), n);
                } else {
                    d.regions[i].keys = vec![pixel_core::document::RegionKey { frame: 0, rect: Some(rect) }];
                }
                d.regions[i].normalize();
            });
            self.region_draft.name.clear();
        }
    }

    fn history_props(&mut self, ui: &mut egui::Ui) {
        ui.heading("Verlauf");
        ui.horizontal(|ui| {
            if ui.add_enabled(self.history.can_undo(), egui::Button::new("Rückgängig")).clicked() {
                self.undo();
            }
            if ui.add_enabled(self.history.can_redo(), egui::Button::new("Wiederholen")).clicked() {
                self.redo();
            }
        });
        // newest first; clicking an older step undoes back to before it
        let undo: Vec<String> = self.history.undo_steps().iter().map(|s| s.to_string()).collect();
        let redo: Vec<String> = self.history.redo_steps().iter().map(|s| s.to_string()).collect();
        let mut undo_to = None;
        egui::ScrollArea::vertical().id_salt("history").max_height(180.0).show(ui, |ui| {
            for d in redo.iter().rev() {
                ui.label(RichText::new(format!("(wiederholbar) {d}")).weak().small());
            }
            for (i, d) in undo.iter().enumerate().rev() {
                let r = ui.selectable_label(i + 1 == undo.len(), RichText::new(d).small());
                if r.on_hover_text("Bis vor diesen Schritt zurückgehen").clicked() {
                    undo_to = Some(undo.len() - i);
                }
            }
            if undo.is_empty() && redo.is_empty() {
                ui.label(RichText::new("noch keine Schritte").weak().small());
            }
        });
        if let Some(n) = undo_to {
            for _ in 0..n {
                self.undo();
            }
        }
        // named snapshots live in the journal next to the file
        let Some(path) = self.path.clone() else { return };
        let journal = Journal::for_document(&path);
        ui.horizontal(|ui| {
            let lbl = ui.label("Schnappschuss");
            ui.add(egui::TextEdit::singleline(&mut self.snapshot_name).desired_width(90.0).hint_text("vor_fix"))
                .labelled_by(lbl.id);
            let name = self.snapshot_name.trim().to_string();
            if ui.add_enabled(!name.is_empty(), egui::Button::new("Sichern")).clicked() {
                match journal.save_snapshot(&name, "GUI", &self.doc) {
                    Ok(_) => self.status = format!("Schnappschuss {name:?} gesichert"),
                    Err(e) => self.status = e,
                }
                self.snapshot_name.clear();
            }
        });
        for s in journal.snapshots() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&s.name).small());
                if ui.small_button("wiederherstellen").clicked() {
                    match journal.load_snapshot(&s.name) {
                        Ok(doc) => {
                            self.drop_floating();
                            let desc = format!("Schnappschuss {:?} wiederhergestellt", s.name);
                            self.edit(desc, |d| *d = doc);
                        }
                        Err(e) => self.status = e,
                    }
                }
            });
        }
    }
}

const NO_RAMPS_HELP: &str =
    "Noch keine Farbrampe: Farben in der Palette mit Strg+Klick von dunkel nach hell markieren, dann „Rampe anlegen“";

/// What a click in a ramp row asks for (applied after the row is drawn).
enum RampAction {
    Pick(u8),
    /// Move the current color one place darker (-1) or lighter (+1).
    Move(usize, i32),
    SortByBrightness(usize),
    AddCurrent(usize),
    RemoveCurrent(usize),
    Delete(usize),
    Rename(usize, String),
}

impl EditorApp {
    /// The color ramps as rows above the palette: a click picks a color,
    /// the buttons reorder, extend, shorten or delete the ramp.
    fn ramps_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.ramps.is_empty() {
            return;
        }
        ui.label(RichText::new("Farbrampen (dunkel > hell)").small().strong());
        let cur = self.color;
        let mut act = None;
        for (ri, ramp) in self.doc.ramps.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                match &mut self.ramp_rename {
                    Some((i, draft)) if *i == ri => {
                        let r = ui.add(egui::TextEdit::singleline(draft).desired_width(60.0));
                        if r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            act = Some(RampAction::Rename(ri, draft.clone()));
                        } else if !r.has_focus() {
                            r.request_focus();
                        }
                    }
                    _ => {
                        let r = ui
                            .add(egui::Label::new(RichText::new(&ramp.name).small()).sense(Sense::click()))
                            .on_hover_text("Doppelklick = umbenennen");
                        if r.double_clicked() {
                            self.ramp_rename = Some((ri, ramp.name.clone()));
                        }
                    }
                }
                for &k in &ramp.keys {
                    let r = swatch(ui, self.doc.color_of(k), 18.0, k == cur).on_hover_text(self.doc.key_label(k));
                    if r.clicked() {
                        act = Some(RampAction::Pick(k));
                    }
                }
                let at = ramp.keys.iter().position(|k| *k == cur);
                let small = |t: &str| egui::Button::new(RichText::new(t).small()).min_size(Vec2::new(16.0, 18.0));
                if ui
                    .add_enabled(at.is_some_and(|i| i > 0), small("<"))
                    .on_hover_text("Aktuelle Farbe eine Stelle dunkler einsortieren")
                    .clicked()
                {
                    act = Some(RampAction::Move(ri, -1));
                }
                if ui
                    .add_enabled(at.is_some_and(|i| i + 1 < ramp.keys.len()), small(">"))
                    .on_hover_text("Aktuelle Farbe eine Stelle heller einsortieren")
                    .clicked()
                {
                    act = Some(RampAction::Move(ri, 1));
                }
                if ui.add(small("hell")).on_hover_text("Nach Helligkeit sortieren").clicked() {
                    act = Some(RampAction::SortByBrightness(ri));
                }
                if ui
                    .add_enabled(at.is_none() && cur != TRANSPARENT, small("+"))
                    .on_hover_text("Aktuelle Farbe hell ans Ende anfügen")
                    .clicked()
                {
                    act = Some(RampAction::AddCurrent(ri));
                }
                if ui
                    .add_enabled(at.is_some() && ramp.keys.len() > 2, small("-"))
                    .on_hover_text("Aktuelle Farbe aus der Rampe nehmen")
                    .clicked()
                {
                    act = Some(RampAction::RemoveCurrent(ri));
                }
                if ui.add(small("×")).on_hover_text("Rampe löschen (die Farben bleiben)").clicked() {
                    act = Some(RampAction::Delete(ri));
                }
            });
        }
        ui.add_space(4.0);
        let Some(act) = act else { return };
        let name = |d: &pixel_core::Document, ri: usize| d.ramps[ri].name.clone();
        match act {
            RampAction::Pick(k) => self.color = k,
            RampAction::Move(ri, dir) => {
                let i = self.doc.ramps[ri].keys.iter().position(|k| *k == cur).unwrap();
                let desc = format!("Rampe {}: {} umsortiert", name(&self.doc, ri), self.doc.key_label(cur));
                self.edit(desc, |d| d.ramps[ri].keys.swap(i, (i as i32 + dir) as usize));
            }
            RampAction::SortByBrightness(ri) => {
                let desc = format!("Rampe {} nach Helligkeit sortiert", name(&self.doc, ri));
                self.edit(desc, |d| {
                    let lum = |k: &u8| d.color_of(*k).map_or(0.0, brightness);
                    let mut keys = d.ramps[ri].keys.clone();
                    keys.sort_by(|a, b| lum(a).total_cmp(&lum(b)));
                    d.ramps[ri].keys = keys;
                });
            }
            RampAction::AddCurrent(ri) => {
                let desc = format!("Rampe {}: {} angefügt", name(&self.doc, ri), self.doc.key_label(cur));
                self.edit(desc, |d| d.ramps[ri].keys.push(cur));
            }
            RampAction::RemoveCurrent(ri) => {
                let desc = format!("Rampe {}: {} entfernt", name(&self.doc, ri), self.doc.key_label(cur));
                self.edit(desc, |d| d.ramps[ri].keys.retain(|k| *k != cur));
            }
            RampAction::Delete(ri) => {
                let desc = format!("Rampe {} gelöscht", name(&self.doc, ri));
                self.edit(desc, |d| d.ramps.remove(ri));
            }
            RampAction::Rename(ri, new) => {
                self.ramp_rename = None;
                let new = new.trim().to_string();
                let taken = self.doc.ramps.iter().enumerate().any(|(i, r)| i != ri && r.name == new);
                if new.is_empty() || taken {
                    self.status = format!("Rampenname {new:?} ist leer oder schon vergeben");
                } else if new != self.doc.ramps[ri].name {
                    let desc = format!("Rampe {} heißt jetzt {new:?}", name(&self.doc, ri));
                    self.edit(desc, |d| d.ramps[ri].name = new);
                }
            }
        }
    }

    /// Under the palette: the colors marked with Strg+Klick become a ramp.
    fn ramp_pick_ui(&mut self, ui: &mut egui::Ui) {
        self.ramp_pick.retain(|k| self.doc.has_key(*k) && *k != TRANSPARENT);
        if self.ramp_pick.is_empty() {
            if self.doc.ramps.is_empty() {
                ui.label(RichText::new("Strg+Klick markiert Farben für eine Farbrampe (dunkel > hell)").small().weak());
            }
            return;
        }
        ui.horizontal(|ui| {
            let n = self.ramp_pick.len();
            if ui
                .add_enabled(n >= 2, egui::Button::new(format!("Rampe anlegen ({n} Farben)")))
                .on_hover_text("Reihenfolge = Reihenfolge der Strg+Klicks, von dunkel nach hell")
                .clicked()
            {
                let name =
                    (1..).map(|i| format!("Rampe {i}")).find(|n| !self.doc.ramps.iter().any(|r| &r.name == n)).unwrap();
                let ramp = pixel_core::Ramp { name: name.clone(), keys: std::mem::take(&mut self.ramp_pick) };
                match self.doc.check_ramp(&ramp, None) {
                    Ok(()) => {
                        let desc = format!("Farbrampe {}", self.doc.ramp_label(&ramp));
                        self.edit(desc, |d| d.ramps.push(ramp));
                        self.status = format!("Farbrampe {name:?} angelegt; Doppelklick auf den Namen benennt sie um");
                    }
                    Err(e) => self.status = e,
                }
            }
            if ui.button("Markierung aufheben").clicked() {
                self.ramp_pick.clear();
            }
        });
    }
}

/// Perceived brightness 0..1 (Rec. 601 weights).
fn brightness(c: Rgba) -> f32 {
    let [r, g, b, _] = c.0;
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// The number of a color marked for a new ramp, in the swatch's corner.
fn pick_badge(ui: &egui::Ui, r: egui::Rect, n: usize) {
    let c = egui::Rect::from_min_size(r.right_top() - Vec2::new(10.0, 0.0), Vec2::splat(10.0));
    ui.painter().rect_filled(c, 2.0, Color32::from_rgb(90, 200, 255));
    ui.painter().text(
        c.center(),
        egui::Align2::CENTER_CENTER,
        n.to_string(),
        egui::FontId::proportional(8.5),
        Color32::BLACK,
    );
}

pub fn direction_label(d: Direction) -> &'static str {
    match d {
        Direction::Forward => "vorwärts",
        Direction::Reverse => "rückwärts",
        Direction::PingPong => "Ping-Pong",
        Direction::PingPongReverse => "Ping-Pong rückwärts",
    }
}

/// A clickable color swatch; `None` draws the transparency checker.
pub(super) fn swatch(ui: &mut egui::Ui, color: Option<Rgba>, size: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    let h = size / 2.0;
    p.rect_filled(rect, 2.0, Color32::from_gray(72));
    p.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::splat(h)), 0.0, Color32::from_gray(110));
    p.rect_filled(egui::Rect::from_min_size(rect.min + Vec2::splat(h), Vec2::splat(h)), 0.0, Color32::from_gray(110));
    if let Some(c) = color {
        p.rect_filled(rect, 2.0, to_color32(c));
    }
    let stroke = if selected {
        Stroke::new(2.0, Color32::WHITE)
    } else if resp.hovered() {
        Stroke::new(1.0, Color32::LIGHT_GRAY)
    } else {
        Stroke::new(1.0, Color32::from_gray(40))
    };
    p.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
    resp
}

pub const POINT_TOOL_HELP: &str =
    "KI zeigen: Bereich markieren, den die KI sehen soll (ziehen = Bereich, Klick = Punkt; \
     die KI liest ihn mit get_user_view)";
