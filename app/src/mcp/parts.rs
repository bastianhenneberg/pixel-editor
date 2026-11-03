//! MCP tools for the parts library (see `pixel_core::parts`): save a
//! reviewed piece of a cel under a name, list the parts, place one again
//! (a pixel write with locks, diff, dry_run, note and history).
//!
//! Two libraries: the project's in `<workdir>/.pxe-parts/`, and a personal
//! one shared by all projects (`$PXE_PARTS_DIR`, else
//! `$XDG_DATA_HOME/pixel-editor/parts`, else
//! `~/.local/share/pixel-editor/parts`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use pixel_core::diff::rows_block;
use pixel_core::document::Rect;
use pixel_core::parts::{self, Part};
use pixel_core::TRANSPARENT;
use serde_json::{json, Map, Value};

use super::{bool_arg, index_or_name, int_of, rect_arg, single_key, str_arg, CelScope, Out, Server};

/// Folder of the project library inside the workdir.
pub const PROJECT_DIR: &str = ".pxe-parts";

/// The personal library folder from the environment (none in tests, which
/// set `Server::personal_parts` themselves).
pub fn personal_dir_from_env() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PXE_PARTS_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if cfg!(test) {
        return None;
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")));
    base.map(|b| b.join("pixel-editor").join("parts"))
}

impl Server {
    /// Library folders to use: the named one, or (`None`) project then personal.
    fn libraries(&self, which: Option<&str>) -> Result<Vec<(&'static str, PathBuf)>, String> {
        let project = ("project", self.workdir.join(PROJECT_DIR));
        let personal = || {
            self.personal_parts
                .clone()
                .map(|p| ("personal", p))
                .ok_or_else(|| "no personal parts library (set PXE_PARTS_DIR, XDG_DATA_HOME or HOME)".to_string())
        };
        match which {
            None => Ok(std::iter::once(project).chain(personal().ok()).collect()),
            Some("project") => Ok(vec![project]),
            Some("personal") => Ok(vec![personal()?]),
            Some(l) => Err(format!("unknown library {l:?} (project, personal)")),
        }
    }

    pub(super) fn save_part(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let name = str_arg(a, "name")?;
        parts::check_name(name)?;
        let layer = doc.find_layer(&index_or_name(a.get("layer")).ok_or("missing \"layer\"")?)?;
        let frame = doc.find_frame(&index_or_name(a.get("frame")).ok_or("missing \"frame\"")?)?;
        let rect = match (a.get("region").and_then(Value::as_str), a.get("rect")) {
            (Some(_), Some(_)) => return Err("give either region or rect, not both".into()),
            (Some(n), None) => doc.regions[doc.find_region(n)?]
                .rect_at(frame)
                .ok_or_else(|| format!("region {n:?} does not exist on frame {}", doc.frame_label_long(frame)))?,
            (None, Some(r)) => rect_arg(r).map_err(|e| format!("rect: {e}"))?,
            (None, None) => return Err("give the area to save: region (name) or rect {x,y,w,h}".into()),
        };
        let library = a.get("library").and_then(Value::as_str).unwrap_or("project");
        let (lib, dir) = self.libraries(Some(library))?.remove(0);
        let file = parts::path_in(&dir, name);
        if file.exists() && !bool_arg(a, "overwrite", false) {
            return Err(format!(
                "part {name:?} already exists in the {lib} library (pass overwrite: true to replace it)"
            ));
        }
        let source = format!(
            "{} layer {:?} frame {} {}",
            self.rel(&path),
            doc.layers[layer].name,
            doc.frame_label(frame),
            rect.describe()
        );
        let part = Part::from_cel(&doc, layer, frame, rect, name, source)?;
        part.save(&file)?;
        Ok(Out::Text(format!(
            "saved part {name:?} ({}x{}, colors {}) to the {lib} library ({})\n{}",
            part.width,
            part.height,
            part.colors.iter().map(|c| format!("{} {}", c.key, c.color)).collect::<Vec<_>>().join(", "),
            if lib == "project" { format!("{PROJECT_DIR}/{name}.json") } else { file.display().to_string() },
            part.rows.join("\n")
        )))
    }

