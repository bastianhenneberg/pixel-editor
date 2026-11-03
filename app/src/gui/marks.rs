//! "KI zeigen" marks: numbered areas on any frames that the user points out
//! to the AI assistant, each with an optional note ("brauner Block weg").
//!
//! With the tool (K): drag = new area, click = new pixel mark, press inside
//! a mark = select it (its note field opens; dragging moves it), the handles
//! on its corners and edges resize it, right click removes the mark under
//! the pointer. Ctrl+Shift+K adds the selection (or the whole frame).
//! With every tool, a right click on a mark's label opens a menu to
//! remove just that mark.
//!
//! The marks live in `.pxe-history/<file>/marks.json` (see `crate::view`).
//! Every change re-reads the file first, so marks the assistant removed
//! (`clear_marks`) never come back; the file is also polled like the
//! document.

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use pixel_core::Rect as PxRect;

use super::canvas::{drag_rect, PIN_COLOR};
use super::{mtime, EditorApp, Tool};
use crate::view::{self, Mark, Marks};

/// A running drag of the "KI zeigen" tool, in art pixels.
#[derive(Clone, Copy, Debug)]
pub enum MarkDrag {
    New { start: (i32, i32), last: (i32, i32) },
    Move { n: u32, grab: (i32, i32), orig: PxRect, moved: bool },
    Resize { n: u32, orig: PxRect, edges: Edges },
}

/// Which edges of a mark a resize handle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// The open right-click menu of a mark label.
#[derive(Clone, Copy, Debug)]
pub struct MarkMenu {
    pub n: u32,
    pub pos: Pos2,
    /// Opened this frame: the press that opened it does not close it.
    pub fresh: bool,
}

/// Grab distance of the resize handles, in points.
const HANDLE: f32 = 5.0;
/// Characters of a note shown on the canvas.
const NOTE_CHARS: usize = 24;

fn shorten(note: &str) -> String {
    if note.chars().count() <= NOTE_CHARS {
        note.to_string()
    } else {
        format!("{}…", note.chars().take(NOTE_CHARS - 1).collect::<String>())
    }
}

/// The four sides of `r` as filled rects of width `w`.
fn outline(r: Rect, w: f32, c: Color32) -> [(Rect, Color32); 4] {
    [
        (Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + w)), c),
        (Rect::from_min_max(Pos2::new(r.min.x, r.max.y - w), r.max), c),
        (Rect::from_min_max(r.min, Pos2::new(r.min.x + w, r.max.y)), c),
        (Rect::from_min_max(Pos2::new(r.max.x - w, r.min.y), r.max), c),
    ]
}

impl EditorApp {
    fn full_frame(&self) -> PxRect {
        PxRect::new(0, 0, self.doc.width, self.doc.height)
    }

    /// The area of a mark (the whole frame for a frame mark).
    pub(super) fn mark_rect(&self, m: &Mark) -> PxRect {
        m.rect.map(PxRect::from).unwrap_or(self.full_frame())
    }

    /// Change the marks: re-read the file, apply `f`, write it back.
    pub(super) fn update_marks<R>(&mut self, f: impl FnOnce(&mut Marks) -> R) -> Option<R> {
        let Some(path) = self.path.clone() else {
            self.status = "KI zeigen: erst speichern, die KI liest nur Dateien".into();
            return None;
        };
        let mut marks = view::read_marks(&path);
        let r = f(&mut marks);
        match view::write_marks(&path, &marks) {
            Ok(()) => {
                self.marks = marks;
                self.marks_mtime = mtime(&view::marks_file(&path));
                if self.mark_sel.is_some_and(|n| !self.marks.marks.iter().any(|m| m.n == n)) {
                    self.mark_sel = None;
                }
                // publish the view right away too
                self.share.last = None;
                self.share.last_write = None;
                Some(r)
            }
            Err(e) => {
                self.status = format!("KI zeigen fehlgeschlagen: {e}");
                None
            }
        }
    }

    /// Load the marks again when the file changed (e.g. `clear_marks`).
    pub(super) fn poll_marks(&mut self, path: &std::path::Path) {
        let m = mtime(&view::marks_file(path));
        if m != self.marks_mtime {
            self.marks_mtime = m;
            self.marks = view::read_marks(path);
            if self.mark_sel.is_some_and(|n| !self.marks.marks.iter().any(|m| m.n == n)) {
                self.mark_sel = None;
            }
        }
    }

