//! Selections (a pixel mask; the rectangle is just one way to make one),
//! moving pixels ("floating" selection) and the clipboard. All pixel
//! changes go through `edit`, so they are undoable steps with a
//! description, and they respect locks.

use pixel_core::transform::{transform_block, transformed_rect, Op};
use pixel_core::{draw, Document, Rect, TRANSPARENT};

use super::{EditorApp, Tool};

/// Selected pixels: a bounding box and which of its pixels belong to the
/// selection. The box may leave the canvas while pixels are moved.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub rect: Rect,
    /// One entry per pixel of `rect`, row by row; None = all of them.
    mask: Option<Vec<bool>>,
}

/// How a new selection gesture combines with the existing selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelOp {
    Replace,
    Add,
    Subtract,
}

impl SelOp {
    /// Shift adds, Alt or Ctrl subtracts, a plain gesture replaces.
    pub fn from_modifiers(m: eframe::egui::Modifiers) -> SelOp {
        if m.alt || m.command || m.ctrl {
            SelOp::Subtract
        } else if m.shift {
            SelOp::Add
        } else {
            SelOp::Replace
        }
    }
}

impl Selection {
    pub fn rect(rect: Rect) -> Selection {
        Selection { rect, mask: None }
    }

    /// The selection of exactly these pixels (None when there are none).
    pub fn from_points(pts: impl IntoIterator<Item = (i32, i32)>) -> Option<Selection> {
        let pts: Vec<(i32, i32)> = pts.into_iter().collect();
        let (&(x0, y0), rest) = pts.split_first()?;
        let (mut x1, mut y1, mut lx, mut ly) = (x0, y0, x0, y0);
        for &(x, y) in rest {
            lx = lx.min(x);
            ly = ly.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let rect = Rect::new(lx, ly, (x1 - lx + 1) as u32, (y1 - ly + 1) as u32);
        let mut mask = vec![false; (rect.w * rect.h) as usize];
        for (x, y) in pts {
            mask[((y - ly) as u32 * rect.w + (x - lx) as u32) as usize] = true;
        }
        Some(Selection { rect, mask: Some(mask) }.normalized())
    }

    /// A full mask becomes a plain rectangle.
    fn normalized(mut self) -> Selection {
        if self.mask.as_ref().is_some_and(|m| m.iter().all(|b| *b)) {
            self.mask = None;
        }
        self
    }

    /// A selection from a box and its mask (None = the whole box).
    pub fn with_mask(rect: Rect, mask: Option<Vec<bool>>) -> Selection {
        Selection { rect, mask }.normalized()
    }

    pub fn mask(&self) -> Option<&[bool]> {
        self.mask.as_deref()
    }

    /// Mirrored or rotated around the centre of its box.
    pub fn transformed(&self, op: Op) -> Selection {
        let mask = self.mask.as_ref().map(|m| transform_block(m, self.rect.w, self.rect.h, op));
        Selection { rect: transformed_rect(self.rect, op), mask }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        match &self.mask {
            None => true,
            Some(m) => m[((y - self.rect.y) as u32 * self.rect.w + (x - self.rect.x) as u32) as usize],
        }
    }

    /// Is pixel `i` of the bounding box (row by row) selected?
    pub fn has(&self, i: usize) -> bool {
        self.mask.as_ref().is_none_or(|m| m[i])
    }

    /// The selected pixels, row by row.
    pub fn points(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        let r = self.rect;
        (0..(r.w * r.h) as usize)
            .filter(|i| self.has(*i))
            .map(move |i| (r.x + (i as u32 % r.w) as i32, r.y + (i as u32 / r.w) as i32))
    }

    pub fn count(&self) -> usize {
        match &self.mask {
            None => (self.rect.w * self.rect.h) as usize,
            Some(m) => m.iter().filter(|b| **b).count(),
        }
    }

    /// Only the pixels on a `w` x `h` canvas (None when nothing is left).
    pub fn clipped(&self, w: u32, h: u32) -> Option<Selection> {
        let r = self.rect;
        let (x0, y0) = (r.x.max(0), r.y.max(0));
        let (x1, y1) = ((r.x + r.w as i32).min(w as i32), (r.y + r.h as i32).min(h as i32));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        if self.mask.is_none() {
            return Some(Selection::rect(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)));
        }
        Selection::from_points(self.points().filter(|&(x, y)| x >= x0 && y >= y0 && x < x1 && y < y1))
    }

    /// `old` combined with `new` (both on a `w` x `h` canvas).
    pub fn combine(old: Option<&Selection>, new: &Selection, op: SelOp, w: u32, h: u32) -> Option<Selection> {
        let old = old.filter(|_| op != SelOp::Replace);
        let result = match (old, op) {
            (None, SelOp::Subtract) => None,
            (None, _) => Some(new.clone()),
            (Some(o), SelOp::Add) => {
                Selection::from_points(o.points().chain(new.points().filter(|p| !o.contains(p.0, p.1))))
            }
            (Some(o), _) => Selection::from_points(o.points().filter(|p| !new.contains(p.0, p.1))),
        };
        result.and_then(|s| s.clipped(w, h))
    }

