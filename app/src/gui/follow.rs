//! "KI folgen" (follow the AI assistant): live sync with external edits (the MCP server; the
//! author is the MCP client: Claude, Codex, ...).
//!
//! Every ~100 ms the editor checks the document and its journal
//! (`.pxe-history/<file>/journal.json`). New journal entries are replayed in
//! order, so rapid successive writes each become a visible step:
//! - `edit`: load the state, add an undo step with the entry's note;
//! - `progress` (paced writes): show the intermediate state without an
//!   undo step; the final `edit` becomes one step from before the first one;
//! - `focus`: only move the view ("now working on fist_left").
//!
//! While following, the view jumps to the edit's frame and layer and brings
//! the changed area into view; changed pixels flash for 600 ms and the note
//! is shown as a banner for 3 s. Manual input pauses following for 5 s.

use std::path::Path;
use std::time::Duration;

use eframe::egui;
use pixel_core::journal::{Entry, Focus, Journal};
use pixel_core::{io, Document, Rect};

use super::{clock, mtime, EditorApp};

pub const POLL: Duration = Duration::from_millis(100);
pub const FLASH_SECS: f64 = 0.6;
pub const BANNER_SECS: f64 = 3.0;
pub const PAUSE_SECS: f64 = 5.0;
/// History description of a reload the journal does not describe.
const EXTERNAL: &str = "Extern geändert";

/// Changed pixels to highlight on one cel.
pub struct Flash {
    pub frame: usize,
    pub points: Vec<(i32, i32)>,
    pub bbox: Option<Rect>,
    pub start: f64,
}

pub struct FollowState {
    pub enabled: bool,
    pub paused_until: f64,
    pub flashes: Vec<Flash>,
    pub banner: Option<(String, f64)>,
    /// Newest journal entry already applied.
    pub seen: u64,
    pub journal_mtime: Option<std::time::SystemTime>,
    /// Document before the first `progress` entry of a paced edit.
    pub progress_base: Option<Document>,
    pub now: f64,
    /// Who made the last followed change ("Claude", "Codex", ...).
    pub last_author: Option<String>,
    /// A `focus` entry arrived: the window switches to this tab.
    pub focus_requested: bool,
}

impl Default for FollowState {
    fn default() -> Self {
        FollowState {
            enabled: true,
            paused_until: 0.0,
            flashes: Vec::new(),
            banner: None,
            seen: 0,
            journal_mtime: None,
            progress_base: None,
            now: 0.0,
            last_author: None,
            focus_requested: false,
        }
    }
}

impl FollowState {
    pub fn paused(&self) -> bool {
        self.now < self.paused_until
    }

    pub fn active(&self) -> bool {
        self.enabled && !self.paused()
    }

    /// "Folgen: an", "Folgen: aus", "Folgen: pausiert 4 s", with the
    /// author of the last followed change: "Folgen: an · Codex"
    pub fn badge(&self) -> String {
        let state = match (self.enabled, self.paused()) {
            (false, _) => "Folgen: aus".into(),
            (true, true) => format!("Folgen: pausiert {} s", (self.paused_until - self.now).ceil() as i64),
            (true, false) => "Folgen: an".into(),
        };
        match &self.last_author {
            Some(a) => format!("{state} · {a}"),
            None => state,
        }
    }

    pub fn banner_text(&self) -> Option<&str> {
        self.banner.as_ref().filter(|(_, t)| self.now - t < BANNER_SECS).map(|(s, _)| s.as_str())
    }
}

/// Display name of a journal author: "claude" (older journals) -> "Claude".
pub fn author_name(raw: &str) -> String {
    let mut c = raw.trim().chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => "KI".into(),
    }
}

/// The entry's note with its author in front ("Codex: set_rows …"), unless
/// the note already starts with it.
pub fn labelled_note(e: &Entry) -> String {
    let name = author_name(&e.author);
    let prefix = format!("{name}:");
    if e.note.to_lowercase().starts_with(&prefix.to_lowercase()) {
        e.note.clone()
    } else {
        format!("{prefix} {}", e.note)
    }
}

/// Changed pixels between two documents on one cel (same layout required).
fn changed_points(a: &Document, b: &Document, layer: usize, frame: usize) -> Vec<(i32, i32)> {
    if a.width != b.width
        || a.height != b.height
        || layer >= a.layers.len().min(b.layers.len())
        || frame >= a.frames.len().min(b.frames.len())
    {
        return Vec::new();
    }
    let w = a.width as usize;
    a.cel(layer, frame)
        .iter()
        .zip(b.cel(layer, frame).iter())
        .enumerate()
        .filter(|(_, (p, q))| p != q)
        .map(|(i, _)| ((i % w) as i32, (i / w) as i32))
        .collect()
}

impl EditorApp {
    /// Remember the journal position when a file is opened, so old entries
    /// are not replayed.
    pub(super) fn follow_reset(&mut self, path: &Path) {
        let j = Journal::for_document(path);
        self.follow.seen = j.last_id();
        self.follow.journal_mtime = mtime(&j.file());
        self.follow.progress_base = None;
        self.follow.flashes.clear();
    }

