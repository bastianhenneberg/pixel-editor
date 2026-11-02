//! MCP server over stdio (JSON-RPC 2.0, one message per line).
//!
//! Every tool loads the document from disk, applies the change through
//! pixel-core and saves it atomically, so an open editor window picks the
//! change up through its file watcher. `.pxe.json` and `.aseprite`/`.ase`
//! files are both supported (the extension decides the format).
//!
//! Consistency rules (the user and Claude must be able to name a part and
//! change only that part):
//! - pixel writes need an explicit target: layer + frame, optionally a
//!   named region or a clip rect; everything outside it is guaranteed
//!   unchanged (changes outside are reverted and reported, and a final
//!   check verifies that no other cel, layer, frame, tag or palette entry
//!   changed);
//! - locked layers, frames and regions refuse writes with a clear error;
//! - every write returns a compact diff (count, bbox, rows before/after),
//!   supports `dry_run`, and is recorded with a note in the journal
//!   (`.pxe-history/`), from where it can be undone or compared with
//!   named snapshots.

use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
use pixel_core::aseprite::ColorMode;
use pixel_core::diff::{self, rows_block};
use pixel_core::document::{Direction, Ramp, Rect, Region, RegionKind, Tag, KEY_ORDER};
use pixel_core::export::{self, ExportOptions, Target};
use pixel_core::journal::{Focus, Journal};
use pixel_core::recolor::{self, Area, Outline, OutlineMode, Pattern};
use pixel_core::render::PreviewOptions;
use pixel_core::seams;
use pixel_core::transform::{self, Op};
use pixel_core::{draw, import, io, palettes, render, Document, PaletteEntry, Rgba, TRANSPARENT};
use serde_json::{json, Map, Value};

const SERVER_NAME: &str = "pixel-editor";
const DEFAULT_PROTOCOL: &str = "2025-06-18";
/// Diffs taller than this are summarised instead of printed.
const DIFF_MAX_ROWS: u32 = 40;

pub fn run(workdir: &Path) -> std::io::Result<()> {
    let workdir = workdir.canonicalize()?;
    let server = Server::new(workdir);
    eprintln!("pixel-editor MCP server, workdir {}", server.workdir.display());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let out: Vec<Value> = batch.iter().filter_map(|m| server.handle(m)).collect();
                (!out.is_empty()).then(|| Value::Array(out))
            }
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{}", serde_json::to_string(&r).unwrap())?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Result of a tool call: content blocks for the client.
enum Out {
    Text(String),
    TextAndImage(String, Vec<u8>),
}

/// Where a pixel write may change pixels: one cel, optionally clipped.
#[derive(Clone, Copy, Debug)]
struct CelScope {
    layer: usize,
    frame: usize,
    clip: Option<Rect>,
}

struct Server {
    workdir: PathBuf,
    /// Who edits through this session, from `initialize` clientInfo.name:
    /// "Claude", "Codex", ... ("KI" until known). Used as journal author and
    /// note prefix, so the GUI can say who changed what.
    author: std::cell::RefCell<String>,
}

/// Display name of an MCP client: "claude-code"/"claude…" -> "Claude",
/// "codex…" -> "Codex", otherwise the name capitalised; "KI" if missing.
fn author_from_client(name: Option<&str>) -> String {
    let name = name.map(str::trim).unwrap_or("");
    let lower = name.to_lowercase();
    if name.is_empty() {
        "KI".into()
    } else if lower.starts_with("claude") {
        "Claude".into()
    } else if lower.starts_with("codex") {
        "Codex".into()
    } else {
        let mut c = name.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    }
}

impl Server {
    fn new(workdir: PathBuf) -> Server {
        Server { workdir, author: std::cell::RefCell::new(author_from_client(None)) }
    }

    fn author(&self) -> String {
        self.author.borrow().clone()
    }

