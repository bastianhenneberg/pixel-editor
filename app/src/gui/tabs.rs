//! Tabs: several documents in one window.
//!
//! The editor's fields describe the document in the active tab. A
//! background tab keeps the same per-document fields in a `TabState`;
//! switching tabs swaps them (see `per_tab!`). Window-wide things stay on
//! the editor: tool, view toggles (grid, onion, clean view, regions), the
//! preview window settings, the clipboard (copy between documents), dialogs,
//! the shared view for the AI assistant and "KI folgen" on/off and its pause.
//!
//! Background tabs are polled like the active one (live reload). A change
//! there marks the tab until it is viewed; only an explicit `focus` from the
//! AI assistant switches to it (while following, not paused).

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers};
use pixel_core::{io, Document, History};

use super::{canvas, follow, panels, select, timeline, transform, EditorApp, Pending};

/// Declares the per-document fields once: `TabState` holds them for a
/// background tab and `swap_tab` exchanges them with the editor's.
macro_rules! per_tab {
    ($($field:ident: $ty:ty,)*) => {
        /// A background tab's document state (the editor's fields of the same names).
        pub struct TabState {
            $($field: $ty,)*
        }

        impl TabState {
            /// Take the per-document fields of a (fresh) editor.
            fn from_app(app: EditorApp) -> Box<TabState> {
                Box::new(TabState { $($field: app.$field,)* })
            }
        }

        impl EditorApp {
            /// Exchange the active document's state with `t`; "KI folgen"
            /// on/off, its pause and the clock stay window-wide.
            fn swap_tab(&mut self, t: &mut TabState) {
                let (enabled, paused_until, now) = (self.follow.enabled, self.follow.paused_until, self.follow.now);
                $(std::mem::swap(&mut self.$field, &mut t.$field);)*
                self.follow.enabled = enabled;
                self.follow.paused_until = paused_until;
                self.follow.now = now;
            }
        }
    };
}

per_tab! {
    doc: Document,
    saved_doc: Option<Document>,
    unsaved_steps: Vec<String>,
    selection: Option<select::Selection>,
    floating: Option<select::Floating>,
    region_draft: panels::RegionDraft,
    snapshot_name: String,
    follow: follow::FollowState,
    path: Option<PathBuf>,
    dirty: bool,
    history: History,
    rev: u64,
    color: u8,
    symmetry: transform::Symmetry,
    tile_mode: super::tiled::TileMode,
    layer: usize,
    frame: usize,
    panel_frame: usize,
    zoom: f32,
    pan: egui::Vec2,
    fit_pending: bool,
    playing: bool,
    play_tag: Option<usize>,
    play_pos: usize,
    play_elapsed_ms: f64,
    play_last: f64,
    canvas: canvas::CanvasState,
    disk_mtime: Option<std::time::SystemTime>,
    external_conflict: bool,
    status: String,
    palette_edit_at: Option<(u8, f64)>,
    tag_draft: panels::TagDraft,
    timeline: timeline::TimelineState,
    marks: crate::view::Marks,
    marks_mtime: Option<std::time::SystemTime>,
    mark_sel: Option<u32>,
    note_draft: String,
}

/// Color of the "changed from outside" dot.
pub const MARK: Color32 = Color32::from_rgb(255, 210, 90);

/// One tab. The active tab's state is the editor itself (`state` is None).
#[derive(Default)]
pub struct Tab {
    state: Option<Box<TabState>>,
    /// Changed from outside while in the background, not viewed since.
    pub marked: bool,
    /// What changed it (the status line of that reload).
    pub note: Option<String>,
}

/// The same file? Compares canonical paths where the files exist.
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().or_else(|_| std::path::absolute(p)).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b)
}

impl EditorApp {
    /// The document path of tab `i`.
    pub(super) fn tab_path(&self, i: usize) -> Option<&Path> {
        match &self.tabs[i].state {
            None => self.path.as_deref(),
            Some(s) => s.path.as_deref(),
        }
    }

    pub(super) fn tab_dirty(&self, i: usize) -> bool {
        match &self.tabs[i].state {
            None => self.dirty,
            Some(s) => s.dirty,
        }
    }

    pub(super) fn any_dirty(&self) -> bool {
        (0..self.tabs.len()).any(|i| self.tab_dirty(i))
    }

    /// "toast_warrior/walk_side": the path relative to the window's workdir,
    /// without the document extension.
    pub(super) fn tab_label(&self, i: usize) -> String {
        let Some(p) = self.tab_path(i) else { return "unbenannt".into() };
        let rel = self.workdir.as_deref().and_then(|w| p.strip_prefix(w).ok()).unwrap_or(p);
        let rel = if rel == p { Path::new(p.file_name().unwrap_or_default()) } else { rel };
        let text = rel.display().to_string();
        [io::EXTENSION, ".aseprite", ".ase"]
            .iter()
            .find_map(|ext| text.strip_suffix(ext))
            .map_or(text.clone(), str::to_string)
    }