    /// The mask as rows of '#' (selected) and '.' (not), or None for a
    /// plain rectangle.
    pub fn mask_rows(&self) -> Option<Vec<String>> {
        let m = self.mask.as_ref()?;
        Some(
            m.chunks(self.rect.w as usize)
                .map(|row| row.iter().map(|b| if *b { '#' } else { '.' }).collect())
                .collect(),
        )
    }

    /// Edges between selected and unselected pixels: (x, y, horizontal),
    /// a unit segment starting at pixel corner (x, y).
    pub fn outline(&self) -> Vec<(i32, i32, bool)> {
        let mut out = Vec::new();
        for (x, y) in self.points() {
            if !self.contains(x, y - 1) {
                out.push((x, y, true));
            }
            if !self.contains(x, y + 1) {
                out.push((x, y + 1, true));
            }
            if !self.contains(x - 1, y) {
                out.push((x, y, false));
            }
            if !self.contains(x + 1, y) {
                out.push((x + 1, y, false));
            }
        }
        out
    }

    pub fn describe(&self) -> String {
        match self.mask {
            None => self.rect.describe(),
            Some(_) => format!("{}, {} Pixel", self.rect.describe(), self.count()),
        }
    }
}

/// The pixels inside a freehand outline (lasso): every pixel whose center
/// lies inside the polygon, plus the pixels on the path itself.
pub fn lasso_points(path: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut pts: Vec<(i32, i32)> = Vec::new();
    for (i, a) in path.iter().enumerate() {
        let b = path[(i + 1) % path.len()];
        pts.extend(draw::line(a.0, a.1, b.0, b.1));
    }
    let Some(sel) = Selection::from_points(pts.iter().copied()) else { return pts };
    let r = sel.rect;
    let inside = |px: f64, py: f64| {
        let mut c = false;
        for (i, a) in path.iter().enumerate() {
            let b = path[(i + 1) % path.len()];
            let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
            if (ay > py) != (by > py) && px < ax + (py - ay) * (bx - ax) / (by - ay) {
                c = !c;
            }
        }
        c
    };
    for y in r.y..r.y + r.h as i32 {
        for x in r.x..r.x + r.w as i32 {
            // path vertices are pixel centers, so test the center offset a hair
            if !sel.contains(x, y) && inside(x as f64 + 1e-3, y as f64 + 1e-3) {
                pts.push((x, y));
            }
        }
    }
    pts
}

/// Pixels lifted from (or pasted onto) a cel, not yet written. Pixels
/// outside the selection mask are transparent.
pub struct Floating {
    pub pixels: Vec<u8>,
    pub rect: Rect,
    /// Where the pixels came from (lifted) or None (pasted).
    pub origin: Option<Rect>,
    pub layer: usize,
    pub frame: usize,
    /// Mirrored or rotated since it was lifted or pasted.
    pub transformed: bool,
}

/// Clipboard content: keys of a w x h block and which pixels were selected.
pub struct Clip {
    pub pixels: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub mask: Option<Vec<bool>>,
}

/// Write floating pixels into `d`: in front (transparent floating pixels
/// keep what is below) or behind (only where the cel is transparent).
/// Returns how many pixels a locked region refused.
pub fn apply_floating(d: &mut Document, fl: &Floating, behind: bool) -> usize {
    let (l, f, r) = (fl.layer, fl.frame, fl.rect);
    let mut skipped = 0;
    for (i, k) in fl.pixels.iter().enumerate() {
        let (x, y) = (r.x + (i as u32 % r.w) as i32, r.y + (i as u32 / r.w) as i32);
        if *k == TRANSPARENT || !d.in_bounds(x, y) {
            continue;
        }
        if behind && d.get(l, f, x, y) != TRANSPARENT {
            continue;
        }
        if d.locked_region_at(l, f, x, y).is_some() {
            skipped += 1;
            continue;
        }
        d.set(l, f, x, y, *k);
    }
    skipped
}

impl EditorApp {
    pub(super) fn set_tool(&mut self, t: Tool) {
        if !t.selects() {
            self.commit_floating();
        }
        self.tool = t;
        self.group_tool[super::toolbar::group_of(t)] = t;
    }

    /// Strg+D: drop floating pixels where they are and select nothing.
    pub(super) fn select_none(&mut self) {
        self.commit_floating();
        self.selection = None;
    }

    /// Strg+A: the whole canvas.
    pub(super) fn select_all(&mut self) {
        self.commit_floating();
        self.selection = Some(Selection::rect(Rect::new(0, 0, self.doc.width, self.doc.height)));
    }