    fn handle(&self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
        let Some(method) = method else {
            // a response from the client or garbage; nothing to answer
            return id.map(|id| rpc_error(id, -32600, "invalid request"));
        };
        let id = id?; // notifications get no reply
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                let client = params.pointer("/clientInfo/name").and_then(Value::as_str);
                *self.author.borrow_mut() = author_from_client(client);
                let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(DEFAULT_PROTOCOL);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools()}),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match check_args(name, &args).and_then(|_| self.call(name, &args)) {
                    Ok(Out::Text(t)) => json!({"content": [{"type": "text", "text": t}], "isError": false}),
                    Ok(Out::TextAndImage(t, png)) => json!({"content": [
                        {"type": "text", "text": t},
                        {"type": "image", "mimeType": "image/png",
                         "data": base64::engine::general_purpose::STANDARD.encode(png)}
                    ], "isError": false}),
                    Err(e) => json!({"content": [{"type": "text", "text": format!("Error: {e}")}], "isError": true}),
                }
            }
            "resources/list" => json!({"resources": []}),
            "prompts/list" => json!({"prompts": []}),
            _ => return Some(rpc_error(id, -32601, &format!("method not found: {method}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    // ---- paths and loading ----------------------------------------------

    /// Resolve a path inside the workdir; `hero` finds `hero.pxe.json`
    /// (or `hero.aseprite`).
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
            let abs = if p.is_absolute() { p.to_path_buf() } else { self.workdir.join(p) };
            let canon =
                abs.parent().and_then(|d| d.canonicalize().ok()).map(|d| d.join(abs.file_name().unwrap_or_default()));
            match canon {
                Some(c) if c.starts_with(&self.workdir) => return Ok(c),
                _ => return Err(format!("path {rel:?} is outside the workdir {}", self.workdir.display())),
            }
        }
        let full = self.workdir.join(p);
        if !full.exists() && !io::is_document_path(p) && !pixel_core::tilemap::is_tilemap_path(p) {
            for ext in [io::EXTENSION, ".aseprite", ".ase", pixel_core::tilemap::EXTENSION] {
                let with_ext = self.workdir.join(format!("{rel}{ext}"));
                if with_ext.exists() {
                    return Ok(with_ext);
                }
            }
        }
        Ok(full)
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.workdir).unwrap_or(p).display().to_string()
    }

    fn load(&self, args: &Value) -> Result<(PathBuf, Document, Vec<String>), String> {
        let rel = str_arg(args, "path")?;
        let path = self.resolve(rel)?;
        if !path.exists() {
            return Err(format!("{rel:?} does not exist in the workdir (list_documents shows what is there)"));
        }
        let (doc, warnings) = io::load_with_warnings(&path)?;
        Ok((path, doc, warnings))
    }

    /// Save `doc` and record it in the journal (with the state before it,
    /// if the journal does not know that one yet).
    fn commit(&self, path: &Path, before: &Document, doc: &Document, note: &str) -> Result<(), String> {
        self.commit_at(path, before, doc, note, None)
    }

    /// `commit` with the edit's location, so an open editor can follow it.
    fn commit_at(
        &self,
        path: &Path,
        before: &Document,
        doc: &Document,
        note: &str,
        focus: Option<Focus>,
    ) -> Result<(), String> {
        let journal = Journal::for_document(path);
        journal.ensure_current(before, "state before the next edit (first edit or changed outside the editor)")?;
        io::save(doc, path)?;
        journal.record_edit(doc, &self.author(), note, focus)?;
        Ok(())
    }

    /// Paced write: show the change being drawn stroke by stroke (row by
    /// row) in an open editor, `pace_ms` apart, before the final commit.
    #[allow(clippy::too_many_arguments)]
    fn draw_paced(
        &self,
        path: &Path,
        before: &Document,
        doc: &Document,
        scopes: &[CelScope],
        note: &str,
        focus: &Focus,
        pace_ms: u64,
    ) -> Result<usize, String> {
        // changed pixels, row by row
        let mut px: Vec<(usize, usize, i32, i32)> = Vec::new();
        for s in scopes {
            let d = diff::cel_diff(before, doc, s.layer, s.frame);
            if let Some(bb) = d.bbox {
                for y in bb.y..bb.y + bb.h as i32 {
                    for x in bb.x..bb.x + bb.w as i32 {
                        if before.get(s.layer, s.frame, x, y) != doc.get(s.layer, s.frame, x, y) {
                            px.push((s.layer, s.frame, x, y));
                        }
                    }
                }
            }
        }
        let mut rows: Vec<Vec<(usize, usize, i32, i32)>> = Vec::new();
        for p in px {
            match rows.last_mut() {
                Some(r) if r[0].3 == p.3 && r[0].1 == p.1 => r.push(p),
                _ => rows.push(vec![p]),
            }
        }
        // at most 48 strokes and 30 s in total
        let strokes = rows.len().clamp(1, 48);
        let per = rows.len().div_ceil(strokes);
        let groups: Vec<Vec<(usize, usize, i32, i32)>> = rows.chunks(per.max(1)).map(|c| c.concat()).collect();
        let pace = pace_ms.min(30_000 / groups.len().max(1) as u64);
        let journal = Journal::for_document(path);
        journal.ensure_current(before, "state before the next edit (first edit or changed outside the editor)")?;
        let mut cur = before.clone();
        let n = groups.len();
        for (i, g) in groups.iter().enumerate().take(n.saturating_sub(1)) {
            for &(l, f, x, y) in g {
                let k = doc.get(l, f, x, y);
                let w = cur.width;
                cur.cel_mut(l, f)[(y as u32 * w + x as u32) as usize] = k;
            }
            io::save(&cur, path)?;
            let (x0, x1) = (g.iter().map(|p| p.2).min().unwrap(), g.iter().map(|p| p.2).max().unwrap());
            let (y0, y1) = (g.iter().map(|p| p.3).min().unwrap(), g.iter().map(|p| p.3).max().unwrap());
            let stroke = Focus {
                frame: g[0].1,
                layer: g[0].0,
                bbox: Some([x0, y0, x1 - x0 + 1, y1 - y0 + 1]),
                region: focus.region.clone(),
            };
            journal.record_progress(&cur, &self.author(), &format!("{note} ({}/{n})", i + 1), Some(stroke))?;
            std::thread::sleep(std::time::Duration::from_millis(pace));
        }
        Ok(n)
    }

    // ---- scoped edits -----------------------------------------------------

    /// Resolve the explicit target of a pixel write: layer, frame and the
    /// optional `region` or `clip`, and check locks and linked cels.
    fn target(&self, d: &Document, a: &Value, layer_arg: &str, frame_arg: &str) -> Result<CelScope, String> {
        let layer = d.find_layer(&index_or_name(a.get(layer_arg)).ok_or(format!("missing {layer_arg:?}"))?)?;
        let frame = d.find_frame(&index_or_name(a.get(frame_arg)).ok_or(format!("missing {frame_arg:?}"))?)?;
        // "rect" is accepted as another name for "clip"
        let clip_arg = match (a.get("clip"), a.get("rect")) {
            (Some(_), Some(_)) => return Err("give either clip or rect (same meaning), not both".into()),
            (c, r) => c.or(r),
        };
        let clip =
            match (a.get("region").and_then(Value::as_str), clip_arg) {
                (Some(_), Some(_)) => return Err("give either region or clip/rect, not both".into()),
                (Some(name), None) => {
                    let r = &d.regions[d.find_region(name)?];
                    if !r.applies_to_layer(d, layer) {
                        return Err(format!(
                            "region {name:?} belongs to layer {:?}, not {:?}",
                            r.layer.as_deref().unwrap_or(""),
                            d.layers[layer].name
                        ));
                    }
                    Some(r.rect_at(frame).ok_or_else(|| {
                        format!("region {name:?} does not exist on frame {}", d.frame_label_long(frame))
                    })?)
                }
                (None, Some(c)) => Some(rect_arg(c).map_err(|e| format!("clip/rect: {e}"))?),
                (None, None) => None,
            };
        self.check_cel(d, a, layer, frame)?;
        Ok(CelScope { layer, frame, clip })
    }

    fn check_cel(&self, d: &Document, a: &Value, layer: usize, frame: usize) -> Result<(), String> {
        if let Some(why) = d.edit_blocked(layer, frame) {
            return Err(format!("{why}; nothing was written (unlock it first if this edit is intended)"));
        }
        let linked = d.layers[layer].linked_frames(frame);
        if linked.len() > 1 && !bool_arg(a, "edit_linked", false) {
            let labels: Vec<String> = linked.iter().map(|f| d.frame_label(*f)).collect();
            return Err(format!(
                "the cel of layer {:?} on frame {} is linked with frames {linked:?} ({}); writing it would change \
                 all of them. Pass edit_linked: true to edit the shared cel, or unlink it first (link_cel unlink: true)",
                d.layers[layer].name,
                d.frame_label(frame),
                labels.join(", ")
            ));
        }
        Ok(())
    }

    /// Finish a pixel write: revert changes outside the scopes, refuse
    /// changes in locked regions, verify nothing else changed, then save
    /// (unless dry_run) and describe the diff.
    #[allow(clippy::too_many_arguments)]
    fn finish_write(
        &self,
        a: &Value,
        path: &Path,
        before: &Document,
        mut doc: Document,
        scopes: &[CelScope],
        tool: &str,
        detail: String,
        regions_may_change: bool,
    ) -> Result<Out, String> {
        // 1. nothing outside the target
        let mut outside = 0;
        for s in scopes {
            let clip = s.clip;
            outside += doc.revert_outside(before, s.layer, s.frame, |x, y| clip.is_none_or(|c| c.contains(x, y)));
        }
        // 2. locked regions
        for s in scopes {
            let d = diff::cel_diff(before, &doc, s.layer, s.frame);
            if let Some(bb) = d.bbox {
                for y in bb.y..bb.y + bb.h as i32 {
                    for x in bb.x..bb.x + bb.w as i32 {
                        if doc.get(s.layer, s.frame, x, y) != before.get(s.layer, s.frame, x, y) {
                            if let Some(r) = doc.locked_region_at(s.layer, s.frame, x, y) {
                                return Err(format!(
                                    "this edit would change pixel ({x}, {y}) inside locked region {:?}; nothing was \
                                     written (use clip/region to stay outside it, or unlock it with set_region locked: false)",
                                    r.name
                                ));
                            }
                        }
                    }
                }
            }
        }
        // 3. guard: only the target cels changed
        let allowed = |l: usize, f: usize| {
            scopes.iter().any(|s| s.layer == l && before.layers[l].linked_frames(s.frame).contains(&f))
        };
        for c in diff::changed_cels(before, &doc) {
            if !allowed(c.layer, c.frame) {
                return Err(format!(
                    "internal check failed: the edit touched layer {} frame {} outside its target; nothing was written",
                    c.layer, c.frame
                ));
            }
        }
        if before.palette != doc.palette
            || before.ramps != doc.ramps
            || before.frames != doc.frames
            || before.tags != doc.tags
            || (before.regions != doc.regions && !regions_may_change)
            || before.layers.len() != doc.layers.len()
        {
            return Err("internal check failed: a pixel tool changed document structure; nothing was written".into());
        }
        doc.validate()?;
        // 4. diff
        let mut text = detail;
        if outside > 0 {
            text += &format!(
                "\n{outside} pixels outside the target ({}) were left unchanged",
                scopes
                    .iter()
                    .map(|s| s.clip.map_or("whole cel".into(), |c| c.describe()))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        let mut total = 0;
        let mut size = String::new();
        for s in scopes {
            let d = diff::cel_diff(before, &doc, s.layer, s.frame);
            total += d.count;
            if let Some(bb) = d.bbox {
                size = format!(" ({}x{})", bb.w, bb.h);
            }
            text += &format!("\n{}", diff::describe_cel_diff(before, &doc, &d, DIFF_MAX_ROWS));
            let linked = doc.layers[s.layer].linked_frames(s.frame);
            if linked.len() > 1 && d.count > 0 {
                text += &format!("(linked cel: frames {linked:?} show the same change)\n");
            }
        }
        let target = scopes
            .iter()
            .map(|s| format!("{} {}", doc.frame_label(s.frame), doc.layers[s.layer].name))
            .collect::<Vec<_>>()
            .join(" -> ");
        let region = a.get("region").and_then(Value::as_str).map(|r| format!(" [{r}]")).unwrap_or_default();
        let note = note_arg(a, || format!("{}: {tool} {target}{region}{size}", self.author()));
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written. Would record: {note:?}\n{text}")));
        }
        if total == 0 {
            return Ok(Out::Text(format!("nothing changed, nothing written\n{text}")));
        }
        // where the edit happened, for an open editor to follow
        let changed: Vec<(CelScope, Rect)> = scopes
            .iter()
            .filter_map(|s| diff::cel_diff(before, &doc, s.layer, s.frame).bbox.map(|b| (*s, b)))
            .collect();
        let focus = changed.first().map(|(s, b)| Focus {
            frame: s.frame,
            layer: s.layer,
            bbox: Some([b.x, b.y, b.w as i32, b.h as i32]),
            region: a.get("region").and_then(Value::as_str).map(str::to_string),
        });
        let mut paced = String::new();
        if let (Some(ms), Some(f)) = (opt_int(a, "pace_ms")?.filter(|ms| *ms > 0), &focus) {
            let n = self.draw_paced(path, before, &doc, scopes, &note, f, ms as u64)?;
            paced = format!(" (drawn in {n} strokes, {ms} ms apart)");
        }
        self.commit_at(path, before, &doc, &note, focus)?;
        Ok(Out::Text(format!("{text}\nrecorded as {note:?}{paced} [{}]", self.rel(path))))
    }

    /// Pixel write on one cel: resolve the target, run `op`, finish.
    fn pixel_write(
        &self,
        a: &Value,
        tool: &str,
        op: impl FnOnce(&mut Document, CelScope) -> Result<String, String>,
    ) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let scope = self.target(&doc, a, "layer", "frame")?;
        let before = doc.clone();
        let mut doc = doc;
        let detail = op(&mut doc, scope)?;
        self.finish_write(a, &path, &before, doc, &[scope], tool, detail, false)
    }

    /// Structural edit (layers, frames, tags, palette, regions): run `op`,
    /// refuse pixel changes on locked targets, save and journal.
    fn meta_edit(
        &self,
        a: &Value,
        tool: &str,
        op: impl FnOnce(&mut Document) -> Result<String, String>,
    ) -> Result<Out, String> {
        let (path, before, _) = self.load(a)?;
        let mut doc = before.clone();
        let msg = op(&mut doc)?;
        doc.validate()?;
        if before.layers.len() == doc.layers.len() && before.frames.len() == doc.frames.len() {
            for c in diff::changed_cels(&before, &doc) {
                if let Some(why) = before.edit_blocked(c.layer, c.frame) {
                    return Err(format!("{why}: this edit would change its pixels; nothing was written"));
                }
            }
        }
        let changes = diff::summary(&before, &doc);
        let note = note_arg(a, || format!("{}: {tool}: {}", self.author(), first_line(&msg)));
        let report = if changes.is_empty() { "no changes".to_string() } else { changes.join("\n") };
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written.\n{msg}\n{report}")));
        }
        if changes.is_empty() && before == doc {
            return Ok(Out::Text(format!("{msg}\nnothing changed, nothing written")));
        }
        self.commit(&path, &before, &doc, &note)?;
        Ok(Out::Text(format!("{msg}\nchanges:\n{report}\nrecorded as {note:?} [{}]", self.rel(&path))))
    }

    // ---- tools --------------------------------------------------------------

    fn call(&self, name: &str, a: &Value) -> Result<Out, String> {
        if let Some(r) = self.tilemap_call(name, a) {
            return r;
        }
        match name {
            "list_documents" => self.list_documents(),
            "get_user_view" => self.get_user_view(),
            "create_document" => self.create_document(a),
            "get_document" => {
                let (path, doc, warnings) = self.load(a)?;
                let mut s = describe(&doc, &self.rel(&path), a)?;
                if !warnings.is_empty() {
                    s = format!("warnings while reading:\n  {}\n{s}", warnings.join("\n  "));
                }
                Ok(Out::Text(s))
            }
            "get_rows" => self.get_rows(a),
            "set_pixels" => self.pixel_write(a, "set_pixels", |d, s| {
                let px = a.get("pixels").and_then(Value::as_array).ok_or("missing array \"pixels\"")?;
                let mut parsed = Vec::with_capacity(px.len());
                for (i, p) in px.iter().enumerate() {
                    let at = |e: String| format!("pixels[{i}]: {e}");
                    let x = int_of(p, "x").map_err(at)?;
                    let y = int_of(p, "y").map_err(at)?;
                    let k = key_of(d, p.get("key")).map_err(at)?;
                    parsed.push((x, y, k));
                }
                let off = parsed.iter().filter(|(x, y, _)| !d.in_bounds(*x, *y)).count();
                for (x, y, k) in parsed.iter().copied() {
                    d.set(s.layer, s.frame, x, y, k);
                }
                Ok(format!("set_pixels: {} pixels given{}", parsed.len(), off_canvas(off)))
            }),
            "set_rows" => self.pixel_write(a, "set_rows", |d, s| {
                let rows: Vec<String> = a
                    .get("rows")
                    .and_then(Value::as_array)
                    .ok_or("missing array \"rows\"")?
                    .iter()
                    .enumerate()
                    .map(|(i, r)| r.as_str().map(str::to_string).ok_or(format!("rows[{i}] must be a string")))
                    .collect::<Result<_, _>>()?;
                let (x, y) = match (opt_int(a, "x")?, opt_int(a, "y")?, s.clip) {
                    (Some(x), Some(y), _) => (x, y),
                    (None, None, Some(c)) => (c.x, c.y), // rows start at the region's corner
                    (None, None, None) => (0, 0),
                    _ => return Err("give both x and y (or neither)".into()),
                };
                let rows = expand_names(d, &rows, a)?;
                let mut off = 0;
                for (dy, row) in rows.iter().enumerate() {
                    for (dx, k) in row.bytes().enumerate() {
                        if k != pixel_core::SKIP && !d.in_bounds(x + dx as i32, y + dy as i32) {
                            off += 1;
                        }
                    }
                }
                d.set_rows(s.layer, s.frame, x, y, &rows)?;
                Ok(format!("set_rows at ({x}, {y}), {} rows{}", rows.len(), off_canvas(off)))
            }),
            "draw_line" => self.pixel_write(a, "draw_line", |d, s| {
                let k = key_of(d, a.get("key"))?;
                let pts = draw::line(int_of(a, "x0")?, int_of(a, "y0")?, int_of(a, "x1")?, int_of(a, "y1")?);
                Ok(format!("line: {}", paint(d, s, &pts, k)))
            }),
            "draw_rect" => self.pixel_write(a, "draw_rect", |d, s| {
                let dither = dither_arg(d, a)?;
                let filled = bool_arg(a, "filled", false);
                if dither.is_some() && !filled {
                    return Err("dither needs filled: true".into());
                }
                let k = match (&dither, a.get("key")) {
                    (Some((_, k, _)), _) => *k,
                    (None, key) => key_of(d, key)?,
                };
                let (x, y, w, h) = (int_of(a, "x")?, int_of(a, "y")?, int_of(a, "w")?, int_of(a, "h")?);
                if w < 1 || h < 1 {
                    return Err(format!("w and h must be >= 1 (got w={w}, h={h})"));
                }
                let pts = draw::rect(x, y, x + w - 1, y + h - 1, filled);
                if let Some((p, a, b)) = dither {
                    let off = pts.iter().filter(|(x, y)| !d.in_bounds(*x, *y)).count();
                    recolor::dither_points(d, s.layer, s.frame, &pts, p, a, b).apply(d, s.layer, s.frame);
                    return Ok(format!(
                        "rect: {} pixels, dither {} of {} and {}{}",
                        pts.len(),
                        p.id(),
                        d.key_label(a),
                        d.key_label(b),
                        off_canvas(off)
                    ));
                }
                Ok(format!("rect: {}", paint(d, s, &pts, k)))
            }),
            "draw_ellipse" => self.pixel_write(a, "draw_ellipse", |d, s| {
                let k = key_of(d, a.get("key"))?;
                let (cx, cy, rx, ry) = (int_of(a, "cx")?, int_of(a, "cy")?, int_of(a, "rx")?, int_of(a, "ry")?);
                if rx < 0 || ry < 0 {
                    return Err("rx and ry must be >= 0".into());
                }
                let pts = draw::ellipse(cx, cy, rx, ry, bool_arg(a, "filled", false));
                Ok(format!("ellipse: {}", paint(d, s, &pts, k)))
            }),
            "fill" => self.pixel_write(a, "fill", |d, s| {
                let dither = dither_arg(d, a)?;
                let k = match (&dither, a.get("key")) {
                    (Some((_, k, _)), _) => *k,
                    (None, key) => key_of(d, key)?,
                };
                let (x, y) = (int_of(a, "x")?, int_of(a, "y")?);
                if !d.in_bounds(x, y) {
                    return Err(format!("({x}, {y}) is outside the {}x{} canvas", d.width, d.height));
                }
                if s.clip.is_some_and(|c| !c.contains(x, y)) {
                    return Err(format!("({x}, {y}) is outside the target {}", s.clip.unwrap().describe()));
                }
                // flood only within the target, never leak out of it
                let old = d.get(s.layer, s.frame, x, y);
                let region: Vec<(i32, i32)> = draw::flood_region(d, s.layer, s.frame, x, y)
                    .into_iter()
                    .filter(|(px, py)| s.clip.is_none_or(|c| c.contains(*px, *py)))
                    .collect();
                let (pattern, a_key, b_key) = dither.unwrap_or((Pattern::Checker, k, k));
                recolor::dither_points(d, s.layer, s.frame, &region, pattern, a_key, b_key).apply(d, s.layer, s.frame);
                let with = match dither {
                    Some((p, a, b)) => format!("dither {} of {} and {}", p.id(), d.key_label(a), d.key_label(b)),
                    None => d.key_label(k),
                };
                Ok(format!("fill from ({x}, {y}): {} -> {with}", d.key_label(old)))
            }),
            "clear" => self.pixel_write(a, "clear", |d, s| {
                let r = s.clip.unwrap_or(Rect::new(0, 0, d.width, d.height));
                let pts: Vec<(i32, i32)> =
                    (r.y..r.y + r.h as i32).flat_map(|y| (r.x..r.x + r.w as i32).map(move |x| (x, y))).collect();
                d.set_points(s.layer, s.frame, &pts, TRANSPARENT);
                Ok(format!("clear {}", r.describe()))
            }),
            "copy_region" | "move_region" => self.copy_or_move(a, name == "move_region"),
            "transform" => self.transform_tool(a),
            "replace_color" | "outline" => self.recolor_tool(a, name),
            "mirror_document" => self.mirror_document(a),
            "list_regions" => {
                let (_, doc, _) = self.load(a)?;
                Ok(Out::Text(list_regions(&doc, a)?))
            }
            "set_region" => self.meta_edit(a, "set_region", |d| set_region(d, a)),
            "delete_region" => self.meta_edit(a, "delete_region", |d| {
                let name = str_arg(a, "name")?;
                let i = d.find_region(name)?;
                if d.regions[i].locked {
                    return Err(format!("region {name:?} is locked; unlock it first (set_region locked: false)"));
                }
                d.regions.remove(i);
                Ok(format!("deleted region {name:?} (pixels are unchanged)"))
            }),
            "add_layer" => self.meta_edit(a, "add_layer", |d| {
                let name = a.get("name").and_then(Value::as_str).unwrap_or("Ebene");
                let index = opt_int(a, "index")?.map(|i| i.max(0) as usize).unwrap_or(d.layers.len());
                let i = d.add_layer(name, index);
                Ok(format!("added layer {i} {:?} (layer 0 is the bottom)", d.layers[i].name))
            }),
            "set_layer" => self.meta_edit(a, "set_layer", |d| {
                let l = d.find_layer(&index_or_name(a.get("layer")).ok_or("missing \"layer\"")?)?;
                let only_lock = a
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|k| matches!(k.as_str(), "path" | "layer" | "locked" | "note" | "dry_run"));
                if d.layers[l].locked && !only_lock && a.get("locked") != Some(&json!(false)) {
                    return Err(format!("layer {:?} is locked; unlock it first (locked: false)", d.layers[l].name));
                }
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    d.rename_layer(l, n);
                }
                if let Some(v) = a.get("visible").and_then(Value::as_bool) {
                    d.layers[l].visible = v;
                }
                if let Some(o) = a.get("opacity").and_then(Value::as_f64) {
                    d.layers[l].opacity = o.clamp(0.0, 1.0) as f32;
                }
                if let Some(v) = a.get("locked").and_then(Value::as_bool) {
                    d.layers[l].locked = v;
                }
                let mut at = l;
                if let Some(to) = opt_int(a, "move_to")? {
                    at = (to.max(0) as usize).min(d.layers.len() - 1);
                    d.move_layer(l, at)?;
                }
                let ly = &d.layers[at];
                Ok(format!(
                    "layer {at} {:?}: visible {}, opacity {:.2}, locked {}",
                    ly.name, ly.visible, ly.opacity, ly.locked
                ))
            }),
            "delete_layer" => self.meta_edit(a, "delete_layer", |d| {
                let l = d.find_layer(&index_or_name(a.get("layer")).ok_or("missing \"layer\"")?)?;
                let name = d.layers[l].name.clone();
                d.delete_layer(l)?;
                Ok(format!("deleted layer {name:?}; layers above moved down by one"))
            }),
            "add_frame" => self.meta_edit(a, "add_frame", |d| {
                let copy = match index_or_name(a.get("copy_from")) {
                    Some(s) => Some(d.find_frame(&s)?),
                    None => None,
                };
                let linked = bool_arg(a, "linked", false);
                if linked && copy.is_none() {
                    return Err("linked: true needs copy_from".into());
                }
                let index = opt_int(a, "index")?.map(|i| i.max(0) as usize).unwrap_or(d.frames.len());
                let i = d.add_frame(index, copy, linked)?;
                if let Some(ms) = opt_int(a, "duration_ms")? {
                    d.frames[i].duration_ms = ms.max(1) as u32;
                }
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    d.frames[i].name = d.unique_frame_name(n);
                }
                let how = match copy {
                    Some(c) if linked => format!(", linked to frame {c} (edits affect both)"),
                    Some(c) => format!(", copy of frame {c}"),
                    None => ", empty".into(),
                };
                let shifted = if i + 1 < d.frames.len() { "; later frames shifted by one" } else { "" };
                Ok(format!("added frame {i} {:?}{how}{shifted}", d.frames[i].name))
            }),
            "delete_frame" => self.meta_edit(a, "delete_frame", |d| {
                let f = d.find_frame(&index_or_name(a.get("frame")).ok_or("missing \"frame\"")?)?;
                let label = d.frame_label_long(f);
                d.delete_frame(f)?;
                Ok(format!("deleted frame {label}; later frames moved down by one"))
            }),
            "set_frame_duration" => self.meta_edit(a, "set_frame_duration", |d| {
                let ms = int_of(a, "duration_ms")?;
                if ms < 1 {
                    return Err("duration_ms must be >= 1".into());
                }
                let which = index_or_name(a.get("frame")).ok_or("missing \"frame\"")?;
                if which == "all" {
                    d.frames.iter_mut().for_each(|f| f.duration_ms = ms as u32);
                    return Ok(format!("all {} frames: {ms} ms", d.frames.len()));
                }
                let f = d.find_frame(&which)?;
                d.frames[f].duration_ms = ms as u32;
                Ok(format!("frame {}: {ms} ms", d.frame_label_long(f)))
            }),
            "set_frame" => self.meta_edit(a, "set_frame", |d| {
                let f = d.find_frame(&index_or_name(a.get("frame")).ok_or("missing \"frame\"")?)?;
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    if d.frames.iter().enumerate().any(|(i, fr)| i != f && fr.name == n) {
                        return Err(format!("another frame is already called {n:?}"));
                    }
                    if n.contains('/') {
                        return Err("frame names must not contain '/' (reserved for tag/index addresses)".into());
                    }
                    d.frames[f].name = n.to_string();
                }
                if let Some(ms) = opt_int(a, "duration_ms")? {
                    d.frames[f].duration_ms = ms.max(1) as u32;
                }
                if let Some(v) = a.get("locked").and_then(Value::as_bool) {
                    d.frames[f].locked = v;
                }
                let mut at = f;
                if let Some(to) = opt_int(a, "move_to")? {
                    at = (to.max(0) as usize).min(d.frames.len() - 1);
                    d.move_frame(f, at)?;
                }
                let fr = &d.frames[at];
                Ok(format!("frame {}: {} ms, locked {}", d.frame_label_long(at), fr.duration_ms, fr.locked))
            }),
            "link_cel" => self.meta_edit(a, "link_cel", |d| {
                let f = d.find_frame(&index_or_name(a.get("frame")).ok_or("missing \"frame\"")?)?;
                let layers: Vec<usize> = match index_or_name(a.get("layer")) {
                    Some(l) => vec![d.find_layer(&l)?],
                    None => (0..d.layers.len()).collect(),
                };
                if bool_arg(a, "unlink", false) {
                    for &l in &layers {
                        d.layers[l].unlink(f);
                    }
                    return Ok(format!("frame {f} now has its own cel copy on layers {layers:?}"));
                }
                let to = d.find_frame(
                    &index_or_name(a.get("to")).ok_or("give \"to\" (frame to link with) or unlink: true")?,
                )?;
                for &l in &layers {
                    if let Some(why) = d.edit_blocked(l, f) {
                        return Err(format!("{why}; nothing was written"));
                    }
                    d.layers[l].link(f, to);
                }
                Ok(format!(
                    "frame {f} now shares the cels of frame {to} on layers {layers:?} (its own pixels were replaced)"
                ))
            }),
            "set_tag" => self.meta_edit(a, "set_tag", |d| set_tag(d, a)),
            "set_palette_color" => self.meta_edit(a, "set_palette_color", |d| {
                let key = single_key(str_arg(a, "key")?)?;
                if bool_arg(a, "remove", false) {
                    if !d.palette.iter().any(|p| p.key == key) {
                        return Err(format!("key {:?} is not in the palette", key as char));
                    }
                    let label = d.key_label(key);
                    d.remove_palette_color(key);
                    return Ok(format!("removed {label}; its pixels are now transparent"));
                }
                let mut added = false;
                if let Some(c) = a.get("color").and_then(Value::as_str) {
                    added = d.set_palette_color(key, Rgba::parse(c)?)?;
                } else if !d.palette.iter().any(|p| p.key == key) {
                    return Err(format!("new key {:?} needs a color", key as char));
                }
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    if n.len() == 1 {
                        return Err("color names need at least 2 characters (1 character would look like a key)".into());
                    }
                    if d.palette.iter().any(|p| p.key != key && p.name == n && !n.is_empty()) {
                        return Err(format!("another color is already called {n:?}"));
                    }
                    d.palette.iter_mut().find(|p| p.key == key).unwrap().name = n.to_string();
                }
                let p = d.palette_entry(key).unwrap();
                Ok(format!("{} {} = {}", if added { "added" } else { "changed" }, p.label(), p.color))
            }),
            "set_ramps" => {
                if pixel_core::aseprite::is_aseprite_path(Path::new(str_arg(a, "path")?)) {
                    return Err("color ramps are stored in .pxe.json only; .aseprite has no place for them \
                                (save_as a .pxe.json copy first)"
                        .into());
                }
                self.meta_edit(a, "set_ramps", |d| {
                    let list = a.get("ramps").and_then(Value::as_array).ok_or("missing array \"ramps\"")?;
                    let mut ramps = Vec::with_capacity(list.len());
                    for (i, r) in list.iter().enumerate() {
                        let at = |e: String| format!("ramps[{i}]: {e}");
                        let name =
                            r.get("name").and_then(Value::as_str).ok_or_else(|| at("missing \"name\"".into()))?;
                        let keys = ramp_keys(d, r.get("keys")).map_err(at)?;
                        ramps.push(Ramp { name: name.to_string(), keys });
                    }
                    d.set_ramps(ramps)?;
                    if d.ramps.is_empty() {
                        return Ok("no ramps".into());
                    }
                    let lines: Vec<String> = d.ramps.iter().map(|r| d.ramp_label(r)).collect();
                    Ok(format!("ramps (dark > light):\n  {}", lines.join("\n  ")))
                })
            }
            "shade" => self.pixel_write(a, "shade", |d, s| {
                let steps = int_of(a, "steps")?;
                if steps == 0 {
                    return Err("steps must not be 0 (+n = n colors lighter, -n = darker)".into());
                }
                if d.ramps.is_empty() {
                    return Err("the document has no color ramps; define them with set_ramps first".into());
                }
                let pts: Vec<(i32, i32)> = match (a.get("pixels").and_then(Value::as_array), s.clip) {
                    (Some(px), _) => px
                        .iter()
                        .enumerate()
                        .map(|(i, p)| Ok((int_of(p, "x").map_err(|e| format!("pixels[{i}]: {e}"))?, int_of(p, "y")?)))
                        .collect::<Result<_, String>>()?,
                    (None, Some(c)) => {
                        (c.y..c.y + c.h as i32).flat_map(|y| (c.x..c.x + c.w as i32).map(move |x| (x, y))).collect()
                    }
                    (None, None) => return Err("give pixels, rect or region: what to shade".into()),
                };
                let skipped = pts
                    .iter()
                    .filter(|&&(x, y)| d.in_bounds(x, y) && d.ramp_of(d.get(s.layer, s.frame, x, y)).is_none())
                    .count();
                let n = draw::shade(d, s.layer, s.frame, &pts, steps);
                let dir = if steps > 0 { "lighter" } else { "darker" };
                Ok(format!(
                    "shade {} step(s) {dir}: {n} pixels changed, {skipped} not in any ramp (unchanged), the rest \
                     already at the end of their ramp",
                    steps.abs()
                ))
            }),
            "render_preview" => self.render_preview(a),
            "check_seams" => self.check_seams(a),
            "focus" => {
                let (path, doc, _) = self.load(a)?;
                let frame = doc.find_frame(&index_or_name(a.get("frame")).ok_or("missing \"frame\"")?)?;
                let layer = match index_or_name(a.get("layer")) {
                    Some(l) => doc.find_layer(&l)?,
                    None => 0,
                };
                let region = a.get("region").and_then(Value::as_str);
                let rect = match (region, a.get("rect")) {
                    (Some(_), Some(_)) => return Err("give either region or rect".into()),
                    (Some(n), None) => Some(doc.regions[doc.find_region(n)?].rect_at(frame).ok_or_else(|| {
                        format!("region {n:?} does not exist on frame {}", doc.frame_label_long(frame))
                    })?),
                    (None, Some(r)) => Some(rect_arg(r)?),
                    (None, None) => None,
                };
                let what =
                    region.map(str::to_string).or(rect.map(|r| r.describe())).unwrap_or_else(|| "den Frame".into());
                let note = note_arg(a, || {
                    format!(
                        "{}: arbeitet an {what} ({} · {})",
                        self.author(),
                        doc.frame_label(frame),
                        doc.layers[layer].name
                    )
                });
                let focus = Focus {
                    frame,
                    layer,
                    bbox: rect.map(|r| [r.x, r.y, r.w as i32, r.h as i32]),
                    region: region.map(str::to_string),
                };
                Journal::for_document(&path).record_focus(&self.author(), &note, focus)?;
                Ok(Out::Text(format!(
                    "focus sent: {note:?} (an editor window in follow mode that has the file open switches to its tab and jumps there; nothing was changed)"
                )))
            }
            "clear_marks" => {
                let (path, doc, _) = self.load(a)?;
                let frame = match index_or_name(a.get("frame")) {
                    Some(f) => Some(doc.find_frame(&f)?),
                    None => None,
                };
                let numbers: Option<Vec<u32>> = match a.get("numbers") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(v)) => Some(
                        v.iter()
                            .map(|n| n.as_u64().map(|n| n as u32).ok_or("numbers must be integers"))
                            .collect::<Result<_, _>>()?,
                    ),
                    Some(_) => return Err("numbers must be an array of mark numbers".into()),
                };
                let mut marks = crate::view::read_marks(&path);
                if let Some(ns) = &numbers {
                    let missing: Vec<String> = ns
                        .iter()
                        .filter(|n| !marks.marks.iter().any(|m| m.n == **n))
                        .map(|n| format!("#{n}"))
                        .collect();
                    if !missing.is_empty() {
                        return Err(format!("no mark {} in {}", missing.join(", "), self.rel(&path)));
                    }
                }
                let gone = marks.remove_where(|m| {
                    frame.is_none_or(|f| m.frame.index == f) && numbers.as_ref().is_none_or(|ns| ns.contains(&m.n))
                });
                crate::view::write_marks(&path, &marks)?;
                let list: Vec<String> = gone.iter().map(|m| format!("#{}", m.n)).collect();
                Ok(Out::Text(format!(
                    "removed {} mark(s) {}from {}; {} left",
                    gone.len(),
                    if list.is_empty() { String::new() } else { format!("({}) ", list.join(", ")) },
                    self.rel(&path),
                    marks.marks.len()
                )))
            }
            "open_editor" => {
                let rel = str_arg(a, "path")?;
                let path = self.resolve(rel)?;
                if !path.exists() {
                    return Err(format!("{rel:?} does not exist in the workdir"));
                }
                Ok(Out::Text(crate::launch::open_editor(&path, bool_arg(a, "dry_run", false), Some(&self.workdir))?))
            }
            "render_animation" => self.render_animation(a),
            "export" => {
                let (path, doc, _) = self.load(a)?;
                let target = Target::parse(str_arg(a, "target")?)?;
                let stem = io::stem(&path);
                let out_dir = match a.get("out_dir").and_then(Value::as_str) {
                    Some(d) => self.resolve(d)?,
                    None => self.workdir.join("export").join(&stem),
                };
                let opts = ExportOptions {
                    scale: opt_int(a, "scale")?.map(|s| s.max(1) as u32),
                    columns: opt_int(a, "columns")?.map(|s| s.max(1) as u32),
                    godot_res_dir: a.get("godot_res_dir").and_then(Value::as_str).map(str::to_string),
                    pixels_per_unit: opt_int(a, "pixels_per_unit")?.map(|s| s.max(1) as u32).unwrap_or(32),
                    color_mode: match a.get("color_mode").and_then(Value::as_str) {
                        Some(m) => ColorMode::parse(m)?,
                        None => ColorMode::Indexed,
                    },
                };
                let files = export::export(&doc, target, &out_dir, &stem, &opts)?;
                Ok(Out::Text(format!(
                    "exported {}:\n{}",
                    target.id(),
                    files.iter().map(|f| format!("  {}", self.rel(f))).collect::<Vec<_>>().join("\n")
                )))
            }
            "save_as" => {
                let (src, doc, warnings) = self.load(a)?;
                let dest = self.new_doc_path(str_arg(a, "to")?, bool_arg(a, "overwrite", false))?;
                io::save(&doc, &dest)?;
                let mut msg = format!("wrote {} as {}", self.rel(&src), self.rel(&dest));
                if !doc.ramps.is_empty() && pixel_core::aseprite::is_aseprite_path(&dest) {
                    msg += &format!("\nnote: the {} color ramp(s) are not stored in .aseprite", doc.ramps.len());
                }
                if !warnings.is_empty() {
                    msg += &format!("\nwarnings while reading:\n  {}", warnings.join("\n  "));
                }
                Ok(Out::Text(msg))
            }
            "import_png" => {
                let src = self.resolve(str_arg(a, "png_path")?)?;
                let tile = match (opt_int(a, "tile_w")?, opt_int(a, "tile_h")?) {
                    (Some(w), Some(h)) => Some((w.max(1) as u32, h.max(1) as u32)),
                    (Some(w), None) => Some((w.max(1) as u32, w.max(1) as u32)),
                    (None, Some(_)) => return Err("tile_h needs tile_w".into()),
                    _ => None,
                };
                let doc = import::import_png(&src, tile, opt_int(a, "duration_ms")?.unwrap_or(100).max(1) as u32)?;
                let dest = self.new_doc_path(str_arg(a, "name")?, bool_arg(a, "overwrite", false))?;
                io::save(&doc, &dest)?;
                Ok(Out::Text(format!(
                    "imported {} as {} ({}x{}, {} frames, {} colors)",
                    self.rel(&src),
                    self.rel(&dest),
                    doc.width,
                    doc.height,
                    doc.frames.len(),
                    doc.palette.len()
                )))
            }
            "history" => {
                let (path, doc, _) = self.load(a)?;
                let j = Journal::for_document(&path);
                let entries = j.entries();
                if entries.is_empty() {
                    return Ok(Out::Text("no history yet (it starts with the first MCP or GUI save)".into()));
                }
                let limit = opt_int(a, "limit")?.unwrap_or(20).max(1) as usize;
                let current = j.entry_for(&doc).map(|e| e.id);
                let lines: Vec<String> = entries
                    .iter()
                    .rev()
                    .take(limit)
                    .map(|e| {
                        // notes usually start with their author already
                        let note = if e.note.to_lowercase().starts_with(&format!("{}:", e.author.to_lowercase())) {
                            e.note.clone()
                        } else {
                            format!("{}: {}", e.author, e.note)
                        };
                        format!(
                            "#{} {} {note}{}",
                            e.id,
                            clock(e.time),
                            if Some(e.id) == current { "  <- current" } else { "" }
                        )
                    })
                    .collect();
                Ok(Out::Text(format!("newest first:\n{}", lines.join("\n"))))
            }
            "undo" => {
                let (path, doc, _) = self.load(a)?;
                let steps = opt_int(a, "steps")?.unwrap_or(1).max(1) as usize;
                let j = Journal::for_document(&path);
                if bool_arg(a, "dry_run", false) {
                    return Ok(Out::Text(format!("DRY RUN: would undo {steps} step(s); see history")));
                }
                let (restored, target) = j.undo(&doc, steps, &self.author())?;
                io::save(&restored, &path)?;
                let changes = diff::summary(&doc, &restored);
                Ok(Out::Text(format!(
                    "restored the state after #{} ({:?})\nchanges:\n{}",
                    target.id,
                    target.note,
                    if changes.is_empty() { "none".into() } else { changes.join("\n") }
                )))
            }
            "snapshot" => {
                let (path, doc, _) = self.load(a)?;
                let name = str_arg(a, "name")?;
                let j = Journal::for_document(&path);
                let s = j.save_snapshot(name, a.get("note").and_then(Value::as_str).unwrap_or(""), &doc)?;
                Ok(Out::Text(format!("snapshot {:?} saved ({})", s.name, clock(s.time))))
            }
            "list_snapshots" => {
                let (path, doc, _) = self.load(a)?;
                let j = Journal::for_document(&path);
                let hash = pixel_core::journal::doc_hash(&doc);
                let list = j.snapshots();
                if list.is_empty() {
                    return Ok(Out::Text("no snapshots".into()));
                }
                Ok(Out::Text(
                    list.iter()
                        .map(|s| {
                            format!(
                                "{:?} {}{}{}",
                                s.name,
                                clock(s.time),
                                if s.note.is_empty() { String::new() } else { format!(": {}", s.note) },
                                if s.hash == hash { "  (= current)" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            }
            "diff_snapshot" => {
                let (path, doc, _) = self.load(a)?;
                let snap = Journal::for_document(&path).load_snapshot(str_arg(a, "name")?)?;
                let changes = diff::summary(&snap, &doc);
                let mut s = if changes.is_empty() {
                    "current document is identical to the snapshot".to_string()
                } else {
                    format!("snapshot -> current:\n{}", changes.join("\n"))
                };
                if let (Some(l), Some(f)) = (index_or_name(a.get("layer")), index_or_name(a.get("frame"))) {
                    let (l, f) = (doc.find_layer(&l)?, doc.find_frame(&f)?);
                    let same_layout = snap.layers.len() == doc.layers.len() && snap.frames.len() == doc.frames.len();
                    if same_layout {
                        let d = diff::cel_diff(&snap, &doc, l, f);
                        s += &format!("\n{}", diff::describe_cel_diff(&snap, &doc, &d, 64));
                    } else {
                        s += "\n(layer/frame layout differs; no row diff)";
                    }
                }
                Ok(Out::Text(s))
            }
            "restore_snapshot" => {
                let (path, doc, _) = self.load(a)?;
                let name = str_arg(a, "name")?;
                let j = Journal::for_document(&path);
                let snap = j.load_snapshot(name)?;
                let changes = diff::summary(&doc, &snap);
                if bool_arg(a, "dry_run", false) {
                    return Ok(Out::Text(format!("DRY RUN: restoring {name:?} would change:\n{}", changes.join("\n"))));
                }
                let note = note_arg(a, || format!("{}: restore snapshot {name}", self.author()));
                self.commit(&path, &doc, &snap, &note)?;
                Ok(Out::Text(format!(
                    "restored snapshot {name:?} (undo reverts it)\nchanges:\n{}",
                    if changes.is_empty() { "none".into() } else { changes.join("\n") }
                )))
            }
            _ => Err(format!("unknown tool {name:?}")),
        }
    }

    /// What the user has open in the editor(s) and what they pointed at.
    fn get_user_view(&self) -> Result<Out, String> {
        use crate::view::{self, Area, FrameRef, LayerRef};
        let mut docs = Vec::new();
        collect_docs(&self.workdir, 0, &mut docs);
        let now = view::now_ms();
        let views = view::live_views(&docs, now);
        let indent = |t: &str| t.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n");
        // get_rows of an area as the tools see it (from the file)
        let rows = |doc: &Path, frame: &FrameRef, layer: &LayerRef, area: Area| -> String {
            let args = json!({"path": self.rel(doc), "frame": frame.index, "layer": layer.index,
                "rect": {"x": area.x, "y": area.y, "w": area.w, "h": area.h}});
            match self.get_rows(&args) {
                Ok(Out::Text(t)) => indent(&t),
                Ok(_) => String::new(),
                Err(e) => format!("    (could not read the pixels: {e})"),
            }
        };
        let place = |frame: &FrameRef, layer: &LayerRef| {
            let mut f = format!("frame {}", frame.address);
            if frame.address != frame.name {
                f += &format!(" (index {}, name {:?})", frame.index, frame.name);
            } else {
                f += &format!(" (index {})", frame.index);
            }
            format!("{f} · layer {} {:?}", layer.index, layer.name)
        };
        let unsaved = "  UNSAVED CHANGES in the editor: the file (what the tools read and write) differs from \
            what the user sees; ask them to save before editing";
        let mut out = String::new();
        if views.is_empty() {
            out += &format!(
                "No pixel-editor GUI is open on a document in this workdir ({}). The user may have closed it or \
                 opened an unsaved/other file; open_editor(path) starts one.\n",
                self.workdir.display()
            );
        } else {
            out += &format!("{} editor(s) open, newest first:\n", views.len());
        }
        for (doc, v) in &views {
            out += &format!(
                "\n- {} · editor pid {} · updated {}\n  {}\n",
                self.rel(doc),
                v.pid,
                view::ago(now, v.time_ms),
                place(&v.frame, &v.layer)
            );
            let follow = match (v.following, v.follow_paused) {
                (false, _) => "off",
                (true, true) => "on (paused by user input)",
                (true, false) => "on",
            };
            out += &format!("  zoom {}x · follow assistant edits {follow}", v.zoom);
            if !v.tool.is_empty() {
                out += &format!(" · tool {}", v.tool);
            }
            out += if v.playing { " · playing\n" } else { "\n" };
            if v.dirty {
                out += unsaved;
                out += "\n";
            }
            match &v.hover {
                Some(h) => {
                    out += &format!("  cursor: x {}, y {} · color {} on this layer", h.x, h.y, h.color);
                    if h.visible != h.color {
                        out += &format!(" · visible there: {}", h.visible);
                    }
                    if !h.regions.is_empty() {
                        out += &format!(" · region {}", h.regions.join(", "));
                    }
                    out += "\n";
                }
                None => out += "  cursor: not over the canvas\n",
            }
            match (v.selection, &v.selection_mask) {
                (Some(s), None) => {
                    out += &format!("  selection: {}\n  selected pixels:\n", Rect::from(s).describe());
                    out += &rows(doc, &v.frame, &v.layer, s);
                    out += "\n";
                }
                (Some(s), Some(mask)) => {
                    let n: usize = mask.iter().map(|r| r.bytes().filter(|b| *b == b'#').count()).sum();
                    out += &format!(
                        "  selection: {n} pixels in the box {} (a shape, not the whole box; '·' = not selected)\n  selected pixels:\n",
                        Rect::from(s).describe()
                    );
                    out += &indent(&masked_rows(doc, &v.frame, &v.layer, s, mask));
                    out += "\n";
                }
                (None, _) => out += "  selection: none\n",
            }
        }
        // "KI zeigen" marks of every document, grouped by frame
        let mut any = false;
        for doc in &docs {
            let marks = view::read_marks(doc);
            if marks.marks.is_empty() {
                continue;
            }
            any = true;
            let open: Vec<u32> = views.iter().filter(|(d, _)| d == doc).map(|(_, v)| v.pid).collect();
            out += &format!(
                "\n\"KI zeigen\" marks in {}: {}{}\n",
                self.rel(doc),
                marks.marks.len(),
                if open.is_empty() { " (document not open in an editor)".to_string() } else { String::new() }
            );
            let (w, h) = io::load(doc).map(|d| (d.width, d.height)).unwrap_or((0, 0));
            let mut frames: Vec<&FrameRef> = Vec::new();
            for m in &marks.marks {
                if !frames.iter().any(|f| f.index == m.frame.index) {
                    frames.push(&m.frame);
                }
            }
            frames.sort_by_key(|f| f.index);
            for f in frames {
                let mut head = format!("  frame {}", f.address);
                head += &if f.address != f.name {
                    format!(" (index {}, name {:?})", f.index, f.name)
                } else {
                    format!(" (index {})", f.index)
                };
                out += &head;
                out += ":\n";
                for m in marks.marks.iter().filter(|m| m.frame.index == f.index) {
                    let area = m.rect.unwrap_or(Area { x: 0, y: 0, w, h });
                    out += &format!(
                        "    #{} {} · layer {} {:?} · {}{}\n",
                        m.n,
                        if m.rect.is_some() { Rect::from(area).describe() } else { "the whole frame".into() },
                        m.layer.index,
                        m.layer.name,
                        view::ago(now, m.time_ms),
                        if m.note.is_empty() { String::new() } else { format!(" · note: {:?}", m.note) }
                    );
                    if m.dirty {
                        out +=
                            "      (the editor had unsaved changes when marking; the pixels below are from the file)\n";
                    }
                    for line in rows(doc, &m.frame, &m.layer, area).lines() {
                        out += &format!("  {line}\n");
                    }
                    out += "\n";
                }
            }
        }
        if any {
            out += "\nWhen you have handled marks, remove them with clear_marks(path, frame?, numbers?).\n";
        } else {
            out += "\nno \"KI zeigen\" marks (the user marks areas or pixels with the \"KI zeigen\" tool (K, menu Werkzeuge), any number on any frames, each with an optional note; Ctrl+Shift+K marks the selection)\n";
        }
        Ok(Out::Text(out.trim_end().to_string()))
    }

    fn get_rows(&self, a: &Value) -> Result<Out, String> {
        let (_, doc, _) = self.load(a)?;
        let f = match index_or_name(a.get("frame")) {
            Some(s) => doc.find_frame(&s)?,
            None => 0,
        };
        let composite = bool_arg(a, "composite", false);
        let l = match index_or_name(a.get("layer")) {
            Some(s) => doc.find_layer(&s)?,
            None => 0,
        };
        let rect = if let Some(r) = a.get("rect") {
            rect_arg(r).map_err(|e| format!("rect: {e}"))?
        } else if let Some(name) = a.get("region").and_then(Value::as_str) {
            doc.regions[doc.find_region(name)?]
                .rect_at(f)
                .ok_or_else(|| format!("region {name:?} does not exist on frame {}", doc.frame_label_long(f)))?
        } else {
            let x = opt_int(a, "x")?.unwrap_or(0).max(0);
            let y = opt_int(a, "y")?.unwrap_or(0).max(0);
            let w = opt_int(a, "w")?.unwrap_or(doc.width as i32 - x);
            let h = opt_int(a, "h")?.unwrap_or(doc.height as i32 - y);
            if w < 1 || h < 1 {
                return Err(format!("empty region w={w} h={h}"));
            }
            Rect::new(x, y, w as u32, h as u32)
        };
        // clip to the canvas
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w as i32).min(doc.width as i32);
        let y1 = (rect.y + rect.h as i32).min(doc.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return Err(format!("{} is outside the {}x{} canvas", rect.describe(), doc.width, doc.height));
        }
        let r = Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
        let head = if composite {
            format!(
                "composite of visible layers, frame {} (topmost non-transparent key per pixel)",
                doc.frame_label_long(f)
            )
        } else {
            format!("layer {l} {:?}, frame {}", doc.layers[l].name, doc.frame_label_long(f))
        };
        let key = |px: i32, py: i32| if composite { doc.top_key(f, px, py) } else { doc.get(l, f, px, py) };
        let used: Vec<u8> = {
            let mut k: Vec<u8> = (r.y..r.y + r.h as i32)
                .flat_map(|y| (r.x..r.x + r.w as i32).map(move |x| (x, y)))
                .map(|(x, y)| key(x, y))
                .filter(|k| *k != TRANSPARENT)
                .collect();
            k.sort();
            k.dedup();
            k
        };
        let legend: Vec<String> = used.iter().map(|k| doc.key_label(*k)).collect();
        Ok(Out::Text(format!(
            "{head}: {}\nkeys used: {}\n{}",
            r.describe(),
            if legend.is_empty() { "none".into() } else { legend.join(", ") },
            rows_block(key, r)
        )))
    }

    /// Copy or move the pixels of a rect (a region or `clip`) on one layer,
    /// within a frame or to another frame/layer, by an offset.
    fn copy_or_move(&self, a: &Value, moving: bool) -> Result<Out, String> {
        let tool = if moving { "move_region" } else { "copy_region" };
        let (path, doc, _) = self.load(a)?;
        if moving {
            if let Some(r) =
                a.get("region").and_then(Value::as_str).and_then(|n| doc.regions.iter().find(|r| r.name == n))
            {
                if r.is_marker() && r.locked {
                    return Err(format!(
                        "marker {:?} is locked: its position is protected, so move_region cannot move it; nothing was \
                         written (pixel writes at the marker work; unlock with set_region locked: false to move it)",
                        r.name
                    ));
                }
            }
        }
        let src = self.target(&doc, a, "layer", "frame")?;
        let rect = src.clip.ok_or("give the area to copy/move: region (name) or clip {x,y,w,h}")?;
        let (dx, dy) = (opt_int(a, "dx")?.unwrap_or(0), opt_int(a, "dy")?.unwrap_or(0));
        let to_frame = match index_or_name(a.get("to_frame")) {
            Some(f) => doc.find_frame(&f)?,
            None => src.frame,
        };
        let to_layer = match index_or_name(a.get("to_layer")) {
            Some(l) => doc.find_layer(&l)?,
            None => src.layer,
        };
        if (to_frame, to_layer, dx, dy) == (src.frame, src.layer, 0, 0) {
            return Err("source and destination are the same; give dx/dy, to_frame or to_layer".into());
        }
        let dest_rect = rect.translate(dx, dy);
        let same_cel = doc.layers[src.layer].link_source(src.frame) == doc.layers[to_layer].link_source(to_frame)
            && src.layer == to_layer;
        if !same_cel {
            self.check_cel(&doc, a, to_layer, to_frame)?;
        }
        let replace = match a.get("mode").and_then(Value::as_str).unwrap_or("over") {
            "over" => false,
            "replace" => true,
            m => return Err(format!("unknown mode {m:?} (over, replace)")),
        };
        let before = doc.clone();
        let mut doc = doc;
        // read the block first, then write
        let block: Vec<(i32, i32, u8)> = (rect.y..rect.y + rect.h as i32)
            .flat_map(|y| (rect.x..rect.x + rect.w as i32).map(move |x| (x, y)))
            .map(|(x, y)| (x, y, before.get(src.layer, src.frame, x, y)))
            .collect();
        if moving {
            for &(x, y, _) in &block {
                doc.set(src.layer, src.frame, x, y, TRANSPARENT);
            }
        }
        for &(x, y, k) in &block {
            if replace || k != TRANSPARENT {
                doc.set(to_layer, to_frame, x + dx, y + dy, k);
            }
        }
        // move the region definition along (for this frame only)
        let mut region_note = String::new();
        if moving && (dx, dy) != (0, 0) && to_frame == src.frame && bool_arg(a, "update_region", true) {
            if let Some(name) = a.get("region").and_then(Value::as_str) {
                let i = doc.find_region(name)?;
                let n = doc.frames.len();
                doc.regions[i].set_rect_for(src.frame, src.frame, Some(dest_rect), n);
                region_note = format!("; region {name:?} now at {} on this frame", dest_rect.describe());
            }
        }
        let mut scopes = vec![CelScope {
            layer: src.layer,
            frame: src.frame,
            clip: Some(if same_cel { rect.union(&dest_rect) } else { rect }),
        }];
        if !same_cel {
            scopes.push(CelScope { layer: to_layer, frame: to_frame, clip: Some(dest_rect) });
        }
        let detail = format!(
            "{tool}: {} of layer {:?} frame {} -> offset ({dx}, {dy}), layer {:?}, frame {} (mode {}){region_note}",
            rect.describe(),
            doc.layers[src.layer].name,
            doc.frame_label(src.frame),
            doc.layers[to_layer].name,
            doc.frame_label(to_frame),
            if replace { "replace" } else { "over" }
        );
        self.finish_write(a, &path, &before, doc, &scopes, tool, detail, true)
    }

    /// Mirror or rotate pixels: a rect/region or a whole cel (one layer and
    /// frame), or whole frames (all layers or one) around the canvas centre.
    fn transform_tool(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let op = Op::parse(str_arg(a, "op")?)?;
        let scope = str_arg(a, "scope")?;
        let before = doc.clone();
        let mut doc = doc;
        let canvas = Rect::new(0, 0, doc.width, doc.height);
        let cut = |n: usize| if n > 0 { format!("; {n} pixels cut off at the canvas edge") } else { String::new() };
        let (scopes, detail, what) = match scope {
            "rect" | "region" | "cel" => {
                let s = self.target(&doc, a, "layer", "frame")?;
                let area = match (scope, s.clip) {
                    ("cel", None) => canvas,
                    ("cel", Some(_)) => return Err("scope cel transforms the whole cel; drop region/clip/rect".into()),
                    ("region", _) if a.get("region").is_none() => return Err("scope region needs region (name)".into()),
                    ("rect", _) if a.get("region").is_some() => {
                        return Err("scope rect needs rect/clip, not region".into())
                    }
                    (_, Some(c)) => c,
                    (_, None) => {
                        return Err(format!(
                            "scope {scope} needs {}",
                            if scope == "rect" { "rect/clip {x,y,w,h}" } else { "region" }
                        ))
                    }
                };
                let moved = transform::transform_area(&mut doc, s.layer, s.frame, area, None, op, false);
                let mut detail = format!(
                    "transform {}: {} of layer {:?} frame {} -> {}{}",
                    op.id(),
                    area.describe(),
                    doc.layers[s.layer].name,
                    doc.frame_label(s.frame),
                    moved.rect.describe(),
                    cut(moved.outcome.clipped)
                );
                if scope == "cel" {
                    // regions of this layer only follow the cel
                    let frames = doc.layers[s.layer].linked_frames(s.frame);
                    let skipped = transform::transform_regions(&mut doc, &frames, Some(s.layer), op, true);
                    detail += &locked_regions_note(&skipped);
                }
                let clip = (scope != "cel").then(|| area.union(&moved.rect));
                let what = format!(
                    "{} {} [{}]",
                    doc.frame_label(s.frame),
                    doc.layers[s.layer].name,
                    a.get("region").and_then(Value::as_str).map_or(area.describe(), str::to_string)
                );
                (vec![CelScope { layer: s.layer, frame: s.frame, clip }], detail, what)
            }
            "frame" | "frames" | "tag" | "all" => {
                if a.get("region").is_some() || a.get("clip").is_some() || a.get("rect").is_some() {
                    return Err(format!(
                        "scope {scope} transforms whole frames; region/clip/rect belong to scope region/rect"
                    ));
                }
                let (frames, layers, label) = self.whole_frames(&doc, a, scope)?;
                let cels = transform::unique_cels(&doc, &layers, &frames);
                let mut clipped = 0;
                for &(l, f) in &cels {
                    clipped += transform::transform_cel(&mut doc, l, f, op, false).clipped;
                }
                let one_layer = (layers.len() == 1 && doc.layers.len() > 1).then_some(layers[0]);
                let skipped = transform::transform_regions(&mut doc, &frames, one_layer, op, true);
                let layer_label = match one_layer {
                    Some(l) => format!("layer {:?}", doc.layers[l].name),
                    None => "all layers".to_string(),
                };
                let detail = format!(
                    "transform {} around the canvas centre: {label}, {layer_label} ({} cels){}{}",
                    op.id(),
                    cels.len(),
                    cut(clipped),
                    locked_regions_note(&skipped)
                );
                let scopes = cels.iter().map(|&(layer, frame)| CelScope { layer, frame, clip: None }).collect();
                (scopes, detail, format!("{label}, {layer_label}"))
            }
            s => return Err(format!("unknown scope {s:?} (rect, region, cel, frame, frames, tag, all)")),
        };
        let mut args = a.clone();
        if args.get("note").and_then(Value::as_str).is_none_or(|n| n.trim().is_empty()) {
            args["note"] = json!(format!("{}: transform {} {what}", self.author(), op.id()));
        }
        self.finish_write(&args, &path, &before, doc, &scopes, "transform", detail, true)
    }

    /// Frames and layers of a whole-frame scope (frame, frames, tag, all;
    /// all layers or only `layer`) and a label. Refuses locked cels and
    /// cels linked to frames outside the scope (unless edit_linked).
    fn whole_frames(&self, doc: &Document, a: &Value, scope: &str) -> Result<(Vec<usize>, Vec<usize>, String), String> {
        let (frames, label) = self.scope_frames(doc, a, scope)?;
        let layers: Vec<usize> = match index_or_name(a.get("layer")) {
            Some(l) => vec![doc.find_layer(&l)?],
            None => (0..doc.layers.len()).collect(),
        };
        for &l in &layers {
            for &f in &frames {
                if let Some(why) = doc.edit_blocked(l, f) {
                    return Err(format!(
                        "{why}; nothing was written (unlock it, or pass layer to leave other layers alone)"
                    ));
                }
                let outside: Vec<usize> =
                    doc.layers[l].linked_frames(f).into_iter().filter(|g| !frames.contains(g)).collect();
                if !outside.is_empty() && !bool_arg(a, "edit_linked", false) {
                    return Err(format!(
                        "the cel of layer {:?} on frame {} is linked with frames {outside:?} outside the scope; \
                         changing it would change them too. Pass edit_linked: true, or unlink it first (link_cel unlink: true)",
                        doc.layers[l].name,
                        doc.frame_label(f)
                    ));
                }
            }
        }
        Ok((frames, layers, label))
    }

    /// replace_color and outline: plan the edit on each cel of the scope
    /// (rect/region/cel of one cel, or whole frames like transform) and
    /// write it. Pixels in locked regions stay and are counted.
    fn recolor_tool(&self, a: &Value, tool: &str) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let has_area = a.get("region").is_some() || a.get("clip").is_some() || a.get("rect").is_some();
        let scope = match a.get("scope").and_then(Value::as_str) {
            Some(s) => s,
            None if a.get("region").is_some() => "region",
            None if has_area => "rect",
            None => "cel",
        };
        let edit = match tool {
            "replace_color" => {
                let from = key_of(&doc, a.get("from")).map_err(|e| format!("from: {e}"))?;
                let to = key_of(&doc, a.get("to")).map_err(|e| format!("to: {e}"))?;
                if from == to {
                    return Err("from and to are the same color".into());
                }
                Edit::Replace(from, to)
            }
            _ => {
                let eight = match a.get("connectivity").map(|v| v.as_i64()) {
                    None | Some(Some(4)) => false,
                    Some(Some(8)) => true,
                    _ => return Err("connectivity must be 4 or 8".into()),
                };
                let mode = OutlineMode::parse(a.get("mode").and_then(Value::as_str).unwrap_or("outside"))?;
                let only_transparent = bool_arg(a, "only_transparent", false);
                if only_transparent && mode == OutlineMode::Inside {
                    return Err("only_transparent works with mode outside (inside, every pixel is colored)".into());
                }
                Edit::Outline(Outline { color: key_of(&doc, a.get("color"))?, mode, eight, only_transparent })
            }
        };
        let canvas = Rect::new(0, 0, doc.width, doc.height);
        let (cels, area, what) = match scope {
            "rect" | "region" | "cel" => {
                let s = self.target(&doc, a, "layer", "frame")?;
                let area = match (scope, s.clip) {
                    ("cel", None) => canvas,
                    ("cel", Some(_)) => return Err("scope cel works on the whole cel; drop region/clip/rect".into()),
                    (_, Some(c)) => c,
                    (_, None) => {
                        return Err(format!(
                            "scope {scope} needs {}",
                            if scope == "rect" { "rect/clip {x,y,w,h}" } else { "region" }
                        ))
                    }
                };
                let what = format!(
                    "{} {} [{}]",
                    doc.frame_label(s.frame),
                    doc.layers[s.layer].name,
                    a.get("region").and_then(Value::as_str).map_or(area.describe(), str::to_string)
                );
                (vec![(s.layer, s.frame)], (scope != "cel").then_some(area), what)
            }
            "frame" | "frames" | "tag" | "all" => {
                if has_area {
                    return Err(format!(
                        "scope {scope} works on whole frames; region/clip/rect belong to scope region/rect"
                    ));
                }
                let (frames, layers, label) = self.whole_frames(&doc, a, scope)?;
                let layer_label = if layers.len() == 1 && doc.layers.len() > 1 {
                    format!("layer {:?}", doc.layers[layers[0]].name)
                } else {
                    "all layers".to_string()
                };
                (transform::unique_cels(&doc, &layers, &frames), None, format!("{label}, {layer_label}"))
            }
            s => return Err(format!("unknown scope {s:?} (rect, region, cel, frame, frames, tag, all)")),
        };
        let before = doc.clone();
        let mut doc = doc;
        let plan_area = Area::rect(area.unwrap_or(canvas));
        // an outside outline may lie one pixel around the area
        let clip = area.map(|r| match edit {
            Edit::Outline(o) if o.mode == OutlineMode::Outside => {
                transform::clip_rect(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2), doc.width, doc.height).unwrap_or(r)
            }
            _ => r,
        });
        let (mut changed, mut protected, mut scopes) = (0, 0, Vec::new());
        for &(layer, frame) in &cels {
            let plan = match edit {
                Edit::Replace(from, to) => recolor::replace_points(&before, layer, frame, plan_area, from, to, true),
                Edit::Outline(o) => recolor::outline_points(&before, layer, frame, plan_area, o, true),
            };
            protected += plan.protected;
            changed += plan.apply(&mut doc, layer, frame);
            scopes.push(CelScope { layer, frame, clip });
        }
        let title = match edit {
            Edit::Replace(from, to) => format!("{} -> {}", before.key_label(from), before.key_label(to)),
            Edit::Outline(o) => format!(
                "{} {} {}-connected{}",
                o.mode.id(),
                before.key_label(o.color),
                if o.eight { 8 } else { 4 },
                if o.only_transparent { ", only transparent" } else { "" }
            ),
        };
        let mut detail = format!("{tool} {title}: {what}: {changed} pixels in {} cels", cels.len());
        if protected > 0 {
            detail += &format!("; {protected} pixels in locked regions left unchanged");
        }
        let mut args = a.clone();
        if args.get("note").and_then(Value::as_str).is_none_or(|n| n.trim().is_empty()) {
            args["note"] = json!(format!("{}: {tool} {title} {what}", self.author()));
        }
        self.finish_write(&args, &path, &before, doc, &scopes, tool, detail, false)
    }

    /// The frames of a whole-frame transform scope and a label for them.
    fn scope_frames(&self, d: &Document, a: &Value, scope: &str) -> Result<(Vec<usize>, String), String> {
        Ok(match scope {
            "frame" => {
                let f = d.find_frame(&index_or_name(a.get("frame")).ok_or("scope frame needs frame")?)?;
                (vec![f], format!("frame {}", d.frame_label(f)))
            }
            "frames" => {
                let frames: Vec<usize> = if let Some(list) = a.get("frames").and_then(Value::as_array) {
                    list.iter()
                        .map(|v| d.find_frame(&index_or_name(Some(v)).ok_or("frames: give indices or names")?))
                        .collect::<Result<_, _>>()?
                } else {
                    let from = d.find_frame(
                        &index_or_name(a.get("from")).ok_or("scope frames needs frames [..] or from/to")?,
                    )?;
                    let to = d.find_frame(&index_or_name(a.get("to")).ok_or("scope frames needs to")?)?;
                    if to < from {
                        return Err(format!("from {from} is after to {to}"));
                    }
                    (from..=to).collect()
                };
                if frames.is_empty() {
                    return Err("frames is empty".into());
                }
                let label = format!("frames {frames:?}");
                (frames, label)
            }
            "tag" => {
                let name = str_arg(a, "tag")?;
                let t = d.tags.iter().find(|t| t.name == name).ok_or_else(|| {
                    format!(
                        "no tag {name:?}; tags are [{}]",
                        d.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
                    )
                })?;
                ((t.from..=t.to).collect(), format!("tag {name} (frames {}..{})", t.from, t.to))
            }
            _ => ((0..d.frames.len()).collect(), "all frames".to_string()),
        })
    }

    /// A mirrored copy of a document (e.g. walk_left from walk_right).
    fn mirror_document(&self, a: &Value) -> Result<Out, String> {
        let (src, doc, _) = self.load(&json!({"path": str_arg(a, "src")?}))?;
        let op = Op::parse(a.get("op").and_then(Value::as_str).unwrap_or("flip_h"))?;
        if !matches!(op, Op::FlipH | Op::FlipV) {
            return Err("mirror_document mirrors: op flip_h (default) or flip_v".into());
        }
        let (mirrored, clipped) = transform::mirror_document(&doc, op);
        mirrored.validate()?;
        let msg = format!(
            "{} frames, {} layers, {} tags, {} regions mirrored ({}); durations and linked cels kept{}",
            mirrored.frames.len(),
            mirrored.layers.len(),
            mirrored.tags.len(),
            mirrored.regions.len(),
            op.id(),
            if clipped > 0 { format!("; {clipped} pixels cut off") } else { String::new() }
        );
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written. Would write a copy of {}: {msg}", self.rel(&src))));
        }
        let dest = self.new_doc_path(str_arg(a, "dst")?, bool_arg(a, "overwrite", false))?;
        if dest == src {
            return Err("dst is the source document; use transform scope all to mirror it in place".into());
        }
        io::save(&mirrored, &dest)?;
        let note = note_arg(a, || format!("{}: mirror_document {} from {}", self.author(), op.id(), self.rel(&src)));
        Journal::for_document(&dest).record_edit(&mirrored, &self.author(), &note, None)?;
        Ok(Out::Text(format!("wrote {}: {msg}\nrecorded as {note:?}", self.rel(&dest))))
    }

    fn new_doc_path(&self, name: &str, overwrite: bool) -> Result<PathBuf, String> {
        let file =
            if io::is_document_path(Path::new(name)) { name.to_string() } else { format!("{name}{}", io::EXTENSION) };
        let path = self.resolve(&file)?;
        if path.exists() && !overwrite {
            return Err(format!("{} already exists (pass overwrite: true to replace it)", self.rel(&path)));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        Ok(path)
    }

    fn list_documents(&self) -> Result<Out, String> {
        let mut found = Vec::new();
        collect_docs(&self.workdir, 0, &mut found);
        found.sort();
        if found.is_empty() {
            return Ok(Out::Text(format!(
                "no .pxe.json, .aseprite or .tilemap.json files in {}",
                self.workdir.display()
            )));
        }
        let lines: Vec<String> = found
            .iter()
            .map(|p| match io::load(p) {
                _ if pixel_core::tilemap::is_tilemap_path(p) => match pixel_core::tilemap::load(p) {
                    Ok(m) => format!(
                        "{}  tilemap {}x{} cells, tileset {}, {} legend keys",
                        self.rel(p),
                        m.width,
                        m.height,
                        m.tileset,
                        m.legend.len()
                    ),
                    Err(e) => format!("{}  (unreadable: {e})", self.rel(p)),
                },
                Ok(d) => format!(
                    "{}  {}x{}  {} layers, {} frames, tags: [{}], regions: [{}]",
                    self.rel(p),
                    d.width,
                    d.height,
                    d.layers.len(),
                    d.frames.len(),
                    d.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
                    d.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")
                ),
                Err(e) => format!("{}  (unreadable: {e})", self.rel(p)),
            })
            .collect();
        Ok(Out::Text(lines.join("\n")))
    }

    fn create_document(&self, a: &Value) -> Result<Out, String> {
        let w = int_of(a, "width")?;
        let h = int_of(a, "height")?;
        if w < 1 || h < 1 {
            return Err("width and height must be >= 1".into());
        }
        let palette = match a.get("palette") {
            None | Some(Value::Null) => Document::builtin_palette("sweetie16")?,
            Some(Value::String(name)) => Document::builtin_palette(name)?,
            Some(Value::Array(list)) => {
                let mut out: Vec<PaletteEntry> = Vec::new();
                for (i, e) in list.iter().enumerate() {
                    let at = |e: String| format!("palette[{i}]: {e}");
                    let (key, color, name) = match e {
                        Value::String(c) => (None, c.as_str(), ""),
                        Value::Object(_) => (
                            e.get("key").and_then(Value::as_str),
                            e.get("color").and_then(Value::as_str).ok_or(at("needs \"color\"".into()))?,
                            e.get("name").and_then(Value::as_str).unwrap_or(""),
                        ),
                        _ => return Err(at("must be {key, color, name?} or a color string".into())),
                    };
                    let key = match key {
                        Some(k) => single_key(k).map_err(at)?,
                        None => KEY_ORDER.as_bytes().get(i).copied().ok_or(at("too many colors".into()))?,
                    };
                    if !pixel_core::document::is_valid_key(key) || out.iter().any(|p| p.key == key) {
                        return Err(at(format!("invalid or duplicate key {:?}", key as char)));
                    }
                    out.push(PaletteEntry { key, color: Rgba::parse(color).map_err(at)?, name: name.to_string() });
                }
                out
            }
            Some(_) => return Err("palette must be a builtin name or a list".into()),
        };
        let mut doc = Document::new(w as u32, h as u32, palette)?;
        if let Some(ms) = opt_int(a, "duration_ms")? {
            doc.frames[0].duration_ms = ms.max(1) as u32;
        }
        if let Some(n) = a.get("layer_name").and_then(Value::as_str) {
            doc.rename_layer(0, n);
        }
        let path = self.new_doc_path(str_arg(a, "name")?, bool_arg(a, "overwrite", false))?;
        io::save(&doc, &path)?;
        let author = self.author();
        Journal::for_document(&path).record(&doc, &author, &note_arg(a, || format!("{author}: create_document")))?;
        Ok(Out::Text(format!("created {} ({w}x{h}, palette: {})", self.rel(&path), palette_line(&doc))))
    }

    /// Filmstrip + onion + diff rows of one tag in play order, with an
    /// analysis (ground line, jumps, stray pixels, duplicates).
    fn render_animation(&self, a: &Value) -> Result<Out, String> {
        use pixel_core::animation::{self, AnimOptions, Area};
        let (path, doc, _) = self.load(a)?;
        let name = str_arg(a, "tag")?;
        let tag = doc.tags.iter().position(|t| t.name == name).ok_or_else(|| {
            format!(
                "no tag {name:?}; tags are [{}]",
                doc.tags.iter().map(|t| format!("{:?}", t.name)).collect::<Vec<_>>().join(", ")
            )
        })?;
        let area = |key: &str| -> Result<Option<Area>, String> {
            match a.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => {
                    let ar = Area::parse(s)?;
                    if let Area::Region(n) = &ar {
                        doc.find_region(n)?;
                    }
                    Ok(Some(ar))
                }
                Some(v) => Ok(Some(Area::Rect(rect_arg(v).map_err(|e| format!("{key}: {e}"))?))),
            }
        };
        let opts = AnimOptions {
            zoom: opt_int(a, "zoom")?.map(|z| z.clamp(1, 32) as u32),
            area: area("region")?,
            track: area("track")?,
            jump_threshold: opt_int(a, "jump_threshold")?.unwrap_or(2).max(0) as u32,
        };
        let analysis = animation::analyze(&doc, tag, &opts)?;
        let img = animation::render(&doc, tag, &analysis, &opts)?;
        let dir = self.workdir.join("preview");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join(format!("{}_{name}_anim.png", io::stem(&path)));
        let png = img.encode_png()?;
        io::write_atomic(&out, &png)?;
        let text = format!(
            "wrote {} ({}x{} px). Rows: frames (red dashed line = first step's ground row; cyan box/yellow path = track), \
             onion (previous step cool at 35%, current warm), diff (magenta = changed vs previous step). The last cell \
             repeats step 0 after the loop.\n{}",
            self.rel(&out),
            img.width,
            img.height,
            animation::summary(&doc, &analysis)
        );
        if bool_arg(a, "inline", true) {
            Ok(Out::TextAndImage(text, png))
        } else {
            Ok(Out::Text(text))
        }
    }

    fn render_preview(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let frames: Vec<usize> = match (a.get("frames").and_then(Value::as_array), index_or_name(a.get("frame"))) {
            (Some(list), _) => list
                .iter()
                .map(|v| doc.find_frame(&index_or_name(Some(v)).unwrap_or_default()))
                .collect::<Result<_, _>>()?,
            (None, Some(f)) if f != "all" => vec![doc.find_frame(&f)?],
            _ => (0..doc.frames.len()).collect(),
        };
        let layer = match index_or_name(a.get("layer")) {
            Some(l) => Some(doc.find_layer(&l)?),
            None => None,
        };
        let tiled = bool_arg(a, "tiled", false);
        let auto = if tiled { PreviewOptions::auto_zoom_tiled(&doc) } else { PreviewOptions::auto_zoom(&doc) };
        let zoom = opt_int(a, "zoom")?.map(|z| z.clamp(1, 64) as u32).unwrap_or(auto);
        let opts = PreviewOptions {
            zoom,
            grid: bool_arg(a, "grid", !tiled),
            layer,
            regions: bool_arg(a, "regions", !tiled),
            tiled,
        };
        let img = render::preview(&doc, &frames, &opts);
        let stem = io::stem(&path);
        let dir = self.workdir.join("preview");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut file = stem.clone();
        if frames.len() == 1 {
            file += &format!("_{}", doc.frames[frames[0]].name);
        }
        if let Some(l) = layer {
            file += &format!("_layer{l}");
        }
        if tiled {
            file += "_tiled";
        }
        let out = dir.join(format!("{file}.png"));
        let png = img.encode_png()?;
        io::write_atomic(&out, &png)?;
        let tiling = if tiled {
            " Tiled: each frame repeated 3x3 (rulers count inside the tile, pink ticks mark the seams); \
             look for edges that break where the copies meet, or run check_seams."
        } else {
            ""
        };
        let text = format!(
            "wrote {} ({}x{} px, zoom {zoom}, frames {frames:?}{}). Each frame: label (index name / ms [tags]), \
             rulers with pixel coordinates, grid (brighter every 8 px), region outlines with names, 1x and 2x \
             thumbnails; palette swatches with keys at the bottom.{tiling}",
            self.rel(&out),
            img.width,
            img.height,
            layer.map(|l| format!(", layer {l} only")).unwrap_or_default()
        );
        if bool_arg(a, "inline", true) {
            Ok(Out::TextAndImage(text, png))
        } else {
            Ok(Out::Text(text))
        }
    }
}