    /// Index of the tab showing `path`.
    pub(super) fn find_tab(&self, path: &Path) -> Option<usize> {
        (0..self.tabs.len()).find(|&i| self.tab_path(i).is_some_and(|p| same_file(p, path)))
    }

    /// An empty untitled document nobody touched: opening a file replaces it.
    fn active_is_blank(&self) -> bool {
        self.path.is_none() && self.rev == 0 && !self.dirty
    }

    pub(super) fn switch_tab(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        let mut s = self.tabs[i].state.take().expect("background tab has its state");
        self.swap_tab(&mut s);
        self.tabs[self.active].state = Some(s);
        self.active = i;
        self.tabs[i].marked = false;
        self.play_last = self.follow.now; // playback goes on from here, no catch-up
        self.preview.tab_switched();
    }

    /// Run `f` on tab `i` as if it were active, without showing it.
    fn with_tab<R>(&mut self, i: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if i == self.active {
            return f(self);
        }
        let mut s = self.tabs[i].state.take().expect("background tab has its state");
        self.swap_tab(&mut s);
        let r = f(self);
        self.swap_tab(&mut s);
        self.tabs[i].state = Some(s);
        r
    }

    /// A new empty tab, made active.
    pub(super) fn new_tab(&mut self) {
        let mut s = TabState::from_app(EditorApp::blank());
        self.swap_tab(&mut s);
        self.tabs[self.active].state = Some(s);
        self.tabs.push(Tab::default());
        self.active = self.tabs.len() - 1;
        self.preview.tab_switched();
    }

