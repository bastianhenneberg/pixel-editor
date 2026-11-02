//! Snapshot-based undo/redo with a description per step. Documents share
//! cel buffers via `Arc`, so a snapshot only costs the metadata plus the
//! cels changed since.

use crate::document::Document;

pub struct History {
    undo: Vec<(Document, String)>,
    redo: Vec<(Document, String)>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        History::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        History { undo: Vec::new(), redo: Vec::new(), limit: limit.max(1) }
    }

    /// Record the state before an edit described by `desc`. Clears redo.
    pub fn push(&mut self, before: Document, desc: impl Into<String>) {
        if self.undo.last().is_some_and(|(d, _)| *d == before) {
            return;
        }
        self.undo.push((before, desc.into()));
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Replace the description of the latest step (e.g. once a stroke ends).
    pub fn describe_last(&mut self, desc: impl Into<String>) {
        if let Some(last) = self.undo.last_mut() {
            last.1 = desc.into();
        }
    }

    /// Drop the last snapshot if the edit turned out to change nothing.
    pub fn discard_if_unchanged(&mut self, current: &Document) {
        if self.undo.last().is_some_and(|(d, _)| d == current) {
            self.undo.pop();
        }
    }

    /// Step back; returns the restored document and the undone step's description.
    pub fn undo(&mut self, current: &Document) -> Option<(Document, String)> {
        let (prev, desc) = self.undo.pop()?;
        self.redo.push((current.clone(), desc.clone()));
        Some((prev, desc))
    }

    pub fn redo(&mut self, current: &Document) -> Option<(Document, String)> {
        let (next, desc) = self.redo.pop()?;
        self.undo.push((current.clone(), desc.clone()));
        Some((next, desc))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Descriptions of undoable steps, oldest first.
    pub fn undo_steps(&self) -> Vec<&str> {
        self.undo.iter().map(|(_, d)| d.as_str()).collect()
    }

    /// Descriptions of redoable steps, next redo first.
    pub fn redo_steps(&self) -> Vec<&str> {
        self.redo.iter().rev().map(|(_, d)| d.as_str()).collect()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_with_descriptions() {
        let mut h = History::default();
        let mut d = Document::new(2, 2, Document::builtin_palette("mono").unwrap()).unwrap();
        let orig = d.clone();
        h.push(d.clone(), "Stift: 1 px");
        d.set(0, 0, 0, 0, b'1');
        let edited = d.clone();
        assert_eq!(h.undo_steps(), vec!["Stift: 1 px"]);
        let (back, desc) = h.undo(&d).unwrap();
        assert_eq!((back.clone(), desc.as_str()), (orig, "Stift: 1 px"));
        d = back;
        assert!(h.undo(&d).is_none());
        assert_eq!(h.redo_steps(), vec!["Stift: 1 px"]);
        d = h.redo(&d).unwrap().0;
        assert_eq!(d, edited);
        assert!(h.can_undo() && !h.can_redo());
        h.describe_last("Stift: 3 px");
        assert_eq!(h.undo_steps(), vec!["Stift: 3 px"]);
    }

    #[test]
    fn noop_edit_is_discarded() {
        let mut h = History::default();
        let d = Document::new(2, 2, Document::builtin_palette("mono").unwrap()).unwrap();
        h.push(d.clone(), "nichts");
        h.discard_if_unchanged(&d);
        assert!(!h.can_undo());
    }
}