impl Server {
    /// Compare the edges of tiles: right column against left, bottom row
    /// against top (see `pixel_core::seams`).
    fn check_seams(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let frames: Vec<usize> = match index_or_name(a.get("frame")) {
            Some(f) if f != "all" => vec![doc.find_frame(&f)?],
            _ => (0..doc.frames.len()).collect(),
        };
        let layer = match index_or_name(a.get("layer")) {
            Some(l) => Some(doc.find_layer(&l)?),
            None => None,
        };
        let threshold = match a.get("threshold") {
            None | Some(Value::Null) => seams::DEFAULT_THRESHOLD,
            Some(v) => v
                .as_f64()
                .filter(|t| (0.0..=1.0).contains(t))
                .ok_or_else(|| format!("argument \"threshold\" must be a number 0..1, got {v}"))?
                as f32,
        };
        let mut text = format!(
            "{}: edges compared as they meet when the frame is repeated ({}, threshold {threshold}: color \
             distance 0..1 from which a pair is a hard edge; the seam is judged against the hard edges inside \
             the tile).\n",
            self.rel(&path),
            layer.map_or("visible composite".to_string(), |l| format!("layer {l} {} only", doc.layers[l].name))
        );
        let mut visible = 0;
        for f in frames {
            let r = seams::check(&doc, f, layer, threshold);
            visible += [&r.left_right, &r.top_bottom].iter().filter(|s| s.stands_out()).count();
            text += &format!("\nframe {}:\n{}", doc.frame_label_long(f), r.describe(&doc));
        }
        text += &format!(
            "\n{}",
            if visible == 0 {
                "No seam stands out.".to_string()
            } else {
                format!("{visible} visible seam(s); render_preview with tiled: true shows them.")
            }
        );
        Ok(Out::Text(text))
    }
}

