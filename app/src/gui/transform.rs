//! Mirroring and rotating (selection, cel, frame, tag, all frames), the
//! mirrored copy of an animation and symmetric drawing.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Key, Modifiers, RichText};
use pixel_core::io;
use pixel_core::journal::Journal;
use pixel_core::transform::{self, Op, Outcome};

use super::select::Selection;
use super::{EditorApp, Tool};

/// What a transform applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The selected pixels (or the floating ones), around the box centre.
    Selection,
    /// The current cel, around the canvas centre.
    Cel,
    /// All layers of the current frame.
    Frame,
    /// All frames of the current tag.
    Tag,
    /// All frames of the document.
    All,
}

/// Keyboard shortcut of an op: Alt + letter (Shift+H is taken).
pub fn op_key(op: Op) -> (Key, Modifiers, &'static str) {
    match op {
        Op::FlipH => (Key::H, Modifiers::ALT, "Alt+H"),
        Op::FlipV => (Key::V, Modifiers::ALT, "Alt+V"),
        Op::Rot90 => (Key::R, Modifiers::ALT, "Alt+R"),
        Op::Rot270 => (Key::L, Modifiers::ALT, "Alt+L"),
        Op::Rot180 => (Key::R, Modifiers::ALT.plus(Modifiers::SHIFT), "Alt+Umschalt+R"),
    }
}

/// Shortcut that switches symmetric drawing through its modes.
pub const SYMMETRY_KEY: &str = "Alt+S";

/// Symmetric drawing: which axes mirror the strokes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SymMode {
    #[default]
    Off,
    /// A vertical axis: left and right mirror each other.
    Vertical,
    /// A horizontal axis: top and bottom mirror each other.
    Horizontal,
    Both,
}

impl SymMode {
    pub const ALL: [SymMode; 4] = [SymMode::Off, SymMode::Vertical, SymMode::Horizontal, SymMode::Both];

    pub fn label(self) -> &'static str {
        match self {
            SymMode::Off => "Aus",
            SymMode::Vertical => "Vertikal",
            SymMode::Horizontal => "Horizontal",
            SymMode::Both => "Beide",
        }
    }

    pub fn vertical(self) -> bool {
        matches!(self, SymMode::Vertical | SymMode::Both)
    }

    pub fn horizontal(self) -> bool {
        matches!(self, SymMode::Horizontal | SymMode::Both)
    }

    fn next(self) -> SymMode {
        SymMode::ALL[(SymMode::ALL.iter().position(|m| *m == self).unwrap() + 1) % 4]
    }
}

/// Symmetry axes. Positions are doubled canvas coordinates, so an axis can
/// lie on a pixel edge (even) or through pixel centres (odd); None = the
/// canvas centre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Symmetry {
    pub mode: SymMode,
    pub x2: Option<i32>,
    pub y2: Option<i32>,
}

/// Which axis is being dragged on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisDrag {
    X,
    Y,
}

impl EditorApp {
    /// Doubled position of the vertical axis (x) and the horizontal one (y).
    pub(super) fn axis2(&self) -> (i32, i32) {
        let (w, h) = (self.doc.width as i32, self.doc.height as i32);
        (self.symmetry.x2.unwrap_or(w).clamp(1, 2 * w - 1), self.symmetry.y2.unwrap_or(h).clamp(1, 2 * h - 1))
    }

    /// `pts` plus their mirror images under the active symmetry.
    pub(super) fn sym_points(&self, pts: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
        let m = self.symmetry.mode;
        if m == SymMode::Off {
            return pts;
        }
        let (ax, ay) = self.axis2();
        let mut out = pts.clone();
        for &(x, y) in &pts {
            if m.vertical() {
                out.push((ax - 1 - x, y));
            }
            if m.horizontal() {
                out.push((x, ay - 1 - y));
            }
            if m == SymMode::Both {
                out.push((ax - 1 - x, ay - 1 - y));
            }
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|p| seen.insert(*p));
        out
    }

    pub(super) fn cycle_symmetry(&mut self) {
        self.symmetry.mode = self.symmetry.mode.next();
        self.status = format!("Symmetrie: {}", self.symmetry.mode.label());
    }

    /// The tag a tag-wide transform uses: the played one if it contains
    /// the current frame, else the first tag that does.
    pub(super) fn current_tag(&self) -> Option<usize> {
        let f = self.frame;
        self.play_tag
            .filter(|t| self.doc.tags.get(*t).is_some_and(|t| t.contains(f)))
            .or_else(|| self.doc.tags.iter().position(|t| t.contains(f)))
    }

