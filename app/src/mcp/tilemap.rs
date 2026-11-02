//! MCP tools for tilemaps (`.tilemap.json`, see `pixel_core::tilemap`):
//! create, read, set tiles with diff and history, edit the legend, resize,
//! world.txt import/export, preview, Godot export, history and undo.

use std::path::{Path, PathBuf};

use pixel_core::document::Rect;
use pixel_core::export::godot_tiles;
use pixel_core::journal::{Focus, Journal};
use pixel_core::tilemap::{self, Assets, LegendEntry, TileMap, Tileset, EMPTY};
use pixel_core::{diff::rows_block, io};
use serde_json::{json, Value};

use super::{bool_arg, int_of, note_arg, opt_int, rect_arg, str_arg, Out, Server, DIFF_MAX_ROWS};

impl Server {
    /// Handle a tilemap tool, or a general tool called on a map path.
    pub(super) fn tilemap_call(&self, name: &str, a: &Value) -> Option<Result<Out, String>> {
        let on_map = a
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|p| self.resolve(p).is_ok_and(|p| tilemap::is_tilemap_path(&p)));
        Some(match name {
            "create_tilemap" => self.create_tilemap(a),
            "get_tilemap" => self.get_tilemap(a),
            "set_tiles" => self.set_tiles(a),
            "set_legend" => self.set_legend(a),
            "resize_tilemap" => self.resize_tilemap(a),
            "import_world" => self.import_world(a),
            "export_world" => self.export_world(a),
            "render_preview" if on_map => self.map_preview(a),
            "export" if on_map => self.map_export(a),
            "export" if a.get("target").and_then(Value::as_str) == Some("godot_tileset") => self.tileset_export(a),
            "history" | "undo" if on_map => self.map_history(name, a),
            _ if on_map && name != "open_editor" => Err(format!(
                "{name} works on pixel documents; {} is a tilemap: use get_tilemap, set_tiles, set_legend, \
                 resize_tilemap, render_preview, export, export_world, history, undo or open_editor",
                a["path"].as_str().unwrap_or("")
            )),
            _ => return None,
        })
    }

    fn load_map(&self, a: &Value) -> Result<(PathBuf, TileMap), String> {
        let rel = str_arg(a, "path")?;
        let path = self.resolve(rel)?;
        if !tilemap::is_tilemap_path(&path) {
            return Err(format!("{rel:?} is not a tilemap ({} file)", tilemap::EXTENSION));
        }
        if !path.exists() {
            return Err(format!("{rel:?} does not exist in the workdir (list_documents shows what is there)"));
        }
        Ok((path.clone(), tilemap::load(&path)?))
    }

    fn new_map_path(&self, name: &str, overwrite: bool) -> Result<PathBuf, String> {
        let file =
            if name.ends_with(tilemap::EXTENSION) { name.to_string() } else { format!("{name}{}", tilemap::EXTENSION) };
        let path = self.resolve(&file)?;
        if path.exists() && !overwrite {
            return Err(format!("{} already exists (pass overwrite: true to replace it)", self.rel(&path)));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        Ok(path)
    }

    /// Save a changed map and journal it (with the state before, if new).
    fn commit_map(
        &self,
        path: &Path,
        before: &TileMap,
        map: &TileMap,
        note: &str,
        bbox: Option<Rect>,
    ) -> Result<(), String> {
        let journal = Journal::for_document(path);
        journal.ensure_current(before, "state before the next edit (first edit or changed outside the editor)")?;
        tilemap::save(map, path)?;
        let focus =
            bbox.map(|b| Focus { frame: 0, layer: 0, bbox: Some([b.x, b.y, b.w as i32, b.h as i32]), region: None });
        journal.record_edit(map, &self.author(), note, focus)?;
        Ok(())
    }

    fn tileset_of(&self, map: &TileMap, path: &Path) -> Option<Tileset> {
        Tileset::load(&map.tileset_path(path)).ok()
    }

    fn create_tilemap(&self, a: &Value) -> Result<Out, String> {
        let ts_rel = str_arg(a, "tileset")?;
        let ts_path = self.resolve(ts_rel)?;
        let ts = Tileset::load(&ts_path)?;
        let path = self.new_map_path(str_arg(a, "name")?, bool_arg(a, "overwrite", false))?;
        let (w, h) = (int_of(a, "width")?.max(0) as u32, int_of(a, "height")?.max(0) as u32);
        let mut map = TileMap::new(&tilemap::relative_to_map(&path, &ts_path), ts.tile_w, ts.tile_h, w, h)?;
        if let Some(fill) = a.get("fill").and_then(Value::as_str) {
            let k = map.find_key(fill, Some(&ts))?;
            map.fill_rect(Rect::new(0, 0, w, h), k);
        }
        tilemap::save(&map, &path)?;
        Journal::for_document(&path).record(
            &map,
            &self.author(),
            &format!("{}: create_tilemap {w}x{h}", self.author()),
        )?;
        Ok(Out::Text(format!(
            "created {} ({w}x{h} cells of {}x{} px, tileset {}, {} tiles: {})",
            self.rel(&path),
            ts.tile_w,
            ts.tile_h,
            map.tileset,
            ts.names.len(),
            ts.names.join(", ")
        )))
    }

    fn get_tilemap(&self, a: &Value) -> Result<Out, String> {
        let (path, map) = self.load_map(a)?;
        let assets = Assets::load(&map, &path);
        let r = match (opt_int(a, "x")?, opt_int(a, "y")?, opt_int(a, "w")?, opt_int(a, "h")?) {
            (None, None, None, None) => Rect::new(0, 0, map.width, map.height),
            (x, y, w, h) => Rect::new(
                x.unwrap_or(0),
                y.unwrap_or(0),
                w.unwrap_or(map.width as i32).max(1) as u32,
                h.unwrap_or(map.height as i32).max(1) as u32,
            ),
        };
        let mut s = format!(
            "{}: {}x{} cells, tiles {}x{} px, tileset {}\n",
            self.rel(&path),
            map.width,
            map.height,
            map.tile_w,
            map.tile_h,
            map.tileset
        );
        for w in &assets.warnings {
            s += &format!("WARNING: {w}\n");
        }
        s += "legend (key name (tile + object)); ' ' = empty:\n";
        for e in &map.legend {
            let n = map.cells.iter().filter(|c| **c == e.key).count();
            s += &format!("  {}  x{n}\n", e.label());
        }
        let used: Vec<&str> = map.legend.iter().filter_map(|e| e.tile.as_deref()).collect();
        let free: Vec<&str> = assets.tileset.names.iter().map(String::as_str).filter(|n| !used.contains(n)).collect();
        if !free.is_empty() {
            s += &format!(
                "tiles of the tileset not in the legend yet (usable by name in set_tiles): {}\n",
                free.join(", ")
            );
        }
        if !map.world_lines.is_empty() {
            s += &format!("imported from world.txt ({} comment lines kept for export_world)\n", map.world_lines.len());
        }
        s += &format!("cells {}:\n", r.describe());
        s += &rows_block(|x, y| map.get(x, y), r);
        Ok(Out::Text(s))
    }

    fn set_tiles(&self, a: &Value) -> Result<Out, String> {
        let (path, before) = self.load_map(a)?;
        let ts = self.tileset_of(&before, &path);
        let mut map = before.clone();
        let mut off = 0;
        let mut given = 0;
        if let Some(cells) = a.get("cells") {
            let cells = cells.as_array().ok_or("cells must be an array of {x, y, tile}")?;
            for (i, c) in cells.iter().enumerate() {
                let at = |e: String| format!("cells[{i}]: {e}");
                let (x, y) = (int_of(c, "x").map_err(at)?, int_of(c, "y").map_err(at)?);
                let t = c.get("tile").and_then(Value::as_str).ok_or_else(|| at("missing \"tile\"".into()))?;
                let k = map.find_key(t, ts.as_ref()).map_err(at)?;
                if !map.in_bounds(x, y) {
                    off += 1;
                }
                map.set(x, y, k);
                given += 1;
            }
        }
        if let Some(rows) = a.get("rows") {
            let rows = rows.as_array().ok_or("rows must be an array of strings")?;
            let (x0, y0) = (opt_int(a, "x")?.unwrap_or(0), opt_int(a, "y")?.unwrap_or(0));
            for (dy, r) in rows.iter().enumerate() {
                let r = r.as_str().ok_or(format!("rows[{dy}] must be a string"))?;
                for (dx, c) in r.chars().enumerate() {
                    if c == '-' {
                        continue; // keep this cell
                    }
                    let k = if c == ' ' { EMPTY } else { c as u8 };
                    if !c.is_ascii() || (k != EMPTY && map.entry(k).is_none()) {
                        return Err(format!(
                            "rows[{dy}][{dx}]: {c:?} is not in the legend (set_legend adds keys; '-' keeps a cell)"
                        ));
                    }
                    let (x, y) = (x0 + dx as i32, y0 + dy as i32);
                    off += !map.in_bounds(x, y) as usize;
                    map.set(x, y, k);
                    given += 1;
                }
            }
        }
        if let Some(r) = a.get("rect") {
            let rect = rect_arg(r)?;
            let t = a.get("tile").and_then(Value::as_str).ok_or("rect needs \"tile\"")?;
            let k = map.find_key(t, ts.as_ref())?;
            map.fill_rect(rect, k);
            given += (rect.w * rect.h) as usize;
        }
        if given == 0 {
            return Err("give cells [{x, y, tile}], rows (+ x, y) or rect + tile".into());
        }
        map.validate()?;
        let mut text = tilemap::describe_diff(&before, &map, DIFF_MAX_ROWS);
        if off > 0 {
            text += &format!("{off} cell(s) outside the map ignored\n");
        }
        let added: Vec<String> =
            map.legend.iter().filter(|e| before.entry(e.key).is_none()).map(LegendEntry::label).collect();
        if !added.is_empty() {
            text += &format!("legend: added {}\n", added.join(", "));
        }
        let (n, bb) = tilemap::cell_diff(&before, &map);
        let size = bb.map(|b| format!(" ({}x{})", b.w, b.h)).unwrap_or_default();
        let note = note_arg(a, || format!("{}: set_tiles {n} Zellen{size}", self.author()));
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written. Would record: {note:?}\n{text}")));
        }
        if n == 0 && added.is_empty() {
            return Ok(Out::Text(format!("nothing changed, nothing written\n{text}")));
        }
        self.commit_map(&path, &before, &map, &note, bb)?;
        Ok(Out::Text(format!("{text}recorded as {note:?} [{}]", self.rel(&path))))
    }

    fn set_legend(&self, a: &Value) -> Result<Out, String> {
        let (path, before) = self.load_map(a)?;
        let mut map = before.clone();
        let key = str_arg(a, "key")?;
        if key.len() != 1 || !tilemap::is_valid_key(key.as_bytes()[0]) {
            return Err(format!("key {key:?} must be one printable ASCII character other than space ; \" \\"));
        }
        let k = key.as_bytes()[0];
        let what = if bool_arg(a, "remove", false) {
            let used = map.cells.iter().filter(|c| **c == k).count();
            if used > 0 {
                return Err(format!("{key:?} is used by {used} cell(s); change them with set_tiles first"));
            }
            map.legend.retain(|e| e.key != k);
            format!("removed {key:?}")
        } else {
            let existed = map.entry(k).is_some();
            if !existed {
                map.legend.push(LegendEntry { key: k, tile: None, object: None, name: String::new() });
            }
            let e = map.legend.iter_mut().find(|e| e.key == k).unwrap();
            let opt = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
            if let Some(t) = a.get("tile") {
                e.tile = opt(t);
            }
            if let Some(o) = a.get("object") {
                e.object = opt(o);
            }
            if let Some(n) = a.get("name").and_then(Value::as_str) {
                e.name = n.into();
            }
            format!("{} {}", if existed { "changed" } else { "added" }, e.label())
        };
        let mut text = what.clone();
        for w in Assets::load(&map, &path).warnings {
            text += &format!("\nWARNING: {w}");
        }
        let note = note_arg(a, || format!("{}: set_legend {what}", self.author()));
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written. Would record: {note:?}\n{text}")));
        }
        if map == before {
            return Ok(Out::Text(format!("nothing changed\n{text}")));
        }
        self.commit_map(&path, &before, &map, &note, None)?;
        Ok(Out::Text(format!("{text}\nrecorded as {note:?}")))
    }

    fn resize_tilemap(&self, a: &Value) -> Result<Out, String> {
        let (path, before) = self.load_map(a)?;
        let ts = self.tileset_of(&before, &path);
        let mut map = before.clone();
        let fill = match a.get("fill").and_then(Value::as_str) {
            Some(f) => map.find_key(f, ts.as_ref())?,
            None => EMPTY,
        };
        let (w, h) = (int_of(a, "width")?.max(0) as u32, int_of(a, "height")?.max(0) as u32);
        map.resize(w, h, fill)?;
        let lost = (0..before.height as i32)
            .flat_map(|y| (0..before.width as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| !map.in_bounds(x, y) && before.get(x, y) != EMPTY)
            .count();
        let text = format!(
            "{}x{} -> {w}x{h} (top-left corner kept){}",
            before.width,
            before.height,
            if lost > 0 { format!("; {lost} non-empty cell(s) cut off") } else { String::new() }
        );
        let note = note_arg(a, || format!("{}: resize_tilemap {w}x{h}", self.author()));
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN, nothing written. Would record: {note:?}\n{text}")));
        }
        self.commit_map(&path, &before, &map, &note, Some(Rect::new(0, 0, w, h)))?;
        Ok(Out::Text(format!("{text}\nrecorded as {note:?}")))
    }

    fn import_world(&self, a: &Value) -> Result<Out, String> {
        let src = self.resolve(str_arg(a, "src")?)?;
        let ts_path = self.resolve(str_arg(a, "tileset")?)?;
        let ts = Tileset::load(&ts_path)?;
        let path = self.new_map_path(str_arg(a, "name")?, bool_arg(a, "overwrite", false))?;
        let text = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", self.rel(&src)))?;
        let (map, notes) =
            tilemap::import_world(&text, &tilemap::relative_to_map(&path, &ts_path), (ts.tile_w, ts.tile_h))?;
        tilemap::save(&map, &path)?;
        Journal::for_document(&path).record(
            &map,
            &self.author(),
            &format!("{}: import_world {}", self.author(), self.rel(&src)),
        )?;
        let mut s = format!(
            "imported {} as {} ({}x{} cells, {} legend entries; tiles from SANDBOX_TILES, the game adds edges, cliff faces and decor itself)\n",
            self.rel(&src),
            self.rel(&path),
            map.width,
            map.height,
            map.legend.len()
        );
        for e in &map.legend {
            s += &format!("  {}\n", e.label());
        }
        for n in notes.iter().chain(&Assets::load(&map, &path).warnings) {
            s += &format!("NOTE: {n}\n");
        }
        Ok(Out::Text(s))
    }

    fn export_world(&self, a: &Value) -> Result<Out, String> {
        let (path, map) = self.load_map(a)?;
        let to = self.resolve(str_arg(a, "to")?)?;
        if to.exists() && !bool_arg(a, "overwrite", false) {
            return Err(format!("{} already exists (pass overwrite: true to replace it)", self.rel(&to)));
        }
        let (text, warnings) = tilemap::export_world(&map);
        io::write_atomic(&to, text.as_bytes())?;
        let mut s = format!("wrote {} as {} ({} lines)", self.rel(&path), self.rel(&to), text.lines().count());
        for w in warnings {
            s += &format!("\nWARNING: {w}");
        }
        Ok(Out::Text(s))
    }

    fn map_preview(&self, a: &Value) -> Result<Out, String> {
        let (path, map) = self.load_map(a)?;
        let assets = Assets::load(&map, &path);
        let zoom = opt_int(a, "zoom")?.map(|z| z.clamp(1, 16) as u32).unwrap_or(1);
        let img = tilemap::preview(&map, &assets, zoom, bool_arg(a, "grid", true));
        let dir = self.workdir.join("preview");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join(format!("{}_map.png", tilemap::stem(&path)));
        let png = img.encode_png()?;
        io::write_atomic(&out, &png)?;
        let mut text = format!(
            "wrote {} ({}x{} px, zoom {zoom}): the whole map, objects standing on their cells, tile grid, cell numbers every 5 along the top and left. Keys without a tile show as purple placeholders with their key.",
            self.rel(&out),
            img.width,
            img.height
        );
        for w in &assets.warnings {
            text += &format!("\nWARNING: {w}");
        }
        if bool_arg(a, "inline", true) {
            Ok(Out::TextAndImage(text, png))
        } else {
            Ok(Out::Text(text))
        }
    }

    fn map_export(&self, a: &Value) -> Result<Out, String> {
        let (path, map) = self.load_map(a)?;
        let stem = tilemap::stem(&path);
        let out_dir = match a.get("out_dir").and_then(Value::as_str) {
            Some(d) => self.resolve(d)?,
            None => self.workdir.join("export").join(&stem),
        };
        let files = match str_arg(a, "target")? {
            "godot" => {
                let assets = Assets::load(&map, &path);
                let ts_name = io::stem(&map.tileset_path(&path));
                let res = a.get("godot_res_dir").and_then(Value::as_str);
                let (files, warnings) = godot_tiles::export_map(&map, &assets, &ts_name, &out_dir, &stem, res)?;
                let mut s = files.iter().map(|f| format!("  {}", self.rel(f))).collect::<Vec<_>>().join("\n");
                for w in warnings {
                    s += &format!("\nWARNING: {w}");
                }
                s
            }
            "world_txt" => {
                let (text, warnings) = tilemap::export_world(&map);
                std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
                let f = out_dir.join("world.txt");
                io::write_atomic(&f, text.as_bytes())?;
                let mut s = format!("  {}", self.rel(&f));
                for w in warnings {
                    s += &format!("\nWARNING: {w}");
                }
                s
            }
            t => return Err(format!("a tilemap exports to godot or world_txt, not {t:?}")),
        };
        Ok(Out::Text(format!("exported {}:\n{files}", self.rel(&path))))
    }

    fn tileset_export(&self, a: &Value) -> Result<Out, String> {
        let path = self.resolve(str_arg(a, "path")?)?;
        let ts = Tileset::load(&path)?;
        let stem = io::stem(&path);
        let out_dir = match a.get("out_dir").and_then(Value::as_str) {
            Some(d) => self.resolve(d)?,
            None => self.workdir.join("export").join(&stem),
        };
        let files = godot_tiles::export_tileset(&ts, &out_dir, &stem, a.get("godot_res_dir").and_then(Value::as_str))?;
        Ok(Out::Text(format!(
            "exported godot_tileset ({} tiles, {} per atlas row, tile names in custom data \"name\"):\n{}",
            ts.names.len(),
            godot_tiles::ATLAS_COLUMNS,
            files.iter().map(|f| format!("  {}", self.rel(f))).collect::<Vec<_>>().join("\n")
        )))
    }

    fn map_history(&self, name: &str, a: &Value) -> Result<Out, String> {
        let (path, map) = self.load_map(a)?;
        let j = Journal::for_document(&path);
        if name == "history" {
            let current = j.entry_for(&map).map(|e| e.id);
            let limit = opt_int(a, "limit")?.unwrap_or(20).max(1) as usize;
            let lines: Vec<String> = j
                .entries()
                .iter()
                .rev()
                .filter(|e| e.kind == "edit")
                .take(limit)
                .map(|e| format!("#{} {}{}", e.id, e.note, if Some(e.id) == current { "  <- current" } else { "" }))
                .collect();
            if lines.is_empty() {
                return Ok(Out::Text("no history yet".into()));
            }
            return Ok(Out::Text(format!("newest first:\n{}", lines.join("\n"))));
        }
        let steps = opt_int(a, "steps")?.unwrap_or(1).max(1) as usize;
        if bool_arg(a, "dry_run", false) {
            return Ok(Out::Text(format!("DRY RUN: would undo {steps} step(s); see history")));
        }
        let (restored, target): (TileMap, _) = j.undo(&map, steps, &self.author())?;
        tilemap::save(&restored, &path)?;
        Ok(Out::Text(format!(
            "restored the state after #{} ({:?})\n{}",
            target.id,
            target.note,
            tilemap::describe_diff(&map, &restored, DIFF_MAX_ROWS)
        )))
    }
}