// ---- region and tag helpers -------------------------------------------------

fn list_regions(doc: &Document, a: &Value) -> Result<String, String> {
    if doc.regions.is_empty() {
        return Ok("no regions (create them with set_region)".into());
    }
    let frame = match index_or_name(a.get("frame")) {
        Some(f) => Some(doc.find_frame(&f)?),
        None => None,
    };
    let mut s = String::new();
    for r in &doc.regions {
        s += &format!(
            "{:?}{}{}{}{}:",
            r.name,
            if r.is_marker() { " MARKER" } else { "" },
            r.layer.as_ref().map(|l| format!(" layer {l:?}")).unwrap_or_default(),
            match (r.locked, r.is_marker()) {
                (true, true) => " LOCKED (definition only; pixels stay editable)",
                (true, false) => " LOCKED",
                _ => "",
            },
            r.color.map(|c| format!(" color {c}")).unwrap_or_default()
        );
        match frame {
            Some(f) => {
                s += &match r.rect_at(f) {
                    Some(rc) if r.is_marker() => format!(" on frame {}: at ({}, {})\n", doc.frame_label(f), rc.x, rc.y),
                    Some(rc) => format!(" on frame {}: {}\n", doc.frame_label(f), rc.describe()),
                    None => format!(" not present on frame {}\n", doc.frame_label(f)),
                }
            }
            None => {
                let n = doc.frames.len();
                let spans: Vec<String> = r
                    .keys
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let end = r.keys.get(i + 1).map_or(n - 1, |next| next.frame - 1);
                        let frames = if k.frame == end {
                            format!("frame {}", k.frame)
                        } else {
                            format!("frames {}..{}", k.frame, end)
                        };
                        match k.rect {
                            Some(rc) if r.is_marker() => format!("{frames}: at ({}, {})", rc.x, rc.y),
                            Some(rc) => format!("{frames}: {}", rc.describe()),
                            None => format!("{frames}: hidden"),
                        }
                    })
                    .collect();
                s += &format!(" {}\n", spans.join("; "));
            }
        }
    }
    Ok(s)
}

