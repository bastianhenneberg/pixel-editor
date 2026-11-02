//! Persistent edit history shared by the GUI and the MCP server.
//!
//! For `dir/hero.pxe.json` the journal lives in
//! `dir/.pxe-history/hero.pxe.json/`:
//! - `journal.json`: entries `{id, time, author, note, state, hash, base}`
//! - `states/<id>.pxe.json`: the document *after* each entry
//! - `snapshots/<name>.pxe.json` + `snapshots.json`: named snapshots
//!
//! Every MCP edit records an entry with its description, so the GUI can
//! show "Claude: set_rows walk_down/2 body (3x2)" (or "Codex: …", the
//! author is the MCP client) when it reloads, and
//! `undo` can step back across processes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::io;

/// How many states are kept (oldest are pruned).
const MAX_STATES: usize = 300;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub id: u64,
    /// Unix seconds.
    pub time: u64,
    /// Who made the change: the AI client's name ("Claude", "Codex", ...;
    /// older journals: "claude"), "user" for GUI saves, "system".
    pub author: String,
    pub note: String,
    /// File name in `states/`.
    pub state: String,
    /// Hash of the document state, to recognise it again.
    pub hash: String,
    /// Id of the entry whose state this equals (itself unless it is an undo).
    pub base: u64,
    /// "edit" (an undoable step with a state), "progress" (an intermediate
    /// state of a paced edit, not a step) or "focus" (view hint, no state).
    #[serde(default = "edit_kind")]
    pub kind: String,
    /// Where the edit happened, for the GUI to follow along.
    #[serde(default)]
    pub focus: Option<Focus>,
}

fn edit_kind() -> String {
    "edit".into()
}

/// Location of an edit or a focus hint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Focus {
    pub frame: usize,
    pub layer: usize,
    /// Changed area (x, y, w, h).
    #[serde(default)]
    pub bbox: Option<[i32; 4]>,
    #[serde(default)]
    pub region: Option<String>,
}