    /// Strg+Umschalt+I: every pixel that is not selected (nothing selected:
    /// everything).
    pub(super) fn invert_selection(&mut self) {
        self.commit_floating();
        let (w, h) = (self.doc.width as i32, self.doc.height as i32);
        let old = self.selection.take();
        let all = (0..h).flat_map(|y| (0..w).map(move |x| (x, y)));
        self.selection = Selection::from_points(all.filter(|p| !old.as_ref().is_some_and(|s| s.contains(p.0, p.1))));
        self.status = match &self.selection {
            Some(s) => format!("Auswahl umgekehrt: {}", s.describe()),
            None => "Auswahl umgekehrt: nichts mehr ausgewählt".into(),
        };
    }

    /// The selection clipped to the canvas.
    pub(super) fn selection_clipped(&self) -> Option<Selection> {
        self.selection.as_ref()?.clipped(self.doc.width, self.doc.height)
    }

    /// The bounding box of the selection, clipped to the canvas.
    pub(super) fn selection_on_canvas(&self) -> Option<Rect> {
        self.selection_clipped().map(|s| s.rect)
    }

    /// Combine the selection with `new` (Shift adds, Alt/Ctrl subtracts).
    pub(super) fn combine_selection(&mut self, base: Option<&Selection>, new: &Selection, op: SelOp) {
        self.selection = Selection::combine(base, new, op, self.doc.width, self.doc.height);
    }

    /// Keys of the selected pixels in the bounding box (others transparent).
    fn read_masked(&self, s: &Selection) -> Vec<u8> {
        let r = s.rect;
        (0..(r.w * r.h) as usize)
            .map(|i| {
                let (x, y) = (r.x + (i as u32 % r.w) as i32, r.y + (i as u32 / r.w) as i32);
                if s.has(i) {
                    self.doc.get(self.layer, self.frame, x, y)
                } else {
                    TRANSPARENT
                }
            })
            .collect()
    }

    /// Cut the selected pixels out of the cel into a floating selection.
    pub(super) fn lift_selection(&mut self) -> bool {
        if self.floating.is_some() {
            return true;
        }
        let Some(s) = self.selection_clipped() else { return false };
        if self.blocked() {
            return false;
        }
        if s.points().any(|(x, y)| self.doc.locked_region_at(self.layer, self.frame, x, y).is_some()) {
            self.status = "Die Auswahl berührt eine gesperrte Region; nicht verschoben".into();
            return false;
        }
        let pixels = self.read_masked(&s);
        let (l, f, r) = (self.layer, self.frame, s.rect);
        let desc = format!("Auswahl angehoben {} · {}", s.describe(), self.here());
        let pts: Vec<(i32, i32)> = s.points().collect();
        self.edit(desc, |d| d.set_points(l, f, &pts, TRANSPARENT));
        self.floating = Some(Floating { pixels, rect: r, origin: Some(r), layer: l, frame: f, transformed: false });
        self.selection = Some(s);
        true
    }

    pub(super) fn move_floating_to(&mut self, x: i32, y: i32) {
        if let Some(fl) = &mut self.floating {
            fl.rect.x = x;
            fl.rect.y = y;
            if let Some(s) = &mut self.selection {
                s.rect.x = x;
                s.rect.y = y;
            }
            self.canvas.invalidate();
        }
    }

    pub(super) fn nudge_selection(&mut self, dx: i32, dy: i32) {
        if self.floating.is_none() && !self.lift_selection() {
            return;
        }
        let r = self.floating.as_ref().unwrap().rect;
        self.move_floating_to(r.x + dx, r.y + dy);
    }

    /// "Vorne" / "Hinten": where moved or pasted pixels end up.
    pub(super) fn toggle_drop_behind(&mut self) {
        self.drop_behind = !self.drop_behind;
        self.canvas.invalidate();
        self.status = if self.drop_behind {
            "Ablegen: hinten (verschobene Pixel füllen nur leere Stellen)".into()
        } else {
            "Ablegen: vorne (verschobene Pixel überdecken)".into()
        };
    }

    /// Write the floating pixels, in front of or behind what is there.
    pub(super) fn commit_floating(&mut self) {
        self.commit_floating_as(self.drop_behind);
    }

