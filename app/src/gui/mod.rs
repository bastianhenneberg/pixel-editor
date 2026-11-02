//! egui editor. UI labels are German (the user's language).

mod canvas;
mod follow;
mod map;
mod marks;
mod panels;
mod preview;
mod recolor;
mod reference;
mod select;
mod settings;
mod share;
mod tabs;
#[cfg(test)]
mod tests;
mod tiled;
mod timeline;
mod toolbar;
mod transform;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers};
use pixel_core::export::{self, ExportOptions, Target};
use pixel_core::{io, palettes, Document, History, Rgba};
use select::Selection;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pencil,
    /// Shading: left steps a pixel one color lighter along its ramp, right darker.
    Shade,
    Eraser,
    Fill,
    Line,
    Rect,
    Ellipse,
    Eyedropper,
    /// Rectangle selection.
    Select,
    /// Paint single pixels into the selection.
    SelectBrush,
    /// Select the connected area of one color.
    Wand,
    /// Freehand outline.
    Lasso,
    /// "KI zeigen": drag a rectangle (or click a pixel) to pin it for the AI.
    Point,
}

impl Tool {
    pub const ALL: [Tool; 13] = [
        Tool::Pencil,
        Tool::Shade,
        Tool::Eraser,
        Tool::Fill,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Eyedropper,
        Tool::Select,
        Tool::SelectBrush,
        Tool::Wand,
        Tool::Lasso,
        Tool::Point,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pencil => "Stift",
            Tool::Shade => "Schattieren",
            Tool::Eraser => "Radierer",
            Tool::Fill => "Füllen",
            Tool::Line => "Linie",
            Tool::Rect => "Rechteck",
            Tool::Ellipse => "Ellipse",
            Tool::Eyedropper => "Pipette",
            Tool::Select => "Auswahl",
            Tool::SelectBrush => "Auswahl-Pinsel",
            Tool::Wand => "Zauberstab",
            Tool::Lasso => "Lasso",
            Tool::Point => "KI zeigen",
        }
    }

    /// Aseprite's default shortcut: key and whether Shift is held.
    pub fn key(self) -> (Key, bool) {
        match self {
            Tool::Pencil => (Key::B, false),
            Tool::Shade => (Key::D, false),
            Tool::Eraser => (Key::E, false),
            Tool::Fill => (Key::G, false),
            Tool::Line => (Key::L, false),
            Tool::Rect => (Key::U, false),
            Tool::Ellipse => (Key::U, true),
            Tool::Eyedropper => (Key::I, false),
            Tool::Select => (Key::M, false),
            Tool::SelectBrush => (Key::M, true),
            Tool::Wand => (Key::W, false),
            Tool::Lasso => (Key::Q, false),
            Tool::Point => (Key::K, false),
        }
    }

    /// One of the selection tools (rectangle, brush, wand, lasso).
    pub fn selects(self) -> bool {
        matches!(self, Tool::Select | Tool::SelectBrush | Tool::Wand | Tool::Lasso)
    }

    pub fn shortcut_label(self) -> String {
        let (k, shift) = self.key();
        if shift {
            format!("Umschalt+{}", k.name())
        } else {
            k.name().to_string()
        }
    }
}

/// Things that would throw away unsaved changes wait for confirmation.
#[derive(Clone)]
enum Pending {
    Reload,
    /// Close the active tab.
    CloseTab,
    Quit,
}

struct NewDialog {
    width: u32,
    height: u32,
    palette: String,
    /// Start in the Kachel-Modus (the tile templates switch it on).
    tiled: bool,
}

/// Debug/verification hook: `PIXEL_EDITOR_SCREENSHOT=out.png` makes the app
/// capture its own window after `PIXEL_EDITOR_SCREENSHOT_DELAY` seconds
/// (default 2), save it and quit. Needs no compositor screencopy support.
struct AutoShot {
    path: PathBuf,
    at: f64,
    requested: bool,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AutoShot {
    fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("PIXEL_EDITOR_SCREENSHOT")?);
        let at = std::env::var("PIXEL_EDITOR_SCREENSHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
        Some(AutoShot { path, at, requested: false, done: Default::default() })
    }

    fn tick(&mut self, ctx: &egui::Context, now: f64) -> bool {
        use std::sync::atomic::Ordering;
        if !self.requested && now >= self.at {
            self.requested = true;
            let (path, done, repaint) = (self.path.clone(), self.done.clone(), ctx.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let pixels: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let out = pixel_core::Image { width: img.size[0] as u32, height: img.size[1] as u32, pixels };
                match out.save_png(&path) {
                    Ok(()) => eprintln!("screenshot saved: {}", path.display()),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                done.store(true, Ordering::SeqCst);
                repaint.request_repaint();
            })));
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        self.done.load(Ordering::SeqCst)
    }
}

struct PathDialog {
    save: bool,
    text: String,
}

pub struct EditorApp {
    doc: Document,
    /// The document as last loaded/saved (for the journal).
    saved_doc: Option<Document>,
    /// Descriptions of edits since the last save.
    unsaved_steps: Vec<String>,
    selection: Option<Selection>,
    floating: Option<select::Floating>,
    clipboard: Option<select::Clip>,
    show_regions: bool,
    region_draft: panels::RegionDraft,
    snapshot_name: String,
    follow: follow::FollowState,
    /// What the editor shows, published for Claude (`get_user_view`).
    share: share::ShareState,
    path: Option<PathBuf>,
    dirty: bool,
    history: History,
    /// Bumped on every document change; drives texture rebuilds.
    rev: u64,

    tool: Tool,
    color: u8,
    filled: bool,
    /// Stift/Radierer: drop the corner pixel of every "L" in a freehand
    /// stroke (as Aseprite's Pixel-Perfect), kept in the settings.
    pixel_perfect: bool,
    /// Palette colors marked with Strg+Klick for a new ramp, in click order.
    ramp_pick: Vec<u8>,
    /// The ramp whose name is being edited, and the draft.
    ramp_rename: Option<(usize, String)>,
    layer: usize,
    frame: usize,
    /// Frame the right panel shows: the current frame, but frozen while
    /// playing, so the panel does not change with every played frame.
    panel_frame: usize,