    /// Shortcut: the selection if there is one, else the current cel.
    pub(super) fn transform_default(&mut self, op: Op) {
        let scope = if self.selection.is_some() || self.floating.is_some() { Scope::Selection } else { Scope::Cel };
        self.transform(op, scope);
    }

    pub(super) fn transform(&mut self, op: Op, scope: Scope) {
        self.playing = false;
        if scope == Scope::Selection {
            self.transform_selection(op);
            return;
        }
        self.commit_floating();
        let (l, f) = (self.layer, self.frame);
        let frames: Vec<usize> = match scope {
            Scope::Cel | Scope::Frame => vec![f],
            Scope::Tag => match self.current_tag() {
                Some(t) => (self.doc.tags[t].from..=self.doc.tags[t].to).collect(),
                None => {
                    self.status = "Der aktuelle Frame gehört zu keinem Tag".into();
                    return;
                }
            },
            _ => (0..self.doc.frames.len()).collect(),
        };
        let layers: Vec<usize> = if scope == Scope::Cel { vec![l] } else { (0..self.doc.layers.len()).collect() };
        // locked layers/frames are left alone and named
        let mut skipped = Vec::new();
        let mut cels = Vec::new();
        for (cl, cf) in transform::unique_cels(&self.doc, &layers, &frames) {
            match self.doc.edit_blocked(cl, cf) {
                Some(why) => {
                    if !skipped.contains(&why) {
                        skipped.push(why)
                    }
                }
                None => cels.push((cl, cf)),
            }
        }
        if cels.is_empty() {
            self.status = format!("Gesperrt: {}", skipped.join(", "));
            return;
        }
        let what = match scope {
            Scope::Cel => format!("Cel {}", self.here()),
            Scope::Frame => format!("Frame {}", self.doc.frame_label(f)),
            Scope::Tag => format!("Tag {}", self.doc.tags[self.current_tag().unwrap()].name),
            _ => "alle Frames".to_string(),
        };
        let linked_out: usize = cels
            .iter()
            .map(|&(cl, cf)| self.doc.layers[cl].linked_frames(cf).iter().filter(|g| !frames.contains(g)).count())
            .sum();
        let region_layer = (scope == Scope::Cel).then_some(l);
        let region_frames: Vec<usize> = match scope {
            Scope::Cel => self.doc.layers[l].linked_frames(f),
            _ => frames.clone(),
        };
        let desc = format!("{}: {what}", op.label());
        let (out, locked_regions) = self.edit(desc, |d| {
            let mut out = Outcome::default();
            for &(cl, cf) in &cels {
                out.add(transform::transform_cel(d, cl, cf, op, true));
            }
            (out, transform::transform_regions(d, &region_frames, region_layer, op, true))
        });
        let mut status = format!("{}: {what}", op.label());
        status += &outcome_text(out);
        if !locked_regions.is_empty() {
            status += &format!(" · gesperrte Regionen bleiben: {}", locked_regions.join(", "));
        }
        if !skipped.is_empty() {
            status += &format!(" · übersprungen: {}", skipped.join(", "));
        }
        if linked_out > 0 {
            status += &format!(" · {linked_out} verknüpfte Frames außerhalb änderten sich mit");
        }
        self.status = status;
        self.canvas.invalidate();
    }

    fn transform_selection(&mut self, op: Op) {
        // floating pixels turn in place, without touching the cel
        if let Some(fl) = &mut self.floating {
            fl.pixels = transform::transform_block(&fl.pixels, fl.rect.w, fl.rect.h, op);
            fl.rect = transform::transformed_rect(fl.rect, op);
            fl.transformed = true;
            let rect = fl.rect;
            self.selection = Some(match &self.selection {
                Some(s) => s.transformed(op),
                None => Selection::rect(rect),
            });
            self.canvas.invalidate();
            let off = self.floating_off_canvas();
            self.status =
                format!("{}: schwebende Auswahl{}", op.label(), outcome_text(Outcome { clipped: off, protected: 0 }));
            return;
        }
        let Some(s) = self.selection_clipped() else {
            self.status = "Nichts ausgewählt".into();
            return;
        };
        if self.blocked() {
            return;
        }
        let (l, f) = (self.layer, self.frame);
        let desc = format!("{}: Auswahl {} · {}", op.label(), s.describe(), self.here());
        let moved = self.edit(desc, |d| transform::transform_area(d, l, f, s.rect, s.mask(), op, true));
        let (w, h) = (self.doc.width, self.doc.height);
        self.selection = Selection::with_mask(moved.rect, moved.mask).clipped(w, h);
        self.status = format!("{}: Auswahl {}{}", op.label(), s.describe(), outcome_text(moved.outcome));
    }