/// Tool definitions (appended to the list in `tool_list`).
pub fn tool_defs() -> Vec<Value> {
    let path =
        json!({"type": "string", "description": "tilemap path relative to the workdir (.tilemap.json may be omitted)"});
    let dry = json!({"type": "boolean", "description": "only report what would change; write nothing"});
    let note = json!({"type": "string", "description": "history description; default generated"});
    let int = |d: &str| json!({"type": "integer", "description": d});
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": required}});
    let tile = "tile: a legend key (\"~\"), a legend name (\"Wasser\") or a frame name of the tileset (\"water0\"; added to the legend with the next free key); \" \" or \"\" = empty";
    vec![
        tool("create_tilemap", "Create a tilemap (.tilemap.json): a grid of tiles from a tileset document (one tile = one frame, by frame name). Lists the tiles of the tileset.", json!({
            "name": {"type": "string", "description": "file name; .tilemap.json is appended"},
            "tileset": {"type": "string", "description": "tileset document (e.g. tiles32.pxe.json); tile size = its canvas"},
            "width": int("width in cells"), "height": int("height in cells"),
            "fill": {"type": "string", "description": format!("fill every cell; {tile}")},
            "overwrite": {"type": "boolean"}}), &["name", "tileset", "width", "height"]),
        tool("get_tilemap", "Read a tilemap: size, tileset, legend (key, name, tile, object, how many cells), unused tiles of the tileset, and the cells as rows of keys with rulers (optionally only x/y/w/h).", json!({
            "path": path, "x": int("area left"), "y": int("area top"), "w": int("area width"), "h": int("area height")}), &["path"]),
        tool("set_tiles", &format!("Set cells of a tilemap; returns the diff (count, bbox, rows before/after) and records it in the history (undo). Give cells [{{x, y, tile}}], rows of legend keys starting at x/y ('-' keeps a cell, ' ' empties it), and/or rect {{x, y, w, h}} with tile. {tile}."), json!({
            "path": path,
            "cells": {"type": "array", "items": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "tile": {"type": "string"}}}},
            "rows": {"type": "array", "items": {"type": "string"}}, "x": int("left of rows, default 0"), "y": int("top of rows, default 0"),
            "rect": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}},
            "tile": {"type": "string", "description": "tile for rect"},
            "dry_run": dry, "note": note}), &["path"]),
        tool("set_legend", "Add or change a legend entry of a tilemap: which tile (frame name) and/or object document (bigger than a tile, e.g. tree64.pxe.json, path relative to the map; stands bottom-centre on the cell, or at its 'origin' marker) a key shows, and its name. remove deletes an unused key.", json!({
            "path": path, "key": {"type": "string", "description": "one character"},
            "tile": {"type": "string", "description": "frame name in the tileset; \"\" = none (placeholder)"},
            "object": {"type": "string", "description": "object document relative to the map; \"\" = none"},
            "name": {"type": "string"}, "remove": {"type": "boolean"}, "dry_run": dry, "note": note}), &["path", "key"]),
        tool("resize_tilemap", "Change the size of a tilemap in cells; the top-left corner stays, new cells get fill (default empty).", json!({
            "path": path, "width": int("new width in cells"), "height": int("new height in cells"),
            "fill": {"type": "string", "description": tile}, "dry_run": dry, "note": note}), &["path", "width", "height"]),
        tool("import_world", "Import a world.txt of the sandbox game as a tilemap: same characters, legend names from its comment lines, tiles from the built-in table SANDBOX_TILES (README); unknown characters stay as placeholder keys (reported). Comment lines are kept, so export_world writes the file back byte for byte. The game adds path/water edges, cliff faces and decor itself; the map shows the base tiles.", json!({
            "src": {"type": "string", "description": "world.txt path relative to the workdir"},
            "name": {"type": "string", "description": "new tilemap name"},
            "tileset": {"type": "string", "description": "tileset document (tiles32.pxe.json); objects (tree64.pxe.json) are expected next to it"},
            "overwrite": {"type": "boolean"}}), &["src", "name", "tileset"]),
        tool("export_world", "Write a tilemap as world.txt (one character per cell, kept comment lines at their places). Warns about keys the legend comments do not explain.", json!({
            "path": path, "to": {"type": "string", "description": "output path relative to the workdir"},
            "overwrite": {"type": "boolean"}}), &["path", "to"]),
    ]
}