    /// "KI zeigen" (button, Ctrl+Shift+K): mark the selection, or the
    /// whole frame without one.
    pub(super) fn point_for_claude(&mut self) {
        let rect = self.selection_on_canvas();
        let what = match rect {
            Some(r) => format!("Auswahl {}", r.describe()),
            None => "ganzer Frame".into(),
        };
        self.add_mark(rect, what);
    }

    /// Add a mark on the current frame and layer (None = the whole frame)
    /// and select it, so its note can be typed right away.
    pub(super) fn add_mark(&mut self, rect: Option<PxRect>, what: String) {
        let mark = Mark {
            n: 0,
            frame: view::frame_ref(&self.doc, self.frame),
            layer: view::layer_ref(&self.doc, self.layer),
            rect: rect.map(Into::into),
            note: String::new(),
            time_ms: view::now_ms(),
            pid: std::process::id(),
            dirty: self.dirty,
        };
        if let Some(n) = self.update_marks(|m| m.add(mark)) {
            self.status = format!("KI-Markierung #{n}: {what} · {}", self.here());
            if self.dirty {
                self.status += " (ungespeichert: die KI sieht den Stand der Datei)";
            }
            self.select_mark(Some(n));
            self.canvas.focus_note = true;
        }
    }

    pub(super) fn select_mark(&mut self, n: Option<u32>) {
        self.mark_sel = n;
        self.note_draft =
            n.and_then(|n| self.marks.marks.iter().find(|m| m.n == n)).map(|m| m.note.clone()).unwrap_or_default();
    }

    pub(super) fn remove_mark(&mut self, n: u32) {
        if self.update_marks(|m| m.remove_where(|x| x.n == n)).is_some() {
            self.status = format!("KI-Markierung #{n} entfernt");
        }
    }

    pub(super) fn clear_marks_of_frame(&mut self) {
        let f = self.frame;
        if let Some(gone) = self.update_marks(|m| m.remove_where(|x| x.frame.index == f)) {
            self.status = format!("{} KI-Markierungen von Frame {} entfernt", gone.len(), self.doc.frame_label(f));
        }
    }

    pub(super) fn clear_all_marks(&mut self) {
        if let Some(gone) = self.update_marks(|m| m.remove_where(|_| true)) {
            self.status = format!("Alle KI-Markierungen entfernt ({})", gone.len());
        }
    }

    fn save_note(&mut self, n: u32) {
        let note = self.note_draft.trim().to_string();
        if self.marks.marks.iter().any(|m| m.n == n && m.note != note) {
            self.update_marks(|m| {
                if let Some(x) = m.get_mut(n) {
                    x.note = note.clone();
                }
            });
            self.status = format!("KI-Markierung #{n}: Notiz {note:?}");
        }
    }

    /// The mark under pixel `p` on the current frame: the smallest one
    /// (frame marks only if `frame_marks`).
    fn mark_at(&self, p: (i32, i32), frame_marks: bool) -> Option<u32> {
        self.marks
            .on_frame(self.frame)
            .filter(|m| frame_marks || m.rect.is_some())
            .filter(|m| self.mark_rect(m).contains(p.0, p.1))
            .min_by_key(|m| (self.mark_rect(m).w * self.mark_rect(m).h, u32::MAX - m.n))
            .map(|m| m.n)
    }

    /// Screen rect of a mark on the canvas.
    pub(super) fn mark_screen(&self, m: &Mark) -> Rect {
        let r = self.mark_rect(m);
        let s = self.canvas.screen;
        let cell = s.width() / self.doc.width as f32;
        Rect::from_min_size(s.min + Vec2::new(r.x as f32, r.y as f32) * cell, Vec2::new(r.w as f32, r.h as f32) * cell)
    }