fn set_region(d: &mut Document, a: &Value) -> Result<String, String> {
    let name = str_arg(a, "name")?.to_string();
    if name.is_empty() || name.contains('/') {
        return Err("region names must be non-empty and contain no '/'".into());
    }
    let existing = d.regions.iter().position(|r| r.name == name);
    let kind = match a.get("kind").and_then(Value::as_str) {
        Some(k) => Some(RegionKind::parse(k)?),
        None => None,
    };
    let marker = kind.unwrap_or_else(|| existing.map_or(RegionKind::Area, |i| d.regions[i].kind)) == RegionKind::Marker;
    let rect = match (opt_int(a, "x")?, opt_int(a, "y")?, opt_int(a, "w")?, opt_int(a, "h")?) {
        // a marker is a point: x, y (stored as a 1x1 rect)
        (Some(x), Some(y), None, None) if marker => Some(Rect::new(x, y, 1, 1)),
        (Some(_), Some(_), Some(_), Some(_)) if marker => return Err("a marker is a point: give only x and y".into()),
        (Some(x), Some(y), Some(w), Some(h)) if w > 0 && h > 0 => Some(Rect::new(x, y, w as u32, h as u32)),
        (None, None, None, None) => None,
        _ if marker => return Err("give x and y of the marker".into()),
        _ => return Err("give all of x, y, w, h (w, h >= 1)".into()),
    };
    // frame scope: a frame, a from/to range, or a tag's range
    let n = d.frames.len();
    let scope = match (
        a.get("tag").and_then(Value::as_str),
        index_or_name(a.get("from")),
        index_or_name(a.get("to")),
        index_or_name(a.get("frame")),
    ) {
        (Some(t), None, None, None) => {
            let tag = d.tags.iter().find(|x| x.name == t).ok_or_else(|| format!("no tag {t:?}"))?;
            Some((tag.from, tag.to))
        }
        (None, Some(f), Some(t), None) => Some((d.find_frame(&f)?, d.find_frame(&t)?)),
        (None, None, None, Some(f)) => {
            let f = d.find_frame(&f)?;
            Some((f, f))
        }
        (None, None, None, None) => None,
        _ => return Err("limit frames with exactly one of: frame, from+to, tag".into()),
    };
    let i = match existing {
        Some(i) => {
            let r = &d.regions[i];
            let only_lock = a
                .as_object()
                .unwrap()
                .keys()
                .all(|k| matches!(k.as_str(), "path" | "name" | "locked" | "note" | "dry_run"));
            if r.locked && !only_lock && a.get("locked") != Some(&json!(false)) {
                return Err(format!("region {name:?} is locked; unlock it first (locked: false)"));
            }
            i
        }
        None => {
            let rc = rect.ok_or_else(|| {
                if marker {
                    format!("new marker {name:?} needs x, y")
                } else {
                    format!("new region {name:?} needs x, y, w, h")
                }
            })?;
            let mut r = Region::new(&name, rc);
            if marker {
                r.kind = RegionKind::Marker;
            }
            if let Some((from, to)) = scope {
                // only on those frames
                r.keys = vec![];
                r.keys.push(pixel_core::document::RegionKey { frame: 0, rect: None });
                r.set_rect_for(from.min(to), from.max(to), Some(rc), n);
            }
            d.regions.push(r);
            d.regions.len() - 1
        }
    };
    if existing.is_some() {
        if let Some(rc) = rect {
            match scope {
                Some((from, to)) => d.regions[i].set_rect_for(from.min(to), from.max(to), Some(rc), n),
                None => d.regions[i].keys = vec![pixel_core::document::RegionKey { frame: 0, rect: Some(rc) }],
            }
        } else if let Some((from, to)) = scope {
            if bool_arg(a, "hide", false) {
                d.regions[i].set_rect_for(from.min(to), from.max(to), None, n);
            }
        }
    }
    if let Some(l) = a.get("layer") {
        d.regions[i].layer = match l {
            Value::Null => None,
            Value::String(s) if s.is_empty() || s == "all" => None,
            other => Some(d.layers[d.find_layer(&index_or_name(Some(other)).unwrap_or_default())?].name.clone()),
        };
    }
    if let Some(k) = kind {
        d.regions[i].kind = k;
        if k == RegionKind::Marker {
            // a marker is a point: keep the top-left of each rect
            for key in &mut d.regions[i].keys {
                if let Some(rc) = &mut key.rect {
                    *rc = Rect::new(rc.x, rc.y, 1, 1);
                }
            }
        }
    }
    if let Some(v) = a.get("locked").and_then(Value::as_bool) {
        d.regions[i].locked = v;
    }
    if let Some(c) = a.get("color").and_then(Value::as_str) {
        d.regions[i].color = Some(Rgba::parse(c)?);
    }
    if let Some(new) = a.get("rename").and_then(Value::as_str) {
        if d.regions.iter().any(|r| r.name == new) {
            return Err(format!("a region is already called {new:?}"));
        }
        d.regions[i].name = new.to_string();
    }
    d.regions[i].normalize();
    if d.regions[i].keys.is_empty() {
        return Err("the region would be hidden on every frame".into());
    }
    let mut out = String::new();
    list_regions_one(d, i, &mut out);
    Ok(format!("{} region {out}", if existing.is_some() { "updated" } else { "created" }))
}