    zoom: f32,
    pan: egui::Vec2,
    fit_pending: bool,
    grid: bool,
    onion: bool,
    /// "Saubere Ansicht": hide every overlay (the toggles above keep their values).
    clean_view: bool,
    /// Moved and pasted pixels go behind the existing ones ("Hinten").
    drop_behind: bool,
    /// Zauberstab: every pixel of the color, not only the connected area.
    wand_all: bool,
    /// Symmetric drawing (per document).
    symmetry: transform::Symmetry,
    /// Kachel-Modus (per document, remembered in `tile_memory`).
    tile_mode: tiled::TileMode,
    /// Kachel-Modus per document path, kept in the settings.
    tile_memory: std::collections::BTreeMap<String, tiled::TileMode>,
    /// "Animation gespiegelt duplizieren": the destination path being typed.
    mirror_dialog: Option<String>,
    /// "Farbe ersetzen" / "Kontur" dialog.
    recolor_dialog: Option<recolor::RecolorDialog>,
    /// Dither option of Füllen and the filled shapes.
    dither: recolor::Dither,
    /// The last used tool of each toolbar group.
    group_tool: [Tool; toolbar::GROUPS.len()],
    /// The open tool group flyout.
    flyout: Option<toolbar::Flyout>,
    preview: preview::PreviewState,
    /// Where settings are kept (None in tests: never touch the user's config).
    settings_path: Option<PathBuf>,
    saved_settings: settings::Settings,

    playing: bool,
    play_tag: Option<usize>,
    play_pos: usize,
    play_elapsed_ms: f64,
    play_last: f64,

    canvas: canvas::CanvasState,

    disk_mtime: Option<SystemTime>,
    last_poll: Instant,
    external_conflict: bool,

    status: String,
    new_dialog: Option<NewDialog>,
    path_dialog: Option<PathDialog>,
    confirm: Option<Pending>,
    allow_close: bool,
    title: String,
    palette_edit_at: Option<(u8, f64)>,
    tag_draft: panels::TagDraft,
    autoshot: Option<AutoShot>,
    timeline: timeline::TimelineState,
    /// The "KI zeigen" marks of this document (see `marks`).
    marks: crate::view::Marks,
    marks_mtime: Option<SystemTime>,
    /// The selected mark (note field, handles).
    mark_sel: Option<u32>,
    /// The note being typed for the selected mark.
    note_draft: String,

    /// The map of a tilemap tab (the document fields are unused then).
    map: Option<Box<map::MapTab>>,
    /// "Neue Karte" / "world.txt importieren".
    map_dialog: Option<map::MapDialog>,
    /// Open documents; the fields above describe `tabs[active]` (see `tabs`).
    tabs: Vec<tabs::Tab>,
    active: usize,
    /// The window's workdir: tab names are relative to it, and other
    /// processes find the window for it (`crate::ipc`).
    workdir: Option<PathBuf>,
    ipc: Option<crate::ipc::Server>,
    debug_keys: bool,
}

