//! `.pxe.json` load/save. See FORMAT.md.
//!
//! The writer is hand-formatted so that each palette entry, layer, frame and
//! tag sits on one line and every pixel row on its own line: readable for
//! humans and Claude, and produces small git diffs.

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::color::Rgba;
use crate::document::{
    Direction, Document, Frame, PaletteEntry, Rect, Region, RegionKey, RegionKind, Tag, FORMAT_VERSION, TRANSPARENT,
};

pub const EXTENSION: &str = ".pxe.json";
pub const FORMAT_NAME: &str = "pixel-editor";

#[derive(Deserialize)]
struct FileDoc {
    #[serde(default)]
    format: Option<String>,
    version: u32,
    width: u32,
    height: u32,
    #[serde(default)]
    palette: Vec<FilePalette>,
    layers: Vec<FileLayer>,
    frames: Vec<FileFrame>,
    #[serde(default)]
    tags: Vec<FileTag>,
    #[serde(default)]
    regions: Vec<FileRegion>,
    #[serde(default)]
    cels: Vec<FileCel>,
}

#[derive(Deserialize)]
struct FilePalette {
    key: String,
    color: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct FileLayer {
    name: String,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default = "one")]
    opacity: f32,
    #[serde(default)]
    locked: bool,
}

#[derive(Deserialize)]
struct FileFrame {
    name: String,
    #[serde(default = "default_ms")]
    duration_ms: u32,
    #[serde(default)]
    locked: bool,
}

#[derive(Deserialize)]
struct FileRegion {
    name: String,
    /// "area" (default) or "marker"
    #[serde(default)]
    kind: Option<String>,
    /// shorthand for a region with one rect on all frames
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    h: Option<u32>,
    #[serde(default)]
    keys: Vec<FileRegionKey>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Deserialize)]
struct FileRegionKey {
    frame: usize,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    w: u32,
    #[serde(default)]
    h: u32,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct FileTag {
    name: String,
    #[serde(default)]
    from: Option<usize>,
    #[serde(default)]
    to: Option<usize>,
    #[serde(default)]
    frames: Option<Vec<usize>>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default = "yes", rename = "loop")]
    looping: bool,
    /// legacy (before directions): true = "pingpong"
    #[serde(default)]
    pingpong: bool,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Deserialize)]
