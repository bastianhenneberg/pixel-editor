//! Shared view for Claude: the editor publishes what it shows (document,
//! frame, layer, selection, cursor, zoom) to `.pxe-history/<file>/views/
//! <pid>.json`. The MCP tool `get_user_view` reads it together with the
//! "KI zeigen" marks (see `marks` and `crate::view`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::view::{self, View};

use super::EditorApp;

#[derive(Default)]
pub struct ShareState {
    /// Last written view (time zeroed), to write only on change.
    pub last: Option<View>,
    pub last_write: Option<Instant>,
    /// Document whose view file this process wrote last.
    pub written: Option<PathBuf>,
}

impl EditorApp {
    /// The current view, with time 0 (set when written).
    fn current_view(&self, path: &Path) -> View {
        let d = &self.doc;
        View {
            pid: std::process::id(),
            time_ms: 0,
            path: std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
            dirty: self.dirty,
            frame: view::frame_ref(d, self.frame),
            layer: view::layer_ref(d, self.layer),
            selection: self.selection_on_canvas().map(Into::into),
            selection_mask: self.selection_clipped().and_then(|s| s.mask_rows()),
            hover: self.canvas.hover.map(|(x, y)| view::hover_at(d, self.layer, self.frame, x, y)),
            zoom: self.zoom,
            following: self.follow.enabled,
            follow_paused: self.follow.enabled && self.follow.paused(),
            tool: self.tool.label().into(),
            playing: self.playing,
        }
    }

    /// Write the view file when the view changed (throttled) or as a
    /// heartbeat; move it along when the document changes.
    pub(super) fn publish_view(&mut self) {
        let path = self.path.clone();
        if self.share.written.is_some() && self.share.written != path {
            self.withdraw_view();
        }
        let Some(path) = path else { return };
        let mut v = self.current_view(&path);
        let since = self.share.last_write.map(|t| t.elapsed());
        let changed = self.share.last.as_ref() != Some(&v);
        let heartbeat = since.is_some_and(|s| s >= Duration::from_millis(view::HEARTBEAT_MS));
        let throttled = since.is_some_and(|s| s < Duration::from_millis(view::THROTTLE_MS));
        // a throttled change is written on a later frame (the editor
        // repaints every 100 ms anyway)
        if !(changed || heartbeat) || throttled {
            return;
        }
        self.share.last = Some(v.clone());
        self.share.last_write = Some(Instant::now());
        self.share.written = Some(path);
        v.time_ms = view::now_ms();
        if let Err(e) = view::write_view(&v) {
            self.status = format!("Ansicht für die KI nicht geschrieben: {e}");
        }
    }

    /// Remove this editor's view file (closing, other document).
    pub(super) fn withdraw_view(&mut self) {
        if let Some(old) = self.share.written.take() {
            view::remove_view(&old, std::process::id());
        }
        self.share.last = None;
        self.share.last_write = None;
    }
}