    /// Opaque floating pixels outside the canvas (cut off when dropped).
    fn floating_off_canvas(&self) -> usize {
        let Some(fl) = &self.floating else { return 0 };
        let r = fl.rect;
        fl.pixels
            .iter()
            .enumerate()
            .filter(|(i, k)| {
                **k != pixel_core::TRANSPARENT
                    && !self.doc.in_bounds(r.x + (*i as u32 % r.w) as i32, r.y + (*i as u32 / r.w) as i32)
            })
            .count()
    }

    pub(super) fn transform_shortcuts(&mut self, ctx: &egui::Context) {
        for op in Op::ALL {
            let (k, m, _) = op_key(op);
            if ctx.input(|i| i.key_pressed(k) && i.modifiers.matches_exact(m)) {
                self.transform_default(op);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::S) && i.modifiers.matches_exact(Modifiers::ALT)) {
            self.cycle_symmetry();
        }
    }

    fn op_buttons(&mut self, ui: &mut egui::Ui, scope: Scope, shortcuts: bool) {
        for op in Op::ALL {
            let label = if shortcuts { format!("{}  ({})", op.label(), op_key(op).2) } else { op.label().to_string() };
            if ui.button(label).clicked() {
                self.transform(op, scope);
                ui.close();
            }
        }
    }

    /// The Bild menu.
    pub(super) fn image_menu_entries(&mut self, ui: &mut egui::Ui) {
        let has_sel = self.selection.is_some() || self.floating.is_some();
        ui.label(RichText::new(if has_sel { "Auswahl:" } else { "Aktueller Cel:" }).small().weak());
        for op in Op::ALL {
            if ui.button(format!("{}  ({})", op.label(), op_key(op).2)).clicked() {
                self.transform_default(op);
                ui.close();
            }
        }
        ui.separator();
        ui.add_enabled_ui(has_sel, |ui| ui.menu_button("Auswahl", |ui| self.op_buttons(ui, Scope::Selection, false)));
        ui.menu_button("Cel", |ui| self.op_buttons(ui, Scope::Cel, false));
        ui.menu_button("Frame (alle Ebenen)", |ui| self.op_buttons(ui, Scope::Frame, false));
        let tag = self.current_tag().map(|t| self.doc.tags[t].name.clone());
        let tag_label = tag.map_or("Frames des Tags".to_string(), |t| format!("Frames des Tags {t}"));
        ui.add_enabled_ui(self.current_tag().is_some(), |ui| {
            ui.menu_button(tag_label, |ui| self.op_buttons(ui, Scope::Tag, false))
        });
        ui.menu_button("Alle Frames", |ui| self.op_buttons(ui, Scope::All, false));
        ui.separator();
        if ui.button("Animation gespiegelt duplizieren…").clicked() {
            self.mirror_dialog = Some(self.mirror_default_path().display().to_string());
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new(format!("Symmetrisch zeichnen ({SYMMETRY_KEY})")).small().weak());
        for m in SymMode::ALL {
            if ui.radio(self.symmetry.mode == m, m.label()).clicked() {
                self.symmetry.mode = m;
                ui.close();
            }
        }
    }

    /// Mirror/rotate entries of the selection tools' context menu.
    pub(super) fn selection_transform_entries(&mut self, ui: &mut egui::Ui) {
        let has = self.selection.is_some() || self.floating.is_some();
        for op in Op::ALL {
            if ui.add_enabled(has, egui::Button::new(format!("{}  ({})", op.label(), op_key(op).2))).clicked() {
                self.transform(op, Scope::Selection);
                ui.close();
            }
        }
    }

