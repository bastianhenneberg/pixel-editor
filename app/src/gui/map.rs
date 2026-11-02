//! Tilemap tabs: stamp tiles of a tileset document onto a map.
//!
//! A tab shows either a pixel document or a map (`EditorApp::map`). A map
//! tab has its own panels: tools and the tile palette on the left (legend
//! keys, then every tile of the tileset), the map in the middle, map
//! properties on the right. Pen, eraser, rectangle and fill set whole
//! tiles; the right mouse button erases. Picking a tile of the tileset
//! that has no legend key yet adds one with the next free key when it is
//! first used (part of that undo step).
//!
//! Live: the map file, the tileset and object documents are polled like a
//! document. If the tileset is open in another tab, the map shows that
//! tab's pixels, unsaved edits included, as they are drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{
    self, Color32, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use pixel_core::document::Rect as CellRect;
use pixel_core::export::godot_tiles;
use pixel_core::journal::Journal;
use pixel_core::render::Image;
use pixel_core::tilemap::{self, Assets, LegendEntry, TileMap, Tileset, EMPTY};
use pixel_core::{io, Document};

use super::{mtime, EditorApp, PathDialog};

/// Zoom levels in screen pixels per art pixel (below 1 for an overview).
pub const SCALES: [f32; 10] = [0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 12.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapTool {
    Pen,
    Eraser,
    Rect,
    Fill,
    Pick,
}

impl MapTool {
    pub const ALL: [MapTool; 5] = [MapTool::Pen, MapTool::Eraser, MapTool::Rect, MapTool::Fill, MapTool::Pick];

    pub fn label(self) -> &'static str {
        match self {
            MapTool::Pen => "Stift (B)",
            MapTool::Eraser => "Radierer (E)",
            MapTool::Rect => "Rechteck (U)",
            MapTool::Fill => "Füllen (G)",
            MapTool::Pick => "Pipette (I)",
        }
    }

    fn key(self) -> Key {
        match self {
            MapTool::Pen => Key::B,
            MapTool::Eraser => Key::E,
            MapTool::Rect => Key::U,
            MapTool::Fill => Key::G,
            MapTool::Pick => Key::I,
        }
    }
}

/// What the tools paint: a legend key, or a tile of the tileset (gets a
/// key when first used).
#[derive(Clone, Debug, PartialEq)]
pub enum Brush {
    Key(u8),
    Tile(String),
}

/// Object texture, its size and anchor (art pixels).
type ObjectTexture = (TextureHandle, [u32; 2], (i32, i32));

struct Textures {
    tiles: Vec<TextureHandle>,
    placeholders: HashMap<u8, TextureHandle>,
    objects: HashMap<u8, ObjectTexture>,
}

struct Drag {
    start: (i32, i32),
    erase: bool,
    before: TileMap,
}

pub struct MapTab {
    pub map: TileMap,
    pub path: PathBuf,
    pub assets: Assets,
    assets_rev: u64,
    tex: Option<Textures>,
    tex_rev: u64,
    /// Tileset and object files with their modification time.
    watched: Vec<(PathBuf, Option<SystemTime>)>,
    map_mtime: Option<SystemTime>,
    /// The tileset as shown by another tab: (tab, its rev).
    live: Option<(usize, u64)>,
    pub dirty: bool,
    pub conflict: bool,
    undo: Vec<(TileMap, String)>,
    redo: Vec<(TileMap, String)>,
    pub brush: Brush,
    pub tool: MapTool,
    /// Index into `SCALES`.
    pub zoom: usize,
    pan: Vec2,
    pub fit_pending: bool,
    pub grid: bool,
    drag: Option<Drag>,
    pub hover: Option<(i32, i32)>,
    /// Screen rect of the whole map and points per cell (for tests).
    pub screen: Rect,
    pub cell: Vec2,
    size_draft: [u32; 2],
    /// Cells changed from outside, highlighted until this time.
    flash: Option<(CellRect, f64)>,
    pub status: String,
}

impl MapTab {
    pub fn load(path: &Path) -> Result<MapTab, String> {
        let map = tilemap::load(path)?;
        let assets = Assets::load(&map, path);
        let brush = map
            .legend
            .iter()
            .find(|e| e.tile.is_some())
            .map(|e| Brush::Key(e.key))
            .or_else(|| assets.tileset.names.first().map(|n| Brush::Tile(n.clone())))
            .unwrap_or(Brush::Key(EMPTY));
        let mut tab = MapTab {
            size_draft: [map.width, map.height],
            map,
            path: path.to_path_buf(),
            assets,
            assets_rev: 1,
            tex: None,
            tex_rev: 0,
            watched: Vec::new(),
            map_mtime: mtime(path),
            live: None,
            dirty: false,
            conflict: false,
            undo: Vec::new(),
            redo: Vec::new(),
            brush,
            tool: MapTool::Pen,
            zoom: 2,
            pan: Vec2::ZERO,
            fit_pending: true,
            grid: true,
            drag: None,
            hover: None,
            screen: Rect::NOTHING,
            cell: Vec2::ZERO,
            flash: None,
            status: String::new(),
        };
        tab.watch();
        Ok(tab)
    }

