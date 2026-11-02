//! Kachel-Modus (tiled mode, as in Aseprite): the canvas shows the document
//! repeated around itself, and every tool works in all copies. A stroke
//! that leaves the tile on one side comes back in on the opposite one,
//! because every point is wrapped (modulo) into the real image; fills run
//! across the edges. Remembered per document in the settings.

use std::path::Path;

use eframe::egui::{self, Key, Modifiers, RichText};
use pixel_core::draw;
use serde::{Deserialize, Serialize};

use super::{EditorApp, Tool};

/// Shortcut that cycles the mode.
pub const TILE_KEY: &str = "Alt+T";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileMode {
    #[default]
    Off,
    /// Copies left and right: the tile repeats side by side.
    Horizontal,
    /// Copies above and below.
    Vertical,
    Both,
}

impl TileMode {
    pub const ALL: [TileMode; 4] = [TileMode::Off, TileMode::Horizontal, TileMode::Vertical, TileMode::Both];

    pub fn label(self) -> &'static str {
        match self {
            TileMode::Off => "Aus",
            TileMode::Horizontal => "Horizontal",
            TileMode::Vertical => "Vertikal",
            TileMode::Both => "Beide",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            TileMode::Off => "nur das Bild",
            TileMode::Horizontal => "Kopien links und rechts",
            TileMode::Vertical => "Kopien oben und unten",
            TileMode::Both => "3×3 Kopien",
        }
    }

    pub fn wraps_x(self) -> bool {
        matches!(self, TileMode::Horizontal | TileMode::Both)
    }

    pub fn wraps_y(self) -> bool {
        matches!(self, TileMode::Vertical | TileMode::Both)
    }

    /// Copies across and down (1 or 3).
    pub fn repeats(self) -> (i32, i32) {
        (if self.wraps_x() { 3 } else { 1 }, if self.wraps_y() { 3 } else { 1 })
    }

    /// Aus → Beide → Horizontal → Vertikal: the common case first.
    fn next(self) -> TileMode {
        match self {
            TileMode::Off => TileMode::Both,
            TileMode::Both => TileMode::Horizontal,
            TileMode::Horizontal => TileMode::Vertical,
            TileMode::Vertical => TileMode::Off,
        }
    }
}

/// Does the file name or a folder on its path say "tile" (tiles32,
/// tileset, Tiles/…)? Such documents start in the Kachel-Modus.
pub fn suggests_tiles(path: &Path) -> bool {
    path.iter().any(|c| c.to_string_lossy().to_lowercase().contains("tile"))
}