    /// Symmetry options in the tool panel (drawing tools).
    pub(super) fn symmetry_panel(&mut self, ui: &mut egui::Ui) {
        if !matches!(self.tool, Tool::Pencil | Tool::Eraser | Tool::Fill | Tool::Line | Tool::Rect | Tool::Ellipse) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Symmetrie:");
            for m in SymMode::ALL {
                ui.selectable_value(&mut self.symmetry.mode, m, m.label());
            }
            ui.label(RichText::new(SYMMETRY_KEY).small().weak());
        });
        if self.symmetry.mode == SymMode::Off {
            return;
        }
        let (ax, ay) = self.axis2();
        let (w, h) = (self.doc.width as f32, self.doc.height as f32);
        ui.horizontal(|ui| {
            if self.symmetry.mode.vertical() {
                let mut x = ax as f32 / 2.0;
                let r = ui.add(egui::DragValue::new(&mut x).speed(0.5).range(0.5..=w - 0.5).prefix("Achse x "));
                if r.changed() {
                    self.symmetry.x2 = Some((x * 2.0).round() as i32);
                }
            }
            if self.symmetry.mode.horizontal() {
                let mut y = ay as f32 / 2.0;
                let r = ui.add(egui::DragValue::new(&mut y).speed(0.5).range(0.5..=h - 0.5).prefix("Achse y "));
                if r.changed() {
                    self.symmetry.y2 = Some((y * 2.0).round() as i32);
                }
            }
            if ui.button("Mitte").on_hover_text("Achsen auf die Leinwandmitte").clicked() {
                self.symmetry.x2 = None;
                self.symmetry.y2 = None;
            }
        });
        ui.label(RichText::new("Achse an den Griffen am Leinwandrand ziehen").small().weak());
    }

    // ---- mirrored copy -------------------------------------------------

    fn mirror_default_path(&self) -> PathBuf {
        let dir = self.work_dir().unwrap_or_default();
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().trim_end_matches(io::EXTENSION).trim_end_matches(".aseprite").to_string())
            .unwrap_or_else(|| "unbenannt".into());
        dir.join(format!("{}{}", transform::mirrored_name(&stem), io::EXTENSION))
    }

    /// Write a horizontally mirrored copy of the document to `path` and
    /// open it in a new tab.
    pub(super) fn mirror_to(&mut self, path: &Path) {
        self.commit_floating();
        if path.exists() {
            self.status = format!("{} gibt es schon; anderen Namen wählen", path.display());
            return;
        }
        if !io::is_document_path(path) {
            self.status = format!("{}: Endung .pxe.json oder .aseprite fehlt", path.display());
            return;
        }
        let (doc, clipped) = transform::mirror_document(&self.doc, Op::FlipH);
        let from = self.path.as_ref().map_or("unbenannt".into(), |p| p.display().to_string());
        let note = format!("gespiegelt dupliziert aus {from}");
        let res = io::save(&doc, path).and_then(|_| Journal::for_document(path).record_edit(&doc, "user", &note, None));
        if let Err(e) = res {
            self.status = format!("Spiegeln fehlgeschlagen: {e}");
            return;
        }
        self.open_in_tab(path);
        self.status = format!(
            "Gespiegelt dupliziert: {} ({} Frames, Tags, Dauern, Verknüpfungen und Regionen übernommen){}",
            path.display(),
            doc.frames.len(),
            outcome_text(Outcome { clipped, protected: 0 })
        );
    }

    pub(super) fn mirror_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(text) = &mut self.mirror_dialog else { return };
        let (mut go, mut cancel) = (false, false);
        egui::Modal::new(egui::Id::unique("mirror_dialog")).show(ctx, |ui| {
            ui.heading("Animation gespiegelt duplizieren");
            ui.label("Alle Frames werden horizontal gespiegelt in ein neues Dokument geschrieben.");
            ui.label("Tags, Dauern, verknüpfte Cels und Regionen bleiben erhalten.");
            ui.label("Neue Datei:");
            let r = ui.add(egui::TextEdit::singleline(text).desired_width(480.0));
            go |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.horizontal(|ui| {
                go |= ui.button("Duplizieren").clicked();
                cancel = ui.button("Abbrechen").clicked();
            });
        });
        if go {
            let p = PathBuf::from(text.trim());
            self.mirror_dialog = None;
            self.mirror_to(&p);
        } else if cancel {
            self.mirror_dialog = None;
        }
    }
}

/// " · 3 Pixel an der Leinwand abgeschnitten · 2 Pixel in gesperrten Regionen blieben"
fn outcome_text(o: Outcome) -> String {
    let mut s = String::new();
    if o.clipped > 0 {
        s += &format!(" · {} Pixel an der Leinwand abgeschnitten", o.clipped);
    }
    if o.protected > 0 {
        s += &format!(" · {} Pixel in gesperrten Regionen blieben", o.protected);
    }
    s
}
