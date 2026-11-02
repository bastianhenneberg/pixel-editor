//! Shared view: what the user has open in the GUI, so Claude can see it
//! (MCP `get_user_view`).
//!
//! Every running editor writes `.pxe-history/<file>/views/<pid>.json` next
//! to the document it shows: on change (at most 5 times a second) and as a
//! heartbeat once a minute. It removes the file when it closes or switches
//! documents. One file per process, so several editors never clash.
//!
//! "KI zeigen" (tool K, button, Ctrl+Shift+K) adds a numbered mark to
//! `.pxe-history/<file>/marks.json`: an area (or a pixel, or the whole
//! frame) on one frame, with an optional note. A document has any number of
//! marks on any frames; they stay until removed (GUI, or MCP `clear_marks`).
//! The older single `pointer.json` is read as mark #1 and replaced on the
//! next write.
//!
//! Readers drop view files whose process is gone or that are older than
//! [`STALE_MS`] (a live editor refreshes its file every minute).

use std::path::{Path, PathBuf};

use pixel_core::journal::Journal;
use pixel_core::{io, Document, Rect, TRANSPARENT};
use serde::{Deserialize, Serialize};

/// View files older than this are ignored and removed.
pub const STALE_MS: u64 = 10 * 60 * 1000;
/// Minimum time between two writes of the view file.
pub const THROTTLE_MS: u64 = 200;
/// An unchanged view is rewritten this often, so readers know it is alive.
pub const HEARTBEAT_MS: u64 = 60 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrameRef {
    pub index: usize,
    pub name: String,
    /// "<tag>/<n>" when the frame is in a tag, else the name.
    pub address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LayerRef {
    pub index: usize,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl From<Rect> for Area {
    fn from(r: Rect) -> Self {
        Area { x: r.x, y: r.y, w: r.w, h: r.h }
    }
}

impl From<Area> for Rect {
    fn from(a: Area) -> Self {
        Rect::new(a.x, a.y, a.w, a.h)
    }
}

/// The pixel under the mouse cursor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hover {
    pub x: i32,
    pub y: i32,
    /// Key on the current layer ('.' = transparent).
    pub key: String,
    /// "9 harness" or "transparent".
    pub color: String,
    /// What is visible there (topmost visible layer), same form.
    pub visible: String,
    /// Regions whose rect on this frame contains the pixel.
    #[serde(default)]
    pub regions: Vec<String>,
}

/// What one editor shows right now.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct View {
    pub pid: u32,
    /// Unix milliseconds of the write.
    pub time_ms: u64,
    /// Absolute path of the document.
    pub path: PathBuf,
    /// The editor has unsaved changes (the file differs from the screen).
    pub dirty: bool,
    pub frame: FrameRef,
    pub layer: LayerRef,
    /// Bounding box of the selection, clipped to the canvas.
    pub selection: Option<Area>,
    /// Which pixels of `selection` are selected, row by row ('#' yes, '.'
    /// no); None = all of them (a rectangle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_mask: Option<Vec<String>>,
    pub hover: Option<Hover>,
    pub zoom: f32,
    /// "KI folgen" is switched on.
    pub following: bool,
    /// Following is paused for a few seconds after manual input.
    #[serde(default)]
    pub follow_paused: bool,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub playing: bool,
}

/// The older single "KI zeigen" pointer (`pointer.json`), read for migration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pointer {
    pub pid: u32,
    pub time_ms: u64,
    pub path: PathBuf,
    pub dirty: bool,
    pub frame: FrameRef,
    pub layer: LayerRef,
    /// The selection; None = the whole frame.
    pub rect: Option<Area>,
}

/// One "KI zeigen" mark.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mark {
    /// Running number per document (1, 2, 3 ...).
    pub n: u32,
    pub frame: FrameRef,
    /// The layer that was current when marking (whose pixels are listed).
    pub layer: LayerRef,
    /// The area; None = the whole frame.
    pub rect: Option<Area>,
    #[serde(default)]
    pub note: String,
    pub time_ms: u64,
    pub pid: u32,
    /// The editor had unsaved changes when marking.
    #[serde(default)]
    pub dirty: bool,
}

/// All marks of a document (`marks.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Marks {
    /// Number of the next mark.
    pub next: u32,
    pub marks: Vec<Mark>,
}

impl Marks {
    /// Add a mark and return its number. Numbers start at 1 again once
    /// every mark is gone.
    pub fn add(&mut self, mut m: Mark) -> u32 {
        if self.marks.is_empty() {
            self.next = 1;
        }
        m.n = self.next.max(self.marks.iter().map(|m| m.n + 1).max().unwrap_or(1));
        self.next = m.n + 1;
        self.marks.push(m);
        self.next - 1
    }

    pub fn get_mut(&mut self, n: u32) -> Option<&mut Mark> {
        self.marks.iter_mut().find(|m| m.n == n)
    }