#[cfg(test)]
mod tests {
    use crate::mcp::test_call;

    #[test]
    fn tilemap_tools() {
        let dir = std::env::temp_dir().join(format!("pxe-mcp-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ts = r##"{"version":1,"width":2,"height":2,"palette":[{"key":"1","color":"#00ff00"},{"key":"2","color":"#0000ff"}],
            "layers":[{"name":"l"}],"frames":[{"name":"grass"},{"name":"water0"}],
            "cels":[{"layer":0,"frame":0,"rows":["11","11"]},{"layer":0,"frame":1,"rows":["22","22"]}]}"##;
        std::fs::write(dir.join("tiles.pxe.json"), ts).unwrap();
        let call = |n: &str, a| test_call(&dir, n, a);
        let (ok, t) = call(
            "create_tilemap",
            serde_json::json!({"name": "maps/m", "tileset": "tiles.pxe.json", "width": 4, "height": 3, "fill": "grass"}),
        );
        assert!(ok, "{t}");
        assert!(t.contains("grass, water0"), "{t}");
        let map = std::fs::read_to_string(dir.join("maps/m.tilemap.json")).unwrap();
        assert!(map.contains("\"tileset\": \"../tiles.pxe.json\""), "{map}");

        let (ok, t) = call(
            "set_tiles",
            serde_json::json!({"path": "maps/m", "cells": [{"x": 1, "y": 1, "tile": "water0"}, {"x": 9, "y": 0, "tile": "0"}], "dry_run": true}),
        );
        assert!(
            ok && t.starts_with("DRY RUN") && t.contains("1 cell(s) changed") && t.contains("1 cell(s) outside"),
            "{t}"
        );
        let (ok, t) = call(
            "set_tiles",
            serde_json::json!({"path": "maps/m", "cells": [{"x": 1, "y": 1, "tile": "water0"}], "note": "Teich"}),
        );
        assert!(ok && t.contains("legend: added 1 water0") && t.contains("\"Teich\""), "{t}");
        let (ok, t) = call("set_tiles", serde_json::json!({"path": "maps/m", "rows": ["-11", "1"], "x": 1, "y": 1}));
        assert!(ok && t.contains("3 cell(s) changed"), "{t}");
        let (ok, t) = call("set_tiles", serde_json::json!({"path": "maps/m", "rows": ["Z"]}));
        assert!(!ok && t.contains("not in the legend"), "{t}");
        let (ok, t) =
            call("set_tiles", serde_json::json!({"path": "maps/m", "cells": [{"x": 0, "y": 0, "tile": "lava"}]}));
        assert!(!ok && t.contains("unknown tile \"lava\""), "{t}");

        let (ok, t) = call("get_tilemap", serde_json::json!({"path": "maps/m"}));
        assert!(ok && t.contains("0 grass") && t.contains("0000") && t.contains("0111"), "{t}");
        let (ok, t) = call("set_legend", serde_json::json!({"path": "maps/m", "key": "1", "name": "Wasser"}));
        assert!(ok && t.contains("changed 1 Wasser (water0)"), "{t}");
        let (ok, t) = call("set_legend", serde_json::json!({"path": "maps/m", "key": "1", "remove": true}));
        assert!(!ok && t.contains("used by 4"), "{t}");

        let (ok, t) = call("history", serde_json::json!({"path": "maps/m"}));
        assert!(ok && t.contains("Teich"), "{t}");
        let (ok, t) = call("undo", serde_json::json!({"path": "maps/m", "steps": 2}));
        assert!(ok && t.contains("Teich"), "{t}");
        let (_, t) = call("get_tilemap", serde_json::json!({"path": "maps/m", "y": 1, "h": 1}));
        assert!(t.contains("1 0100"), "{t}");

        let (ok, t) = call("resize_tilemap", serde_json::json!({"path": "maps/m", "width": 2, "height": 2}));
        assert!(ok && t.contains("cut off"), "{t}");
        let (ok, t) = call("render_preview", serde_json::json!({"path": "maps/m", "zoom": 2, "inline": false}));
        assert!(ok && t.contains("preview/m_map.png"), "{t}");
        let (ok, t) = call("export", serde_json::json!({"path": "maps/m", "target": "godot"}));
        assert!(ok && t.contains("m.tscn") && t.contains("tiles_tileset.tres"), "{t}");
        let (ok, t) = call("export", serde_json::json!({"path": "tiles", "target": "godot_tileset"}));
        assert!(ok && t.contains("tiles_atlas.png"), "{t}");
        let (ok, t) = call(
            "set_pixels",
            serde_json::json!({"path": "maps/m.tilemap.json", "layer": 0, "frame": 0, "pixels": []}),
        );
        assert!(!ok && t.contains("is a tilemap"), "{t}");

        // world.txt round trip
        let world = "; .  Gras   ~  Wasser   X  Unbekannt\n..~\n.Q.\n";
        std::fs::write(dir.join("world.txt"), world).unwrap();
        let (ok, t) =
            call("import_world", serde_json::json!({"src": "world.txt", "name": "w", "tileset": "tiles.pxe.json"}));
        assert!(ok && t.contains("NOTE") && t.contains("'Q'"), "{t}");
        let (ok, t) = call("export_world", serde_json::json!({"path": "w", "to": "back.txt"}));
        assert!(ok, "{t}");
        assert_eq!(std::fs::read_to_string(dir.join("back.txt")).unwrap(), world);
        let (ok, t) = call("list_documents", serde_json::json!({}));
        assert!(ok && t.contains("w.tilemap.json") && t.contains("3x2 cells"), "{t}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