impl Entry {
    /// An undoable step (has a state and is not an intermediate one).
    pub fn is_step(&self) -> bool {
        self.kind == "edit"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub note: String,
    pub time: u64,
    pub hash: String,
}

/// Stable content hash of a document (FNV-1a over its JSON form).
pub fn doc_hash(doc: &Document) -> String {
    format!("{:016x}", crate::export::fnv64(&io::to_json(doc)))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Journal {
    dir: PathBuf,
}

impl Journal {
    pub fn for_document(path: &Path) -> Journal {
        let parent = path.parent().unwrap_or(Path::new("."));
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Journal { dir: parent.join(".pxe-history").join(name) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self) -> Vec<Entry> {
        std::fs::read_to_string(self.dir.join("journal.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write_entries(&self, entries: &[Entry]) -> Result<(), String> {
        io::write_atomic(&self.dir.join("journal.json"), serde_json::to_string_pretty(entries).unwrap().as_bytes())
    }

    /// Record `doc` as the state after an edit described by `note`.
    pub fn record(&self, doc: &Document, author: &str, note: &str) -> Result<Entry, String> {
        self.record_full(Some(doc), author, note, "edit", None, None)
    }

    /// An undoable edit with its location (the GUI jumps there).
    pub fn record_edit(&self, doc: &Document, author: &str, note: &str, focus: Option<Focus>) -> Result<Entry, String> {
        self.record_full(Some(doc), author, note, "edit", focus, None)
    }

    /// An intermediate state of a paced edit (shown live, not a history step).
    pub fn record_progress(
        &self,
        doc: &Document,
        author: &str,
        note: &str,
        focus: Option<Focus>,
    ) -> Result<Entry, String> {
        self.record_full(Some(doc), author, note, "progress", focus, None)
    }

    /// A view hint without any change ("now working on fist_left").
    pub fn record_focus(&self, author: &str, note: &str, focus: Focus) -> Result<Entry, String> {
        self.record_full(None, author, note, "focus", Some(focus), None)
    }

    fn record_with_base(&self, doc: &Document, author: &str, note: &str, base: Option<u64>) -> Result<Entry, String> {
        self.record_full(Some(doc), author, note, "edit", None, base)
    }

    fn record_full(
        &self,
        doc: Option<&Document>,
        author: &str,
        note: &str,
        kind: &str,
        focus: Option<Focus>,
        base: Option<u64>,
    ) -> Result<Entry, String> {
        let states = self.dir.join("states");
        std::fs::create_dir_all(&states).map_err(|e| format!("{}: {e}", states.display()))?;
        let mut entries = self.entries();
        let id = entries.last().map_or(1, |e| e.id + 1);
        let (state, hash) = match doc {
            Some(doc) => {
                let state = format!("{id:06}.pxe.json");
                io::write_atomic(&states.join(&state), io::to_json(doc).as_bytes())?;
                (state, doc_hash(doc))
            }
            None => (String::new(), String::new()),
        };
        let entry = Entry {
            id,
            time: now(),
            author: author.into(),
            note: note.into(),
            state,
            hash,
            base: base.unwrap_or(id),
            kind: kind.into(),
            focus,
        };
        entries.push(entry.clone());
        while entries.len() > MAX_STATES {
            let old = entries.remove(0);
            if !old.state.is_empty() {
                let _ = std::fs::remove_file(states.join(&old.state));
            }
        }
        self.write_entries(&entries)?;
        Ok(entry)
    }

    /// Make sure the journal knows the current state; records it as
    /// `note` if the last entry differs (first edit, or changes made
    /// outside the editor/MCP).
    pub fn ensure_current(&self, doc: &Document, note: &str) -> Result<(), String> {
        let hash = doc_hash(doc);
        if self.entries().iter().rev().find(|e| e.is_step()).is_none_or(|e| e.hash != hash) {
            self.record(doc, "system", note)?;
        }
        Ok(())
    }

    /// The newest edit entry whose state equals `doc`.
    pub fn entry_for(&self, doc: &Document) -> Option<Entry> {
        let hash = doc_hash(doc);
        self.entries().into_iter().rev().find(|e| e.is_step() && e.hash == hash)
    }

    /// Path of journal.json (the GUI watches its modification time).
    pub fn file(&self) -> PathBuf {
        self.dir.join("journal.json")
    }

    /// Id of the newest entry (0 if none).
    pub fn last_id(&self) -> u64 {
        self.entries().last().map_or(0, |e| e.id)
    }

    /// Entries newer than `id`, oldest first.
    pub fn entries_after(&self, id: u64) -> Vec<Entry> {
        self.entries().into_iter().filter(|e| e.id > id).collect()
    }

    pub fn load_state(&self, e: &Entry) -> Result<Document, String> {
        io::load(&self.dir.join("states").join(&e.state))
    }

    /// Step back `steps` edits from the current state and record the undo.
    /// Returns the restored document and the entry that was undone to.
    pub fn undo(&self, current: &Document, steps: usize, author: &str) -> Result<(Document, Entry), String> {
        self.ensure_current(current, "changes outside the journal")?;
        // only undoable steps count (not progress or focus entries)
        let entries: Vec<Entry> = self.entries().into_iter().filter(|e| e.is_step()).collect();
        let cur = entries.last().unwrap();
        let pos = entries.iter().position(|e| e.id == cur.base).unwrap_or(entries.len() - 1);
        if steps == 0 || steps > pos {
            return Err(format!("cannot undo {steps} step(s): only {pos} earlier state(s) in the history"));
        }
        let target = entries[pos - steps].clone();
        let doc = self.load_state(&target)?;
        let undone: Vec<&str> = entries[pos - steps + 1..=pos].iter().map(|e| e.note.as_str()).collect();
        let note = format!("undo: {}", undone.join(" / "));
        self.record_with_base(&doc, author, &note, Some(target.base))?;
        Ok((doc, target))
    }

    // ---- snapshots ------------------------------------------------------

    fn snapshot_path(&self, name: &str) -> Result<PathBuf, String> {
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(format!("invalid snapshot name {name:?}"));
        }
        Ok(self.dir.join("snapshots").join(format!("{name}.pxe.json")))
    }

    pub fn snapshots(&self) -> Vec<Snapshot> {
        std::fs::read_to_string(self.dir.join("snapshots.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save_snapshot(&self, name: &str, note: &str, doc: &Document) -> Result<Snapshot, String> {
        let path = self.snapshot_path(name)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        io::write_atomic(&path, io::to_json(doc).as_bytes())?;
        let mut list = self.snapshots();
        list.retain(|s| s.name != name);
        let snap = Snapshot { name: name.into(), note: note.into(), time: now(), hash: doc_hash(doc) };
        list.push(snap.clone());
        io::write_atomic(&self.dir.join("snapshots.json"), serde_json::to_string_pretty(&list).unwrap().as_bytes())?;
        Ok(snap)
    }

    pub fn load_snapshot(&self, name: &str) -> Result<Document, String> {
        let path = self.snapshot_path(name)?;
        if !path.exists() {
            let names: Vec<String> = self.snapshots().into_iter().map(|s| s.name).collect();
            return Err(format!("no snapshot {name:?}; snapshots are {names:?}"));
        }
        io::load(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_undo_snapshots() {
        let dir = std::env::temp_dir().join(format!("pxe-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let j = Journal::for_document(&dir.join("a.pxe.json"));
        let mut d = Document::new(2, 2, Document::builtin_palette("mono").unwrap()).unwrap();
        let v0 = d.clone();
        j.ensure_current(&d, "start").unwrap();
        d.set(0, 0, 0, 0, b'1');
        let v1 = d.clone();
        j.record(&d, "claude", "one").unwrap();
        d.set(0, 0, 1, 0, b'1');
        j.record(&d, "claude", "two").unwrap();
        assert_eq!(j.entries().len(), 3);
        assert_eq!(j.entry_for(&v1).unwrap().note, "one");

        let (back, target) = j.undo(&d, 1, "Claude").unwrap();
        assert_eq!((back.clone(), target.note.as_str()), (v1.clone(), "one"));
        // a second undo continues backwards instead of toggling
        let (back2, _) = j.undo(&back, 1, "Claude").unwrap();
        assert_eq!(back2, v0);
        assert!(j.undo(&back2, 1, "Claude").is_err());
        assert!(j.entries().last().unwrap().note.starts_with("undo: one"));

        j.save_snapshot("before_fix", "vor dem Fix", &v1).unwrap();
        assert_eq!(j.load_snapshot("before_fix").unwrap(), v1);
        assert_eq!(j.snapshots()[0].note, "vor dem Fix");
        assert!(j.load_snapshot("nope").unwrap_err().contains("before_fix"));
        assert!(j.save_snapshot("../x", "", &v1).is_err());

        // focus and progress entries are not undo steps
        let last = j.last_id();
        let f = Focus { frame: 0, layer: 0, bbox: Some([0, 0, 1, 1]), region: Some("eye".into()) };
        j.record_focus("Claude", "Claude: arbeitet an eye", f.clone()).unwrap();
        j.record_progress(&v1, "Claude", "Claude: Zeile 1/2", Some(f.clone())).unwrap();
        j.record_edit(&v1, "claude", "Claude: eye", Some(f)).unwrap();
        let new = j.entries_after(last);
        assert_eq!(new.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["focus", "progress", "edit"]);
        assert_eq!(new[0].focus.as_ref().unwrap().region.as_deref(), Some("eye"));
        assert_eq!(j.entry_for(&v1).unwrap().note, "Claude: eye");
        let (back, _) = j.undo(&v1, 1, "Claude").unwrap();
        assert_ne!(back, v1, "undo skips progress/focus entries");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
