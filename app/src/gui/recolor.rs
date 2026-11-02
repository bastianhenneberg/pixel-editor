//! Farbe ersetzen and Kontur (outline) in the Bild menu, and the dither
//! option of the fill and filled-shape tools.

use eframe::egui::{self, Key, RichText};
use pixel_core::recolor::{self, Area, Outline, OutlineMode, Pattern, Plan};
use pixel_core::{Document, TRANSPARENT};

use super::transform::{Scope, ScopeCels};
use super::{EditorApp, Tool};

/// Dither option of Füllen and Rechteck/Ellipse "gefüllt".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dither {
    pub on: bool,
    pub pattern: Pattern,
    /// The second color (the current color is the first one).
    pub second: u8,
}

impl Default for Dither {
    fn default() -> Self {
        Dither { on: false, pattern: Pattern::Checker, second: TRANSPARENT }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    Replace { from: u8, to: u8 },
    Outline(Outline),
}

/// The open "Farbe ersetzen" / "Kontur" dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecolorDialog {
    pub edit: Edit,
    pub scope: Scope,
    /// Frame/Tag/Alle Frames: only the current layer.
    pub one_layer: bool,
}

/// What an edit would do: per cel the pixels to set.
pub struct Planned {
    pub plans: Vec<((usize, usize), Plan)>,
    pub what: String,
    pub skipped: Vec<String>,
}

impl Planned {
    pub fn changed(&self) -> usize {
        self.plans.iter().map(|(_, p)| p.pixels.len()).sum()
    }

    fn protected(&self) -> usize {
        self.plans.iter().map(|(_, p)| p.protected).sum()
    }

    fn cels(&self) -> usize {
        self.plans.iter().filter(|(_, p)| !p.pixels.is_empty()).count()
    }

    /// "120 Pixel in 3 Cels · 2 Pixel in gesperrten Regionen bleiben · ..."
    pub fn summary(&self) -> String {
        let mut s =
            format!("{} Pixel in {} Cel{}", self.changed(), self.cels(), if self.cels() == 1 { "" } else { "s" });
        if self.protected() > 0 {
            s += &format!(" · {} Pixel in gesperrten Regionen bleiben", self.protected());
        }
        if !self.skipped.is_empty() {
            s += &format!(" · übersprungen: {}", self.skipped.join(", "));
        }
        s
    }
}

fn plan_cel(d: &Document, edit: Edit, l: usize, f: usize, area: Area) -> Plan {
    match edit {
        Edit::Replace { from, to } => recolor::replace_points(d, l, f, area, from, to, true),
        Edit::Outline(o) => recolor::outline_points(d, l, f, area, o, true),
    }
}

impl EditorApp {
    /// Open "Farbe ersetzen" for color `from` (the selection if there is
    /// one, else the current cel).
    pub(super) fn open_replace(&mut self, from: u8) {
        self.commit_floating();
        let to = if self.color != from {
            self.color
        } else {
            self.doc.palette.iter().map(|p| p.key).find(|k| *k != from).unwrap_or(TRANSPARENT)
        };
        self.recolor_dialog =
            Some(RecolorDialog { edit: Edit::Replace { from, to }, scope: self.default_scope(), one_layer: false });
    }

    pub(super) fn open_outline(&mut self) {
        self.commit_floating();
        let o = Outline { color: self.color, mode: OutlineMode::Outside, eight: false, only_transparent: true };
        self.recolor_dialog =
            Some(RecolorDialog { edit: Edit::Outline(o), scope: self.default_scope(), one_layer: false });
    }

    fn default_scope(&self) -> Scope {
        if self.selection.is_some() {
            Scope::Selection
        } else {
            Scope::Cel
        }
    }

    /// Plan `edit` on every cel of `scope` (each linked cel once).
    pub(super) fn plan_recolor(&self, dlg: &RecolorDialog) -> Result<Planned, String> {
        let ScopeCels { cels, skipped, mut what, .. } = self.scope_cels(dlg.scope, dlg.one_layer)?;
        let canvas = Area::canvas(&self.doc);
        let sel = if dlg.scope == Scope::Selection {
            let s = self.selection_clipped().ok_or("Nichts ausgewählt")?;
            what = format!("Auswahl {} · {}", s.describe(), self.here());
            Some(s)
        } else {
            None
        };
        let area = sel.as_ref().map_or(canvas, |s| Area { rect: s.rect, mask: s.mask() });
        let plans = cels.into_iter().map(|(l, f)| ((l, f), plan_cel(&self.doc, dlg.edit, l, f, area))).collect();
        Ok(Planned { plans, what, skipped })
    }