    /// Manual input pauses following so it never fights the user.
    pub(super) fn follow_note_input(&mut self, ctx: &egui::Context) {
        let input = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::PointerButton { pressed: true, .. } | egui::Event::Key { pressed: true, .. })
            })
        });
        if input {
            self.follow.paused_until = self.follow.now + PAUSE_SECS;
        }
    }

    /// Check document and journal; apply external changes (every tab, see
    /// `sync_all`).
    pub(super) fn poll_disk(&mut self) {
        let Some(path) = self.path.clone() else { return };
        self.poll_marks(&path);
        let journal = Journal::for_document(&path);
        let jm = mtime(&journal.file());
        let dm = mtime(&path);
        let journal_changed = jm.is_some() && jm != self.follow.journal_mtime;
        let doc_changed = dm.is_some() && dm != self.disk_mtime;
        if !journal_changed && !doc_changed {
            return;
        }
        if self.dirty {
            if doc_changed {
                // unsaved local edits: never overwrite, ask (conflict bar)
                self.disk_mtime = dm;
                self.external_conflict = true;
            }
            self.follow.journal_mtime = jm;
            return;
        }
        if journal_changed {
            self.follow.journal_mtime = jm;
            for e in journal.entries_after(self.follow.seen) {
                self.follow.seen = e.id;
                self.apply_entry(&journal, &e);
            }
        }
        if doc_changed {
            self.disk_mtime = dm;
            match io::load(&path) {
                Ok(doc) if doc != self.doc => {
                    // a change the journal does not (yet) describe
                    self.drop_floating();
                    self.history.push(self.doc.clone(), EXTERNAL);
                    self.saved_doc = Some(doc.clone());
                    self.replace_doc(doc);
                    self.status = format!("Neu geladen ({}): {EXTERNAL}", clock());
                }
                Ok(_) => {}
                Err(e) => self.status = format!("Extern geändert, aber nicht lesbar: {e}"),
            }
        }
    }

    fn apply_entry(&mut self, journal: &Journal, e: &Entry) {
        if e.author == "user" {
            return; // our own saves
        }
        let note = labelled_note(e);
        if e.author != "system" {
            self.follow.last_author = Some(author_name(&e.author));
        }
        match e.kind.as_str() {
            "focus" => {
                self.follow.focus_requested = true;
                self.show_banner(&note);
                if let Some(f) = &e.focus {
                    self.follow_to(f);
                    if let Some(b) = f.bbox {
                        self.add_flash(f, Vec::new(), Some(Rect::new(b[0], b[1], b[2] as u32, b[3] as u32)));
                    }
                }
            }
            "progress" | "edit" => {
                let Ok(state) = journal.load_state(e) else { return };
                let progress = e.kind == "progress";
                if state != self.doc {
                    self.drop_floating();
                    let (layer, frame) = self.entry_cel(e, &state);
                    let points = changed_points(&self.doc, &state, layer, frame);
                    if progress {
                        if self.follow.progress_base.is_none() {
                            self.follow.progress_base = Some(self.doc.clone());
                        }
                    } else {
                        let before = self.follow.progress_base.take().unwrap_or_else(|| self.doc.clone());
                        self.history.push(before, note.clone());
                        self.saved_doc = Some(state.clone());
                    }
                    self.replace_doc(state);
                    let focus = e.focus.clone().unwrap_or(Focus { frame, layer, bbox: None, region: None });
                    self.follow_to(&focus);
                    self.add_flash(&focus, points, None);
                } else if !progress {
                    // the document was already reloaded before the entry was
                    // written: give that step its proper description
                    self.follow.progress_base = None;
                    if self.history.undo_steps().last() == Some(&EXTERNAL) {
                        self.history.describe_last(note.clone());
                    }
                    if let Some(f) = &e.focus {
                        self.follow_to(f);
                        let b = f.bbox.map(|b| Rect::new(b[0], b[1], b[2] as u32, b[3] as u32));
                        self.add_flash(f, Vec::new(), b);
                    }
                }
                self.show_banner(&note);
                self.status = format!("Neu geladen ({}): {note}", clock());
            }
            _ => {}
        }
    }

    /// The cel an entry is about: its focus, or the first changed cel.
    fn entry_cel(&self, e: &Entry, state: &Document) -> (usize, usize) {
        if let Some(f) = &e.focus {
            return (f.layer, f.frame);
        }
        if state.layers.len() == self.doc.layers.len() && state.frames.len() == self.doc.frames.len() {
            if let Some(c) = pixel_core::diff::changed_cels(&self.doc, state).first() {
                return (c.layer, c.frame);
            }
        }
        (self.layer, self.frame)
    }

    fn show_banner(&mut self, note: &str) {
        self.follow.banner = Some((note.to_string(), self.follow.now));
    }

    fn add_flash(&mut self, f: &Focus, points: Vec<(i32, i32)>, bbox: Option<Rect>) {
        let bbox = bbox.or_else(|| {
            let (x0, x1) = (points.iter().map(|p| p.0).min()?, points.iter().map(|p| p.0).max()?);
            let (y0, y1) = (points.iter().map(|p| p.1).min()?, points.iter().map(|p| p.1).max()?);
            Some(Rect::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
        });
        self.follow.flashes.retain(|fl| self.follow.now - fl.start < FLASH_SECS);
        self.follow.flashes.push(Flash { frame: f.frame, points, bbox, start: self.follow.now });
    }

    /// Jump to the edit's frame and layer and bring its area into view
    /// (only while following and not paused).
    fn follow_to(&mut self, f: &Focus) {
        if !self.follow.active() {
            return;
        }
        if f.frame < self.doc.frames.len() {
            self.playing = false;
            self.frame = f.frame;
        }
        if f.layer < self.doc.layers.len() {
            self.layer = f.layer;
        }
        let rect = f.bbox.map(|b| Rect::new(b[0], b[1], b[2].max(1) as u32, b[3].max(1) as u32)).or_else(|| {
            let r = f.region.as_ref()?;
            self.doc.regions.iter().find(|x| &x.name == r)?.rect_at(f.frame)
        });
        if let Some(r) = rect {
            self.canvas.focus_request = Some(r);
        }
    }
}