impl EditorApp {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut app = EditorApp::blank();
        app.autoshot = AutoShot::from_env();
        if let Some(sp) = settings::default_path().filter(|_| !cfg!(test)) {
            app.use_settings_file(sp);
        }
        if let Some(p) = path {
            if p.exists() {
                app.open(&p);
            } else {
                // start a new document that will be saved under this name
                app.tile_mode = app.initial_tile_mode(&p);
                app.path = Some(p.clone());
                app.status = format!("Neue Datei {}", p.display());
            }
        }
        app
    }

    /// An editor with an empty document and default settings (also the
    /// state of a new tab).
    fn blank() -> Self {
        let doc = Document::new(64, 64, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        EditorApp {
            saved_doc: None,
            unsaved_steps: Vec::new(),
            selection: None,
            floating: None,
            clipboard: None,
            show_regions: true,
            region_draft: Default::default(),
            snapshot_name: String::new(),
            follow: Default::default(),
            share: Default::default(),
            color: doc.palette.first().map_or(b'.', |p| p.key),
            doc,
            path: None,
            dirty: false,
            history: History::default(),
            rev: 0,
            tool: Tool::Pencil,
            filled: false,
            pixel_perfect: true,
            ramp_pick: Vec::new(),
            ramp_rename: None,
            layer: 0,
            frame: 0,
            panel_frame: 0,
            zoom: 8.0,
            pan: egui::Vec2::ZERO,
            fit_pending: true,
            grid: true,
            onion: false,
            clean_view: false,
            drop_behind: false,
            wand_all: false,
            symmetry: Default::default(),
            tile_mode: Default::default(),
            tile_memory: Default::default(),
            mirror_dialog: None,
            recolor_dialog: None,
            dither: Default::default(),
            group_tool: toolbar::GROUPS.map(|(_, tools)| tools[0]),
            flyout: None,
            preview: Default::default(),
            settings_path: None,
            saved_settings: Default::default(),
            playing: false,
            play_tag: None,
            play_pos: 0,
            play_elapsed_ms: 0.0,
            play_last: 0.0,
            canvas: Default::default(),
            disk_mtime: None,
            last_poll: Instant::now(),
            external_conflict: false,
            status: String::new(),
            new_dialog: None,
            path_dialog: None,
            confirm: None,
            allow_close: false,
            title: String::new(),
            palette_edit_at: None,
            tag_draft: Default::default(),
            autoshot: None,
            timeline: Default::default(),
            marks: Default::default(),
            marks_mtime: None,
            mark_sel: None,
            note_draft: String::new(),
            map: None,
            map_dialog: None,
            tabs: vec![Default::default()],
            active: 0,
            workdir: None,
            ipc: None,
            debug_keys: std::env::var_os("PIXEL_EDITOR_DEBUG_KEYS").is_some_and(|v| v != "0"),
        }
    }

    // ---- settings --------------------------------------------------------

    fn settings(&self) -> settings::Settings {
        let mut open_tabs = self.saved_settings.open_tabs.clone();
        if let Some(w) = &self.workdir {
            let paths = self.tab_paths();
            let key = w.display().to_string();
            if paths.is_empty() {
                open_tabs.remove(&key);
            } else {
                open_tabs.insert(key, paths);
            }
        }
        settings::Settings {
            preview: self.preview.settings.clone(),
            open_tabs,
            tile_modes: self.tile_memory.clone(),
            pixel_perfect: self.pixel_perfect,
        }
    }

    /// Load settings from `path` and save changes there from now on.
    fn use_settings_file(&mut self, path: PathBuf) {
        let loaded = settings::Settings::load(&path);
        self.apply_settings(&loaded);
        self.settings_path = Some(path);
    }

    fn apply_settings(&mut self, s: &settings::Settings) {
        self.preview.settings = s.preview.clone();
        self.preview.settings.scale = s.preview.scale.clamp(1, 3);
        self.tile_memory = s.tile_modes.clone();
        self.pixel_perfect = s.pixel_perfect;
        self.saved_settings = s.clone();
    }

    /// Save changed settings, but not in the middle of a drag.
    fn save_settings(&mut self, ctx: &egui::Context) {
        let mut current = self.settings();
        if current == self.saved_settings || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        if let Some(p) = &self.settings_path {
            // keep the tab lists and tile modes other windows saved meanwhile
            let disk = settings::Settings::load(p);
            for (k, m) in disk.tile_modes {
                if !self.saved_settings.tile_modes.contains_key(&k) {
                    self.tile_memory.entry(k.clone()).or_insert(m);
                    current.tile_modes.entry(k).or_insert(m);
                }
            }
            let mut on_disk = disk.open_tabs;
            for k in self.saved_settings.open_tabs.keys().chain(current.open_tabs.keys()) {
                on_disk.remove(k);
            }
            current.open_tabs.extend(on_disk);
            if let Err(e) = current.save(p) {
                self.status = format!("Einstellungen nicht gespeichert: {e}");
            }
        }
        self.saved_settings = current;
    }

    // ---- document state ----------------------------------------------

    /// Record an undo step described by `desc`, then apply an edit.
    fn edit<R>(&mut self, desc: impl Into<String>, f: impl FnOnce(&mut Document) -> R) -> R {
        let desc = desc.into();
        self.history.push(self.doc.clone(), desc.clone());
        let r = f(&mut self.doc);
        if self.history.undo_steps().last().is_some_and(|d| *d == desc) {
            self.history.discard_if_unchanged(&self.doc);
        }
        if self.history.undo_steps().last() == Some(&desc.as_str()) {
            self.unsaved_steps.push(desc);
        }
        self.touch();
        r
    }

    /// "walk_down/2 · body": where edits go, for descriptions and the status bar.
    fn here(&self) -> String {
        format!("{} · {}", self.doc.frame_label(self.frame), self.doc.layers[self.layer].name)
    }

    /// Why pixels of the current layer/frame cannot be edited, if so; shown
    /// in the status bar.
    fn blocked(&mut self) -> bool {
        match self.doc.edit_blocked(self.layer, self.frame) {
            Some(why) => {
                self.status = format!("Gesperrt: {why}");
                true
            }
            None => false,
        }
    }

    /// Pixels a GUI tool may change: inside the selection (if any) and
    /// outside locked regions.
    fn may_paint(&self, x: i32, y: i32) -> bool {
        self.selection.as_ref().is_none_or(|s| s.contains(x, y))
            && self.doc.locked_region_at(self.layer, self.frame, x, y).is_none()
    }

    fn touch(&mut self) {
        self.rev += 1;
        self.dirty = true;
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        self.layer = self.layer.min(self.doc.layers.len() - 1);
        self.frame = self.frame.min(self.doc.frames.len() - 1);
        if self.play_tag.is_some_and(|t| t >= self.doc.tags.len()) {
            self.play_tag = None;
        }
        if !self.doc.has_key(self.color) {
            self.color = self.doc.palette.first().map_or(b'.', |p| p.key);
        }
    }

    fn replace_doc(&mut self, doc: Document) {
        if doc.width != self.doc.width || doc.height != self.doc.height {
            self.fit_pending = true;
        }
        self.doc = doc;
        self.rev += 1;
        self.clamp_selection();
    }

    /// Let the "KI zeigen" marks follow their frames (after frames moved).
    fn sync_marks(&mut self) {
        let doc = self.doc.clone();
        let mut m = self.marks.clone();
        if m.follow_frames(&doc) {
            if self.path.is_some() {
                self.update_marks(|m| m.follow_frames(&doc));
            } else {
                self.marks = m;
            }
        }
    }

    fn undo(&mut self) {
        self.drop_floating();
        if let Some((d, desc)) = self.history.undo(&self.doc) {
            self.replace_doc(d);
            self.sync_marks();
            self.timeline.sel.clear(); // the indices may mean other frames now
                                       // back at the saved state = nothing unsaved
            self.dirty = self.saved_doc.as_ref() != Some(&self.doc);
            self.unsaved_steps.push(format!("Rückgängig: {desc}"));
            self.status = format!("Rückgängig: {desc}");
        }
    }

    fn redo(&mut self) {
        self.drop_floating();
        if let Some((d, desc)) = self.history.redo(&self.doc) {
            self.replace_doc(d);
            self.sync_marks();
            self.timeline.sel.clear(); // the indices may mean other frames now
            self.dirty = self.saved_doc.as_ref() != Some(&self.doc);
            self.unsaved_steps.push(format!("Wiederholt: {desc}"));
            self.status = format!("Wiederholt: {desc}");
        }
    }

    // ---- files ---------------------------------------------------------

    /// Load `path` into the active tab (false: error in the status line).
    fn open(&mut self, path: &Path) -> bool {
        if pixel_core::tilemap::is_tilemap_path(path) {
            return self.open_map(path);
        }
        match io::load_with_warnings(path) {
            Ok((doc, warnings)) => {
                self.map = None;
                self.history.clear();
                self.saved_doc = Some(doc.clone());
                self.unsaved_steps.clear();
                self.selection = None;
                self.floating = None;
                self.replace_doc(doc);
                if let Some(old) = self.path.replace(path.to_path_buf()) {
                    if old != path {
                        crate::launch::unregister_gui(&old);
                    }
                }
                self.dirty = false;
                self.external_conflict = false;
                self.disk_mtime = mtime(path);
                self.marks = crate::view::read_marks(path);
                self.tile_mode = self.initial_tile_mode(path);
                self.marks_mtime = mtime(&crate::view::marks_file(path));
                self.mark_sel = None;
                self.follow_reset(path);
                if !cfg!(test) {
                    crate::launch::register_gui(path);
                }
                self.frame = 0;
                self.layer = self.doc.layers.len() - 1;
                self.playing = false;
                self.play_tag = None;
                self.fit_pending = true;
                self.status = format!("Geöffnet: {}", path.display());
                if !warnings.is_empty() {
                    self.status += &format!(" – {} Hinweise: {}", warnings.len(), warnings.join("; "));
                }
                true
            }
            Err(e) => {
                self.status = format!("Fehler beim Öffnen: {e}");
                false
            }
        }
    }

    /// Open a file in a tab (an open file's tab is selected).
    fn open_in_tab(&mut self, path: &Path) {
        if let Err(e) = self.open_tab(path) {
            self.status = e;
        }
    }

    fn save(&mut self) {
        if let Some(m) = self.map.as_mut() {
            if let Err(e) = m.save() {
                m.status = format!("Fehler beim Speichern: {e}");
            }
            return;
        }
        match self.path.clone() {
            Some(p) => self.save_to(&p),
            None => self.save_as(),
        }
    }

    fn save_to(&mut self, path: &Path) {
        let mut path = path.to_path_buf();
        if !io::is_document_path(&path) {
            path = PathBuf::from(format!("{}{}", path.display(), io::EXTENSION));
        }
        self.commit_floating();
        match io::save(&self.doc, &path) {
            Ok(()) => {
                // journal: the state before the GUI edits, then the saved one
                let journal = pixel_core::journal::Journal::for_document(&path);
                if let Some(before) = &self.saved_doc {
                    let _ = journal.ensure_current(before, "Zustand vor den GUI-Änderungen");
                }
                let note = if self.unsaved_steps.is_empty() {
                    "Benutzer: gespeichert".to_string()
                } else {
                    let n = self.unsaved_steps.len();
                    let mut steps: Vec<&str> = self.unsaved_steps.iter().rev().take(5).map(String::as_str).collect();
                    steps.reverse();
                    format!("Benutzer: {} ({n} Schritte)", steps.join(" / "))
                };
                let _ = journal.record(&self.doc, "user", &note);
                self.follow.seen = journal.last_id();
                self.follow.journal_mtime = mtime(&journal.file());
                self.saved_doc = Some(self.doc.clone());
                self.unsaved_steps.clear();
                // remember the mode unless the name brings it anyway
                if self.tile_mode != self.initial_tile_mode(&path) {
                    self.tile_memory.insert(tiled::memory_key(&path), self.tile_mode);
                }
                self.path = Some(path.clone());
                self.dirty = false;
                self.external_conflict = false;
                self.disk_mtime = mtime(&path);
                self.status = format!("Gespeichert: {}", path.display());
                if !self.doc.ramps.is_empty() && pixel_core::aseprite::is_aseprite_path(&path) {
                    self.status += " · Farbrampen stehen nur in .pxe.json, nicht in .aseprite";
                }
                if self.doc.reference.is_some() && pixel_core::aseprite::is_aseprite_path(&path) {
                    self.status += " · die Referenz steht nur in .pxe.json, nicht in .aseprite";
                }
            }
            Err(e) => self.status = format!("Fehler beim Speichern: {e}"),
        }
    }

    fn save_as(&mut self) {
        let mut dlg = rfd::FileDialog::new()
            .add_filter("Pixel-Editor", &["json"])
            .add_filter("Aseprite", &["aseprite", "ase"])
            .set_file_name(
                self.path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or("unbenannt.pxe.json".into(), |n| n.to_string_lossy().into_owned()),
            );
        if let Some(dir) = self.work_dir() {
            dlg = dlg.set_directory(dir);
        }
        match dlg.save_file() {
            Some(p) => self.save_to(&p),
            None => {
                // Portal missing or cancelled: offer the built-in path dialog.
                self.path_dialog = Some(PathDialog { save: true, text: self.default_path_text() });
            }
        }
    }

    fn open_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new().add_filter("Pixel-Editor / Aseprite", &["json", "aseprite", "ase"]);
        if let Some(dir) = self.work_dir() {
            dlg = dlg.set_directory(dir);
        }
        match dlg.pick_file() {
            Some(p) => self.open_in_tab(&p),
            None => self.path_dialog = Some(PathDialog { save: false, text: self.default_path_text() }),
        }
    }

    fn work_dir(&self) -> Option<PathBuf> {
        self.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| std::env::current_dir().ok())
    }

    fn default_path_text(&self) -> String {
        match &self.path {
            Some(p) => p.display().to_string(),
            None => self.work_dir().map(|d| d.join("unbenannt.pxe.json").display().to_string()).unwrap_or_default(),
        }
    }

    /// Request an action that would discard unsaved changes.
    fn request(&mut self, action: Pending) {
        if self.tab_dirty(self.active) {
            self.confirm = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn new_document(&mut self) {
        // in a tile folder (or next to a tile document) the tile template comes first
        let tiles = self.path.as_deref().or(self.workdir.as_deref()).is_some_and(tiled::suggests_tiles);
        let (size, tiled) = if tiles { (32, true) } else { (64, false) };
        self.new_dialog = Some(NewDialog { width: size, height: size, palette: "sweetie16".into(), tiled });
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::CloseTab => self.close_tab(self.active),
            Pending::Reload if self.map.is_some() => {
                let p = self.map.as_ref().unwrap().path.clone();
                self.open_map(&p);
            }
            Pending::Reload => {
                if let Some(p) = self.path.clone() {
                    let (frame, layer) = (self.frame, self.layer);
                    self.open(&p);
                    self.frame = frame;
                    self.layer = layer;
                    self.clamp_selection();
                }
            }
            Pending::Quit => self.allow_close = true,
        }
    }

    fn export(&mut self, target: Target) {
        let stem = self.path.as_deref().map(io::stem).unwrap_or_else(|| "unbenannt".into());
        let Some(dir) = self.work_dir() else { return };
        let out = dir.join("export").join(&stem);
        match export::export(&self.doc, target, &out, &stem, &ExportOptions::default()) {
            Ok(files) => {
                self.status = format!(
                    "Exportiert ({}): {}",
                    target.id(),
                    files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ")
                )
            }
            Err(e) => self.status = format!("Export fehlgeschlagen: {e}"),
        }
    }

    // ---- playback -----------------------------------------------------

    fn toggle_play(&mut self, now: f64) {
        self.playing = !self.playing;
        if self.playing {
            let seq = self.doc.sequence(self.play_tag);
            self.play_pos = seq.iter().position(|f| *f == self.frame).unwrap_or(0);
            self.frame = seq[self.play_pos];
            self.play_elapsed_ms = 0.0;
            self.play_last = now;
        }
    }

    fn advance_playback(&mut self, ctx: &egui::Context, now: f64) {
        if !self.playing {
            return;
        }
        let seq = self.doc.sequence(self.play_tag);
        let looping = self.play_tag.and_then(|t| self.doc.tags.get(t)).is_none_or(|t| t.looping);
        self.play_elapsed_ms += (now - self.play_last) * 1000.0;
        self.play_last = now;
        self.play_pos = self.play_pos.min(seq.len() - 1);
        loop {
            let ms = self.doc.frames[seq[self.play_pos]].duration_ms as f64;
            if self.play_elapsed_ms < ms {
                ctx.request_repaint_after(Duration::from_secs_f64((ms - self.play_elapsed_ms) / 1000.0));
                break;
            }
            self.play_elapsed_ms -= ms;
            if self.play_pos + 1 < seq.len() {
                self.play_pos += 1;
            } else if looping {
                self.play_pos = 0;
            } else {
                self.playing = false;
                break;
            }
        }
        self.frame = seq[self.play_pos];
    }

    // ---- input ----------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let cmd_shift = |k| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let hit = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        self.tab_shortcuts(ctx);
        // shift variants first: consume_shortcut ignores extra shift
        if hit(cmd_shift(Key::Z)) || hit(cmd(Key::Y)) {
            self.redo();
        }
        if hit(cmd(Key::Z)) {
            self.undo();
        }
        if hit(cmd_shift(Key::S)) {
            self.save_as();
        }
        if hit(cmd_shift(Key::K)) {
            self.point_for_claude();
        }
        if hit(cmd(Key::S)) {
            self.save();
        }
        if hit(cmd(Key::O)) {
            self.open_dialog();
        }
        if hit(cmd(Key::N)) {
            self.new_document();
        }
        if hit(cmd(Key::Quote)) {
            self.grid = !self.grid;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Aseprite-style shortcuts
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        let shift = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.matches_exact(Modifiers::SHIFT));
        let alt = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.matches_exact(Modifiers::ALT));
        for t in Tool::ALL {
            let (k, with_shift) = t.key();
            if (with_shift && shift(k)) || (!with_shift && pressed(k)) {
                self.set_tool(t);
            }
        }
        // clipboard: egui turns Ctrl+C/X/V into Copy/Cut/Paste events
        let (copy, cut, paste) = ctx.input(|i| {
            let ev = |f: &dyn Fn(&egui::Event) -> bool| i.events.iter().any(f);
            (
                ev(&|e| matches!(e, egui::Event::Copy)),
                ev(&|e| matches!(e, egui::Event::Cut)),
                ev(&|e| matches!(e, egui::Event::Paste(_))),
            )
        });
        if copy || hit(cmd(Key::C)) {
            self.copy_selection();
        }
        if cut || hit(cmd(Key::X)) {
            self.cut_selection();
        }
        if paste || hit(cmd(Key::V)) {
            self.paste();
        }
        if hit(cmd(Key::A)) {
            self.select_all();
        }
        if hit(cmd(Key::D)) {
            self.select_none();
        }
        if hit(cmd_shift(Key::I)) {
            self.invert_selection();
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selection();
        }
        if pressed(Key::Escape) {
            if self.tool == Tool::Point && self.mark_sel.is_some() {
                self.select_mark(None);
            } else if self.floating.is_some() {
                self.cancel_floating();
            } else {
                self.selection = None;
            }
        }
        if pressed(Key::F3) {
            self.onion = !self.onion;
        }
        if pressed(Key::H) {
            self.clean_view = !self.clean_view;
        }
        if shift(Key::H) {
            self.toggle_drop_behind();
        }
        if pressed(Key::P) {
            self.preview.settings.open = !self.preview.settings.open;
        }
        if pressed(Key::Enter) {
            if self.floating.is_some() {
                self.commit_floating();
            } else {
                self.toggle_play(ctx.input(|i| i.time));
            }
        }
        let n = self.doc.frames.len();
        // with a selection, arrows nudge it (and its pixels when floating)
        let nudge = self.selection.is_some() && self.tool.selects();
        let arrow = |k| pressed(k) || shift(k);
        if nudge && (arrow(Key::ArrowLeft) || arrow(Key::ArrowRight) || arrow(Key::ArrowUp) || arrow(Key::ArrowDown)) {
            let step = if ctx.input(|i| i.modifiers.shift) { 8 } else { 1 };
            let dx = if arrow(Key::ArrowLeft) {
                -step
            } else if arrow(Key::ArrowRight) {
                step
            } else {
                0
            };
            let dy = if arrow(Key::ArrowUp) {
                -step
            } else if arrow(Key::ArrowDown) {
                step
            } else {
                0
            };
            self.nudge_selection(dx, dy);
        } else {
            if pressed(Key::ArrowLeft) || pressed(Key::Comma) || alt(Key::Comma) {
                self.goto_frame((self.frame + n - 1) % n);
            }
            if pressed(Key::ArrowRight) || pressed(Key::Period) || alt(Key::Period) {
                self.goto_frame((self.frame + 1) % n);
            }
            if pressed(Key::ArrowUp) && self.layer + 1 < self.doc.layers.len() {
                self.commit_floating();
                self.layer += 1;
            }
            if pressed(Key::ArrowDown) && self.layer > 0 {
                self.commit_floating();
                self.layer -= 1;
            }
        }
        self.transform_shortcuts(ctx);
        self.tile_shortcuts(ctx);
        if alt(Key::N) {
            self.new_frame(true, false);
        }
        if alt(Key::B) {
            self.new_frame(false, false);
        }
        if alt(Key::C) && n > 1 {
            let sel = self.selected_frames();
            self.delete_frames(&sel);
        }
        if shift(Key::N) {
            self.new_layer();
        }
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.zoom_step(1, None);
        }
        if pressed(Key::Minus) {
            self.zoom_step(-1, None);
        }
        if pressed(Key::Num0) {
            self.fit_pending = true;
        }
    }

    // ---- dialogs --------------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.confirm.clone() {
            let mut close = false;
            // quitting concerns every tab, the rest only the active one
            let all = matches!(action, Pending::Quit);
            let dirty = self.dirty_labels();
            egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
                ui.heading("Ungespeicherte Änderungen");
                if all && dirty.len() > 1 {
                    ui.label(format!(
                        "{} Dokumente haben ungespeicherte Änderungen: {}",
                        dirty.len(),
                        dirty.join(", ")
                    ));
                } else if all {
                    ui.label(format!("{} hat ungespeicherte Änderungen.", dirty.join("")));
                } else {
                    ui.label(format!("{} hat ungespeicherte Änderungen.", self.tab_label(self.active)));
                }
                ui.horizontal(|ui| {
                    if ui.button(if all && dirty.len() > 1 { "Alle speichern" } else { "Speichern" }).clicked() {
                        if all {
                            self.save_all();
                        } else {
                            self.save();
                        }
                        if !(if all { self.any_dirty() } else { self.tab_dirty(self.active) }) {
                            self.perform(action.clone());
                        }
                        close = true;
                    }
                    if ui.button("Verwerfen").clicked() {
                        self.dirty = false;
                        if let Some(m) = self.map.as_mut() {
                            m.dirty = false;
                        }
                        self.perform(action.clone());
                        close = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = None;
                if self.allow_close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        if let Some(dlg) = &mut self.new_dialog {
            let mut create = None;
            let mut cancel = false;
            egui::Modal::new(egui::Id::unique("new")).show(ctx, |ui| {
                ui.heading("Neues Dokument");
                ui.horizontal(|ui| {
                    if ui.button("Figur 48×48").clicked() {
                        (dlg.width, dlg.height, dlg.tiled) = (48, 48, false);
                    }
                    if ui.button("Tile 32×32").clicked() {
                        (dlg.width, dlg.height, dlg.tiled) = (32, 32, true);
                    }
                    if ui.button("Tile 16×16").clicked() {
                        (dlg.width, dlg.height, dlg.tiled) = (16, 16, true);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Breite");
                    ui.add(egui::DragValue::new(&mut dlg.width).range(1..=1024));
                    ui.label("Höhe");
                    ui.add(egui::DragValue::new(&mut dlg.height).range(1..=1024));
                });
                egui::ComboBox::from_label("Palette").selected_text(&dlg.palette).show_ui(ui, |ui| {
                    for name in palettes::names() {
                        ui.selectable_value(&mut dlg.palette, name.to_string(), name);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Erstellen").clicked() {
                        create = Some((dlg.width, dlg.height, dlg.palette.clone(), dlg.tiled));
                    }
                    if ui.button("Abbrechen").clicked() {
                        cancel = true;
                    }
                });
            });
            if let Some((w, h, pal, tiled)) = create {
                let doc = Document::new(w, h, Document::builtin_palette(&pal).unwrap()).unwrap();
                if self.path.is_some() || self.dirty || self.rev > 0 {
                    self.new_tab(); // keep the current document in its tab
                }
                self.history.clear();
                self.replace_doc(doc);
                self.path = None;
                self.dirty = false;
                self.disk_mtime = None;
                self.frame = 0;
                self.layer = 0;
                self.play_tag = None;
                self.playing = false;
                self.fit_pending = true;
                self.tile_mode = if tiled { tiled::TileMode::Both } else { tiled::TileMode::Off };
                self.status = format!("Neues Dokument {w}×{h}");
                self.new_dialog = None;
            } else if cancel {
                self.new_dialog = None;
            }
        }

        if let Some(dlg) = &mut self.path_dialog {
            let mut chosen: Option<PathBuf> = None;
            let mut cancel = false;
            let save = dlg.save;
            egui::Modal::new(egui::Id::unique("path")).show(ctx, |ui| {
                ui.heading(if save { "Speichern unter" } else { "Öffnen" });
                ui.label("Pfad:");
                let r = ui.add(egui::TextEdit::singleline(&mut dlg.text).desired_width(480.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    chosen = Some(PathBuf::from(dlg.text.trim()));
                }
                let dir = Path::new(dlg.text.trim()).parent().filter(|d| d.is_dir()).map(Path::to_path_buf);
                if let Some(dir) = dir {
                    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                        .map(|rd| {
                            rd.flatten()
                                .map(|e| e.path())
                                .filter(|p| io::is_document_path(p) || pixel_core::tilemap::is_tilemap_path(p))
                                .collect()
                        })
                        .unwrap_or_default();
                    files.sort();
                    if !files.is_empty() {
                        ui.label(format!("Dokumente in {}:", dir.display()));
                        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            for f in files {
                                let name = f.file_name().unwrap().to_string_lossy().into_owned();
                                if ui.selectable_label(false, name).clicked() {
                                    dlg.text = f.display().to_string();
                                    if !save {
                                        chosen = Some(f);
                                    }
                                }
                            }
                        });
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button(if save { "Speichern" } else { "Öffnen" }).clicked() {
                        chosen = Some(PathBuf::from(dlg.text.trim()));
                    }
                    if ui.button("Abbrechen").clicked() {
                        cancel = true;
                    }
                });
            });
            if let Some(p) = chosen {
                self.path_dialog = None;
                if save {
                    self.save_to(&p);
                } else {
                    self.open_in_tab(&p);
                }
            } else if cancel {
                self.path_dialog = None;
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
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
                if ui.add_enabled(self.path.is_some(), egui::Button::new("Neu laden")).clicked() {
                    self.request(Pending::Reload);
                }
                ui.separator();
                if ui.button("Speichern  (Strg+S)").clicked() {
                    self.save();
                }
                if ui.button("Speichern unter…  (Strg+Umschalt+S)").clicked() {
                    self.save_as();
                }
                if ui.button("Speichern unter (Pfad)…").clicked() {
                    self.path_dialog = Some(PathDialog { save: true, text: self.default_path_text() });
                }
                ui.separator();
                if ui.button("Tab schließen  (Strg+W)").clicked() {
                    self.request_close_tab(self.active);
                }
                if ui.button("Beenden").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Bearbeiten", |ui| {
                if ui.add_enabled(self.history.can_undo(), egui::Button::new("Rückgängig  (Strg+Z)")).clicked() {
                    self.undo();
                }
                if ui
                    .add_enabled(self.history.can_redo(), egui::Button::new("Wiederholen  (Strg+Umschalt+Z)"))
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                if ui.button("Ebene leeren").clicked() {
                    let (l, f) = (self.layer, self.frame);
                    if !self.blocked() {
                        let desc = format!("Cel geleert: {}", self.here());
                        self.edit(desc, |d| *d.cel_mut(l, f) = vec![pixel_core::TRANSPARENT; d.pixel_count()]);
                    }
                }
            });
            ui.menu_button("Auswahl", |ui| {
                self.selection_menu_entries(ui);
            });
            ui.menu_button("Bild", |ui| {
                self.image_menu_entries(ui);
            });
            ui.menu_button("Werkzeuge", |ui| {
                self.tool_menu_entries(ui);
                ui.separator();
                let mut behind = self.drop_behind;
                if ui.checkbox(&mut behind, "Verschobene Pixel hinten ablegen  (Umschalt+H)").clicked() {
                    self.toggle_drop_behind();
                }
                ui.checkbox(&mut self.wand_all, "Zauberstab: alle gleichfarbigen");
                ui.separator();
                self.mark_menu_entries(ui);
            });
            ui.menu_button("Ansicht", |ui| {
                ui.checkbox(&mut self.grid, "Pixelraster  (Strg+')");
                ui.checkbox(&mut self.onion, "Onion Skin  (F3)");
                ui.checkbox(&mut self.show_regions, "Regionen anzeigen");
                ui.checkbox(&mut self.clean_view, "Saubere Ansicht: alle Einblendungen aus  (H)");
                ui.checkbox(&mut self.preview.settings.open, "Vorschau  (P)");
                ui.checkbox(&mut self.canvas.reference.open, "Referenz…");
                self.tile_menu(ui);
                ui.checkbox(&mut self.follow.enabled, "KI folgen (bei externen Änderungen mitspringen)");
                self.mark_menu_entries(ui);
                if ui.button("Einpassen  (0)").clicked() {
                    self.fit_pending = true;
                }
            });
            ui.menu_button("Export", |ui| {
                let labels = [
                    (Target::PngSheet, "PNG-Sheet + JSON"),
                    (Target::Godot, "Godot (SpriteFrames .tres)"),
                    (Target::Unity, "Unity (PNG + .meta)"),
                    (Target::Gif, "GIF je Animation"),
                    (Target::Aseprite, "Aseprite (.aseprite, indiziert)"),
                ];
                for (t, label) in labels {
                    if ui.button(label).clicked() {
                        self.export(t);
                    }
                }
                ui.label(egui::RichText::new("Ziel: <Dokumentordner>/export/<name>/").small().weak());
            });
            // the tabs share the menu row, so the panels keep their size
            ui.separator();
            self.tab_bar(ui);
        });
    }

    /// "Frame walk_down/2 (f7) · Ebene body · x 12, y 20 · Farbe 9 harness · Region face"
    fn status_line(&self) -> String {
        let d = &self.doc;
        let f = self.frame;
        let mut frame = d.frame_label(f);
        if frame != d.frames[f].name {
            frame += &format!(" ({})", d.frames[f].name);
        }
        let mut parts = vec![format!("Frame {frame}"), format!("Ebene {}", d.layers[self.layer].name)];
        if d.frames[f].locked || d.layers[self.layer].locked {
            parts.push("GESPERRT".into());
        }
        if let Some((x, y)) = self.canvas.hover {
            parts.push(format!("x {x}, y {y}"));
            let k = d.get(self.layer, f, x, y);
            let shown = if k == pixel_core::TRANSPARENT { d.top_key(f, x, y) } else { k };
            parts.push(format!(
                "Farbe {}",
                if shown == pixel_core::TRANSPARENT { "transparent".into() } else { d.key_label(shown) }
            ));
            let regions: Vec<&str> = d
                .regions
                .iter()
                .filter(|r| r.rect_at(f).is_some_and(|rc| rc.contains(x, y)))
                .map(|r| r.name.as_str())
                .collect();
            if !regions.is_empty() {
                parts.push(format!("Region {}", regions.join(", ")));
            }
        } else {
            parts.push(format!("Stift {}", d.key_label(self.color)));
        }
        if let Some(s) = &self.selection {
            parts.push(format!("Auswahl {}", s.describe()));
        }
        parts.join(" · ")
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.tab_label(self.active);
        let title = format!("{}{} – pixel-editor", if self.tab_dirty(self.active) { "• " } else { "" }, name);
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for EditorApp {
    fn on_exit(&mut self) {
        for p in self.tab_paths() {
            crate::launch::unregister_gui(&p);
        }
        self.withdraw_view();
        self.ipc = None; // removes the socket
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if ctx.input(|i| i.viewport().close_requested()) && self.any_dirty() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Pending::Quit);
        }
        self.follow.now = now;
        if self.debug_keys {
            // PIXEL_EDITOR_DEBUG_KEYS=1: what the window receives, to gui.log
            ctx.input(|i| {
                for e in &i.events {
                    if let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = e {
                        eprintln!("key {key:?} (physical {physical_key:?}) {modifiers:?}");
                    }
                }
            });
        }
        self.follow_note_input(&ctx);
        self.handle_ipc(&ctx);
        self.sync_all();
        ctx.request_repaint_after(follow::POLL);
        if self.follow.banner_text().is_some() || !self.follow.flashes.is_empty() {
            ctx.request_repaint(); // animate flash and banner
        }
        if self.map.is_some() {
            self.map_shortcuts(&ctx);
            egui::Panel::top("menu").show(ui, |ui| self.map_menu_bar(ui));
            self.map_ui(ui, &ctx, now);
            self.save_settings(&ctx);
            self.dialogs(&ctx);
            self.map_dialog_ui(&ctx);
            self.update_title(&ctx);
            self.publish_view();
            return;
        }
        self.shortcuts(&ctx);
        self.advance_playback(&ctx, now);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            // the names the user can read off and tell Claude
            ui.horizontal(|ui| {
                let badge = self.follow.badge();
                if ui
                    .selectable_label(self.follow.enabled, badge)
                    .on_hover_text("KI folgen: bei Änderungen von außen zu Frame, Ebene und Bereich springen")
                    .clicked()
                {
                    self.follow.enabled = !self.follow.enabled;
                }
                if ui
                    .button("KI zeigen")
                    .on_hover_text(
                        "Auswahl (oder ganzen Frame) für die KI markieren, Strg+Umschalt+K; die KI liest sie mit get_user_view",
                    )
                    .clicked()
                {
                    self.point_for_claude();
                }
                if !self.marks.marks.is_empty()
                    && ui
                        .small_button(format!("KI-Markierungen ({}) ×", self.marks.marks.len()))
                        .on_hover_text("Alle KI-Markierungen dieses Dokuments entfernen")
                        .clicked()
                {
                    self.clear_all_marks();
                }
                let clean = if self.clean_view { "Saubere Ansicht: an" } else { "Saubere Ansicht" };
                if ui
                    .selectable_label(self.clean_view, clean)
                    .on_hover_text("Raster, Regionen, Marker, Auswahlrahmen, Onion Skin, Aufleuchten und Banner ausblenden (H)")
                    .clicked()
                {
                    self.clean_view = !self.clean_view;
                }
                ui.separator();
                ui.strong(self.status_line());
            });
            if let Some(note) = self.follow.banner_text().map(str::to_string) {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 210, 90), note);
                });
            }
            ui.horizontal(|ui| {
                ui.label(format!("{}×{} · Zoom {}×", self.doc.width, self.doc.height, self.zoom));
                ui.separator();
                ui.label(&self.status);
            });
        });
        if self.external_conflict {
            egui::Panel::top("conflict").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(255, 200, 80),
                        "Die Datei wurde extern geändert, du hast aber ungespeicherte Änderungen.",
                    );
                    if ui.button("Neu laden (meine Änderungen verwerfen)").clicked() {
                        self.dirty = false;
                        self.perform(Pending::Reload);
                    }
                    if ui.button("Meine Version behalten").clicked() {
                        self.external_conflict = false;
                    }
                });
            });
        }
        egui::Panel::bottom("timeline").resizable(true).default_size(230.0).show(ui, |ui| self.timeline_panel(ui, now));
        egui::Panel::left("tools").resizable(true).default_size(210.0).show(ui, |ui| self.tools_panel(ui));
        egui::Panel::right("properties").resizable(true).default_size(240.0).show(ui, |ui| self.properties_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.canvas_ui(ui));
        self.note_field(&ctx);
        self.preview_window(&ctx, now);
        self.save_settings(&ctx);

        self.dialogs(&ctx);
        self.map_dialog_ui(&ctx);
        self.mirror_dialog_ui(&ctx);
        self.reference_window(&ctx);
        self.recolor_dialog_ui(&ctx);
        self.update_title(&ctx);
        self.publish_view();
        if self.autoshot.as_mut().is_some_and(|s| s.tick(&ctx, now)) {
            self.autoshot = None;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub fn to_color32(c: Rgba) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn clock() -> String {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // local time offset is not available without extra deps; show UTC
    format!("{:02}:{:02}:{:02} UTC", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

/// Start the window. `workdir` (default: see `ipc::default_workdir`) names
/// the tabs and lets `pixel-editor open` find this window.
pub fn run(path: Option<PathBuf>, workdir: Option<PathBuf>) -> Result<(), String> {
    // test hooks for `open`/`open_editor` (no window): fail at once, or
    // stay alive for a while and exit
    if let Some(msg) = std::env::var_os("PIXEL_EDITOR_TEST_FAIL") {
        return Err(msg.to_string_lossy().into_owned());
    }
    if let Some(ms) = std::env::var("PIXEL_EDITOR_TEST_HOLD_MS").ok().and_then(|s| s.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
        return Ok(());
    }
    let path = path.map(|p| p.canonicalize().unwrap_or(p));
    let workdir = workdir
        .map(|w| w.canonicalize().unwrap_or(w))
        .or_else(|| path.as_deref().map(crate::ipc::default_workdir))
        .or_else(|| std::env::current_dir().ok());
    // listen before the window exists, so a second `open` right after this
    // one finds the window instead of starting another
    let ipc = workdir.as_deref().and_then(|w| match crate::ipc::Server::listen(&crate::ipc::socket_dir(), w) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("no tabs from other processes: {e}");
            None
        }
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pixel-editor")
            .with_app_id("pixel-editor")
            .with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "pixel-editor",
        options,
        Box::new(move |cc| {
            let mut app = EditorApp::new(path);
            app.workdir = workdir;
            if let Some(s) = &ipc {
                let ctx = cc.egui_ctx.clone();
                s.set_waker(move || ctx.request_repaint());
            }
            app.ipc = ipc;
            let saved = app.workdir.as_ref().and_then(|w| app.saved_settings.open_tabs.get(&w.display().to_string()));
            if let Some(saved) = saved.cloned() {
                app.restore_tabs(&saved);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}