/// Key of a document in the remembered modes.
pub fn memory_key(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

impl EditorApp {
    /// Bring a canvas point into the image along the tiled axes.
    pub(super) fn wrap(&self, (x, y): (i32, i32)) -> (i32, i32) {
        let (w, h) = (self.doc.width as i32, self.doc.height as i32);
        let m = self.tile_mode;
        (if m.wraps_x() { x.rem_euclid(w) } else { x }, if m.wraps_y() { y.rem_euclid(h) } else { y })
    }

    /// Inside the shown copies (always true for points in the image).
    pub(super) fn in_tiled_view(&self, (x, y): (i32, i32)) -> bool {
        let (w, h) = (self.doc.width as i32, self.doc.height as i32);
        let (nx, ny) = self.tile_mode.repeats();
        let (x0, y0) = (-(nx / 2) * w, -(ny / 2) * h);
        x >= x0 && y >= y0 && x < x0 + nx * w && y < y0 + ny * h
    }

    /// Points of a selection gesture, wrapped into the image; points outside
    /// the shown copies are dropped.
    pub(super) fn wrap_points(&self, pts: impl IntoIterator<Item = (i32, i32)>) -> Vec<(i32, i32)> {
        let mut seen = std::collections::HashSet::new();
        pts.into_iter()
            .filter(|p| self.tile_mode == TileMode::Off || self.in_tiled_view(*p))
            .map(|p| self.wrap(p))
            .filter(|p| seen.insert(*p))
            .collect()
    }

    /// The pixels a drawing tool changes for the canvas points `pts`: wrapped
    /// into the image, mirrored by the symmetry, wrapped again (an axis off
    /// centre mirrors past the edge), limited to what may be painted.
    pub(super) fn paint_points(&self, pts: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
        let pts = self.wrap_points(pts);
        let mut seen = std::collections::HashSet::new();
        self.sym_points(pts)
            .into_iter()
            .map(|p| self.wrap(p))
            .filter(|&(x, y)| seen.insert((x, y)) && self.may_paint(x, y))
            .collect()
    }

    /// The area a fill from (x, y) covers, across the tiled edges.
    pub(super) fn flood(&self, x: i32, y: i32) -> Vec<(i32, i32)> {
        let m = self.tile_mode;
        draw::flood_region_wrapped(&self.doc, self.layer, self.frame, x, y, m.wraps_x(), m.wraps_y())
    }

    pub(super) fn set_tile_mode(&mut self, mode: TileMode) {
        if mode == self.tile_mode {
            return;
        }
        self.tile_mode = mode;
        self.fit_pending = true;
        if let Some(p) = &self.path {
            self.tile_memory.insert(memory_key(p), mode);
        }
        self.status = format!("Kachel-Modus: {} ({})", mode.label(), mode.hint());
    }

    /// The mode a document starts with: remembered, else on for tile-like names.
    pub(super) fn initial_tile_mode(&self, path: &Path) -> TileMode {
        match self.tile_memory.get(&memory_key(path)) {
            Some(m) => *m,
            None if suggests_tiles(path) => TileMode::Both,
            None => TileMode::Off,
        }
    }

    pub(super) fn tile_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(Key::T) && i.modifiers.matches_exact(Modifiers::ALT)) {
            self.set_tile_mode(self.tile_mode.next());
        }
    }

    /// Entries in the Ansicht menu.
    pub(super) fn tile_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(format!("Kachel-Modus: {}", self.tile_mode.label()), |ui| {
            for m in TileMode::ALL {
                let text = format!("{} – {}", m.label(), m.hint());
                if ui.radio(self.tile_mode == m, text).clicked() {
                    self.set_tile_mode(m);
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new(format!("{TILE_KEY} schaltet weiter")).small().weak());
            ui.checkbox(&mut self.preview.settings.tiled, "Vorschau auch gekachelt");
        });
    }

    /// The Kachel-Modus line in the tool panel (drawing and selection tools).
    pub(super) fn tile_panel(&mut self, ui: &mut egui::Ui) {
        if self.tool == Tool::Point || self.tool == Tool::Eyedropper {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Kacheln:");
            for m in TileMode::ALL {
                if ui.selectable_label(self.tile_mode == m, m.label()).on_hover_text(m.hint()).clicked() {
                    self.set_tile_mode(m);
                }
            }
            ui.label(RichText::new(TILE_KEY).small().weak());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_names() {
        for p in ["art/tiles32.pxe.json", "Tiles/grass.pxe.json", "world/tileset.aseprite"] {
            assert!(suggests_tiles(Path::new(p)), "{p}");
        }
        for p in ["toast_warrior/walk_side.pxe.json", "art/slime.pxe.json"] {
            assert!(!suggests_tiles(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn wrap_and_symmetry() {
        let mut app = EditorApp::blank(); // 64x64
        app.tile_mode = TileMode::Horizontal;
        assert_eq!(app.wrap((-1, 5)), (63, 5));
        assert_eq!(app.wrap((70, -2)), (6, -2), "only x wraps");
        assert!(app.in_tiled_view((-64, 0)) && !app.in_tiled_view((-65, 0)) && !app.in_tiled_view((0, -1)));
        // a line across the left edge comes back in on the right
        let pts = app.paint_points(draw::line(-2, 3, 1, 3));
        assert_eq!(pts, [(62, 3), (63, 3), (0, 3), (1, 3)]);
        // with a vertical symmetry axis at x = 10 the mirror wraps too
        app.symmetry.mode = super::super::transform::SymMode::Vertical;
        app.symmetry.x2 = Some(20);
        let pts = app.paint_points(vec![(-1, 0)]);
        assert_eq!(pts, [(63, 0), (20 - 1 - 63 + 64, 0)]);
    }
}