    pub(super) fn list_parts(&self, a: &Value) -> Result<Out, String> {
        let mut lines = Vec::new();
        for (lib, dir) in self.libraries(a.get("library").and_then(Value::as_str))? {
            let found = parts::list(&dir);
            if found.is_empty() {
                lines.push(format!("{lib} library: no parts ({})", dir.display()));
                continue;
            }
            lines.push(format!("{lib} library ({}):", dir.display()));
            for (name, p) in found {
                match p {
                    Err(e) => lines.push(format!("- {name}: unreadable: {e}")),
                    Ok(p) => {
                        let keys: String = p.colors.iter().map(|c| c.key.as_str()).collect();
                        lines.push(format!("- {name} {}x{}, keys {keys}, from {}", p.width, p.height, p.source));
                        // small parts in full, big ones cut
                        for row in p.rows.iter().take(16) {
                            lines.push(format!("    {}", row.chars().take(48).collect::<String>()));
                        }
                        if p.rows.len() > 16 {
                            lines.push(format!("    … {} more rows", p.rows.len() - 16));
                        }
                    }
                }
            }
        }
        Ok(Out::Text(lines.join("\n")))
    }

    pub(super) fn place_part(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let name = str_arg(a, "name")?;
        parts::check_name(name)?;
        let libs = self.libraries(a.get("library").and_then(Value::as_str))?;
        let (lib, file) =
            libs.iter().map(|(l, d)| (*l, parts::path_in(d, name))).find(|(_, f)| f.exists()).ok_or_else(|| {
                format!(
                    "no part {name:?} in the {} librar{} (list_parts shows what is there)",
                    libs.iter().map(|l| l.0).collect::<Vec<_>>().join(" or "),
                    if libs.len() > 1 { "ies" } else { "y" }
                )
            })?;
        let part = Part::load(&file)?;
        let (x, y) = (int_of(a, "x")?, int_of(a, "y")?);
        let replace = match a.get("mode").and_then(Value::as_str).unwrap_or("over") {
            "over" => false,
            "replace" => true,
            m => return Err(format!("unknown mode {m:?} (over, replace)")),
        };
        let (flip_h, flip_v) = (bool_arg(a, "flip_h", false), bool_arg(a, "flip_v", false));
        let mut explicit = BTreeMap::new();
        if let Some(m) = a.get("map").and_then(Value::as_object) {
            for (from, to) in m {
                let to = to.as_str().ok_or("map: values must be keys or color names")?;
                explicit.insert(single_key(from).map_err(|e| format!("map: {e}"))?, doc.find_key(to)?);
            }
        }
        let (map, notes) = part.key_map(&doc, &explicit)?;
        // the frames: frame or frames, each one explicit target
        let frame_args: Vec<Value> = match (a.get("frames").and_then(Value::as_array), a.get("frame")) {
            (Some(_), Some(_)) => return Err("give either frame or frames, not both".into()),
            (Some(list), None) if !list.is_empty() => list.clone(),
            (None, Some(f)) => vec![f.clone()],
            _ => return Err("give the target frame (frame) or frames [..]".into()),
        };
        let rect = Rect::new(x, y, part.width, part.height);
        let mut scopes: Vec<CelScope> = Vec::new();
        for f in frame_args {
            let mut args = Map::new();
            args.insert("layer".into(), a.get("layer").cloned().unwrap_or(Value::Null));
            args.insert("frame".into(), f);
            args.insert("edit_linked".into(), json!(bool_arg(a, "edit_linked", false)));
            let mut s = self.target(&doc, &Value::Object(args), "layer", "frame")?;
            s.clip = Some(rect);
            let cel = doc.layers[s.layer].link_source(s.frame);
            if !scopes.iter().any(|o| o.layer == s.layer && doc.layers[o.layer].link_source(o.frame) == cel) {
                scopes.push(s);
            }
        }
        let before = doc.clone();
        let mut doc = doc;
        let pixels = part.pixels(flip_h, flip_v);
        let off = pixels
            .iter()
            .filter(|(dx, dy, k)| (replace || *k != TRANSPARENT) && !doc.in_bounds(x + dx, y + dy))
            .count();
        for s in &scopes {
            for &(dx, dy, k) in &pixels {
                if k == TRANSPARENT {
                    if replace {
                        doc.set(s.layer, s.frame, x + dx, y + dy, TRANSPARENT);
                    }
                } else {
                    doc.set(s.layer, s.frame, x + dx, y + dy, map[&k]);
                }
            }
        }
        let mut detail = format!(
            "place_part {name:?} ({lib} library, {}x{}) at ({x}, {y}){}, mode {}, on {} cel(s)",
            part.width,
            part.height,
            match (flip_h, flip_v) {
                (true, true) => " flipped h+v",
                (true, false) => " flipped h",
                (false, true) => " flipped v",
                _ => "",
            },
            if replace { "replace" } else { "over" },
            scopes.len()
        );
        if !notes.is_empty() {
            detail += &format!("\ncolors mapped by value: {}", notes.join(", "));
        }
        if off > 0 {
            detail += &format!("\n{off} part pixels lie outside the canvas and were dropped");
        }
        let shown: Vec<(i32, i32, u8)> =
            pixels.iter().map(|&(dx, dy, k)| (dx, dy, map.get(&k).copied().unwrap_or(k))).collect();
        let key = |px: i32, py: i32| shown.iter().find(|p| (p.0, p.1) == (px, py)).map_or(TRANSPARENT, |p| p.2);
        detail += &format!("\npart as placed:\n{}", rows_block(key, Rect::new(0, 0, part.width, part.height)));
        let mut args = a.clone();
        if args.get("note").and_then(Value::as_str).is_none_or(|n| n.trim().is_empty()) {
            args["note"] = json!(format!(
                "{}: place_part {name} {} {} at ({x}, {y})",
                self.author(),
                scopes.iter().map(|s| doc.frame_label(s.frame)).collect::<Vec<_>>().join(","),
                doc.layers[scopes[0].layer].name
            ));
        }
        self.finish_write(&args, &path, &before, doc, &scopes, "place_part", detail, false)
    }
}