fn list_regions_one(d: &Document, i: usize, out: &mut String) {
    let doc = Document { regions: vec![d.regions[i].clone()], ..d.clone() };
    *out = list_regions(&doc, &json!({})).unwrap_or_default().trim_end().to_string();
}

fn set_tag(d: &mut Document, a: &Value) -> Result<String, String> {
    let name = str_arg(a, "name")?.to_string();
    if name.contains('/') {
        return Err("tag names must not contain '/' (reserved for tag/index addresses)".into());
    }
    let existing = d.tags.iter().position(|t| t.name == name);
    if bool_arg(a, "delete", false) {
        let i = existing.ok_or_else(|| format!("no tag {name:?}"))?;
        d.tags.remove(i);
        return Ok(format!("deleted tag {name:?}"));
    }
    if let Some(list) = a.get("frames").and_then(Value::as_array) {
        // arbitrary order: built from linked frames (Aseprite-compatible)
        let frames: Vec<usize> =
            list.iter().map(|v| d.find_frame(&index_or_name(Some(v)).unwrap_or_default())).collect::<Result<_, _>>()?;
        let before = d.frames.len();
        let (from, to) = d.sequence_tag(&name, &frames)?;
        apply_tag_options(d, &name, a)?;
        let added = d.frames.len() - before;
        let extra = if added > 0 {
            format!("; appended {added} linked frames {before}..{} so the tag can be a range", d.frames.len() - 1)
        } else {
            String::new()
        };
        return Ok(format!(
            "tag {name:?} = frames {from}..{to}, plays {:?}{extra}",
            d.tags.iter().find(|t| t.name == name).unwrap().sequence()
        ));
    }
    let mut tag = existing.map(|i| d.tags[i].clone()).unwrap_or_else(|| Tag::new(&name, 0, 0));
    let from = match index_or_name(a.get("from")) {
        Some(s) => Some(d.find_frame(&s)?),
        None => None,
    };
    let to = match index_or_name(a.get("to")) {
        Some(s) => Some(d.find_frame(&s)?),
        None => None,
    };
    match (from, to, existing) {
        (Some(f), Some(t), _) => (tag.from, tag.to) = (f.min(t), f.max(t)),
        (Some(f), None, None) => (tag.from, tag.to) = (f, f),
        (None, None, Some(_)) => {}
        (None, Some(_), _) => return Err("give \"from\" together with \"to\"".into()),
        (Some(f), None, Some(_)) => tag.from = f.min(tag.to),
        (None, None, None) => return Err(format!("new tag {name:?} needs \"from\" and \"to\" (or \"frames\")")),
    }
    d.set_tag(tag)?;
    apply_tag_options(d, &name, a)?;
    let t = d.tags.iter().find(|t| t.name == name).unwrap();
    Ok(format!(
        "tag {name:?} = frames {}..{} {}, plays {:?}{}",
        t.from,
        t.to,
        t.direction.id(),
        t.sequence(),
        if t.looping { " (loops)" } else { " (once)" }
    ))
}

fn apply_tag_options(d: &mut Document, name: &str, a: &Value) -> Result<(), String> {
    let t = d.tags.iter_mut().find(|t| t.name == name).unwrap();
    if let Some(dir) = a.get("direction").and_then(Value::as_str) {
        t.direction = Direction::parse(dir)?;
    }
    if let Some(l) = a.get("loop").and_then(Value::as_bool) {
        t.looping = l;
    }
    if let Some(c) = a.get("color").and_then(Value::as_str) {
        t.color = Some(Rgba::parse(c)?);
    }
    Ok(())
}

// ---- formatting -----------------------------------------------------------

fn collect_docs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if depth < 4
                && !name.starts_with('.')
                && !matches!(name.as_str(), "target" | "export" | "preview" | "node_modules")
            {
                collect_docs(&p, depth + 1, out);
            }
        } else if io::is_document_path(&p) || pixel_core::tilemap::is_tilemap_path(&p) {
            out.push(p);
        }
    }
}

fn off_canvas(n: usize) -> String {
    if n > 0 {
        format!(" ({n} of them outside the canvas)")
    } else {
        String::new()
    }
}

fn paint(d: &mut Document, s: CelScope, pts: &[(i32, i32)], key: u8) -> String {
    let off = pts.iter().filter(|(x, y)| !d.in_bounds(*x, *y)).count();
    d.set_points(s.layer, s.frame, pts, key);
    format!("{} pixels with {}{}", pts.len(), d.key_label(key), off_canvas(off))
}

/// The `dither` argument {pattern, colors: [a, b]}: pattern and both keys
/// (b defaults to transparent; a defaults to `key`).
fn dither_arg(d: &Document, a: &Value) -> Result<Option<(Pattern, u8, u8)>, String> {
    let Some(v) = a.get("dither").filter(|v| !v.is_null()) else { return Ok(None) };
    let pattern = Pattern::parse(v.get("pattern").and_then(Value::as_str).unwrap_or("checker"))?;
    let colors: Vec<&Value> = match v.get("colors") {
        Some(Value::Array(c)) => c.iter().collect(),
        Some(_) => return Err("dither.colors must be an array [first, second]".into()),
        None => Vec::new(),
    };
    if colors.len() > 2 {
        return Err("dither.colors takes two colors".into());
    }
    let first = match colors.first() {
        Some(k) => key_of(d, Some(k)).map_err(|e| format!("dither.colors[0]: {e}"))?,
        None => key_of(d, a.get("key")).map_err(|e| format!("dither needs colors [a, b] or key: {e}"))?,
    };
    let second = match colors.get(1) {
        Some(k) => key_of(d, Some(k)).map_err(|e| format!("dither.colors[1]: {e}"))?,
        None => TRANSPARENT,
    };
    Ok(Some((pattern, first, second)))
}

#[derive(Clone, Copy)]
enum Edit {
    Replace(u8, u8),
    Outline(Outline),
}

fn palette_line(doc: &Document) -> String {
    doc.palette.iter().map(|p| format!("{}={}", p.label(), p.color)).collect::<Vec<_>>().join(" ")
}

/// "; locked regions kept their rect: a, b" (or nothing).
fn locked_regions_note(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!("; locked regions kept their rect: {}", names.join(", "))
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// Ramp keys: a string of keys ("389a") or an array of keys or color names.
fn ramp_keys(d: &Document, v: Option<&Value>) -> Result<Vec<u8>, String> {
    match v {
        Some(Value::String(s)) => s.bytes().map(|k| d.check_key(k).map(|_| k)).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|k| k.as_str().ok_or("keys must be strings".to_string()).and_then(|k| d.find_key(k)))
            .collect(),
        _ => Err("missing \"keys\": dark to light, e.g. \"389a\" or [\"crust_dark\", \"crust\", ...]".into()),
    }
}

fn clock(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}Z", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

/// Summary plus, per cel, the rows of its content (cropped to the
/// non-transparent area by default) with absolute coordinate rulers.
fn describe(doc: &Document, rel: &str, a: &Value) -> Result<String, String> {
    let mut s = format!(
        "{rel}: {}x{} px, {} layers, {} frames\npalette ('.' = transparent):\n",
        doc.width,
        doc.height,
        doc.layers.len(),
        doc.frames.len()
    );
    for p in &doc.palette {
        s += &format!("  {} {}\n", p.label(), p.color);
    }
    if !doc.ramps.is_empty() {
        s += "color ramps (dark > light; shade steps along them):\n";
        for r in &doc.ramps {
            s += &format!("  {}\n", doc.ramp_label(r));
        }
    }
    s += "layers (0 = bottom):\n";
    for (i, l) in doc.layers.iter().enumerate() {
        s += &format!(
            "  {i} {:?}{}{}{}\n",
            l.name,
            if l.visible { "" } else { " (hidden)" },
            if l.opacity < 1.0 { format!(" opacity {:.2}", l.opacity) } else { String::new() },
            if l.locked { " LOCKED" } else { "" }
        );
    }
    s += "frames (address: index, name, or tag/n):\n";
    for (i, f) in doc.frames.iter().enumerate() {
        let tags: Vec<String> =
            doc.tags.iter().filter(|t| t.contains(i)).map(|t| format!("{}/{}", t.name, i - t.from)).collect();
        s += &format!(
            "  {i} {:?} {} ms{}{}\n",
            f.name,
            f.duration_ms,
            if tags.is_empty() { String::new() } else { format!(" = {}", tags.join(" = ")) },
            if f.locked { " LOCKED" } else { "" }
        );
    }
    if doc.tags.is_empty() {
        s += "tags: none (all frames play in order)\n";
    } else {
        s += "tags:\n";
        for t in &doc.tags {
            s += &format!(
                "  {:?} frames {}..{} {} {} -> plays {:?}\n",
                t.name,
                t.from,
                t.to,
                t.direction.id(),
                if t.looping { "loop" } else { "once" },
                t.sequence()
            );
        }
    }
    if !doc.regions.is_empty() {
        s += "regions:\n";
        for line in list_regions(doc, &json!({}))?.lines() {
            s += &format!("  {line}\n");
        }
    }
    if !bool_arg(a, "pixels", true) {
        return Ok(s);
    }
    let crop = bool_arg(a, "crop", true);
    let layers: Vec<usize> = match index_or_name(a.get("layer")) {
        Some(l) => vec![doc.find_layer(&l)?],
        None => (0..doc.layers.len()).collect(),
    };
    let frames: Vec<usize> = match index_or_name(a.get("frame")) {
        Some(f) => vec![doc.find_frame(&f)?],
        None => (0..doc.frames.len()).collect(),
    };
    s += if crop {
        "cels (cropped to their non-transparent pixels; ruler digits and row labels are absolute x/y):\n"
    } else {
        "cels (full canvas; ruler digits and row labels are absolute x/y):\n"
    };
    for &f in &frames {
        for &l in &layers {
            let head = format!("--- layer {l} {:?}, frame {}", doc.layers[l].name, doc.frame_label_long(f));
            let src = doc.layers[l].link_source(f);
            if src != f {
                s += &format!("{head}: linked to frame {src} (same pixels) ---\n");
                continue;
            }
            let bounds = if crop { doc.cel_bounds(l, f) } else { Some((0, 0, doc.width, doc.height)) };
            let Some((x, y, w, h)) = bounds else {
                s += &format!("{head}: empty ---\n");
                continue;
            };
            let r = Rect::new(x as i32, y as i32, w, h);
            s += &format!("{head}: {} ---\n", r.describe());
            s += &rows_block(|px, py| doc.get(l, f, px, py), r);
        }
    }
    Ok(s)
}

// ---- argument helpers ---------------------------------------------------

/// Reject arguments the tool does not know (catches typos like "colour").
fn check_args(tool: &str, args: &Value) -> Result<(), String> {
    let Some(def) = tools().as_array().unwrap().iter().find(|t| t["name"] == tool) else {
        return Err(format!("unknown tool {tool:?}"));
    };
    let props = def["inputSchema"]["properties"].as_object().unwrap();
    let Some(obj) = args.as_object() else { return Err("arguments must be an object".into()) };
    for k in obj.keys() {
        if !props.contains_key(k) {
            let mut valid: Vec<&str> = props.keys().map(String::as_str).collect();
            valid.sort();
            return Err(format!("{tool}: unknown argument {k:?}; valid arguments: {}", valid.join(", ")));
        }
    }
    for r in def["inputSchema"]["required"].as_array().unwrap() {
        let r = r.as_str().unwrap();
        if obj.get(r).is_none_or(Value::is_null) {
            return Err(format!("{tool}: missing required argument {r:?}"));
        }
    }
    Ok(())
}

fn note_arg(a: &Value, default: impl FnOnce() -> String) -> String {
    match a.get("note").and_then(Value::as_str) {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => default(),
    }
}

fn rect_arg(v: &Value) -> Result<Rect, String> {
    let (x, y, w, h) = (int_of(v, "x")?, int_of(v, "y")?, int_of(v, "w")?, int_of(v, "h")?);
    if w < 1 || h < 1 {
        return Err(format!("w and h must be >= 1 (got w={w}, h={h})"));
    }
    Ok(Rect::new(x, y, w as u32, h as u32))
}

fn str_arg<'a>(a: &'a Value, name: &str) -> Result<&'a str, String> {
    a.get(name).and_then(Value::as_str).ok_or_else(|| format!("missing string argument \"{name}\""))
}

fn bool_arg(a: &Value, name: &str, default: bool) -> bool {
    a.get(name).and_then(Value::as_bool).unwrap_or(default)
}

fn opt_int(a: &Value, name: &str) -> Result<Option<i32>, String> {
    match a.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(|i| Some(i.clamp(i32::MIN as i64, i32::MAX as i64) as i32))
            .ok_or_else(|| format!("argument \"{name}\" must be an integer, got {v}")),
    }
}

fn int_of(a: &Value, name: &str) -> Result<i32, String> {
    opt_int(a, name)?.ok_or_else(|| format!("missing integer argument \"{name}\""))
}

