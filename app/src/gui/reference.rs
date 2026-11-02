//! The reference ("Referenzebene") in the editor: a PNG/JPG or another
//! document laid semi-transparent under or over the canvas, to trace or
//! compare against. It lives in the document (`Document::reference`), so it
//! is saved per document and undo covers it, but it is only ever painted
//! here: exports, previews and pixel tools never see it.
//!
//! Moving: switch on "Verschieben" in the panel, then drag on the canvas
//! (a locked reference does not move). Position, size and opacity are also
//! fields in the panel.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use pixel_core::document::{RefSource, Reference, REF_SCALE};
use pixel_core::reference as core_ref;

use super::EditorApp;

/// What the loaded texture shows (reloaded when any of it changes).
#[derive(Clone, PartialEq)]
struct Key {
    path: PathBuf,
    /// The frame shown of a document reference.
    frame: Option<usize>,
    mtime: Option<SystemTime>,
}

#[derive(Default)]
pub struct RefView {
    tex: Option<TextureHandle>,
    key: Option<Key>,
    /// Size of the loaded picture in its own pixels.
    pub size: Option<[u32; 2]>,
    /// Why the picture could not be loaded.
    pub error: Option<String>,
    /// "Verschieben": a canvas drag moves the reference instead of painting.
    pub moving: bool,
    /// The running move: pointer start and the reference position then.
    drag: Option<(Pos2, f32, f32)>,
    /// The path typed in the panel.
    pub draft: String,
    /// The "Referenz" window is open.
    pub open: bool,
}