    fn watch(&mut self) {
        let mut files = vec![self.map.tileset_path(&self.path)];
        files.extend(self.map.object_paths(&self.path).into_iter().map(|(_, p)| p));
        self.watched = files.into_iter().map(|p| (p.clone(), mtime(&p))).collect();
    }

    fn reload_assets(&mut self) {
        self.assets = Assets::load(&self.map, &self.path);
        self.assets_rev += 1;
        self.live = None;
        self.watch();
    }

    /// Scale: screen pixels per art pixel.
    pub fn scale(&self) -> f32 {
        SCALES[self.zoom]
    }

    /// The key the brush paints (adds a legend entry for a new tile).
    fn brush_key(&mut self) -> u8 {
        match self.brush.clone() {
            Brush::Key(k) => k,
            Brush::Tile(name) => self.map.key_for_tile(&name).unwrap_or(EMPTY),
        }
    }

    fn brush_label(&self) -> String {
        match &self.brush {
            Brush::Key(k) => self.map.key_label(*k),
            Brush::Tile(t) => match self.map.legend.iter().find(|e| e.tile.as_deref() == Some(t) && e.object.is_none())
            {
                Some(e) => e.label(),
                None => format!("{t} (neue Taste beim ersten Setzen)"),
            },
        }
    }

    fn push_undo(&mut self, before: TileMap, desc: String) {
        if before != self.map {
            self.undo.push((before, desc.clone()));
            self.redo.clear();
            self.dirty = true;
            self.status = desc;
            if self.map.legend != self.undo.last().unwrap().0.legend {
                self.reload_assets(); // a new key may bring an object
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some((m, desc)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.map, m), desc.clone()));
            self.dirty = true;
            self.status = format!("Rückgängig: {desc}");
        }
    }

    pub fn redo(&mut self) {
        if let Some((m, desc)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.map, m), desc.clone()));
            self.dirty = true;
            self.status = format!("Wiederholt: {desc}");
        }
    }

    pub fn save(&mut self) -> Result<(), String> {
        let journal = Journal::for_document(&self.path);
        if let Some((first, _)) = self.undo.first() {
            let _ = journal.ensure_current(first, "Zustand vor den GUI-Änderungen");
        }
        tilemap::save(&self.map, &self.path)?;
        let n = self.undo.len();
        let steps: Vec<&str> = self.undo.iter().rev().take(5).map(|(_, d)| d.as_str()).collect();
        let note = if n == 0 {
            "Benutzer: gespeichert".into()
        } else {
            format!("Benutzer: {} ({n} Schritte)", steps.join(" / "))
        };
        let _ = journal.record(&self.map, "user", &note);
        self.dirty = false;
        self.conflict = false;
        self.map_mtime = mtime(&self.path);
        self.status = format!("Gespeichert: {}", self.path.display());
        Ok(())
    }

    /// Check the map file and the tileset/object files; true if anything
    /// was reloaded.
    pub fn poll(&mut self, now: f64) -> bool {
        let mut changed = false;
        let mt = mtime(&self.path);
        if mt.is_some() && mt != self.map_mtime {
            self.map_mtime = mt;
            if self.dirty {
                self.conflict = true;
                changed = true;
            } else {
                match tilemap::load(&self.path) {
                    Ok(m) if m != self.map => {
                        let (_, bb) = tilemap::cell_diff(&self.map, &m);
                        let before = std::mem::replace(&mut self.map, m);
                        self.undo.push((before, "von außen geändert".into()));
                        let note = Journal::for_document(&self.path)
                            .entries()
                            .into_iter()
                            .rev()
                            .find(|e| e.is_step())
                            .map(|e| e.note)
                            .unwrap_or_else(|| "von außen geändert".into());
                        self.status = format!("Neu geladen: {note}");
                        self.flash = bb.map(|b| (b, now + 1.2));
                        self.size_draft = [self.map.width, self.map.height];
                        self.reload_assets();
                        changed = true;
                    }
                    Ok(_) => {}
                    Err(e) => self.status = format!("Karte extern geändert, aber nicht lesbar: {e}"),
                }
            }
        }
        if self.watched.iter().any(|(p, t)| mtime(p) != *t) {
            self.reload_assets();
            self.status = "Tileset oder Objekt geändert, Karte aktualisiert".into();
            changed = true;
        }
        changed
    }

    fn textures(&mut self, ctx: &egui::Context) {
        if self.tex.is_some() && self.tex_rev == self.assets_rev {
            return;
        }
        let load = |name: String, img: &Image| {
            let c = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
            ctx.load_texture(name, c, TextureOptions::NEAREST)
        };
        let tiles =
            self.assets.tileset.images.iter().enumerate().map(|(i, img)| load(format!("tile-{i}"), img)).collect();
        let mut placeholders = HashMap::new();
        for e in &self.map.legend {
            if e.tile.as_deref().is_none_or(|t| self.assets.tileset.index(t).is_none()) {
                let img = tilemap::placeholder(self.map.tile_w, self.map.tile_h, e.key);
                placeholders.insert(e.key, load(format!("ph-{}", e.key), &img));
            }
        }
        let objects = self
            .assets
            .objects
            .iter()
            .map(|(k, o)| (*k, (load(format!("obj-{k}"), &o.image), [o.image.width, o.image.height], o.anchor)))
            .collect();
        self.tex = Some(Textures { tiles, placeholders, objects });
        self.tex_rev = self.assets_rev;
    }

    /// Texture showing `key` on a cell.
    fn key_texture(&self, key: u8) -> Option<&TextureHandle> {
        let tex = self.tex.as_ref()?;
        let e = self.map.entry(key);
        match e.and_then(|e| e.tile.as_deref()).and_then(|t| self.assets.tileset.index(t)) {
            Some(i) => tex.tiles.get(i),
            None if e.is_some_and(|e| e.tile.is_none() && e.object.is_some()) => None,
            None => tex.placeholders.get(&key),
        }
    }

    fn cell_at(&self, pos: Pos2) -> (i32, i32) {
        let d = pos - self.screen.min;
        ((d.x / self.cell.x).floor() as i32, (d.y / self.cell.y).floor() as i32)
    }

    /// Screen centre of cell (x, y) (for tests).
    #[cfg(test)]
    pub fn pan_for_test(&mut self, pan: Vec2) {
        self.pan = pan;
    }

    #[cfg(test)]
    pub fn size_draft_for_test(&mut self, size: [u32; 2]) {
        self.size_draft = size;
    }

    #[cfg(test)]
    pub fn cell_center(&self, x: i32, y: i32) -> Pos2 {
        self.screen.min + Vec2::new((x as f32 + 0.5) * self.cell.x, (y as f32 + 0.5) * self.cell.y)
    }
}