    /// A resize handle of the selected mark at screen position `sp`.
    fn handle_at(&self, sp: Pos2) -> Option<(u32, Edges)> {
        let n = self.mark_sel?;
        let m = self.marks.on_frame(self.frame).find(|m| m.n == n)?;
        let sr = self.mark_screen(m);
        if !sr.expand(HANDLE).contains(sp) {
            return None;
        }
        let (dl, dr, dt, db) =
            ((sp.x - sr.min.x).abs(), (sp.x - sr.max.x).abs(), (sp.y - sr.min.y).abs(), (sp.y - sr.max.y).abs());
        // on a small mark both sides are near: the closer one wins
        let edges = Edges {
            left: dl <= HANDLE && dl <= dr,
            right: dr <= HANDLE && dr < dl,
            top: dt <= HANDLE && dt <= db,
            bottom: db <= HANDLE && db < dt,
        };
        (edges.left || edges.right || edges.top || edges.bottom).then_some((n, edges))
    }

    /// Mouse input of the "KI zeigen" tool on the canvas.
    pub(super) fn mark_input(&mut self, resp: &egui::Response, pointer: &egui::PointerState, pos: Option<(i32, i32)>) {
        let primary = resp.hovered() && pointer.primary_pressed();
        let secondary = resp.hovered() && pointer.secondary_pressed();
        if primary || secondary {
            if self.canvas.note_focused {
                return; // this click only closes the note field (which saves it)
            }
            let Some(p) = pos else { return };
            if secondary {
                if let Some(n) = self.mark_at(p, true) {
                    self.remove_mark(n);
                }
                return;
            }
            if let Some((n, edges)) = pointer.latest_pos().and_then(|sp| self.handle_at(sp)) {
                let orig = self.marks.marks.iter().find(|m| m.n == n).map(|m| self.mark_rect(m)).unwrap();
                self.canvas.mark_drag = Some(MarkDrag::Resize { n, orig, edges });
                return;
            }
            if let Some(n) = self.mark_at(p, false) {
                self.select_mark(Some(n));
                let orig = self.marks.marks.iter().find(|m| m.n == n).map(|m| self.mark_rect(m)).unwrap();
                self.canvas.mark_drag =
                    Some(MarkDrag::Move { n, grab: (p.0 - orig.x, p.1 - orig.y), orig, moved: false });
                return;
            }
            self.select_mark(None);
            self.canvas.mark_drag = Some(MarkDrag::New { start: p, last: p });
            return;
        }
        let Some(mut drag) = self.canvas.mark_drag else { return };
        let (w, h) = (self.doc.width as i32, self.doc.height as i32);
        if let Some(p) = pos {
            match &mut drag {
                MarkDrag::New { last, .. } => *last = p,
                MarkDrag::Move { n, grab, orig, moved } => {
                    let x = (p.0 - grab.0).clamp(0, (w - orig.w as i32).max(0));
                    let y = (p.1 - grab.1).clamp(0, (h - orig.h as i32).max(0));
                    let r = PxRect::new(x, y, orig.w, orig.h);
                    *moved |= r != *orig;
                    self.set_mark_rect_live(*n, r);
                }
                MarkDrag::Resize { n, orig, edges } => {
                    let (px, py) = (p.0.clamp(0, w - 1), p.1.clamp(0, h - 1));
                    let (mut x0, mut y0) = (orig.x, orig.y);
                    let (mut x1, mut y1) = (orig.x + orig.w as i32 - 1, orig.y + orig.h as i32 - 1);
                    if edges.left {
                        x0 = px.min(x1);
                    }
                    if edges.right {
                        x1 = px.max(x0);
                    }
                    if edges.top {
                        y0 = py.min(y1);
                    }
                    if edges.bottom {
                        y1 = py.max(y0);
                    }
                    self.set_mark_rect_live(*n, drag_rect((x0, y0), (x1, y1)));
                }
            }
        }
        self.canvas.mark_drag = Some(drag);
        if pointer.primary_down() {
            return;
        }
        self.canvas.mark_drag = None;
        match drag {
            MarkDrag::New { start, last } => self.finish_new_mark(start, last),
            MarkDrag::Move { moved: false, .. } => self.canvas.focus_note = true, // a click: edit the note
            MarkDrag::Move { n, .. } | MarkDrag::Resize { n, .. } => {
                let r = self.marks.marks.iter().find(|m| m.n == n).map(|m| self.mark_rect(m));
                if let Some(r) = r {
                    let full = r == self.full_frame();
                    self.update_marks(|m| {
                        if let Some(x) = m.get_mut(n) {
                            x.rect = (!full).then(|| r.into());
                        }
                    });
                    self.status = format!("KI-Markierung #{n}: {}", r.describe());
                }
            }
        }
    }