    pub fn on_frame(&self, f: usize) -> impl Iterator<Item = &Mark> {
        self.marks.iter().filter(move |m| m.frame.index == f)
    }

    /// Remove the marks `remove` picks; returns them.
    pub fn remove_where(&mut self, mut remove: impl FnMut(&Mark) -> bool) -> Vec<Mark> {
        let (gone, kept): (Vec<Mark>, Vec<Mark>) = std::mem::take(&mut self.marks).into_iter().partition(|m| remove(m));
        self.marks = kept;
        gone
    }
}

pub fn marks_file(doc: &Path) -> PathBuf {
    Journal::for_document(doc).dir().join("marks.json")
}

/// The marks of `doc`; an old `pointer.json` counts as mark #1.
pub fn read_marks(doc: &Path) -> Marks {
    if let Ok(t) = std::fs::read_to_string(marks_file(doc)) {
        return serde_json::from_str(&t).unwrap_or_default();
    }
    let old: Option<Pointer> =
        std::fs::read_to_string(pointer_file(doc)).ok().and_then(|t| serde_json::from_str(&t).ok());
    match old {
        Some(p) => Marks {
            next: 2,
            marks: vec![Mark {
                n: 1,
                frame: p.frame,
                layer: p.layer,
                rect: p.rect,
                note: String::new(),
                time_ms: p.time_ms,
                pid: p.pid,
                dirty: p.dirty,
            }],
        },
        None => Marks::default(),
    }
}

/// Save the marks (no marks: no file); the old `pointer.json` goes away.
pub fn write_marks(doc: &Path, m: &Marks) -> Result<(), String> {
    let _ = std::fs::remove_file(pointer_file(doc));
    if m.marks.is_empty() {
        let _ = std::fs::remove_file(marks_file(doc));
        return Ok(());
    }
    write_json(&marks_file(doc), m)
}

pub fn frame_ref(doc: &Document, f: usize) -> FrameRef {
    FrameRef { index: f, name: doc.frames[f].name.clone(), address: doc.frame_label(f) }
}

pub fn layer_ref(doc: &Document, l: usize) -> LayerRef {
    LayerRef { index: l, name: doc.layers[l].name.clone() }
}

pub fn hover_at(doc: &Document, layer: usize, frame: usize, x: i32, y: i32) -> Hover {
    let label = |k: u8| if k == TRANSPARENT { "transparent".to_string() } else { doc.key_label(k) };
    let k = doc.get(layer, frame, x, y);
    Hover {
        x,
        y,
        key: (k as char).to_string(),
        color: label(k),
        visible: label(doc.top_key(frame, x, y)),
        regions: doc
            .regions
            .iter()
            .filter(|r| r.rect_at(frame).is_some_and(|rc| rc.contains(x, y)))
            .map(|r| r.name.clone())
            .collect(),
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn views_dir(doc: &Path) -> PathBuf {
    Journal::for_document(doc).dir().join("views")
}

pub fn view_file(doc: &Path, pid: u32) -> PathBuf {
    views_dir(doc).join(format!("{pid}.json"))
}

pub fn pointer_file(doc: &Path) -> PathBuf {
    Journal::for_document(doc).dir().join("pointer.json")
}

fn write_json<T: Serialize>(file: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    io::write_atomic(file, serde_json::to_string_pretty(value).unwrap().as_bytes())
}

pub fn write_view(v: &View) -> Result<(), String> {
    write_json(&view_file(&v.path, v.pid), v)
}

pub fn remove_view(doc: &Path, pid: u32) {
    let _ = std::fs::remove_file(view_file(doc, pid));
}

/// Whether a process with this pid exists. Without /proc (not Linux) every
/// pid counts as alive and only the age decides.
pub fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        Path::new(&format!("/proc/{pid}")).exists()
    } else {
        true
    }
}

/// Live views of the editors showing any of `docs`, newest first. Files of
/// dead processes and stale files are removed.
pub fn live_views(docs: &[PathBuf], now_ms: u64) -> Vec<(PathBuf, View)> {
    let mut out = Vec::new();
    for doc in docs {
        let Ok(entries) = std::fs::read_dir(views_dir(doc)) else { continue };
        for e in entries.flatten() {
            let file = e.path();
            if file.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let view: Option<View> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok());
            match view {
                Some(v) if pid_alive(v.pid) && now_ms.saturating_sub(v.time_ms) <= STALE_MS => {
                    out.push((doc.clone(), v))
                }
                // dead, stale or unreadable (a half-written file is impossible:
                // writes are atomic)
                _ => {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
    }
    out.sort_by_key(|(_, v)| std::cmp::Reverse(v.time_ms));
    out
}

/// "3 s ago", "4 min ago", "2 h ago"
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let s = now_ms.saturating_sub(then_ms) / 1000;
    match s {
        0..=89 => format!("{s} s ago"),
        90..=5399 => format!("{} min ago", (s + 30) / 60),
        _ => format!("{} h ago", (s + 1800) / 3600),
    }
}