/// The "Neue Karte" / "world.txt importieren" dialog.
#[derive(Clone, Debug, Default)]
pub struct MapDialog {
    pub import: bool,
    pub world: String,
    pub tileset: String,
    pub map: String,
    pub width: u32,
    pub height: u32,
}

impl EditorApp {
    /// Open a `.tilemap.json` in the active tab.
    pub(super) fn open_map(&mut self, path: &Path) -> bool {
        match MapTab::load(path) {
            Ok(mut t) => {
                if let Some(old) = self.path.take() {
                    crate::launch::unregister_gui(&old);
                }
                for w in &t.assets.warnings {
                    t.status += &format!("{w}; ");
                }
                if t.status.is_empty() {
                    t.status = format!("Karte geöffnet: {}", path.display());
                }
                self.map = Some(Box::new(t));
                self.dirty = false;
                self.rev += 1;
                true
            }
            Err(e) => {
                self.status = format!("Fehler beim Öffnen: {e}");
                false
            }
        }
    }

    /// An open document of another tab at `path`: (tab, document, its rev).
    fn open_doc_of(&self, path: &Path) -> Option<(usize, &Document, u64)> {
        (0..self.tabs.len()).filter(|&i| i != self.active).find_map(|i| {
            let (p, d, r) = self.tab_doc(i)?;
            super::tabs::same_file(p, path).then_some((i, d, r))
        })
    }

    /// Show the tileset as another tab has it (unsaved edits included).
    fn sync_live_tileset(&mut self) {
        let Some(m) = &self.map else { return };
        let ts_path = m.map.tileset_path(&m.path);
        let found = self.open_doc_of(&ts_path).map(|(i, d, r)| (i, r, d));
        let Some(m) = self.map.as_deref() else { return };
        match found {
            Some((i, r, d)) if m.live != Some((i, r)) => {
                let ts = Tileset::from_document(d);
                let m = self.map.as_mut().unwrap();
                m.assets.tileset = ts;
                m.assets.warnings.retain(|w| !w.starts_with("tileset"));
                m.assets_rev += 1;
                m.live = Some((i, r));
            }
            None if m.live.is_some() => self.map.as_mut().unwrap().reload_assets(),
            _ => {}
        }
    }