    /// Run the dialog's edit as one undo step.
    pub(super) fn apply_recolor(&mut self, dlg: &RecolorDialog) {
        self.playing = false;
        self.commit_floating();
        let planned = match self.plan_recolor(dlg) {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let title = match dlg.edit {
            Edit::Replace { from, to } => {
                format!("Farbe ersetzen {} > {}", self.doc.key_label(from), self.doc.key_label(to))
            }
            Edit::Outline(o) => format!(
                "Kontur {} ({}, {}er) {}",
                o.mode.label(),
                self.doc.key_label(o.color),
                if o.eight { 8 } else { 4 },
                if o.only_transparent { "nur transparente" } else { "überschreibend" }
            ),
        };
        if planned.changed() == 0 {
            self.status = format!("{title}: {}: nichts zu ändern · {}", planned.what, planned.summary());
            return;
        }
        let desc = format!("{title}: {} Pixel · {}", planned.changed(), planned.what);
        self.edit(desc, |d| {
            for ((l, f), p) in &planned.plans {
                p.apply(d, *l, *f);
            }
        });
        self.status = format!("{title}: {} · {}", planned.what, planned.summary());
        self.canvas.invalidate();
    }

    /// The Bild menu entries of this module.
    pub(super) fn recolor_menu_entries(&mut self, ui: &mut egui::Ui) {
        if ui.button("Farbe ersetzen…").clicked() {
            self.open_replace(self.color);
            ui.close();
        }
        if ui.button("Kontur (Outline)…").clicked() {
            self.open_outline();
            ui.close();
        }
    }

    pub(super) fn recolor_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.recolor_dialog else { return };
        let (mut go, mut cancel) = (false, false);
        let has_sel = self.selection.is_some();
        let tag = self.current_tag().map(|t| self.doc.tags[t].name.clone());
        let layer = self.doc.layers[self.layer].name.clone();
        let this = &*self;
        let doc = &this.doc;
        egui::Modal::new(egui::Id::unique("recolor_dialog")).show(ctx, |ui| {
            match &mut dlg.edit {
                Edit::Replace { from, to } => {
                    ui.heading("Farbe ersetzen");
                    egui::Grid::new("replace_colors").num_columns(2).show(ui, |ui| {
                        ui.label("Farbe");
                        key_combo(ui, "replace_from", doc, from, true);
                        ui.end_row();
                        ui.label("ersetzen durch");
                        key_combo(ui, "replace_to", doc, to, true);
                        ui.end_row();
                    });
                    if ui.button("Tauschen").clicked() {
                        std::mem::swap(from, to);
                    }
                }
                Edit::Outline(o) => {
                    ui.heading("Kontur (Outline)");
                    ui.label("Zieht eine 1-Pixel-Kontur um die nicht-transparenten Pixel.");
                    egui::Grid::new("outline_opts").num_columns(2).show(ui, |ui| {
                        ui.label("Farbe");
                        key_combo(ui, "outline_color", doc, &mut o.color, true);
                        ui.end_row();
                        ui.label("Lage");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut o.mode, OutlineMode::Outside, "außen")
                                .on_hover_text("auf die Pixel rund um die Form");
                            ui.radio_value(&mut o.mode, OutlineMode::Inside, "innen")
                                .on_hover_text("auf die Randpixel der Form selbst");
                        });
                        ui.end_row();
                        ui.label("Nachbarschaft");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut o.eight, false, "4er")
                                .on_hover_text("nur gerade Nachbarn: Ecken bleiben frei");
                            ui.radio_value(&mut o.eight, true, "8er")
                                .on_hover_text("auch diagonale Nachbarn: geschlossene Ecken");
                        });
                        ui.end_row();
                    });
                    ui.add_enabled_ui(o.mode == OutlineMode::Outside, |ui| {
                        ui.checkbox(&mut o.only_transparent, "Nur transparente Pixel füllen").on_hover_text(
                            "Vorhandene Farbe bleibt stehen, z. B. wenn die Kontur einer Auswahl über anderes läuft",
                        );
                    });
                }
            }
            ui.separator();
            ui.label("Bereich");
            ui.add_enabled_ui(has_sel, |ui| ui.radio_value(&mut dlg.scope, Scope::Selection, "Auswahl"));
            ui.radio_value(&mut dlg.scope, Scope::Cel, "Cel");
            ui.radio_value(&mut dlg.scope, Scope::Frame, "Frame (alle Ebenen)");
            ui.add_enabled_ui(tag.is_some(), |ui| {
                let label = tag.as_ref().map_or("Frames des Tags".into(), |t| format!("Frames des Tags {t}"));
                ui.radio_value(&mut dlg.scope, Scope::Tag, label)
            });
            ui.radio_value(&mut dlg.scope, Scope::All, "Alle Frames");
            ui.add_enabled_ui(matches!(dlg.scope, Scope::Frame | Scope::Tag | Scope::All), |ui| {
                ui.checkbox(&mut dlg.one_layer, format!("nur Ebene {layer}"));
            });
            ui.separator();
            // after the inputs above, so the count matches what is shown
            let preview = this.plan_recolor(&dlg);
            match &preview {
                Ok(p) => ui.label(RichText::new(format!("Vorschau: {}", p.summary())).strong()),
                Err(e) => ui.label(RichText::new(e).color(egui::Color32::from_rgb(230, 120, 90))),
            };
            let ok = preview.as_ref().is_ok_and(|p| p.changed() > 0);
            ui.horizontal(|ui| {
                let label = if matches!(dlg.edit, Edit::Replace { .. }) { "Ersetzen" } else { "Kontur ziehen" };
                go |= ui.add_enabled(ok, egui::Button::new(label)).clicked();
                cancel = ui.button("Abbrechen").clicked();
            });
            go |= ok && ui.input(|i| i.key_pressed(Key::Enter));
            cancel |= ui.input(|i| i.key_pressed(Key::Escape));
        });
        if go {
            self.recolor_dialog = None;
            self.apply_recolor(&dlg);
        } else if cancel {
            self.recolor_dialog = None;
        } else {
            self.recolor_dialog = Some(dlg);
        }
    }

    // ---- dither ---------------------------------------------------------

    /// Pattern and both colors when the current tool dithers with `key`
    /// (Füllen, or a filled Rechteck/Ellipse; painting transparent never).
    pub(super) fn active_dither(&self, key: u8) -> Option<(Pattern, u8, u8)> {
        let shape = matches!(self.tool, Tool::Rect | Tool::Ellipse) && self.filled;
        let second = if self.doc.has_key(self.dither.second) { self.dither.second } else { TRANSPARENT };
        (self.dither.on && key != TRANSPARENT && (self.tool == Tool::Fill || shape)).then_some((
            self.dither.pattern,
            key,
            second,
        ))
    }

    /// The pixels a fill or shape sets: `key` everywhere, or the dither.
    pub(super) fn tool_plan(&self, pts: &[(i32, i32)], key: u8) -> Plan {
        let (l, f) = (self.layer, self.frame);
        // without dither: a "pattern" of one color
        let (pattern, a, b) = self.active_dither(key).unwrap_or((Pattern::Checker, key, key));
        recolor::dither_points(&self.doc, l, f, pts, pattern, a, b)
    }

    /// The dither options in the tool panel.
    pub(super) fn dither_panel(&mut self, ui: &mut egui::Ui) {
        let shape = matches!(self.tool, Tool::Rect | Tool::Ellipse) && self.filled;
        if self.tool != Tool::Fill && !shape {
            return;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.dither.on, "Dither")
                .on_hover_text("mit einem Muster aus zwei Farben füllen; an der Leinwand ausgerichtet, nahtlos");
            ui.add_enabled_ui(self.dither.on, |ui| {
                egui::ComboBox::from_id_salt("dither_pattern").selected_text(self.dither.pattern.label()).show_ui(
                    ui,
                    |ui| {
                        for p in Pattern::MENU {
                            ui.selectable_value(&mut self.dither.pattern, p, p.label());
                        }
                    },
                );
            });
        });
        if !self.dither.on {
            return;
        }
        ui.horizontal(|ui| {
            ui.label("2. Farbe");
            key_combo(ui, "dither_second", &self.doc, &mut self.dither.second, true);
        });
        let ramp = self.doc.ramp_of(self.color).is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(ramp, |ui| {
                for (label, steps) in [("Rampe dunkler", -1), ("Rampe heller", 1)] {
                    if ui.button(label).on_hover_text("2. Farbe = Nachbarstufe der Farbrampe").clicked() {
                        if let Some(k) = self.doc.shade_key(self.color, steps) {
                            self.dither.second = k;
                        }
                    }
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} und {}; Muster an der Leinwand ausgerichtet",
                self.doc.key_label(self.color),
                self.doc.key_label(self.active_dither(self.color).map_or(TRANSPARENT, |d| d.2))
            ))
            .small()
            .weak(),
        );
    }
}

/// A color picker over the palette (and transparent).
fn key_combo(ui: &mut egui::Ui, id: &str, doc: &Document, key: &mut u8, transparent: bool) {
    let label = |k: u8| if k == TRANSPARENT { "'.' transparent".to_string() } else { doc.key_label(k) };
    ui.horizontal(|ui| {
        super::panels::swatch(ui, doc.color_of(*key), 16.0, false);
        egui::ComboBox::from_id_salt(id).selected_text(label(*key)).show_ui(ui, |ui| {
            if transparent {
                ui.selectable_value(key, TRANSPARENT, label(TRANSPARENT));
            }
            for p in &doc.palette {
                ui.horizontal(|ui| {
                    super::panels::swatch(ui, Some(p.color), 14.0, false);
                    ui.selectable_value(key, p.key, label(p.key));
                });
            }
        });
    });
}