    /// Show a mark's new rect while dragging (saved on release).
    fn set_mark_rect_live(&mut self, n: u32, r: PxRect) {
        if let Some(m) = self.marks.get_mut(n) {
            m.rect = Some(r.into());
        }
    }

    /// A new-mark drag ended: the area (or the pixel of a click), clipped
    /// to the canvas.
    fn finish_new_mark(&mut self, start: (i32, i32), end: (i32, i32)) {
        let r = drag_rect(start, end);
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = (r.x + r.w as i32).min(self.doc.width as i32);
        let y1 = (r.y + r.h as i32).min(self.doc.height as i32);
        if x1 <= x0 || y1 <= y0 {
            self.status = "KI zeigen: außerhalb der Leinwand".into();
            return;
        }
        let r = PxRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
        let what =
            if r.w == 1 && r.h == 1 { format!("Punkt x {x0}, y {y0}") } else { format!("Bereich {}", r.describe()) };
        self.add_mark(Some(r), what);
    }

    /// Outlines, handles and labels of the marks on the current frame (and
    /// the area being dragged), for the canvas.
    #[allow(clippy::type_complexity)]
    pub(super) fn mark_overlays(
        &self,
        to_screen: impl Fn(PxRect) -> Rect,
        canvas: Rect,
        px: f32,
    ) -> (Vec<(Rect, Color32)>, Vec<(Pos2, String, Color32)>, Vec<u32>) {
        let (mut rects, mut labels, mut ns) = (Vec::new(), Vec::new(), Vec::new());
        for m in self.marks.on_frame(self.frame) {
            let sr = to_screen(self.mark_rect(m)).expand(2.0 * px); // the marked pixels stay visible
            rects.extend(outline(sr.expand(px), px, Color32::BLACK));
            rects.extend(outline(sr, 2.0 * px, PIN_COLOR));
            let mut label = format!("#{}", m.n);
            if !m.note.is_empty() {
                label += &format!(" {}", shorten(&m.note));
            }
            let at = if m.rect.is_none() { canvas.left_top() } else { sr.left_top() };
            labels.push((at, label, PIN_COLOR));
            ns.push(m.n);
            if self.mark_sel == Some(m.n) && self.tool == Tool::Point {
                rects.extend(outline(sr.shrink(2.0 * px), px, Color32::WHITE));
                let (l, r, t, b) = (sr.min.x, sr.max.x, sr.min.y, sr.max.y);
                let (cx, cy) = (sr.center().x, sr.center().y);
                for c in [(l, t), (cx, t), (r, t), (l, cy), (r, cy), (l, b), (cx, b), (r, b)] {
                    let h = Rect::from_center_size(Pos2::new(c.0, c.1), Vec2::splat(7.0));
                    rects.push((h, PIN_COLOR));
                    rects.push((h.shrink(1.5), Color32::WHITE));
                }
            }
        }
        if let Some(MarkDrag::New { start, last }) = self.canvas.mark_drag {
            rects.extend(outline(to_screen(drag_rect(start, last)), px, PIN_COLOR));
        }
        (rects, labels, ns)
    }

    /// A right click on a mark label (any tool) opens its menu; true when
    /// this press belongs to the label or the open menu, not to the tool.
    pub(super) fn mark_label_input(&mut self, resp: &egui::Response, pointer: &egui::PointerState) -> bool {
        if self.canvas.mark_menu.is_some() {
            // the menu is open: a press goes to it (or closes it)
            return pointer.any_pressed() || pointer.any_down();
        }
        if !(resp.hovered() && pointer.secondary_pressed()) || self.canvas.note_focused {
            return false;
        }
        let Some(sp) = pointer.latest_pos() else { return false };
        // the topmost label wins (painted last)
        let Some(&(_, n)) = self.canvas.mark_labels.iter().rev().find(|(r, _)| r.contains(sp)) else {
            return false;
        };
        self.canvas.mark_menu = Some(MarkMenu { n, pos: sp, fresh: true });
        true
    }