    pub(super) fn map_shortcuts(&mut self, ctx: &egui::Context) {
        self.tab_shortcuts(ctx);
        let hit = |m: Modifiers, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let Some(m) = self.map.as_mut() else { return };
        if hit(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || hit(Modifiers::COMMAND, Key::Y) {
            m.redo();
        }
        if hit(Modifiers::COMMAND, Key::Z) {
            m.undo();
        }
        if hit(Modifiers::COMMAND, Key::Quote) {
            m.grid = !m.grid;
        }
        if hit(Modifiers::COMMAND, Key::S) {
            self.save();
        }
        if hit(Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if hit(Modifiers::COMMAND, Key::N) {
            self.new_document();
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let Some(m) = self.map.as_mut() else { return };
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        for t in MapTool::ALL {
            if pressed(t.key()) {
                m.tool = t;
            }
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            m.zoom = (m.zoom + 1).min(SCALES.len() - 1);
        }
        if pressed(Key::Minus) {
            m.zoom = m.zoom.saturating_sub(1);
        }
        if pressed(Key::Num0) {
            m.fit_pending = true;
        }
    }

    pub(super) fn map_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Datei", |ui| {
                if ui.button("Neu…  (Strg+N)").clicked() {
                    self.new_document();
                }
                self.map_file_entries(ui);
                if ui.button("Öffnen…  (Strg+O)").clicked() {
                    self.open_dialog();
                }
                if ui.button("Öffnen über Pfad…").clicked() {
                    self.path_dialog = Some(PathDialog { save: false, text: self.default_path_text() });
                }
                if ui.button("Neu laden").clicked() {
                    self.request(super::Pending::Reload);
                }
                ui.separator();
                if ui.button("Speichern  (Strg+S)").clicked() {
                    self.save();
                }
                ui.separator();
                if ui.button("Tab schließen  (Strg+W)").clicked() {
                    self.request_close_tab(self.active);
                }
                if ui.button("Beenden").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            let Some(m) = self.map.as_mut() else { return };
            ui.menu_button("Bearbeiten", |ui| {
                if ui.add_enabled(!m.undo.is_empty(), egui::Button::new("Rückgängig  (Strg+Z)")).clicked() {
                    m.undo();
                }
                if ui.add_enabled(!m.redo.is_empty(), egui::Button::new("Wiederholen  (Strg+Umschalt+Z)")).clicked() {
                    m.redo();
                }
            });
            ui.menu_button("Werkzeuge", |ui| {
                for t in MapTool::ALL {
                    ui.radio_value(&mut m.tool, t, t.label());
                }
            });
            ui.menu_button("Ansicht", |ui| {
                ui.checkbox(&mut m.grid, "Kachelraster  (Strg+')");
                if ui.button("Einpassen  (0)").clicked() {
                    m.fit_pending = true;
                }
            });
            ui.menu_button("Export", |ui| {
                if ui.button("Godot (Atlas + TileSet .tres + Szene .tscn)").clicked() {
                    self.export_map("godot");
                }
                if ui.button("world.txt (Sandbox)").clicked() {
                    self.export_map("world_txt");
                }
                ui.label(egui::RichText::new("Ziel: <Kartenordner>/export/<name>/").small().weak());
            });
            ui.separator();
            self.tab_bar(ui);
        });
    }