struct FileCel {
    layer: usize,
    frame: usize,
    #[serde(default)]
    rows: Option<Vec<String>>,
    /// linked cel: shares the cel of this frame
    #[serde(default)]
    link: Option<usize>,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn default_ms() -> u32 {
    100
}

pub fn from_json(text: &str) -> Result<Document, String> {
    let f: FileDoc = serde_json::from_str(text).map_err(|e| format!("invalid document JSON: {e}"))?;
    if let Some(fmt) = &f.format {
        if fmt != FORMAT_NAME {
            return Err(format!("unknown format {fmt:?}, expected {FORMAT_NAME:?}"));
        }
    }
    if f.version > FORMAT_VERSION {
        return Err(format!("file version {} is newer than supported ({FORMAT_VERSION})", f.version));
    }
    let mut palette = Vec::new();
    for p in f.palette {
        let key = match p.key.as_bytes() {
            [k] => *k,
            _ => return Err(format!("palette key {:?} must be exactly one ASCII character", p.key)),
        };
        if key == TRANSPARENT {
            continue; // "." is implicit; tolerate it being listed
        }
        palette.push(PaletteEntry { key, color: Rgba::parse(&p.color)?, name: p.name });
    }
    let mut doc = Document::new(f.width, f.height, palette)?;
    if f.layers.is_empty() || f.frames.is_empty() {
        return Err("document needs at least one layer and one frame".into());
    }
    doc.frames = f
        .frames
        .into_iter()
        .map(|fr| Frame { name: fr.name, duration_ms: fr.duration_ms.max(1), locked: fr.locked })
        .collect();
    doc.layers.clear();
    for l in f.layers {
        let i = doc.add_layer(&l.name, doc.layers.len());
        let layer = &mut doc.layers[i];
        layer.name = l.name;
        layer.visible = l.visible;
        layer.opacity = l.opacity.clamp(0.0, 1.0);
        layer.locked = l.locked;
    }
    let mut links = Vec::new();
    for c in f.cels {
        let at = format!("cel (layer {}, frame {})", c.layer, c.frame);
        doc.check_layer(c.layer).map_err(|e| format!("{at}: {e}"))?;
        doc.check_frame(c.frame).map_err(|e| format!("{at}: {e}"))?;
        let rows = match (c.rows, c.link) {
            (Some(rows), None) => rows,
            (None, Some(target)) => {
                doc.check_frame(target).map_err(|e| format!("{at}: link: {e}"))?;
                links.push((c.layer, c.frame, target));
                continue;
            }
            _ => return Err(format!("{at}: needs either \"rows\" or \"link\"")),
        };
        if rows.len() != doc.height as usize {
            return Err(format!("{at}: {} rows, expected {}", rows.len(), doc.height));
        }
        let mut px = Vec::with_capacity(doc.pixel_count());
        for (y, row) in rows.iter().enumerate() {
            if row.len() != doc.width as usize || !row.is_ascii() {
                return Err(format!(
                    "{at} row {y}: {} characters, expected {} ASCII keys",
                    row.chars().count(),
                    doc.width
                ));
            }
            px.extend_from_slice(row.as_bytes());
        }
        doc.layers[c.layer].set_cel(c.frame, Arc::new(px));
    }
    for (layer, frame, target) in links {
        doc.layers[layer].link(frame, target);
    }
    for r in f.regions {
        let at = |e: String| format!("region {:?}: {e}", r.name);
        let mut keys: Vec<RegionKey> = r
            .keys
            .iter()
            .map(|k| RegionKey {
                frame: k.frame,
                rect: (!k.hidden && k.w > 0 && k.h > 0).then(|| Rect::new(k.x, k.y, k.w, k.h)),
            })
            .collect();
        if let (Some(x), Some(y), Some(w), Some(h)) = (r.x, r.y, r.w, r.h) {
            if !keys.is_empty() {
                return Err(at("give either x/y/w/h or keys, not both".into()));
            }
            keys.push(RegionKey { frame: 0, rect: Some(Rect::new(x, y, w, h)) });
        }
        if keys.is_empty() {
            return Err(at("needs x/y/w/h or keys".into()));
        }
        let color = match &r.color {
            Some(c) => Some(Rgba::parse(c).map_err(at)?),
            None => None,
        };
        let kind = match &r.kind {
            Some(k) => RegionKind::parse(k).map_err(at)?,
            None => RegionKind::Area,
        };
        let mut region = Region { name: r.name.clone(), kind, keys, layer: r.layer, locked: r.locked, color };
        region.keys.sort_by_key(|k| k.frame);
        doc.regions.push(region);
    }
    for t in f.tags {
        let (from, to) = match (t.frames, t.from, t.to) {
            (_, Some(a), Some(b)) => (a.min(b), a.max(b)),
            (_, Some(a), None) => (a, a),
            (Some(list), None, None) if !list.is_empty() && list.windows(2).all(|w| w[1] == w[0] + 1) => {
                (list[0], list[list.len() - 1])
            }
            (Some(_), None, None) => {
                return Err(format!(
                    "tag {:?}: \"frames\" must be a contiguous range; tags are from/to ranges \
                     (use linked frames to repeat a frame)",
                    t.name
                ))
            }
            _ => return Err(format!("tag {:?} needs \"from\" and \"to\"", t.name)),
        };
        let direction = match (&t.direction, t.pingpong) {
            (Some(d), _) => Direction::parse(d).map_err(|e| format!("tag {:?}: {e}", t.name))?,
            (None, true) => Direction::PingPong,
            (None, false) => Direction::Forward,
        };
        let color = match &t.color {
            Some(c) => Some(Rgba::parse(c).map_err(|e| format!("tag {:?}: {e}", t.name))?),
            None => None,
        };
        doc.tags.push(Tag { name: t.name, from, to, direction, looping: t.looping, color });
    }
    doc.validate()?;
    Ok(doc)
}

pub fn to_json(doc: &Document) -> String {
    let line = |v: serde_json::Value| inline(&v);
    let list = |items: Vec<String>| -> String {
        if items.is_empty() {
            "[]".to_string()
        } else {
            format!("[\n    {}\n  ]", items.join(",\n    "))
        }
    };
    let palette = doc
        .palette
        .iter()
        .map(|p| {
            let mut v = json!({"key": (p.key as char).to_string(), "color": p.color.to_hex()});
            if !p.name.is_empty() {
                v["name"] = json!(p.name);
            }
            line(v)
        })
        .collect();
    let layers = doc
        .layers
        .iter()
        .map(|l| {
            let mut v = json!({"name": l.name, "visible": l.visible, "opacity": round3(l.opacity)});
            if l.locked {
                v["locked"] = json!(true);
            }
            line(v)
        })
        .collect();
    let frames = doc
        .frames
        .iter()
        .map(|f| {
            let mut v = json!({"name": f.name, "duration_ms": f.duration_ms});
            if f.locked {
                v["locked"] = json!(true);
            }
            line(v)
        })
        .collect();
    let regions = doc
        .regions
        .iter()
        .map(|r| {
            let keys: Vec<serde_json::Value> = r
                .keys
                .iter()
                .map(|k| match k.rect {
                    Some(rc) => json!({"frame": k.frame, "x": rc.x, "y": rc.y, "w": rc.w, "h": rc.h}),
                    None => json!({"frame": k.frame, "hidden": true}),
                })
                .collect();
            let mut v = json!({"name": r.name});
            if r.is_marker() {
                v["kind"] = json!("marker");
            }
            if let Some(l) = &r.layer {
                v["layer"] = json!(l);
            }
            if r.locked {
                v["locked"] = json!(true);
            }
            if let Some(c) = r.color {
                v["color"] = json!(c.to_hex());
            }
            v["keys"] = json!(keys);
            line(v)
        })
        .collect();
    let tags = doc
        .tags
        .iter()
        .map(|t| {
            let mut v = json!({"name": t.name, "from": t.from, "to": t.to,
                               "direction": t.direction.id(), "loop": t.looping});
            if let Some(c) = t.color {
                v["color"] = json!(c.to_hex());
            }
            line(v)
        })
        .collect();
    let mut cels = Vec::new();
    for (li, l) in doc.layers.iter().enumerate() {
        for fi in 0..doc.frames.len() {
            let source = l.link_source(fi);
            if source != fi {
                cels.push(format!("{{\"layer\": {li}, \"frame\": {fi}, \"link\": {source}}}"));
                continue;
            }
            if doc.cel_is_empty(li, fi) {
                continue;
            }
            let rows: Vec<String> = doc.rows(li, fi).iter().map(|r| line(json!(r))).collect();
            cels.push(format!(
                "{{\"layer\": {li}, \"frame\": {fi}, \"rows\": [\n      {}\n    ]}}",
                rows.join(",\n      ")
            ));
        }
    }
    format!(
        "{{\n  \"format\": \"{FORMAT_NAME}\",\n  \"version\": {FORMAT_VERSION},\n  \"width\": {},\n  \"height\": {},\n  \
         \"palette\": {},\n  \"layers\": {},\n  \"frames\": {},\n  \"tags\": {},\n  \"regions\": {},\n  \"cels\": {}\n}}\n",
        doc.width,
        doc.height,
        list(palette),
        list(layers),
        list(frames),
        list(tags),
        list(regions),
        list(cels)
    )
}

/// Single-line JSON with a space after `:` and `,`.
pub(crate) fn inline(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let parts: Vec<String> =
                m.iter().map(|(k, v)| format!("{}: {}", Value::String(k.clone()), inline(v))).collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(inline).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

fn round3(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

/// Load a `.pxe.json` or `.aseprite`/`.ase` file (by extension).
pub fn load(path: &Path) -> Result<Document, String> {
    load_with_warnings(path).map(|(d, _)| d)
}

/// Like `load`, plus warnings about things an `.aseprite` file contained
/// that the document model cannot represent.
pub fn load_with_warnings(path: &Path) -> Result<(Document, Vec<String>), String> {
    if crate::aseprite::is_aseprite_path(path) {
        return crate::aseprite::load(path);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_json(&text).map(|d| (d, Vec::new())).map_err(|e| format!("{}: {e}", path.display()))
}

/// Atomic save: write a temp file next to the target, then rename, so a
/// watcher never sees a half-written document. `.aseprite`/`.ase` paths
/// are written as Aseprite files (indexed color).
pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    if crate::aseprite::is_aseprite_path(path) {
        return crate::aseprite::save(doc, path, crate::aseprite::ColorMode::Indexed);
    }
    write_atomic(path, to_json(doc).as_bytes())
}

/// True for file names the editor can open.
pub fn is_document_path(path: &Path) -> bool {
    path.to_string_lossy().ends_with(EXTENSION) || crate::aseprite::is_aseprite_path(path)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path.file_name().ok_or_else(|| format!("bad path {}", path.display()))?;
    let tmp = path.with_file_name(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// File name without the `.pxe.json` (or other) extension.
pub fn stem(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.strip_suffix(EXTENSION)
        .map(str::to_string)
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut d = Document::new(3, 2, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        d.set_palette_color(b'g', Rgba::parse("#ff000080").unwrap()).unwrap();
        d.set_rows(0, 0, 0, 0, &["0g.".into(), ".c.".into()]).unwrap();
        d.add_frame(1, Some(0), false).unwrap();
        d.add_frame(2, Some(0), true).unwrap();
        d.add_layer("top", 1);
        d.layers[1].opacity = 0.5;
        d.layers[1].visible = false;
        d.frames[1].duration_ms = 250;
        d.set_tag(Tag::new("idle", 0, 1)).unwrap();
        d.palette[2].name = "crust_dark".into();
        d.layers[0].locked = true;
        d.frames[1].locked = true;
        let mut r = Region::new("face", Rect::new(0, 0, 2, 1));
        r.set_rect_for(1, 1, None, 3);
        r.layer = Some("top".into());
        r.locked = true;
        d.regions.push(r);
        let mut origin = Region::marker("origin", 1, 0);
        origin.locked = true;
        d.regions.push(origin);
        let mut t = Tag::new("hit", 1, 2);
        t.direction = Direction::PingPongReverse;
        t.looping = false;
        t.color = Some(Rgba::parse("#ff8800").unwrap());
        d.set_tag(t).unwrap();
        let text = to_json(&d);
        let back = from_json(&text).unwrap();
        assert_eq!(back, d);
        // stable output
        assert_eq!(to_json(&back), text);
        assert!(text.contains("\"from\": 0"));
        assert!(text.contains("{\"layer\": 0, \"frame\": 2, \"link\": 0}"));
        assert!(back.layers[0].is_linked(2));
        assert!(text.contains("\"direction\": \"pingpong_reverse\""));
        assert!(text.contains("\"name\": \"crust_dark\""));
        assert!(text.contains("{\"frame\": 1, \"hidden\": true}"), "{text}");
        assert!(text.contains("{\"name\": \"origin\", \"kind\": \"marker\""), "{text}");
        assert_eq!(back.regions[1].kind, RegionKind::Marker);
        assert_eq!(back.regions[0].kind, RegionKind::Area, "old files / no kind = area");
        assert!(text.contains("      \"0g.\",\n"));
    }

    #[test]
    fn rejects_bad_rows() {
        let base = r##"{"version":1,"width":2,"height":1,"palette":[{"key":"a","color":"#fff"}],
            "layers":[{"name":"l"}],"frames":[{"name":"f","duration_ms":100}],"cels":[{"layer":0,"frame":0,"rows":[ROW]}]}"##;
        assert!(from_json(&base.replace("ROW", "\"a.\"")).is_ok());
        assert!(from_json(&base.replace("ROW", "\"a\"")).is_err());
        assert!(from_json(&base.replace("ROW", "\"ab\"")).unwrap_err().contains("not in the palette"));
    }

    #[test]
    fn legacy_tags() {
        let base = r##"{"version":1,"width":1,"height":1,"layers":[{"name":"l"}],
            "frames":[{"name":"a"},{"name":"b"},{"name":"c"}],"tags":[TAG]}"##;
        let d = from_json(&base.replace("TAG", r#"{"name":"t","frames":[0,1,2],"pingpong":true}"#)).unwrap();
        assert_eq!((d.tags[0].from, d.tags[0].to, d.tags[0].direction), (0, 2, Direction::PingPong));
        let e = from_json(&base.replace("TAG", r#"{"name":"t","frames":[0,2]}"#)).unwrap_err();
        assert!(e.contains("contiguous"), "{e}");
    }

    #[test]
    fn stem_strips_extension() {
        assert_eq!(stem(Path::new("/a/hero.pxe.json")), "hero");
        assert_eq!(stem(Path::new("/a/sheet.png")), "sheet");
    }
}