/// Layer/frame references may be given as index or name.
fn index_or_name(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// The pixels of a shaped selection (from the file): `mask` rows of '#'
/// (selected) and '.' over the box `s`; unselected pixels show as '·'.
fn masked_rows(
    path: &Path,
    frame: &crate::view::FrameRef,
    layer: &crate::view::LayerRef,
    s: crate::view::Area,
    mask: &[String],
) -> String {
    let d = match io::load(path) {
        Ok(d) => d,
        Err(e) => return format!("(could not read the pixels: {e})"),
    };
    let (l, f) = (layer.index, frame.index);
    let r = Rect::new(s.x, s.y, s.w, s.h);
    if l >= d.layers.len() || f >= d.frames.len() || !d.in_bounds(r.x + r.w as i32 - 1, r.y + r.h as i32 - 1) {
        return "(the selection no longer fits the file)".into();
    }
    let selected = |x: i32, y: i32| {
        mask.get((y - r.y) as usize).and_then(|row| row.as_bytes().get((x - r.x) as usize)) == Some(&b'#')
    };
    const NOT: u8 = 1; // shown as '·'
    let key = |x: i32, y: i32| if selected(x, y) { d.get(l, f, x, y) } else { NOT };
    let mut used: Vec<u8> = (r.y..r.y + r.h as i32)
        .flat_map(|y| (r.x..r.x + r.w as i32).map(move |x| (x, y)))
        .map(|(x, y)| key(x, y))
        .filter(|k| *k != TRANSPARENT && *k != NOT)
        .collect();
    used.sort();
    used.dedup();
    let legend: Vec<String> = used.iter().map(|k| d.key_label(*k)).collect();
    format!(
        "layer {l} {:?}, frame {}: {}\nkeys used: {}\n{}",
        d.layers[l].name,
        d.frame_label_long(f),
        r.describe(),
        if legend.is_empty() { "none".into() } else { legend.join(", ") },
        rows_block(key, r).replace(NOT as char, "·")
    )
}

fn single_key(s: &str) -> Result<u8, String> {
    match s.as_bytes() {
        [k] => Ok(*k),
        _ => Err(format!("key must be a single ASCII character, got {s:?}")),
    }
}

/// A palette key given as key character or color name.
fn key_of(d: &Document, v: Option<&Value>) -> Result<u8, String> {
    let s = v
        .and_then(Value::as_str)
        .ok_or("missing string argument \"key\" (palette key or color name, '.' = transparent)")?;
    d.find_key(s)
}

/// `set_rows` accepts keys only; this hook exists to report a clear error
/// when someone passes color names in rows.
fn expand_names(d: &Document, rows: &[String], _a: &Value) -> Result<Vec<String>, String> {
    for (i, r) in rows.iter().enumerate() {
        if d.palette.iter().any(|p| !p.name.is_empty() && r == &p.name) {
            return Err(format!("rows[{i}] is a color name; rows must be strings of single-character keys"));
        }
    }
    Ok(rows.to_vec())
}

const INSTRUCTIONS: &str = "Pixel-art documents (.pxe.json or .aseprite) in the workdir, edited together by \
the user and the AI assistant (you; the editor names you after your MCP clientInfo, e.g. Claude or Codex). \
Pixels are palette keys (single characters, '.' = transparent); colors also have names (e.g. '9 harness') \
usable as key. Coordinates: (0,0) top-left, x right, y down. Layer 0 is the bottom layer. Frames are \
addressed by index, unique name or '<tag>/<n>' (e.g. 'walk_down/2'). Named regions ('left_shoe') are rects \
per frame. CONSISTENCY RULES: every pixel write needs an explicit target (layer + frame, optionally region or clip) and \
never changes anything outside it; locked layers/frames/regions refuse writes; every write returns a diff \
(count, bbox, rows before/after), accepts dry_run and a note, and is recorded in the history (undo, \
snapshots). Change only what the user named. Typical loop: get_document / list_regions -> get_rows of the \
area -> edit with a region or clip -> check the returned diff -> render_preview. Art direction: characters \
48x48, the companion dog 48x36, the slime 32x32; tiles 32x32. Attack animations use bigger canvases \
(104x56, 80x96) with a locked marker region 'origin' at the feet. One document per animation, in a folder \
per character (e.g. toast_warrior/walk_side.pxe.json); repeated frames inside an animation are linked cels. \
The user may watch the file in the pixel-editor GUI, which follows your edits live and shows you as their \
author: use focus to show where you are about to work and pace_ms on writes so they can watch it being \
drawn. When the user says 'this', 'here', 'the marks' or 'what I selected', call get_user_view: it shows the \
document, frame, layer, selection (with its pixels) and cursor of every open editor, and all 'KI zeigen' marks \
of every document grouped by frame: numbered areas or pixels with an optional note (e.g. #2 'brauner Block \
weg') and their pixels. Marks are the user's instructions; after handling them remove them with clear_marks. \
Mirroring/rotating: transform (op flip_h|flip_v|rot90|rot180|rot270; scope rect/region/cel of one layer+frame, or \
frame/frames/tag/all around the canvas centre; regions follow). A facing-left animation from a facing-right one: \
mirror_document(src, dst) writes a mirrored copy with tags, durations, linked cels and regions kept; never redraw it. \
Shading: a document can have color ramps (palette keys dark to light, e.g. crust 3>8>9>a; set_ramps, shown by \
get_document); shade(pixels|rect|region, steps +n lighter / -n darker) moves pixels along their ramp and leaves \
colors outside any ramp alone. \
Recoloring: replace_color(from, to) and outline(color, mode outside|inside, connectivity 4|8, only_transparent) \
take the same scopes as transform (rect/region/cel of one layer+frame, or frame/frames/tag/all); fill and draw_rect \
(filled) accept dither {pattern checker|25|75|bayer1..15|lines_h|lines_d, colors [a, b]}, aligned to the canvas so \
neighbouring fills and tiles of 4n px stay seamless. \
Tiles must repeat seamlessly: render_preview with tiled: true shows a frame 3x3, check_seams reports hard edges where \
the right/bottom edge meets the left/top one. \
Environments are tilemaps (.tilemap.json): a grid of one-character keys whose legend maps each key to a tile (a frame \
name of the tileset document, e.g. tiles32.pxe.json) and optionally an object document standing on the cell (tree64). \
Read with get_tilemap, change cells with set_tiles (diff, dry_run, note, history/undo like pixel writes), keys with \
set_legend, size with resize_tilemap, look with render_preview. Editing a tile in the tileset changes it everywhere on \
the map. The sandbox game's world.txt: import_world / export_world (byte-exact round trip; the game adds edges, cliff \
faces and decor itself). Godot: export target godot on a map (TileSet + TileMapLayer scene) or godot_tileset on a tileset.";

fn tools() -> &'static Value {
    static TOOLS: OnceLock<Value> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let mut list = tool_list();
        list.as_array_mut().unwrap().extend(tilemap::tool_defs());
        list
    })
}