    /// "Neue Karte…" and "world.txt importieren…" (also in the document menu).
    pub(super) fn map_file_entries(&mut self, ui: &mut egui::Ui) {
        let dir = self.work_dir().unwrap_or_default();
        let tileset = self
            .tab_paths()
            .into_iter()
            .find(|p| io::is_document_path(p) && super::tiled::suggests_tiles(p))
            .or_else(|| self.map.as_ref().map(|m| m.map.tileset_path(&m.path)))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| dir.join("tiles32.pxe.json").display().to_string());
        if ui.button("Neue Karte…").clicked() {
            self.map_dialog = Some(MapDialog {
                import: false,
                tileset: tileset.clone(),
                map: dir.join("karte.tilemap.json").display().to_string(),
                width: 20,
                height: 12,
                ..Default::default()
            });
        }
        if ui.button("world.txt importieren…").clicked() {
            self.map_dialog = Some(MapDialog {
                import: true,
                world: dir.join("world.txt").display().to_string(),
                tileset,
                map: dir.join("world.tilemap.json").display().to_string(),
                ..Default::default()
            });
        }
    }

    pub(super) fn map_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.map_dialog else { return };
        let mut go = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("map-dialog")).show(ctx, |ui| {
            ui.heading(if dlg.import { "world.txt importieren" } else { "Neue Karte" });
            egui::Grid::new("map-dialog-grid").num_columns(2).show(ui, |ui| {
                if dlg.import {
                    let l = ui.label("world.txt");
                    ui.add(egui::TextEdit::singleline(&mut dlg.world).desired_width(420.0)).labelled_by(l.id);
                    ui.end_row();
                }
                let l = ui.label("Tileset");
                ui.add(egui::TextEdit::singleline(&mut dlg.tileset).desired_width(420.0)).labelled_by(l.id);
                ui.end_row();
                let l = ui.label("Karte");
                ui.add(egui::TextEdit::singleline(&mut dlg.map).desired_width(420.0)).labelled_by(l.id);
                ui.end_row();
                if !dlg.import {
                    ui.label("Größe (Kacheln)");
                    ui.horizontal(|ui| {
                        let lw = ui.label("Breite");
                        ui.add(egui::DragValue::new(&mut dlg.width).range(1..=tilemap::MAX_CELLS)).labelled_by(lw.id);
                        let lh = ui.label("Höhe");
                        ui.add(egui::DragValue::new(&mut dlg.height).range(1..=tilemap::MAX_CELLS)).labelled_by(lh.id);
                    });
                    ui.end_row();
                }
            });
            if dlg.import {
                ui.label(
                    egui::RichText::new(
                        "Ränder, Felswände und Deko ergänzt das Spiel selbst; die Karte zeigt die Basiskacheln.",
                    )
                    .small()
                    .weak(),
                );
            }
            ui.horizontal(|ui| {
                if ui.button(if dlg.import { "Importieren" } else { "Erstellen" }).clicked() {
                    go = true;
                }
                if ui.button("Abbrechen").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.map_dialog = None;
        }
        if go {
            let dlg = self.map_dialog.clone().unwrap();
            match create_map(&dlg) {
                Ok((path, notes)) => {
                    self.map_dialog = None;
                    self.open_in_tab(&path);
                    if let Some(m) = self.map.as_mut() {
                        if !notes.is_empty() {
                            m.status = format!("{} – {}", m.status, notes.join("; "));
                        }
                    }
                }
                Err(e) => self.status = e,
            }
        }
    }

    pub(super) fn export_map(&mut self, target: &str) {
        let Some(m) = self.map.as_mut() else { return };
        let stem = tilemap::stem(&m.path);
        let out = m.path.parent().unwrap_or(Path::new(".")).join("export").join(&stem);
        let result = match target {
            "godot" => {
                let ts_name = io::stem(&m.map.tileset_path(&m.path));
                godot_tiles::export_map(&m.map, &m.assets, &ts_name, &out, &stem, None)
            }
            _ => {
                let (text, warnings) = tilemap::export_world(&m.map);
                std::fs::create_dir_all(&out)
                    .map_err(|e| e.to_string())
                    .and_then(|_| io::write_atomic(&out.join("world.txt"), text.as_bytes()))
                    .map(|_| (vec![out.join("world.txt")], warnings))
            }
        };
        m.status = match result {
            Ok((files, warnings)) => {
                let mut s = format!(
                    "Exportiert ({target}): {}",
                    files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ")
                );
                if !warnings.is_empty() {
                    s += &format!(" – {}", warnings.join("; "));
                }
                s
            }
            Err(e) => format!("Export fehlgeschlagen: {e}"),
        };
    }

    /// Everything of a map tab below the menu bar.
    pub(super) fn map_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, now: f64) {
        self.sync_live_tileset();
        let Some(m) = self.map.as_mut() else { return };
        m.textures(ctx);
        egui::Panel::bottom("map-status").show(ui, |ui| {
            let mut parts = Vec::new();
            if let Some((x, y)) = m.hover.filter(|&(x, y)| m.map.in_bounds(x, y)) {
                parts.push(format!("Zelle x {x}, y {y}"));
                parts.push(m.map.key_label(m.map.get(x, y)));
            }
            parts.push(format!("Pinsel {}", m.brush_label()));
            parts.push(format!("{}×{} Kacheln · Zoom {}×", m.map.width, m.map.height, m.scale()));
            ui.strong(parts.join(" · "));
            ui.label(&m.status);
        });
        if m.conflict {
            egui::Panel::top("map-conflict").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(255, 200, 80),
                        "Die Karte wurde extern geändert, du hast aber ungespeicherte Änderungen.",
                    );
                    if ui.button("Neu laden (meine Änderungen verwerfen)").clicked() {
                        m.dirty = false;
                        m.conflict = false;
                        m.map_mtime = None;
                    }
                    if ui.button("Meine Version behalten").clicked() {
                        m.conflict = false;
                    }
                });
            });
        }
        egui::Panel::left("map-tools").resizable(true).default_size(250.0).show(ui, |ui| palette_panel(ui, m));
        let mut open_tileset = None;
        egui::Panel::right("map-props").resizable(true).default_size(240.0).show(ui, |ui| {
            open_tileset = props_panel(ui, m);
        });
        egui::CentralPanel::default().show(ui, |ui| map_canvas(ui, m, now));
        if let Some(p) = open_tileset {
            self.open_in_tab(&p);
        }
    }
}