pub fn tool_defs() -> Vec<Value> {
    let path =
        json!({"type": "string", "description": "document path relative to the workdir; the extension may be omitted"});
    let library = |d: &str| json!({"type": "string", "enum": ["project", "personal"], "description": d});
    let rect = json!({"type": "object", "description": "{x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}});
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": required}});
    vec![
        tool("save_part", "Save a reviewed part (e.g. the sword at one angle) under a name, to place it again with place_part instead of redrawing it: the pixels of one layer+frame inside region or rect, transparent pixels kept as transparent, with the color of every key. Project library: <workdir>/.pxe-parts/<name>.json; personal library (all projects): $PXE_PARTS_DIR or ~/.local/share/pixel-editor/parts. Writes no document.", json!({
            "path": path, "layer": {"type": ["integer", "string"], "description": "layer index or name"},
            "frame": {"type": ["integer", "string"], "description": "frame index, name or tag/n"},
            "region": {"type": "string", "description": "the part is this region's rect on that frame"},
            "rect": rect,
            "name": {"type": "string", "description": "part name: lowercase letters, digits, '-', '_' (e.g. sword_45)"},
            "library": library("project (default) or personal"),
            "overwrite": {"type": "boolean", "description": "replace a part of that name"}}), &["path", "layer", "frame", "name"]),
        tool("place_part", "Place a saved part into one or more cels with its top-left corner at (x, y): a pixel write (explicit layer + frame(s), locks refuse, diff, dry_run, note, history). Colors are mapped by value: the same key if it has the part's color, else any key with that color; colors the target palette lacks are an error (or give map). mode over (default) keeps the cel where the part is transparent; replace clears it. flip_h/flip_v mirror the part.", json!({
            "path": path, "layer": {"type": ["integer", "string"], "description": "layer index or name"},
            "frame": {"type": ["integer", "string"], "description": "frame index, name or tag/n"},
            "frames": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "several frames instead of frame"},
            "name": {"type": "string", "description": "part name"},
            "library": library("project or personal; default: project, then personal"),
            "x": {"type": "integer", "description": "left edge of the part on the canvas"},
            "y": {"type": "integer", "description": "top edge of the part on the canvas"},
            "mode": {"type": "string", "enum": ["over", "replace"], "description": "over (default) or replace"},
            "flip_h": {"type": "boolean", "description": "mirror the part left-right"},
            "flip_v": {"type": "boolean", "description": "mirror the part top-bottom"},
            "map": {"type": "object", "description": "explicit colors: {part key: target key or color name}, wins over the mapping by value"},
            "edit_linked": {"type": "boolean", "description": "allow writing a cel linked to other frames (changes all of them)"},
            "dry_run": {"type": "boolean", "description": "only report what would change; write nothing"},
            "note": {"type": "string", "description": "history description; default \"<assistant>: place_part <name> <frames> <layer> at (x, y)\""},
            "pace_ms": {"type": "integer", "description": "draw the change row by row in an open editor, this many ms apart"}}), &["path", "layer", "name", "x", "y"]),
        tool("list_parts", "List the saved parts (project library, then personal) with size, keys, where they came from and their rows.", json!({
            "library": library("only this library; default both")}), &[]),
    ]
}