    /// The right-click menu of a mark label: remove that mark.
    pub(super) fn mark_menu_ui(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.canvas.mark_menu else { return };
        let Some(m) = self.marks.on_frame(self.frame).find(|m| m.n == menu.n) else {
            self.canvas.mark_menu = None; // removed meanwhile (clear_marks) or other frame
            return;
        };
        let mut title = format!("#{}", m.n);
        if !m.note.is_empty() {
            title += &format!(" {}", shorten(&m.note));
        }
        let mut remove = false;
        let area = egui::Area::new(egui::Id::unique("mark_menu"))
            .fixed_pos(menu.pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.colored_label(PIN_COLOR, title);
                    remove = ui.button("Entfernen").clicked();
                });
            });
        let (pressed, escape, at) =
            ctx.input(|i| (i.pointer.any_pressed(), i.key_pressed(egui::Key::Escape), i.pointer.interact_pos()));
        let outside = pressed && !menu.fresh && at.is_some_and(|p| !area.response.rect.contains(p));
        if remove {
            self.canvas.mark_menu = None;
            self.remove_mark(menu.n);
        } else if escape || outside {
            self.canvas.mark_menu = None;
        } else if let Some(mm) = &mut self.canvas.mark_menu {
            mm.fresh = false;
        }
    }

    /// The note field of the selected mark, under it on the canvas. Enter
    /// or clicking elsewhere saves it.
    pub(super) fn note_field(&mut self, ctx: &egui::Context) {
        let mark = self.mark_sel.and_then(|n| self.marks.on_frame(self.frame).find(|m| m.n == n));
        let Some(m) = mark.filter(|_| self.tool == Tool::Point && !self.clean_view && self.canvas.mark_drag.is_none())
        else {
            self.canvas.note_focused = false;
            return;
        };
        let n = m.n;
        let sr = self.mark_screen(m);
        let area = self.canvas.area;
        let mut pos = Pos2::new(sr.min.x, sr.max.y + 6.0);
        if pos.y + 34.0 > area.max.y {
            pos.y = sr.min.y - 40.0;
        }
        pos.x = pos.x.clamp(area.min.x + 4.0, (area.max.x - 260.0).max(area.min.x + 4.0));
        pos.y = pos.y.clamp(area.min.y + 4.0, (area.max.y - 34.0).max(area.min.y + 4.0));
        let mut focused = false;
        let mut save = false;
        egui::Area::new(egui::Id::unique("mark_note")).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(PIN_COLOR, format!("#{n}"));
                    let out = egui::TextEdit::singleline(&mut self.note_draft)
                        .hint_text("Notiz für die KI (Enter)")
                        .desired_width(200.0)
                        .show(ui);
                    let r = out.response;
                    if std::mem::take(&mut self.canvas.focus_note) {
                        r.request_focus();
                        // keep typing after the note, not before it
                        let mut state = out.state;
                        let end = egui::text::CCursor::new(self.note_draft.chars().count());
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                        state.store(ui.ctx(), r.id);
                    }
                    focused = r.has_focus();
                    save = r.lost_focus();
                });
            });
        });
        self.canvas.note_focused = focused;
        if save {
            self.save_note(n);
        }
    }

    /// Menu entries (Werkzeuge, Ansicht).
    pub(super) fn mark_menu_entries(&mut self, ui: &mut egui::Ui) {
        if ui.button("KI zeigen: Auswahl oder ganzen Frame  (Strg+Umschalt+K)").clicked() {
            self.point_for_claude();
        }
        let here = self.marks.on_frame(self.frame).count();
        if ui.add_enabled(here > 0, egui::Button::new("KI-Markierungen dieses Frames entfernen")).clicked() {
            self.clear_marks_of_frame();
        }
        if ui.add_enabled(!self.marks.marks.is_empty(), egui::Button::new("Alle KI-Markierungen entfernen")).clicked() {
            self.clear_all_marks();
        }
    }

    /// Buttons under the tool: remove the marks of this frame / all.
    pub(super) fn mark_buttons(&mut self, ui: &mut egui::Ui) {
        let (here, all) = (self.marks.on_frame(self.frame).count(), self.marks.marks.len());
        if all == 0 {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if here > 0 && ui.small_button(format!("Frame: {here} entfernen")).clicked() {
                self.clear_marks_of_frame();
            }
            if ui.small_button(format!("Alle {all} entfernen")).clicked() {
                self.clear_all_marks();
            }
        });
    }
}