    /// Open `path` in a tab: switch to it if it is open already, reuse an
    /// untouched empty tab, else add one. Returns "shown", "switched",
    /// "opened" or the error.
    pub(super) fn open_tab(&mut self, path: &Path) -> Result<&'static str, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.find_tab(&path) {
            if i == self.active {
                return Ok("shown");
            }
            self.switch_tab(i);
            return Ok("switched");
        }
        if self.active_is_blank() {
            return if self.open(&path) { Ok("opened") } else { Err(self.status.clone()) };
        }
        let back = self.active;
        self.new_tab();
        if self.open(&path) {
            Ok("opened")
        } else {
            let err = self.status.clone();
            self.close_tab(self.active);
            self.switch_tab(back);
            self.status = err.clone();
            Err(err)
        }
    }

    /// Close tab `i` (unsaved changes are lost: see `request_close_tab`).
    /// The last tab is replaced by an empty document.
    pub(super) fn close_tab(&mut self, i: usize) {
        if let Some(p) = self.tab_path(i).map(Path::to_path_buf) {
            crate::launch::unregister_gui(&p);
        }
        if self.tabs.len() == 1 {
            let mut s = TabState::from_app(EditorApp::blank());
            self.swap_tab(&mut s);
            self.tabs[0] = Tab::default();
            self.preview.tab_switched();
            return;
        }
        if i == self.active {
            self.switch_tab(if i + 1 < self.tabs.len() { i + 1 } else { i - 1 });
        }
        self.tabs.remove(i);
        if i < self.active {
            self.active -= 1;
        }
    }

    /// Close a tab, asking first if it has unsaved changes (it is shown
    /// for that).
    pub(super) fn request_close_tab(&mut self, i: usize) {
        if self.tab_dirty(i) {
            self.switch_tab(i);
            self.confirm = Some(Pending::CloseTab);
        } else {
            self.close_tab(i);
        }
    }

    /// Save every tab with unsaved changes. An untitled background tab is
    /// skipped (its file dialog would belong to the shown tab) and stays dirty.
    pub(super) fn save_all(&mut self) {
        for i in 0..self.tabs.len() {
            if self.tab_dirty(i) && (i == self.active || self.tab_path(i).is_some()) {
                self.with_tab(i, |app| app.save());
            }
        }
    }

    /// Labels of the tabs with unsaved changes.
    pub(super) fn dirty_labels(&self) -> Vec<String> {
        (0..self.tabs.len()).filter(|&i| self.tab_dirty(i)).map(|i| self.tab_label(i)).collect()
    }

    /// Poll every tab's file and journal (every `follow::POLL`).
    pub(super) fn sync_all(&mut self) {
        if self.last_poll.elapsed() < follow::POLL {
            return;
        }
        self.last_poll = Instant::now();
        self.poll_disk();
        self.follow.focus_requested = false;
        let mut focus_to = None;
        for i in 0..self.tabs.len() {
            if i == self.active {
                continue;
            }
            let (changed, focus, status) = self.with_tab(i, |app| {
                let (rev, conflict) = (app.rev, app.external_conflict);
                app.poll_disk();
                let changed = app.rev != rev || app.external_conflict != conflict;
                (changed, std::mem::take(&mut app.follow.focus_requested), app.status.clone())
            });
            if changed || focus {
                self.tabs[i].marked = true;
                self.tabs[i].note = Some(status);
            }
            if focus && self.follow.active() {
                focus_to = Some(i);
            }
        }
        if let Some(i) = focus_to {
            self.switch_tab(i);
        }
    }

    /// Requests from `pixel-editor open` / MCP `open_editor` (see `crate::ipc`).
    pub(super) fn handle_ipc(&mut self, ctx: &egui::Context) {
        let mut requests = Vec::new();
        while let Some(r) = self.ipc.as_ref().and_then(|s| s.try_recv()) {
            requests.push(r);
        }
        for r in requests {
            let workdir = self.ipc.as_ref().map(|s| s.workdir.clone()).unwrap_or_default();
            let reply = match (r.msg["cmd"].as_str(), r.msg["path"].as_str()) {
                (Some("open"), Some(p)) => match self.open_tab(Path::new(p)) {
                    Ok(result) => crate::ipc::open_reply(&workdir, result),
                    Err(e) => serde_json::json!({"ok": false, "error": e}),
                },
                _ => serde_json::json!({"ok": false, "error": format!("unknown request {}", r.msg)}),
            };
            r.reply(reply);
            ctx.request_repaint();
        }
    }

    /// Paths of the open tabs (for restoring them on the next start).
    pub(super) fn tab_paths(&self) -> Vec<PathBuf> {
        (0..self.tabs.len()).filter_map(|i| self.tab_path(i).map(Path::to_path_buf)).collect()
    }

    /// Reopen the tabs this workdir had last time, around the one just
    /// opened (which stays active).
    pub(super) fn restore_tabs(&mut self, saved: &[PathBuf]) {
        let wanted = self.path.clone();
        for p in saved.iter().filter(|p| p.exists()) {
            let _ = self.open_tab(p);
        }
        let order = |p: Option<&Path>| p.and_then(|p| saved.iter().position(|s| same_file(s, p))).unwrap_or(usize::MAX);
        let mut keyed: Vec<(usize, Tab)> = std::mem::take(&mut self.tabs)
            .into_iter()
            .map(|t| {
                let p = match &t.state {
                    None => self.path.as_deref(),
                    Some(s) => s.path.as_deref(),
                };
                (order(p), t)
            })
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        self.tabs = keyed.into_iter().map(|(_, t)| t).collect();
        self.active = self.tabs.iter().position(|t| t.state.is_none()).unwrap_or(0);
        if let Some(i) = wanted.as_deref().and_then(|w| self.find_tab(w)) {
            self.switch_tab(i);
        }
    }

    pub(super) fn tab_shortcuts(&mut self, ctx: &egui::Context) {
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let n = self.tabs.len();
        // shift variant first: consume_shortcut ignores extra shift
        if hit(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
            self.switch_tab((self.active + n - 1) % n);
        }
        if hit(Modifiers::COMMAND, Key::Tab) {
            self.switch_tab((self.active + 1) % n);
        }
        if hit(Modifiers::COMMAND, Key::W) {
            self.request_close_tab(self.active);
        }
    }

    /// One tab per document (in the menu row): name, "•" for unsaved
    /// changes, a dot for changes from outside not yet viewed, and a close
    /// button.
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut close = None;
        ui.scope(|ui| {
            for i in 0..self.tabs.len() {
                let label = self.tab_label(i);
                let dirty = self.tab_dirty(i);
                let marked = self.tabs[i].marked;
                let text = format!("{}{label}", if dirty { "• " } else { "" });
                let mut hover = self.tab_path(i).map_or("noch nicht gespeichert".into(), |p| p.display().to_string());
                if dirty {
                    hover += "\nungespeicherte Änderungen";
                }
                if marked {
                    hover += &format!(
                        "\nvon außen geändert, noch nicht angesehen{}",
                        self.tabs[i].note.as_deref().map(|n| format!(": {n}")).unwrap_or_default()
                    );
                }
                let r = ui.selectable_label(i == self.active, text).on_hover_text(hover);
                if marked {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, MARK);
                }
                if r.clicked() {
                    switch = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if ui.small_button("×").on_hover_text("Tab schließen (Strg+W)").clicked() {
                    close = Some(i);
                }
                ui.separator();
            }
            if ui.small_button("+").on_hover_text("Neuer Tab (Strg+N: neues Dokument)").clicked() {
                self.new_tab();
            }
        });
        if let Some(i) = switch {
            self.switch_tab(i);
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        }
    }
}