fn tool_list() -> Value {
    let path =
        json!({"type": "string", "description": "document path relative to the workdir; the extension may be omitted"});
    let layer = json!({"type": ["integer", "string"], "description": "layer index (0 = bottom) or name"});
    let frame = json!({"type": ["integer", "string"], "description": "frame index, unique name, or \"<tag>/<n>\""});
    let key = json!({"type": "string", "description": "palette key (one character) or color name; '.' = transparent"});
    let int = |d: &str| json!({"type": "integer", "description": d});
    let boolean = |d: &str| json!({"type": "boolean", "description": d});
    let note = json!({"type": "string", "description": "history description; default generated (\"<assistant>: <tool> <frame> <layer> ...\", e.g. \"Claude: ...\" or \"Codex: ...\")"});
    let dry = boolean("only report what would change; write nothing");
    let dither = json!({"type": "object", "description": "fill with a two-color pattern aligned to the canvas (seamless next to other fills and in tiles of 4n px)", "properties": {"pattern": {"type": "string", "description": "checker (50 %), 25, 75 (share of the first color), bayer1..bayer15 (Bayer 4x4, n/16 first color), lines_h, lines_d"}, "colors": {"type": "array", "items": {"type": "string"}, "description": "[first, second] palette keys or color names, e.g. two ramp steps; second default '.'; first default key"}}});
    // the scope of replace_color and outline (as transform)
    let scoped = |extra: Value| -> Value {
        let mut m = Map::new();
        m.insert("path".into(), path.clone());
        m.insert("scope".into(), json!({"type": "string", "enum": ["rect", "region", "cel", "frame", "frames", "tag", "all"], "description": "rect (rect/clip) / region (region) / cel: one cel (layer + frame); frame / frames / tag / all: whole frames, all layers or only layer. Default: region or rect when given, else cel"}));
        m.insert("layer".into(), json!({"type": ["integer", "string"], "description": "layer (required for rect/region/cel; for whole frames: only this layer)"}));
        m.insert(
            "frame".into(),
            json!({"type": ["integer", "string"], "description": "frame (rect/region/cel/frame)"}),
        );
        m.insert(
            "region".into(),
            json!({"type": "string", "description": "scope region: this region's rect on that frame"}),
        );
        m.insert("clip".into(), json!({"type": "object", "description": "scope rect: {x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}}));
        m.insert("rect".into(), json!({"type": "object", "description": "same as clip", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}}));
        m.insert("frames".into(), json!({"type": "array", "items": {"type": ["integer", "string"]}, "description": "scope frames: these frames"}));
        m.insert("from".into(), json!({"type": ["integer", "string"], "description": "scope frames: first frame"}));
        m.insert(
            "to".into(),
            json!({"type": ["integer", "string"], "description": "scope frames: last frame (inclusive)"}),
        );
        m.insert("tag".into(), json!({"type": "string", "description": "scope tag: the frames of this tag"}));
        m.insert("edit_linked".into(), boolean("allow changing cels linked to frames outside the target"));
        m.insert("dry_run".into(), dry.clone());
        m.insert("note".into(), note.clone());
        m.insert("pace_ms".into(), int("show the change row by row in an open editor, this many ms apart"));
        m.extend(extra.as_object().unwrap().clone());
        Value::Object(m)
    };
    // explicit target for every pixel write
    let write = |extra: Value| -> Value {
        let mut m = Map::new();
        m.insert("path".into(), path.clone());
        m.insert("layer".into(), layer.clone());
        m.insert("frame".into(), frame.clone());
        m.insert("region".into(), json!({"type": "string", "description": "only change pixels inside this named region (its rect on that frame)"}));
        m.insert("clip".into(), json!({"type": "object", "description": "only change pixels inside {x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}}));
        m.insert(
            "rect".into(),
            json!({"type": "object", "description": "same as clip: only change pixels inside {x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}}),
        );
        m.insert(
            "edit_linked".into(),
            boolean("allow writing a cel that is linked to other frames (changes all of them)"),
        );
        m.insert("dry_run".into(), dry.clone());
        m.insert("note".into(), note.clone());
        m.insert("pace_ms".into(), int("draw the change stroke by stroke (row by row), this many ms apart, so the user can watch it in the editor; default 0 = at once"));
        m.extend(extra.as_object().unwrap().clone());
        Value::Object(m)
    };
    let meta = |extra: Value| -> Value {
        let mut m = Map::new();
        m.insert("path".into(), path.clone());
        m.insert("dry_run".into(), dry.clone());
        m.insert("note".into(), note.clone());
        m.extend(extra.as_object().unwrap().clone());
        Value::Object(m)
    };
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({"name": name, "description": desc,
               "inputSchema": {"type": "object", "properties": props, "required": required}})
    };
    let w_req = |extra: &[&'static str]| -> Vec<&'static str> {
        let mut v = vec!["path", "layer", "frame"];
        v.extend_from_slice(extra);
        v
    };
    json!([
        tool("list_documents", "List .pxe.json and .aseprite documents in the workdir with size, layers, frames, tags and regions, and tilemaps (.tilemap.json) with size and tileset.", json!({}), &[]),
        tool("create_document", "Create a new document with one layer and one frame. Name ending in .aseprite creates an Aseprite file. Art direction: characters 48x48 (companion dog 48x36, slime 32x32), tiles 32x32, attack animations bigger (104x56, 80x96) with a locked marker 'origin' at the feet; one document per animation in a folder per character.",
            json!({
                "name": {"type": "string", "description": "file name (\".pxe.json\" is appended unless it ends in .pxe.json/.aseprite/.ase), may include subdirectories"},
                "width": int("width in pixels"), "height": int("height in pixels"),
                "palette": {"description": format!("builtin palette name ({}) or a list of {{key, color, name?}} objects / color strings; default sweetie16", palettes::names().join(", "))},
                "duration_ms": int("duration of the first frame (default 100)"),
                "layer_name": {"type": "string", "description": "name of the first layer"},
                "overwrite": boolean("replace an existing file"), "note": note.clone()
            }), &["name", "width", "height"]),
        tool("get_document", "Summary (named palette, layers with locks, frames with their tag/n addresses, tags with play order, regions) and the pixels of each cel as palette-key rows, cropped to the non-transparent area, with rulers giving absolute x and row labels giving absolute y. Filter with layer/frame to keep the output short.",
            json!({"path": path, "layer": {"type": ["integer", "string"], "description": "only this layer"},
                   "frame": {"type": ["integer", "string"], "description": "only this frame (index, name or tag/n)"},
                   "pixels": boolean("include pixel rows; default true"),
                   "crop": boolean("crop each cel to its content; default true (false = full canvas)")}), &["path"]),
        tool("get_rows", "Read a rectangle (x,y,w,h) or a named region of one cel, or of the composite of visible layers, as palette-key rows with absolute coordinate rulers and a legend of the colors used.",
            json!({"path": path, "layer": {"type": ["integer", "string"], "description": "layer (default 0)"},
                   "frame": {"type": ["integer", "string"], "description": "frame (default 0)"},
                   "region": {"type": "string", "description": "read this region's rect on that frame"},
                   "rect": {"type": "object", "description": "read {x, y, w, h} (alternative to x/y/w/h)"},
                   "x": int("left, default 0"), "y": int("top, default 0"),
                   "w": int("width, default to the right edge"), "h": int("height, default to the bottom edge"),
                   "composite": boolean("topmost visible key over all layers instead of one layer")}), &["path"]),
        tool("set_pixels", "Set individual pixels of one cel.", write(json!({
                "pixels": {"type": "array", "items": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "key": {"type": "string"}}, "required": ["x", "y", "key"]}}
            })), &w_req(&["pixels"])),
        tool("set_rows", "Write rows of palette keys with the top-left corner at (x, y) (default: the region's/clip's corner). '-' leaves a pixel unchanged, '.' makes it transparent. Nothing is written if any key is invalid.",
            write(json!({"x": int("left column"), "y": int("top row"),
                       "rows": {"type": "array", "items": {"type": "string"}}})), &w_req(&["rows"])),
        tool("draw_line", "Bresenham line, endpoints inclusive, 1 px wide.", write(json!({
                "x0": int("start x"), "y0": int("start y"), "x1": int("end x"), "y1": int("end y"), "key": key})),
            &w_req(&["x0", "y0", "x1", "y1", "key"])),
        tool("draw_rect", "Rectangle outline (1 px) or filled (optionally with a dither pattern).", write(json!({
                "x": int("left"), "y": int("top"), "w": int("width"), "h": int("height"), "key": key,
                "filled": boolean("default false"), "dither": dither.clone()})),
            &w_req(&["x", "y", "w", "h"])),
        tool("draw_ellipse", "Ellipse outline (1 px) or filled, centred on pixel (cx, cy) with radii rx, ry: covers x cx-rx..cx+rx and y cy-ry..cy+ry. Circle: rx = ry.", write(json!({
                "cx": int("centre x"), "cy": int("centre y"), "rx": int("horizontal radius"), "ry": int("vertical radius"), "key": key,
                "filled": boolean("default false")})),
            &w_req(&["cx", "cy", "rx", "ry", "key"])),
        tool("fill", "Flood fill (4-neighbourhood) of the same-key area around (x, y); never spreads outside the region/clip. With dither the area gets a two-color pattern instead of key.", write(json!({"x": int("x"), "y": int("y"), "key": key, "dither": dither.clone()})),
            &w_req(&["x", "y"])),
        tool("clear", "Make the target transparent: the region/clip, or the whole cel.", write(json!({})), &w_req(&[])),
        tool("copy_region", "Copy the pixels of a region (or clip rect) to an offset and/or another frame/layer. mode over (default): transparent source pixels keep the destination; replace: copy them too.", write(json!({
                "dx": int("x offset"), "dy": int("y offset"),
                "to_frame": {"type": ["integer", "string"], "description": "destination frame (default same)"},
                "to_layer": {"type": ["integer", "string"], "description": "destination layer (default same)"},
                "mode": {"type": "string", "enum": ["over", "replace"]}})), &w_req(&[])),
        tool("move_region", "Move the pixels of a region (or clip rect) by dx/dy (vacated pixels become transparent), optionally to another frame/layer. A named region's rect moves along on that frame (update_region, default true).", write(json!({
                "dx": int("x offset"), "dy": int("y offset"),
                "to_frame": {"type": ["integer", "string"], "description": "destination frame (default same)"},
                "to_layer": {"type": ["integer", "string"], "description": "destination layer (default same)"},
                "mode": {"type": "string", "enum": ["over", "replace"]},
                "update_region": boolean("move the region's rect too (same frame only), default true")})), &w_req(&[])),
        tool("transform", "Mirror or rotate pixels, with diff and history like the other writes. scope rect (rect/clip) or region (region): that area of one cel (layer + frame), turned around its centre; a non-square area turned by 90° occupies the turned box on the same centre. scope cel: the whole cel (layer + frame), around the canvas centre. scope frame (frame) / frames (frames [..] or from/to) / tag (tag) / all: whole frames, all layers or only layer, around the canvas centre; each linked cel is turned once, region rects and markers on those frames follow (locked ones stay). Pixels that leave the canvas are cut off and counted. Locked layers/frames/regions refuse.", json!({
                "path": path,
                "op": {"type": "string", "enum": ["flip_h", "flip_v", "rot90", "rot180", "rot270"], "description": "flip_h = mirror left/right, flip_v = top/bottom, rot90 = 90° clockwise, rot270 = 90° counter-clockwise"},
                "scope": {"type": "string", "enum": ["rect", "region", "cel", "frame", "frames", "tag", "all"]},
                "layer": {"type": ["integer", "string"], "description": "layer (required for rect/region/cel; for whole frames: only this layer)"},
                "frame": {"type": ["integer", "string"], "description": "frame (rect/region/cel/frame)"},
                "region": {"type": "string", "description": "scope region: this region's rect on that frame"},
                "clip": {"type": "object", "description": "scope rect: {x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}},
                "rect": {"type": "object", "description": "same as clip", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}},
                "frames": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "scope frames: these frames"},
                "from": {"type": ["integer", "string"], "description": "scope frames: first frame"},
                "to": {"type": ["integer", "string"], "description": "scope frames: last frame (inclusive)"},
                "tag": {"type": "string", "description": "scope tag: the frames of this tag"},
                "edit_linked": boolean("allow changing cels linked to frames outside the target"),
                "dry_run": dry.clone(), "note": note.clone(),
                "pace_ms": int("show the change row by row in an open editor, this many ms apart")}), &["path", "op", "scope"]),
        tool("replace_color", "Replace one color by another everywhere in the scope (with diff and history). Pixels in locked regions stay and are counted; locked layers/frames refuse.", scoped(json!({
                "from": {"type": "string", "description": "palette key or color name to replace ('.' = transparent)"},
                "to": {"type": "string", "description": "palette key or color name it becomes ('.' = transparent)"}})), &["path", "from", "to"]),
        tool("outline", "Draw a 1-px outline around the non-transparent pixels of each cel in the scope (with diff and history). mode outside (default): on the pixels around the shape (may lie 1 px outside a rect/region); inside: on the shape's own edge pixels (the canvas edge counts as outside). connectivity 4 (default, corners stay free) or 8 (diagonals too). only_transparent (outside only): pixels that already have a color stay. Pixels in locked regions stay and are counted.", scoped(json!({
                "color": {"type": "string", "description": "palette key or color name of the outline"},
                "mode": {"type": "string", "enum": ["outside", "inside"]},
                "connectivity": {"type": "integer", "enum": [4, 8]},
                "only_transparent": boolean("outside only: draw only on transparent pixels (default false)")})), &["path", "color"]),
        tool("mirror_document", "Write a mirrored copy of a document (e.g. walk_left from walk_right): every frame mirrored around the canvas centre; tags, durations, linked cels, layers, locks and regions are kept, region rects and markers are mirrored too. The source is not changed.", json!({
                "src": {"type": "string", "description": "source document path relative to the workdir"},
                "dst": {"type": "string", "description": "new document name (\".pxe.json\" is appended unless given; .aseprite works too)"},
                "op": {"type": "string", "enum": ["flip_h", "flip_v"], "description": "flip_h (default, left/right) or flip_v"},
                "overwrite": boolean("replace an existing file"), "dry_run": dry.clone(), "note": note.clone()}), &["src", "dst"]),
        tool("list_regions", "List named regions with their rects per frame range, layer restriction and lock state. With frame: the rects on that frame.",
            json!({"path": path, "frame": {"type": ["integer", "string"], "description": "only show rects on this frame"}}), &["path"]),
        tool("set_region", "Create or update a named region (a rect the user and the AI assistant use to name a part, e.g. \"left_shoe\"). Without frame/from+to/tag the rect applies to all frames; with them only to those frames (other frames keep their rect). layer restricts it to one layer. locked protects its pixels from every edit. kind marker makes it a named point (origin, hand anchor): locked then protects only its position and name, never the pixels; it can be used as a render_animation track anchor.", meta(json!({
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": ["area", "marker"], "description": "area (default): a rect; locked protects its pixels. marker: a point (x, y only); locked protects only its position/name, never pixels"},
                "x": int("left (marker: point x)"), "y": int("top (marker: point y)"), "w": int("width (not for markers)"), "h": int("height (not for markers)"),
                "frame": {"type": ["integer", "string"], "description": "only this frame"},
                "from": {"type": ["integer", "string"]}, "to": {"type": ["integer", "string"]},
                "tag": {"type": "string", "description": "only the frames of this tag"},
                "hide": boolean("hide the region on the given frames"),
                "layer": {"type": ["integer", "string", "null"], "description": "restrict to a layer; \"all\" or null = all layers"},
                "locked": boolean("lock/unlock"), "color": {"type": "string", "description": "overlay color #rrggbb"},
                "rename": {"type": "string"}})), &["path", "name"]),
        tool("delete_region", "Delete a region definition (pixels stay).", meta(json!({"name": {"type": "string"}})), &["path", "name"]),
        tool("add_layer", "Add an empty layer. Default position: on top.", meta(json!({
                "name": {"type": "string"}, "index": int("position, 0 = bottom; default top")})), &["path"]),
        tool("set_layer", "Rename, show/hide, set opacity, lock/unlock or move a layer. Locked layers refuse all pixel edits.", meta(json!({
                "layer": layer, "name": {"type": "string"}, "visible": boolean("show or hide"),
                "opacity": {"type": "number", "description": "0..1"}, "locked": boolean("lock/unlock"), "move_to": int("new index")})), &["path", "layer"]),
        tool("delete_layer", "Delete a layer (not the last one, not a locked one).", meta(json!({"layer": layer})), &["path", "layer"]),
        tool("add_frame", "Add a frame on all layers: empty, a copy of another frame, or linked to another frame (linked cels share pixels: editing one edits both, like Aseprite). Default position: at the end. Tags spanning the insert position grow.", meta(json!({
                "copy_from": {"type": ["integer", "string"], "description": "frame to copy"},
                "linked": boolean("link instead of copying (needs copy_from)"),
                "index": int("insert position; default end"), "duration_ms": int("duration"), "name": {"type": "string"}})), &["path"]),
        tool("delete_frame", "Delete a frame (not the last one, not a locked one). Later frame indices shift down; tags and region keys follow.",
            meta(json!({"frame": frame})), &["path", "frame"]),
        tool("set_frame_duration", "Set how long a frame is shown, in milliseconds. frame may be \"all\".", meta(json!({
                "frame": frame, "duration_ms": int("milliseconds")})), &["path", "frame", "duration_ms"]),
        tool("set_frame", "Rename a frame (unique name), set its duration, lock/unlock it, or move it (tags keep their index ranges).", meta(json!({
                "frame": frame, "name": {"type": "string"}, "duration_ms": int("milliseconds"), "locked": boolean("lock/unlock"), "move_to": int("new index")})), &["path", "frame"]),
        tool("link_cel", "Link a frame's cel to another frame (they then share pixels), or unlink it (give it its own copy). All layers unless layer is given.", meta(json!({
                "frame": frame, "to": {"type": ["integer", "string"], "description": "frame to link with"},
                "unlink": boolean("give the cel its own copy instead"), "layer": {"type": ["integer", "string"], "description": "only this layer; default all layers"}})), &["path", "frame"]),
        tool("set_tag", "Create or update an animation tag: a frame range from..to with a direction (forward, reverse, pingpong, pingpong_reverse), like Aseprite. With frames (any order, e.g. [0,1,0,2]) the needed linked frames are appended and the tag covers them. Updating keeps unspecified fields.", meta(json!({
                "name": {"type": "string"},
                "from": {"type": ["integer", "string"], "description": "first frame"},
                "to": {"type": ["integer", "string"], "description": "last frame (inclusive)"},
                "frames": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "arbitrary frame order (built from linked frames)"},
                "direction": {"type": "string", "enum": ["forward", "reverse", "pingpong", "pingpong_reverse"]},
                "loop": boolean("loop (default) or play once"),
                "color": {"type": "string", "description": "timeline color #rrggbb"},
                "delete": boolean("delete the tag")})), &["path", "name"]),
        tool("set_palette_color", "Add or change a palette color (\"#rrggbb\"/\"#rrggbbaa\") and/or its name (e.g. \"harness\"), or remove it (its pixels become transparent).", meta(json!({
                "key": {"type": "string", "description": "palette key (one character)"}, "color": {"type": "string"},
                "name": {"type": "string", "description": "color name, at least 2 characters; \"\" removes it"},
                "remove": boolean("remove the key")})), &["path", "key"]),
        tool("set_ramps", "Set the color ramps of a document (replaces all; [] removes them): ordered rows of palette colors from dark to light, e.g. crust 3 > 8 > 9 > a. shade and the editor's Schattieren tool step pixels along them. Stored in .pxe.json only. get_document lists them.", meta(json!({
                "ramps": {"type": "array", "description": "the ramps, each {name, keys}", "items": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "keys": {"type": ["string", "array"], "items": {"type": "string"}, "description": "dark to light: a key string (\"389a\") or keys/color names"}},
                    "required": ["name", "keys"]}}})), &["path", "ramps"]),
        tool("shade", "Shade along the color ramps: each target pixel becomes the color steps places lighter (+) or darker (-) in its ramp, stopping at the ramp's end. Pixels whose color is in no ramp (and transparent ones) stay unchanged; each pixel changes at most once. Target: pixels, or every pixel of rect/region.", write(json!({
                "steps": int("+n = n colors lighter, -n = n colors darker"),
                "pixels": {"type": "array", "description": "pixels to shade (instead of a whole rect/region)", "items": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}, "required": ["x", "y"]}}})),
            &w_req(&["steps"])),
        tool("render_preview", "Render frames to preview/<name>.png and return the image: per frame a label (index, name, ms, tags), coordinate rulers, the zoomed frame with pixel grid (brighter every 8 px) and region outlines with names, 1x and 2x thumbnails; palette swatches with keys. Nearest-neighbour only. Look at it before calling art done. On a tilemap (.tilemap.json): the whole map with objects, tile grid (grid) and cell numbers, zoom default 1 (preview/<name>_map.png).", json!({
                "path": path, "frame": {"type": ["integer", "string"], "description": "one frame; default all"},
                "frames": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "several frames"},
                "layer": {"type": ["integer", "string"], "description": "show only this layer"},
                "zoom": int("screen pixels per art pixel; default fits ~512 px"), "grid": boolean("default true"),
                "regions": boolean("draw region outlines, default true (false when tiled)"),
                "tiled": boolean("show each frame repeated 3x3 like a seamless tile (default false); grid off unless asked"),
                "inline": boolean("also return the PNG as image content; default true")}), &["path"]),
        tool("check_seams", "Check whether tiles repeat seamlessly: the rightmost column is compared with the leftmost, the bottom row with the top row, as they meet when the frame is repeated. Reports per seam how many pairs differ and which are hard edges (color distance >= threshold, as runs with both keys), judged against the hard edges inside the tile: VISIBLE SEAM, or no more contrast than inside. Read-only.", json!({
                "path": path, "frame": {"type": ["integer", "string"], "description": "one frame; default all"},
                "layer": {"type": ["integer", "string"], "description": "only this layer; default the visible composite"},
                "threshold": {"type": "number", "description": "color distance 0..1 from which a pair is a hard edge; default 0.25"}}), &["path"]),
        tool("render_animation", "Review an animation as still images: one tag in real play order (direction/pingpong expanded, loop transition shown) as a filmstrip, an onion-skin row and a difference row (changed pixels highlighted with counts), plus a text analysis: per-step changed pixels and silhouette shift, total duration, and warnings (ground line changes, silhouette jumps, isolated/new orphan pixels, identical frames). region zooms into an area; track marks an area's opaque bbox per step and plots its centre path (feet, head, sword tip).", json!({
                "path": path, "tag": {"type": "string", "description": "tag name"},
                "region": {"description": "only this area, enlarged: region name, \"x,y,w,h\" or {x,y,w,h}"},
                "track": {"description": "area to track: region name, \"x,y,w,h\" or {x,y,w,h}"},
                "zoom": int("screen pixels per art pixel; default: cells ~200 px"),
                "jump_threshold": int("silhouette edge moves above this many px are warnings; default 2"),
                "inline": boolean("also return the PNG as image content; default true")}), &["path", "tag"]),
        tool("focus", "Live hint for an open editor (follow mode): switch to the document's tab, jump to a frame/layer and show an area, with a banner, without changing anything. A document that is not open in the editor is not opened (use open_editor). Use it before drawing, e.g. \"now working on fist_left\".", json!({
                "path": path, "frame": frame, "layer": {"type": ["integer", "string"], "description": "layer (default 0)"},
                "region": {"type": "string", "description": "show this region"},
                "rect": {"type": "object", "description": "show {x, y, w, h}"},
                "note": {"type": "string", "description": "banner text; default \"<assistant>: arbeitet an <region> (<frame> · <layer>)\""}}), &["path", "frame"]),
        tool("get_user_view", "What the user sees in the pixel-editor GUI right now: for every open editor on a document in the workdir the document, frame (index, name, tag/n), layer, selection with its pixels (get_rows of its box, from the file; a shaped selection made with the wand, selection brush or lasso shows unselected pixels as '·', so only the listed pixels are meant), the pixel under the cursor (key, color, regions), zoom and whether it follows the assistant's edits; plus all \"KI zeigen\" marks of every document, grouped by frame (number, rect or pixel, layer, note and the pixels). Call it when the user refers to \"this\", \"here\", \"the marks\" or their selection. Says clearly when no editor is open. Remove handled marks with clear_marks.", json!({}), &[]),
        tool("clear_marks", "Remove \"KI zeigen\" marks the user made (see get_user_view) once you have handled them: all marks of the document, those of one frame, or the given numbers. The editor updates live.", json!({
                "path": path,
                "frame": {"type": ["integer", "string"], "description": "only marks on this frame (index, name or tag/n)"},
                "numbers": {"type": "array", "items": {"type": "integer"}, "description": "only these mark numbers (#1, #2 ...)"}}), &["path"]),
        tool("open_editor", "Open a document in the pixel-editor GUI on the user's screen. If an editor window for this workdir is running, the document opens there as a tab (or its tab is selected); otherwise a new window starts (detached). The editor follows the assistant's edits live; edits to a background tab mark the tab without switching to it.", json!({
                "path": path, "dry_run": boolean("only report the command")}), &["path"]),
        tool("export", "Export for engines/tools. png_sheet: sheet PNG + JSON (pixelart-studio shape). godot: sheet PNG + SpriteFrames .tres (one animation per tag). unity: sheet PNG + .png.meta + .anim.json. gif: one GIF per tag. aseprite: .aseprite file (regions become slices). godot_tileset: a tileset document as atlas PNG (8 tiles per row) + Godot 4 TileSet .tres (tile names in custom data \"name\"). On a tilemap: godot = atlas + TileSet + <name>.tscn with a TileMapLayer and a Sprite2D per object; world_txt = world.txt. Default out_dir: export/<name>/.", json!({
                "path": path, "target": {"type": "string", "enum": ["png_sheet", "godot", "unity", "gif", "aseprite", "godot_tileset", "world_txt"]},
                "out_dir": {"type": "string", "description": "directory relative to the workdir"},
                "scale": int("integer upscale (default 1, gif 4)"), "columns": int("sheet columns (default up to 16 per row)"),
                "godot_res_dir": {"type": "string", "description": "res:// directory the PNG will live in; default: path relative to the .tres"},
                "pixels_per_unit": int("Unity pixels per unit, default 32"),
                "color_mode": {"type": "string", "enum": ["indexed", "rgba"], "description": "aseprite color mode, default indexed"}}), &["path", "target"]),
        tool("save_as", "Write a document under a new name; the extension picks the format (.pxe.json or .aseprite).", json!({
                "path": path, "to": {"type": "string", "description": "new file name relative to the workdir"},
                "overwrite": boolean("replace an existing file")}), &["path", "to"]),
        tool("import_png", "Create a document from a PNG; with tile_w/tile_h a sheet is cut into frames (left to right, top to bottom). Colors are kept exactly.", json!({
                "png_path": {"type": "string", "description": "PNG path relative to the workdir"},
                "name": {"type": "string", "description": "new document name"},
                "tile_w": int("tile width"), "tile_h": int("tile height (default = tile_w)"),
                "duration_ms": int("frame duration, default 100"), "overwrite": boolean("replace an existing file")}), &["png_path", "name"]),
        tool("history", "Edit history of a document (MCP and GUI edits with their notes), newest first.", json!({"path": path, "limit": int("entries, default 20")}), &["path"]),
        tool("undo", "Undo the last edit(s) recorded in the history (also GUI saves). Repeated calls step further back.", json!({"path": path, "steps": int("default 1"), "dry_run": dry}), &["path"]),
        tool("snapshot", "Save the current state under a name, to compare or roll back later.", json!({"path": path, "name": {"type": "string"}, "note": {"type": "string"}}), &["path", "name"]),
        tool("list_snapshots", "List named snapshots.", json!({"path": path}), &["path"]),
        tool("diff_snapshot", "Compare a snapshot with the current document: structure changes and changed pixels per cel (count + bbox); with layer and frame also rows before/after.", json!({
                "path": path, "name": {"type": "string"}, "layer": {"type": ["integer", "string"]}, "frame": {"type": ["integer", "string"]}}), &["path", "name"]),
        tool("restore_snapshot", "Restore a snapshot (recorded in the history, so undo reverts it).", json!({"path": path, "name": {"type": "string"}, "dry_run": dry, "note": note}), &["path", "name"]),
    ])
}

/// Call a tool in-process (for tests elsewhere in the crate).
#[cfg(test)]
pub(crate) fn test_call(workdir: &Path, name: &str, args: Value) -> (bool, String) {
    let s = Server::new(workdir.canonicalize().unwrap());
    s.handle(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"clientInfo": {"name": "claude-code"}}}));
    let r = s
        .handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}),
        )
        .unwrap();
    let res = &r["result"];
    (res["isError"] == false, res["content"][0]["text"].as_str().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests;
mod tilemap;