    pub(super) fn commit_floating_as(&mut self, behind: bool) {
        let Some(fl) = self.floating.take() else { return };
        if let Some(why) = self.doc.edit_blocked(fl.layer, fl.frame) {
            self.status = format!("Gesperrt: {why}; Auswahl verworfen");
            return;
        }
        let r = fl.rect;
        let what = self.selection.as_ref().map_or_else(|| r.describe(), |s| s.describe());
        let mut desc = match fl.origin {
            Some(o) if o == r && fl.transformed => format!("Auswahl gedreht/gespiegelt {what} · {}", self.here()),
            Some(o) if o == r => format!("Auswahl unverändert abgelegt · {}", self.doc.frame_label(fl.frame)),
            Some(o) => format!("Auswahl verschoben um ({}, {}) {what} · {}", r.x - o.x, r.y - o.y, self.here()),
            None => format!("Eingefügt {what} · {}", self.here()),
        };
        if behind && fl.origin != Some(r) {
            desc += " · hinten";
        }
        let skipped = self.edit(desc, |d| apply_floating(d, &fl, behind));
        if skipped > 0 {
            self.status = format!("{skipped} Pixel lagen in einer gesperrten Region und wurden nicht geschrieben");
        }
        self.selection = self.selection_clipped();
        self.canvas.invalidate();
    }

    /// Esc: put lifted pixels back where they were, or drop a paste.
    pub(super) fn cancel_floating(&mut self) {
        let Some(origin) = self.floating.as_ref().map(|fl| fl.origin) else { return };
        match origin {
            Some(o) => {
                self.move_floating_to(o.x, o.y);
                self.commit_floating_as(false);
            }
            None => self.drop_floating(),
        }
    }

    /// Forget the floating pixels without writing them.
    pub(super) fn drop_floating(&mut self) {
        if self.floating.take().is_some() {
            self.canvas.invalidate();
        }
    }

    pub(super) fn copy_selection(&mut self) {
        let mask = |s: Option<&Selection>| s.and_then(|s| s.mask.clone());
        let clip = match &self.floating {
            Some(fl) => {
                Clip { pixels: fl.pixels.clone(), w: fl.rect.w, h: fl.rect.h, mask: mask(self.selection.as_ref()) }
            }
            None => {
                let Some(s) = self.selection_clipped() else {
                    self.status = "Nichts ausgewählt (Auswahl-Werkzeug M)".into();
                    return;
                };
                Clip { pixels: self.read_masked(&s), w: s.rect.w, h: s.rect.h, mask: mask(Some(&s)) }
            }
        };
        let n = clip.mask.as_ref().map_or((clip.w * clip.h) as usize, |m| m.iter().filter(|b| **b).count());
        self.status = format!("Kopiert: {n} Pixel ({}x{}) aus {}", clip.w, clip.h, self.here());
        self.clipboard = Some(clip);
    }

    pub(super) fn cut_selection(&mut self) {
        self.copy_selection();
        self.delete_selection();
    }

    pub(super) fn delete_selection(&mut self) {
        if self.floating.take().is_some() {
            self.canvas.invalidate();
            return; // already removed from the cel when lifted
        }
        let Some(s) = self.selection_clipped() else { return };
        if self.blocked() {
            return;
        }
        let (l, f) = (self.layer, self.frame);
        let desc = format!("Auswahl gelöscht {} · {}", s.describe(), self.here());
        let pts: Vec<(i32, i32)> =
            s.points().filter(|&(x, y)| self.doc.locked_region_at(l, f, x, y).is_none()).collect();
        self.edit(desc, |d| d.set_points(l, f, &pts, TRANSPARENT));
    }

    /// Paste the clipboard as a floating selection at the selection's
    /// corner (or the top-left), on the current layer and frame.
    pub(super) fn paste(&mut self) {
        self.commit_floating();
        let Some(clip) = &self.clipboard else {
            self.status = "Zwischenablage ist leer".into();
            return;
        };
        let (x, y) = self.selection.as_ref().map_or((0, 0), |s| (s.rect.x, s.rect.y));
        let rect = Rect::new(x, y, clip.w, clip.h);
        self.floating = Some(Floating {
            pixels: clip.pixels.clone(),
            rect,
            origin: None,
            layer: self.layer,
            frame: self.frame,
            transformed: false,
        });
        self.selection = Some(Selection { rect, mask: clip.mask.clone() });
        if !self.tool.selects() {
            self.tool = Tool::Select;
        }
        self.canvas.invalidate();
        self.status = "Eingefügt (schwebend): ziehen oder Pfeiltasten, Enter legt ab, Esc verwirft".into();
    }

    /// Zauberstab: the connected area of the clicked pixel's color on the
    /// current layer (4 neighbours), or every pixel of that color.
    pub(super) fn wand_points(&self, x: i32, y: i32) -> Vec<(i32, i32)> {
        if !self.doc.in_bounds(x, y) {
            return Vec::new();
        }
        if !self.wand_all {
            return self.flood(x, y);
        }
        let k = self.doc.get(self.layer, self.frame, x, y);
        let w = self.doc.width as i32;
        self.doc
            .cel(self.layer, self.frame)
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == k)
            .map(|(i, _)| (i as i32 % w, i as i32 / w))
            .collect()
    }
}