/// Create the map of a dialog: a new empty one or one imported from world.txt.
fn create_map(dlg: &MapDialog) -> Result<(PathBuf, Vec<String>), String> {
    let path = PathBuf::from(dlg.map.trim());
    if !tilemap::is_tilemap_path(&path) {
        return Err(format!("Der Kartenname muss auf {} enden", tilemap::EXTENSION));
    }
    if path.exists() {
        return Err(format!("{} gibt es schon", path.display()));
    }
    let ts_path = PathBuf::from(dlg.tileset.trim());
    let ts = Tileset::load(&ts_path).map_err(|e| format!("Tileset: {e}"))?;
    let rel = tilemap::relative_to_map(&path, &ts_path);
    let (map, notes) = if dlg.import {
        let text = std::fs::read_to_string(dlg.world.trim()).map_err(|e| format!("{}: {e}", dlg.world))?;
        tilemap::import_world(&text, &rel, (ts.tile_w, ts.tile_h))?
    } else {
        let mut map = TileMap::new(&rel, ts.tile_w, ts.tile_h, dlg.width, dlg.height)?;
        if let Some(first) = ts.names.first() {
            let k = map.key_for_tile(first)?;
            map.fill_rect(CellRect::new(0, 0, dlg.width, dlg.height), k);
        }
        (map, Vec::new())
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    tilemap::save(&map, &path)?;
    let _ = Journal::for_document(&path).record(&map, "user", "Benutzer: Karte angelegt");
    Ok((path, notes))
}

/// Tools, the legend and the tiles of the tileset.
fn palette_panel(ui: &mut egui::Ui, m: &mut MapTab) {
    ui.heading("Karte");
    ui.horizontal_wrapped(|ui| {
        for t in MapTool::ALL {
            ui.selectable_value(&mut m.tool, t, t.label());
        }
    });
    ui.label(egui::RichText::new("Rechte Maustaste: Kachel entfernen · Alt+Klick: aufnehmen").small().weak());
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong("Legende");
        let side = 28.0;
        let legend = m.map.legend.clone();
        for e in &legend {
            ui.horizontal(|ui| {
                thumb(ui, m, e, side);
                let sel = m.brush == Brush::Key(e.key);
                let n = m.map.cells.iter().filter(|c| **c == e.key).count();
                if ui.selectable_label(sel, format!("{}  ×{n}", e.label())).clicked() {
                    m.brush = Brush::Key(e.key);
                    if m.tool == MapTool::Eraser || m.tool == MapTool::Pick {
                        m.tool = MapTool::Pen;
                    }
                }
            });
        }
        ui.separator();
        ui.strong(format!("Tileset ({} Tiles)", m.assets.tileset.names.len()));
        let Some(tex) = m.tex.as_ref() else { return };
        let mut pick = None;
        let size = Vec2::splat(m.map.tile_w.max(m.map.tile_h) as f32 * 1.5);
        ui.horizontal_wrapped(|ui| {
            for (i, name) in m.assets.tileset.names.iter().enumerate() {
                let key = m.map.legend.iter().find(|e| e.tile.as_deref() == Some(name) && e.object.is_none());
                let sel = match &m.brush {
                    Brush::Tile(t) => t == name,
                    Brush::Key(k) => key.is_some_and(|e| e.key == *k),
                };
                let hover = match key {
                    Some(e) => format!("{name} · Taste {}", e.key as char),
                    None => format!("{name} · noch nicht in der Legende"),
                };
                let r = ui.add(egui::Button::image(egui::Image::new((tex.tiles[i].id(), size))).selected(sel));
                if r.on_hover_text(hover).clicked() {
                    pick = Some(name.clone());
                }
            }
        });
        if let Some(name) = pick {
            m.brush = Brush::Tile(name);
            if m.tool == MapTool::Eraser || m.tool == MapTool::Pick {
                m.tool = MapTool::Pen;
            }
        }
    });
}

/// Small picture of a legend entry: its tile, else the object, else the placeholder.
fn thumb(ui: &mut egui::Ui, m: &MapTab, e: &LegendEntry, side: f32) {
    let tex = m.key_texture(e.key).or_else(|| m.tex.as_ref().and_then(|t| t.objects.get(&e.key).map(|o| &o.0)));
    match tex {
        Some(t) => {
            ui.add(egui::Image::new((t.id(), Vec2::splat(side))));
        }
        None => {
            ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        }
    }
}