impl EditorApp {
    /// Load (or reload) the reference picture into a texture.
    fn reference_texture(&mut self, ctx: &egui::Context) -> Option<(egui::TextureId, [u32; 2])> {
        let r = self.doc.reference.as_ref()?;
        let path = core_ref::resolve(self.path.as_deref(), r);
        let frame = matches!(r.source, RefSource::Document(_)).then_some(self.frame);
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let key = Key { path, frame, mtime };
        let view = &mut self.canvas.reference;
        if view.key.as_ref() != Some(&key) {
            match core_ref::load(self.path.as_deref(), r, self.frame) {
                Ok(img) => {
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [img.width as usize, img.height as usize],
                        &img.pixels,
                    );
                    match &mut view.tex {
                        Some(t) => t.set(ci, TextureOptions::NEAREST),
                        None => view.tex = Some(ctx.load_texture("reference", ci, TextureOptions::NEAREST)),
                    }
                    view.size = Some([img.width, img.height]);
                    view.error = None;
                }
                Err(e) => {
                    view.tex = None;
                    view.size = None;
                    view.error = Some(e);
                }
            }
            view.key = Some(key);
        }
        Some((view.tex.as_ref()?.id(), view.size?))
    }

    /// Screen rect of the reference for a canvas at `canvas` with `cell`
    /// points per pixel.
    fn reference_rect(r: &Reference, size: [u32; 2], canvas: Rect, cell: f32) -> Rect {
        Rect::from_min_size(
            canvas.min + Vec2::new(r.x, r.y) * cell,
            Vec2::new(size[0] as f32, size[1] as f32) * r.scale * cell,
        )
    }

    /// Paint the reference if it belongs to this layer of the picture
    /// (`above` = over the pixels). Clipped to the canvas.
    pub(super) fn paint_reference(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        canvas: Rect,
        cell: f32,
        above: bool,
    ) {
        let Some(r) = self.doc.reference.clone() else { return };
        if !r.visible || r.above != above {
            return;
        }
        let Some((tex, size)) = self.reference_texture(ctx) else { return };
        let rect = Self::reference_rect(&r, size, canvas, cell);
        let tint = Color32::from_white_alpha((r.opacity * 255.0).round() as u8);
        painter.with_clip_rect(canvas.intersect(painter.clip_rect())).image(
            tex,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }

    /// "Verschieben": a primary drag on the canvas moves the reference.
    /// Returns true when the input was used for that.
    pub(super) fn reference_input(&mut self, resp: &egui::Response, pointer: &egui::PointerState, cell: f32) -> bool {
        if !self.canvas.reference.moving {
            return false;
        }
        let Some(r) = self.doc.reference.as_ref() else {
            self.canvas.reference.moving = false;
            return false;
        };
        if r.locked {
            if resp.hovered() && pointer.primary_pressed() {
                self.status = "Die Referenz ist gesperrt (Häkchen „sperren“ im Fenster Referenz)".into();
            }
            return true;
        }
        if resp.hovered() && pointer.primary_pressed() {
            if let Some(p) = pointer.interact_pos() {
                self.canvas.reference.drag = Some((p, r.x, r.y));
                let desc = "Referenz verschoben".to_string();
                self.history.push(self.doc.clone(), desc.clone());
                self.unsaved_steps.push(desc);
            }
        }
        if let Some((start, x0, y0)) = self.canvas.reference.drag {
            if let Some(p) = pointer.latest_pos() {
                // whole canvas pixels, so the reference lines up with the grid
                let d = (p - start) / cell;
                let (x, y) = (x0 + d.x.round(), y0 + d.y.round());
                let r = self.doc.reference.as_mut().unwrap();
                if (r.x, r.y) != (x, y) {
                    r.x = x;
                    r.y = y;
                    self.touch();
                }
            }
            if !pointer.primary_down() {
                self.canvas.reference.drag = None;
                self.history.discard_if_unchanged(&self.doc);
            }
        }
        true
    }

    /// Set a new reference from a path typed or picked in the panel.
    pub(super) fn load_reference(&mut self, path: &str) {
        let rel = self.relative_to_doc(Path::new(path.trim()));
        let source = match core_ref::source_for(&rel) {
            Ok(s) => s,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let mut r = match self.doc.reference.clone() {
            Some(old) => Reference { source, ..old },
            None => Reference::new(source),
        };
        if let Err(e) = core_ref::load(self.path.as_deref(), &r, self.frame) {
            self.status = format!("Referenz nicht geladen: {e}");
            return;
        }
        r.visible = true;
        self.canvas.reference.key = None;
        self.edit(format!("Referenz: {rel}"), |d| d.reference = Some(r));
        self.status = format!("Referenz {rel} geladen (wird nie exportiert)");
    }

    /// `p` relative to the document's folder when it lies inside it.
    fn relative_to_doc(&self, p: &Path) -> String {
        let dir = self.path.as_deref().and_then(Path::parent);
        match dir.and_then(|d| p.strip_prefix(d).ok()) {
            Some(rel) if p.is_absolute() => rel.to_string_lossy().into_owned(),
            _ => p.to_string_lossy().into_owned(),
        }
    }

    /// The "Referenz" window (menu Ansicht, or the button at the layer).
    pub(super) fn reference_window(&mut self, ctx: &egui::Context) {
        if !self.canvas.reference.open || self.map.is_some() {
            return;
        }
        let mut open = true;
        let at = self.canvas.area.left_bottom() + Vec2::new(12.0, -300.0);
        egui::Window::new("Referenz")
            .open(&mut open)
            .default_pos(at)
            .resizable(false)
            .collapsible(true)
            .show(ctx, |ui| self.reference_props(ui));
        self.canvas.reference.open &= open;
    }

    /// Contents of the "Referenz" window.
    pub(super) fn reference_props(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(
                "Vorlage unter oder über der Leinwand. Wird nie exportiert und nie in Pixel geschrieben.",
            )
            .weak()
            .small(),
        );
        ui.horizontal(|ui| {
            let lbl = ui.label("Datei");
            ui.add(
                egui::TextEdit::singleline(&mut self.canvas.reference.draft)
                    .hint_text("bild.png / anderes.pxe.json")
                    .desired_width(130.0),
            )
            .labelled_by(lbl.id);
            if ui.button("Laden").on_hover_text("PNG, JPG oder ein Dokument (relativ zum Dokument)").clicked() {
                let p = self.canvas.reference.draft.clone();
                self.load_reference(&p);
            }
            if ui.button("…").on_hover_text("Datei wählen").clicked() && !cfg!(test) {
                let mut dlg = rfd::FileDialog::new()
                    .add_filter("Bild oder Dokument", &["png", "jpg", "jpeg", "json", "aseprite", "ase"]);
                if let Some(dir) = self.path.as_deref().and_then(Path::parent) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.pick_file() {
                    let p = p.to_string_lossy().into_owned();
                    self.canvas.reference.draft = self.relative_to_doc(Path::new(&p));
                    self.load_reference(&p);
                }
            }
        });
        let Some(r) = self.doc.reference.clone() else {
            ui.label(egui::RichText::new("keine Referenz").weak());
            return;
        };
        ui.label(format!(
            "{}: {}{}",
            if matches!(r.source, RefSource::Image(_)) { "Bild" } else { "Dokument" },
            r.source.path(),
            self.canvas.reference.size.map_or(String::new(), |[w, h]| format!(" ({w}×{h})"))
        ));
        if let Some(e) = &self.canvas.reference.error {
            ui.colored_label(Color32::from_rgb(230, 110, 90), format!("nicht geladen: {e}"));
        }
        ui.horizontal(|ui| {
            for (label, desc_on, desc_off, get) in [
                ("zeigen", "eingeblendet", "ausgeblendet", 0),
                ("sperren", "gesperrt", "entsperrt", 1),
                ("über den Pixeln", "über die Pixel", "unter die Pixel", 2),
            ] {
                let mut v = match get {
                    0 => r.visible,
                    1 => r.locked,
                    _ => r.above,
                };
                let hover = match get {
                    0 => "Referenz ein-/ausblenden",
                    1 => "Gesperrt: lässt sich nicht verschieben oder skalieren",
                    _ => "Über den Pixeln statt darunter",
                };
                if ui.checkbox(&mut v, label).on_hover_text(hover).changed() {
                    let desc = format!("Referenz {}", if v { desc_on } else { desc_off });
                    self.edit(desc, |d| {
                        let r = d.reference.as_mut().unwrap();
                        match get {
                            0 => r.visible = v,
                            1 => r.locked = v,
                            _ => r.above = v,
                        }
                    });
                }
            }
        });
        let locked = r.locked;
        let mut moving = self.canvas.reference.moving;
        if ui.add_enabled(!locked, egui::Checkbox::new(&mut moving, "Verschieben (auf der Leinwand ziehen)")).changed()
        {
            self.canvas.reference.moving = moving;
        }
        if locked {
            self.canvas.reference.moving = false;
        }
        // numeric fields: one undo step per drag (like the layer opacity)
        let field = |ui: &mut egui::Ui, app: &mut EditorApp, label: &str, which: u8| {
            let r = app.doc.reference.as_ref().unwrap();
            let mut v = match which {
                0 => r.x,
                1 => r.y,
                2 => r.scale,
                _ => r.opacity,
            };
            let lbl = ui.label(label);
            let widget = match which {
                0 | 1 => egui::DragValue::new(&mut v).speed(0.25).suffix(" px"),
                2 => egui::DragValue::new(&mut v).speed(0.01).range(REF_SCALE).prefix("×"),
                _ => egui::DragValue::new(&mut v).speed(0.01).range(0.0..=1.0),
            };
            let resp = ui.add_enabled(!locked || which == 3, widget).labelled_by(lbl.id);
            if resp.drag_started() || (resp.changed() && !resp.dragged()) {
                let desc = format!("Referenz {label}");
                app.history.push(app.doc.clone(), desc.clone());
                app.unsaved_steps.push(desc);
            }
            if resp.changed() {
                let r = app.doc.reference.as_mut().unwrap();
                match which {
                    0 => r.x = v,
                    1 => r.y = v,
                    2 => r.scale = v.clamp(*REF_SCALE.start(), *REF_SCALE.end()),
                    _ => r.opacity = v.clamp(0.0, 1.0),
                }
                app.touch();
            }
        };
        ui.horizontal(|ui| {
            field(ui, self, "X", 0);
            field(ui, self, "Y", 1);
        });
        ui.horizontal(|ui| {
            field(ui, self, "Größe", 2);
            field(ui, self, "Deckkraft", 3);
        });
        if ui.button("Referenz entfernen").clicked() {
            self.canvas.reference.moving = false;
            self.edit("Referenz entfernt", |d| d.reference = None);
        }
    }
}