/// Map size, tileset, hints. Returns the tileset path to open in a tab.
fn props_panel(ui: &mut egui::Ui, m: &mut MapTab) -> Option<PathBuf> {
    let mut open = None;
    ui.heading("Eigenschaften");
    ui.label(format!("Datei: {}", m.path.file_name().unwrap_or_default().to_string_lossy()));
    ui.label(format!("Tileset: {}", m.map.tileset));
    if m.live.is_some() {
        ui.label(egui::RichText::new("live aus dem offenen Tab").small().color(super::tabs::MARK));
    }
    if ui
        .button("Tileset im Tab öffnen")
        .on_hover_text("Tiles bearbeiten; die Karte zeigt die Änderungen sofort")
        .clicked()
    {
        open = Some(m.map.tileset_path(&m.path));
    }
    ui.label(format!("Kachel {}×{} px", m.map.tile_w, m.map.tile_h));
    ui.separator();
    ui.strong("Kartengröße (Kacheln)");
    ui.horizontal(|ui| {
        let lw = ui.label("Breite");
        ui.add(egui::DragValue::new(&mut m.size_draft[0]).range(1..=tilemap::MAX_CELLS)).labelled_by(lw.id);
        let lh = ui.label("Höhe");
        ui.add(egui::DragValue::new(&mut m.size_draft[1]).range(1..=tilemap::MAX_CELLS)).labelled_by(lh.id);
    });
    let [w, h] = m.size_draft;
    if ui
        .add_enabled((w, h) != (m.map.width, m.map.height), egui::Button::new("Größe übernehmen"))
        .on_hover_text("Die linke obere Ecke bleibt; neue Zellen sind leer")
        .clicked()
    {
        let before = m.map.clone();
        if m.map.resize(w, h, EMPTY).is_ok() {
            m.push_undo(before, format!("Kartengröße {w}×{h}"));
            m.fit_pending = true;
        }
    }
    ui.checkbox(&mut m.grid, "Kachelraster");
    ui.horizontal(|ui| {
        if ui.button("-").clicked() {
            m.zoom = m.zoom.saturating_sub(1);
        }
        ui.label(format!("Zoom {}×", m.scale()));
        if ui.button("+").clicked() {
            m.zoom = (m.zoom + 1).min(SCALES.len() - 1);
        }
        if ui.button("Einpassen").clicked() {
            m.fit_pending = true;
        }
    });
    if !m.map.world_lines.is_empty() {
        ui.separator();
        ui.label(
            egui::RichText::new(
                "Aus world.txt: Weg- und Wasserränder, Felswände und Deko ergänzt das Spiel selbst. Export > world.txt schreibt die Kommentare unverändert zurück.",
            )
            .small(),
        );
    }
    if !m.assets.warnings.is_empty() {
        ui.separator();
        ui.colored_label(Color32::from_rgb(255, 200, 80), "Hinweise");
        for w in &m.assets.warnings {
            ui.label(egui::RichText::new(w).small());
        }
    }
    ui.separator();
    ui.strong("Verlauf");
    for (_, d) in m.undo.iter().rev().take(12) {
        ui.label(egui::RichText::new(d).small());
    }
    open
}

fn map_canvas(ui: &mut egui::Ui, m: &mut MapTab, now: f64) {
    let ppp = ui.ctx().pixels_per_point();
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let area = resp.rect;
    painter.rect_filled(area, 0.0, Color32::from_gray(40));
    let (tw, th) = (m.map.tile_w as f32, m.map.tile_h as f32);
    let (w, h) = (m.map.width as f32 * tw, m.map.height as f32 * th);
    if m.fit_pending && area.width() > 10.0 {
        let fit = ((area.width() * ppp) / w).min((area.height() * ppp) / h);
        m.zoom = SCALES.iter().rposition(|s| *s <= fit).unwrap_or(0);
        m.pan = Vec2::ZERO;
        m.fit_pending = false;
    }
    // zoom around the pointer, pan with the middle button or space + drag
    if let Some(p) = resp.hover_pos() {
        let scroll: f32 = ui.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum()
        });
        if scroll != 0.0 {
            let old = m.scale();
            m.zoom = if scroll > 0.0 { (m.zoom + 1).min(SCALES.len() - 1) } else { m.zoom.saturating_sub(1) };
            let k = m.scale() / old;
            let centre = area.center() + m.pan;
            m.pan += (p - centre) * (1.0 - k);
        }
    }
    let space = ui.input(|i| i.key_down(Key::Space));
    if resp.dragged_by(PointerButton::Middle) || (space && resp.dragged_by(PointerButton::Primary)) {
        m.pan += resp.drag_delta();
    }
    let s = m.scale() / ppp; // points per art pixel
    m.cell = Vec2::new(tw * s, th * s);
    let size = Vec2::new(w * s, h * s);
    let min = area.center() + m.pan - size / 2.0;
    let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
    m.screen = Rect::from_min_size(min, size);
    let painter = painter.with_clip_rect(area);
    // tiles
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let (x0, y0) = m.cell_at(area.min.max(m.screen.min));
    let (x1, y1) = m.cell_at(area.max.min(m.screen.max));
    let preview = m.drag.as_ref().filter(|_| m.tool == MapTool::Rect).map(|d| d.start);
    for y in y0.max(0)..=y1.min(m.map.height as i32 - 1) {
        for x in x0.max(0)..=x1.min(m.map.width as i32 - 1) {
            let r = Rect::from_min_size(m.screen.min + Vec2::new(x as f32 * m.cell.x, y as f32 * m.cell.y), m.cell);
            if let Some(t) = m.key_texture(m.map.get(x, y)) {
                painter.image(t.id(), r, uv, Color32::WHITE);
            }
        }
    }
    // objects, row by row (lower rows in front)
    if let Some(tex) = &m.tex {
        for y in 0..m.map.height as i32 {
            for x in 0..m.map.width as i32 {
                let k = m.map.get(x, y);
                let Some((t, [ow, oh], anchor)) = tex.objects.get(&k) else { continue };
                let foot = Vec2::new(x as f32 * tw + tw / 2.0 - anchor.0 as f32, (y + 1) as f32 * th - anchor.1 as f32);
                let r = Rect::from_min_size(m.screen.min + foot * s, Vec2::new(*ow as f32, *oh as f32) * s);
                if r.intersects(area) {
                    painter.image(t.id(), r, uv, Color32::WHITE);
                }
            }
        }
    }
    let px = 1.0 / ppp;
    if m.grid && m.cell.x * ppp >= 6.0 {
        let line = Stroke::new(px, Color32::from_black_alpha(80));
        for x in 0..=m.map.width {
            let sx = m.screen.min.x + x as f32 * m.cell.x;
            painter.line_segment([Pos2::new(sx, m.screen.min.y), Pos2::new(sx, m.screen.max.y)], line);
        }
        for y in 0..=m.map.height {
            let sy = m.screen.min.y + y as f32 * m.cell.y;
            painter.line_segment([Pos2::new(m.screen.min.x, sy), Pos2::new(m.screen.max.x, sy)], line);
        }
    }
    painter.rect_stroke(m.screen, 0.0, Stroke::new(px, Color32::from_gray(160)), egui::StrokeKind::Outside);
    let cells_rect = |a: (i32, i32), b: (i32, i32)| {
        let (x0, y0, x1, y1) = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
        Rect::from_min_max(
            m.screen.min + Vec2::new(x0 as f32 * m.cell.x, y0 as f32 * m.cell.y),
            m.screen.min + Vec2::new((x1 + 1) as f32 * m.cell.x, (y1 + 1) as f32 * m.cell.y),
        )
    };
    if let Some((r, until)) = m.flash {
        if now < until {
            let rr = cells_rect((r.x, r.y), (r.x + r.w as i32 - 1, r.y + r.h as i32 - 1));
            painter.rect_stroke(rr, 0.0, Stroke::new(2.0, Color32::from_rgb(255, 210, 90)), egui::StrokeKind::Inside);
            ui.ctx().request_repaint();
        } else {
            m.flash = None;
        }
    }

    // input
    m.hover = resp.hover_pos().map(|p| m.cell_at(p));
    if let (Some(start), Some(h)) = (preview, m.hover) {
        painter.rect_stroke(cells_rect(start, h), 0.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
    } else if let Some(h) = m.hover.filter(|&(x, y)| m.map.in_bounds(x, y)) {
        painter.rect_stroke(cells_rect(h, h), 0.0, Stroke::new(px, Color32::WHITE), egui::StrokeKind::Inside);
    }
    if space {
        return;
    }
    let (primary, secondary) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down()));
    let pressed = resp.is_pointer_button_down_on() && (primary || secondary);
    let alt = ui.input(|i| i.modifiers.alt);
    // while dragging the pointer may leave the canvas: keep following it
    let pointer = if m.drag.is_some() { ui.input(|i| i.pointer.latest_pos()) } else { resp.hover_pos() };
    let Some(cell) = pointer.map(|p| m.cell_at(p)) else { return };
    if pressed && m.drag.is_none() {
        let erase = secondary && !primary;
        if alt || (m.tool == MapTool::Pick && !erase) {
            let k = m.map.get(cell.0, cell.1);
            if k != EMPTY {
                m.brush = Brush::Key(k);
                m.status = format!("Aufgenommen: {}", m.map.key_label(k));
            }
            return;
        }
        m.drag = Some(Drag { start: cell, erase, before: m.map.clone() });
        if m.tool == MapTool::Fill {
            let k = if erase { EMPTY } else { m.brush_key() };
            m.map.flood_fill(cell.0, cell.1, k);
        }
    }
    let Some(d) = &m.drag else { return };
    let erase = d.erase;
    if pressed && matches!(m.tool, MapTool::Pen | MapTool::Eraser) {
        let k = if erase || m.tool == MapTool::Eraser { EMPTY } else { m.brush_key() };
        // fill gaps of a fast stroke
        let from = m.drag.as_ref().unwrap().start;
        for (x, y) in pixel_core::draw::line(from.0, from.1, cell.0, cell.1) {
            m.map.set(x, y, k);
        }
        m.drag.as_mut().unwrap().start = cell;
    }
    if !pressed {
        let d = m.drag.take().unwrap();
        if m.tool == MapTool::Rect {
            let k = if d.erase { EMPTY } else { m.brush_key() };
            let (x0, y0) = (d.start.0.min(cell.0), d.start.1.min(cell.1));
            let (x1, y1) = (d.start.0.max(cell.0), d.start.1.max(cell.1));
            m.map.fill_rect(CellRect::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32), k);
        }
        let (n, bb) = tilemap::cell_diff(&d.before, &m.map);
        let what = if d.erase || m.tool == MapTool::Eraser { "entfernt".to_string() } else { m.brush_label() };
        let tool = m.tool.label().split(' ').next().unwrap_or("").to_string();
        let at = bb.map(|b| format!(" {}", b.describe())).unwrap_or_default();
        m.push_undo(d.before, format!("{tool}: {n} Zellen {what}{at}"));
    }
}
