//! Tilemaps: a grid of tiles taken from a tileset document.
//!
//! A map is its own file (`<name>.tilemap.json`, see FORMAT.md). It refers
//! to a tileset document (one tile = one frame of that document, composited
//! from its visible layers) and stores the grid like `world.txt` of the
//! sandbox game: one character per cell. The `legend` maps characters to
//! tiles by frame *name* (reordering the tileset breaks nothing) and
//! optionally to an object document (a tree that is bigger than a tile),
//! which stands on the cell. A space is an empty cell.
//!
//! The same grid reads and writes the sandbox's `world.txt` without loss:
//! comment lines are kept verbatim with their position, the legend comes
//! from those comments plus [`SANDBOX_TILES`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::document::{Document, Rect, KEY_ORDER};
use crate::render::{composite, Image};
use crate::{font, io};

pub const EXTENSION: &str = ".tilemap.json";
pub const FORMAT: &str = "pixel-editor-tilemap";
pub const VERSION: u64 = 1;
/// An empty cell.
pub const EMPTY: u8 = b' ';
/// Largest map side in cells.
pub const MAX_CELLS: u32 = 1024;

/// Which tile (and object) the sandbox game draws for a `world.txt`
/// character: `(key, tile frame in tiles32, object document next to the
/// tileset)`. Used when importing a `world.txt`; edges, cliff faces and
/// decor are added by the game itself and are not part of the map.
pub const SANDBOX_TILES: &[(char, &str, Option<&str>)] = &[
    ('.', "grass", None),
    (',', "grass2", None),
    ('*', "flowers", None),
    ('=', "path", None),
    ('g', "tallgrass0", None),
    ('~', "water0", None),
    ('#', "wall_top", None),
    ('b', "bush", None),
    ('T', "grass", Some("tree64.pxe.json")),
    ('^', "grass_high", None),
    ('H', "stairs", None),
    ('@', "grass", None),
    ('s', "grass", None),
];

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub key: u8,
    /// Frame name in the tileset; None = placeholder (shown with its key).
    pub tile: Option<String>,
    /// Document drawn standing on the cell, path relative to the map.
    pub object: Option<String>,
    /// Readable name ("Gras", "Baum").
    pub name: String,
}

impl LegendEntry {
    pub fn label(&self) -> String {
        let what = match (&self.tile, &self.object) {
            (Some(t), Some(o)) => format!("{t} + {o}"),
            (Some(t), None) => t.clone(),
            (None, Some(o)) => o.clone(),
            (None, None) => "(kein Tile)".into(),
        };
        if self.name.is_empty() {
            format!("{} {what}", self.key as char)
        } else {
            format!("{} {} ({what})", self.key as char, self.name)
        }
    }
}

/// A comment or blank line of an imported `world.txt`, kept for the way back.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldLine {
    /// Number of map rows before this line.
    pub at: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileMap {
    /// Tileset document, relative to the map file.
    pub tileset: String,
    pub tile_w: u32,
    pub tile_h: u32,
    pub width: u32,
    pub height: u32,
    pub legend: Vec<LegendEntry>,
    /// Keys, row-major, `EMPTY` for nothing.
    pub cells: Vec<u8>,
    /// Comment lines of the imported `world.txt`.
    pub world_lines: Vec<WorldLine>,
    /// The imported `world.txt` ended with a newline.
    pub world_final_newline: bool,
}

/// Keys a legend may use: printable ASCII except space (empty), `;`
/// (a comment in world.txt), `"` and `\`.
pub fn is_valid_key(k: u8) -> bool {
    k.is_ascii_graphic() && !matches!(k, b';' | b'"' | b'\\')
}

impl TileMap {
    pub fn new(tileset: &str, tile_w: u32, tile_h: u32, width: u32, height: u32) -> Result<TileMap, String> {
        check_size(width, height)?;
        Ok(TileMap {
            tileset: tileset.into(),
            tile_w,
            tile_h,
            width,
            height,
            legend: Vec::new(),
            cells: vec![EMPTY; (width * height) as usize],
            world_lines: Vec::new(),
            world_final_newline: true,
        })
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    pub fn get(&self, x: i32, y: i32) -> u8 {
        if self.in_bounds(x, y) {
            self.cells[(y as u32 * self.width + x as u32) as usize]
        } else {
            EMPTY
        }
    }

    /// Set a cell; true if it changed.
    pub fn set(&mut self, x: i32, y: i32, key: u8) -> bool {
        if !self.in_bounds(x, y) {
            return false;
        }
        let i = (y as u32 * self.width + x as u32) as usize;
        let changed = self.cells[i] != key;
        self.cells[i] = key;
        changed
    }

    pub fn rows(&self) -> Vec<String> {
        self.cells.chunks(self.width as usize).map(|r| String::from_utf8_lossy(r).into_owned()).collect()
    }

    pub fn entry(&self, key: u8) -> Option<&LegendEntry> {
        self.legend.iter().find(|e| e.key == key)
    }

    /// "~ Wasser (water0)", "leer" for an empty cell.
    pub fn key_label(&self, key: u8) -> String {
        if key == EMPTY {
            return "leer".into();
        }
        self.entry(key).map_or_else(|| format!("{} (nicht in der Legende)", key as char), LegendEntry::label)
    }

    pub fn next_free_key(&self) -> Option<u8> {
        KEY_ORDER.bytes().chain(b'!'..=b'~').find(|k| is_valid_key(*k) && self.entry(*k).is_none())
    }

    /// The key showing exactly `tile` without an object; a new legend entry
    /// with the next free key if there is none yet.
    pub fn key_for_tile(&mut self, tile: &str) -> Result<u8, String> {
        if let Some(e) = self.legend.iter().find(|e| e.tile.as_deref() == Some(tile) && e.object.is_none()) {
            return Ok(e.key);
        }
        let key = self.next_free_key().ok_or("the legend is full")?;
        self.legend.push(LegendEntry { key, tile: Some(tile.into()), object: None, name: String::new() });
        Ok(key)
    }

    /// A key ("~") or, failing that, a tile frame name ("water0", added
    /// to the legend if needed) or a legend name ("Wasser"). `""`/" " =
    /// empty.
    pub fn find_key(&mut self, s: &str, tileset: Option<&Tileset>) -> Result<u8, String> {
        if s.is_empty() || s == " " {
            return Ok(EMPTY);
        }
        if s.len() == 1 && self.entry(s.as_bytes()[0]).is_some() {
            return Ok(s.as_bytes()[0]);
        }
        if let Some(e) = self.legend.iter().find(|e| e.name == s) {
            return Ok(e.key);
        }
        if let Some(e) = self.legend.iter().find(|e| e.tile.as_deref() == Some(s) && e.object.is_none()) {
            return Ok(e.key);
        }
        if let Some(ts) = tileset {
            if ts.index(s).is_some() {
                return self.key_for_tile(s);
            }
        }
        let keys: Vec<String> = self.legend.iter().map(LegendEntry::label).collect();
        Err(format!(
            "unknown tile {s:?}: not a legend key, legend name or tile of the tileset; legend: {}",
            if keys.is_empty() { "(empty)".into() } else { keys.join(", ") }
        ))
    }

    /// Resize, keeping the top-left corner; new cells get `fill`.
    pub fn resize(&mut self, width: u32, height: u32, fill: u8) -> Result<(), String> {
        check_size(width, height)?;
        let mut cells = vec![fill; (width * height) as usize];
        for y in 0..height.min(self.height) {
            for x in 0..width.min(self.width) {
                cells[(y * width + x) as usize] = self.cells[(y * self.width + x) as usize];
            }
        }
        self.cells = cells;
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn fill_rect(&mut self, r: Rect, key: u8) -> usize {
        let mut n = 0;
        for y in r.y..r.y + r.h as i32 {
            for x in r.x..r.x + r.w as i32 {
                n += self.set(x, y, key) as usize;
            }
        }
        n
    }

    /// Fill the 4-connected area of equal keys at (x, y).
    pub fn flood_fill(&mut self, x: i32, y: i32, key: u8) -> usize {
        if !self.in_bounds(x, y) {
            return 0;
        }
        let from = self.get(x, y);
        if from == key {
            return 0;
        }
        let mut stack = vec![(x, y)];
        let mut n = 0;
        while let Some((x, y)) = stack.pop() {
            if self.get(x, y) != from || !self.in_bounds(x, y) {
                continue;
            }
            self.set(x, y, key);
            n += 1;
            stack.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
        n
    }

    /// Keys used in the grid that have no legend entry.
    pub fn unknown_keys(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for &k in &self.cells {
            if k != EMPTY && self.entry(k).is_none() && !out.contains(&k) {
                out.push(k);
            }
        }
        out
    }

    pub fn validate(&self) -> Result<(), String> {
        check_size(self.width, self.height)?;
        if self.cells.len() != (self.width * self.height) as usize {
            return Err("cell count does not match the size".into());
        }
        for (i, e) in self.legend.iter().enumerate() {
            if !is_valid_key(e.key) {
                return Err(format!("legend[{i}]: invalid key {:?}", e.key as char));
            }
            if self.legend[..i].iter().any(|o| o.key == e.key) {
                return Err(format!("legend[{i}]: key {:?} defined twice", e.key as char));
            }
        }
        if let Some(k) = self.unknown_keys().first() {
            let (i, _) = self.cells.iter().enumerate().find(|(_, c)| *c == k).unwrap();
            let (x, y) = (i as u32 % self.width, i as u32 / self.width);
            return Err(format!("row {y}, x {x}: key {:?} is not in the legend", *k as char));
        }
        Ok(())
    }

    /// Path of the tileset document for a map stored at `map_path`.
    pub fn tileset_path(&self, map_path: &Path) -> PathBuf {
        resolve(map_path, &self.tileset)
    }

    /// Paths of the object documents, by legend key.
    pub fn object_paths(&self, map_path: &Path) -> Vec<(u8, PathBuf)> {
        self.legend.iter().filter_map(|e| e.object.as_ref().map(|o| (e.key, resolve(map_path, o)))).collect()
    }
}

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_CELLS || h > MAX_CELLS {
        return Err(format!("map size {w}x{h} out of range (1..{MAX_CELLS} cells per side)"));
    }
    Ok(())
}

fn resolve(map_path: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        map_path.parent().unwrap_or(Path::new(".")).join(p)
    }
}

/// `target` relative to the directory of `map_path` (for the `tileset`
/// field); `../` steps where needed.
pub fn relative_to_map(map_path: &Path, target: &Path) -> String {
    let abs = |p: &Path| {
        let p = if p.as_os_str().is_empty() { Path::new(".") } else { p };
        let a = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        // drop "." and resolve ".." textually
        let mut out = PathBuf::new();
        for c in a.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                c => out.push(c),
            }
        }
        out
    };
    let base = abs(map_path.parent().unwrap_or(Path::new(".")));
    let target = abs(target);
    let (b, t): (Vec<_>, Vec<_>) = (base.components().collect(), target.components().collect());
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target.display().to_string();
    }
    let mut parts: Vec<String> = vec!["..".into(); b.len() - common];
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

pub fn is_tilemap_path(path: &Path) -> bool {
    path.to_string_lossy().ends_with(EXTENSION)
}

/// File name without `.tilemap.json`.
pub fn stem(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.strip_suffix(EXTENSION).map(str::to_string).unwrap_or_else(|| io::stem(path))
}

// ---- file format ------------------------------------------------------

pub fn from_json(text: &str) -> Result<TileMap, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    if let Some(f) = v.get("format").and_then(Value::as_str) {
        if f != FORMAT {
            return Err(format!("format is {f:?}, expected {FORMAT:?}"));
        }
    }
    let version = v.get("version").and_then(Value::as_u64).unwrap_or(VERSION);
    if version > VERSION {
        return Err(format!("version {version} is newer than this editor ({VERSION})"));
    }
    let num = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n as u32).ok_or(format!("missing number {k:?}"));
    let tileset = v.get("tileset").and_then(Value::as_str).ok_or("missing \"tileset\"")?.to_string();
    let (width, height) = (num("width")?, num("height")?);
    let (tile_w, tile_h) = match v.get("tile_size").and_then(Value::as_array) {
        Some(a) if a.len() == 2 => (
            a[0].as_u64().ok_or("tile_size must be [w, h]")? as u32,
            a[1].as_u64().ok_or("tile_size must be [w, h]")? as u32,
        ),
        _ => (32, 32),
    };
    let mut map = TileMap::new(&tileset, tile_w, tile_h, width, height)?;
    for (i, e) in v.get("legend").and_then(Value::as_array).cloned().unwrap_or_default().iter().enumerate() {
        let key = e.get("key").and_then(Value::as_str).ok_or(format!("legend[{i}]: missing \"key\""))?;
        if key.len() != 1 {
            return Err(format!("legend[{i}]: key {key:?} must be exactly one character"));
        }
        let s = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        map.legend.push(LegendEntry {
            key: key.as_bytes()[0],
            tile: s("tile"),
            object: s("object"),
            name: s("name").unwrap_or_default(),
        });
    }
    let rows = v.get("rows").and_then(Value::as_array).ok_or("missing \"rows\"")?;
    if rows.len() != height as usize {
        return Err(format!("{} rows for height {height}", rows.len()));
    }
    for (y, r) in rows.iter().enumerate() {
        let r = r.as_str().ok_or(format!("rows[{y}] must be a string"))?;
        if r.len() != width as usize || !r.is_ascii() {
            return Err(format!("rows[{y}]: {} characters for width {width} (ASCII only)", r.chars().count()));
        }
        map.cells[y * width as usize..(y + 1) * width as usize].copy_from_slice(r.as_bytes());
    }
    if let Some(w) = v.get("world") {
        for l in w.get("lines").and_then(Value::as_array).cloned().unwrap_or_default() {
            map.world_lines.push(WorldLine {
                at: l.get("at").and_then(Value::as_u64).unwrap_or(0) as usize,
                text: l.get("text").and_then(Value::as_str).unwrap_or("").into(),
            });
        }
        map.world_final_newline = w.get("final_newline").and_then(Value::as_bool).unwrap_or(true);
    }
    map.validate()?;
    Ok(map)
}

pub fn to_json(map: &TileMap) -> String {
    let inline = |v: Value| io::inline(&v);
    let list = |items: Vec<String>| {
        if items.is_empty() {
            "[]".to_string()
        } else {
            format!("[\n    {}\n  ]", items.join(",\n    "))
        }
    };
    let legend = map
        .legend
        .iter()
        .map(|e| {
            let mut v = json!({"key": (e.key as char).to_string()});
            if let Some(t) = &e.tile {
                v["tile"] = json!(t);
            }
            if let Some(o) = &e.object {
                v["object"] = json!(o);
            }
            if !e.name.is_empty() {
                v["name"] = json!(e.name);
            }
            inline(v)
        })
        .collect();
    let rows = map.rows().iter().map(|r| serde_json::to_string(r).unwrap()).collect();
    let mut out = format!(
        "{{\n  \"format\": \"{FORMAT}\",\n  \"version\": {VERSION},\n  \"tileset\": {},\n  \"tile_size\": [{}, {}],\n  \
         \"width\": {},\n  \"height\": {},\n  \"legend\": {},\n  \"rows\": {}",
        serde_json::to_string(&map.tileset).unwrap(),
        map.tile_w,
        map.tile_h,
        map.width,
        map.height,
        list(legend),
        list(rows)
    );
    if !map.world_lines.is_empty() || !map.world_final_newline {
        let lines = map.world_lines.iter().map(|l| inline(json!({"at": l.at, "text": l.text}))).collect();
        out += &format!(
            ",\n  \"world\": {{\"final_newline\": {}, \"lines\": {}}}",
            map.world_final_newline,
            list(lines).replace("\n    ", "\n      ").replace("\n  ]", "\n    ]")
        );
    }
    out += "\n}\n";
    out
}

pub fn load(path: &Path) -> Result<TileMap, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(map: &TileMap, path: &Path) -> Result<(), String> {
    map.validate()?;
    io::write_atomic(path, to_json(map).as_bytes())
}

// ---- tileset and rendering ----------------------------------------------

/// The tiles of a tileset document: one image per frame, by frame name.
#[derive(Clone, Debug, Default)]
pub struct Tileset {
    pub tile_w: u32,
    pub tile_h: u32,
    pub names: Vec<String>,
    pub images: Vec<Image>,
}

impl Tileset {
    pub fn from_document(doc: &Document) -> Tileset {
        Tileset {
            tile_w: doc.width,
            tile_h: doc.height,
            names: doc.frames.iter().map(|f| f.name.clone()).collect(),
            images: (0..doc.frames.len()).map(|f| composite(doc, f)).collect(),
        }
    }

    pub fn load(path: &Path) -> Result<Tileset, String> {
        Ok(Tileset::from_document(&io::load(path)?))
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    pub fn image(&self, name: &str) -> Option<&Image> {
        self.index(name).map(|i| &self.images[i])
    }
}

/// An object document drawn standing on a cell.
#[derive(Clone, Debug)]
pub struct Object {
    pub image: Image,
    /// The point of the image placed on the cell's foot point: the
    /// `origin` marker if the document has one, else its bottom centre.
    pub anchor: (i32, i32),
}

impl Object {
    pub fn from_document(doc: &Document) -> Object {
        let anchor = doc
            .regions
            .iter()
            .find(|r| r.is_marker() && r.name == "origin")
            .and_then(|r| r.point_at(0))
            .unwrap_or((doc.width as i32 / 2, doc.height as i32));
        Object { image: composite(doc, 0), anchor }
    }

    /// Top-left corner in map pixels for an object on cell (x, y).
    pub fn top_left(&self, map: &TileMap, x: i32, y: i32) -> (i32, i32) {
        let foot = (x * map.tile_w as i32 + map.tile_w as i32 / 2, (y + 1) * map.tile_h as i32);
        (foot.0 - self.anchor.0, foot.1 - self.anchor.1)
    }
}

/// Everything needed to draw a map: its tileset and objects.
#[derive(Clone, Debug, Default)]
pub struct Assets {
    pub tileset: Tileset,
    pub objects: HashMap<u8, Object>,
    /// Problems while loading (missing tileset, object, tile name).
    pub warnings: Vec<String>,
}

impl Assets {
    /// Load the tileset and object documents of `map` (stored at `map_path`).
    /// Missing files become warnings and placeholders.
    pub fn load(map: &TileMap, map_path: &Path) -> Assets {
        let mut a = Assets::default();
        let ts = map.tileset_path(map_path);
        match Tileset::load(&ts) {
            Ok(t) => {
                if (t.tile_w, t.tile_h) != (map.tile_w, map.tile_h) {
                    a.warnings.push(format!(
                        "tileset {} is {}x{}, the map expects {}x{} tiles",
                        map.tileset, t.tile_w, t.tile_h, map.tile_w, map.tile_h
                    ));
                }
                a.tileset = t;
            }
            Err(e) => a.warnings.push(format!("tileset: {e}")),
        }
        for (key, p) in map.object_paths(map_path) {
            match io::load(&p) {
                Ok(d) => {
                    a.objects.insert(key, Object::from_document(&d));
                }
                Err(e) => a.warnings.push(format!("object for {:?}: {e}", key as char)),
            }
        }
        for e in &map.legend {
            if let Some(t) = &e.tile {
                if !a.tileset.names.is_empty() && a.tileset.index(t).is_none() {
                    a.warnings.push(format!("legend {:?}: tile {t:?} is not in the tileset", e.key as char));
                }
            }
        }
        a
    }
}

/// Placeholder for a key without a tile: dark checker with the key.
pub fn placeholder(w: u32, h: u32, key: u8) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let dark = ((x / 4) + (y / 4)) % 2 == 0;
            img.put(x, y, if dark { [70, 20, 70, 255] } else { [110, 40, 110, 255] });
        }
    }
    let scale = (w.min(h) / 12).max(1);
    let text = (key as char).to_string();
    let (tw, th) = (font::text_width(&text, scale), font::text_height(scale));
    font::draw_text(
        &mut img,
        (w as i32 - tw as i32) / 2,
        (h as i32 - th as i32) / 2,
        &text,
        scale,
        [255, 230, 120, 255],
    );
    img
}

/// The tile image of one cell (None for an empty cell).
pub fn cell_image<'a>(
    map: &TileMap,
    assets: &'a Assets,
    key: u8,
    cache: &'a mut HashMap<u8, Image>,
) -> Option<&'a Image> {
    if key == EMPTY {
        return None;
    }
    let entry = map.entry(key);
    if let Some(t) = entry.and_then(|e| e.tile.as_deref()) {
        if assets.tileset.index(t).is_some() {
            return assets.tileset.image(t);
        }
    } else if entry.is_some_and(|e| e.object.is_some()) {
        return None; // an object alone, on nothing
    }
    Some(cache.entry(key).or_insert_with(|| placeholder(map.tile_w, map.tile_h, key)))
}

/// The whole map at 1x: tiles, then objects row by row (lower rows in front).
pub fn render(map: &TileMap, assets: &Assets) -> Image {
    let (tw, th) = (map.tile_w, map.tile_h);
    let mut img = Image::new(map.width * tw, map.height * th);
    let mut cache = HashMap::new();
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            if let Some(tile) = cell_image(map, assets, map.get(x, y), &mut cache) {
                img.overlay(tile, x as u32 * tw, y as u32 * th);
            }
        }
    }
    draw_objects(map, assets, &mut img, 1);
    img
}

/// Objects onto an image of the map drawn at `scale`.
pub fn draw_objects(map: &TileMap, assets: &Assets, img: &mut Image, scale: u32) {
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            if let Some(o) = assets.objects.get(&map.get(x, y)) {
                let (ox, oy) = o.top_left(map, x, y);
                overlay_clipped(img, &o.image, ox * scale as i32, oy * scale as i32, scale);
            }
        }
    }
}

/// Blend `src` scaled by `scale` at (x, y), clipping at the edges.
pub fn overlay_clipped(dst: &mut Image, src: &Image, x: i32, y: i32, scale: u32) {
    for sy in 0..src.height * scale {
        for sx in 0..src.width * scale {
            let (dx, dy) = (x + sx as i32, y + sy as i32);
            if dx >= 0 && dy >= 0 && (dx as u32) < dst.width && (dy as u32) < dst.height {
                dst.blend(dx as u32, dy as u32, src.get(sx / scale, sy / scale));
            }
        }
    }
}

/// Review image: the map at `zoom`, with tile grid lines (`grid`) and
/// cell numbers every 5 cells along the top and left edge.
pub fn preview(map: &TileMap, assets: &Assets, zoom: u32, grid: bool) -> Image {
    let zoom = zoom.max(1);
    let base = render(map, assets).scaled(zoom);
    let (tw, th) = (map.tile_w * zoom, map.tile_h * zoom);
    let margin = 14;
    let mut img = Image::filled(base.width + margin, base.height + margin, [32, 32, 40, 255]);
    img.paste(&base, margin, margin);
    let line = [0, 0, 0, 90];
    if grid {
        for x in 1..map.width {
            for y in 0..base.height {
                img.blend(margin + x * tw, margin + y, line);
            }
        }
        for y in 1..map.height {
            for x in 0..base.width {
                img.blend(margin + x, margin + y * th, line);
            }
        }
    }
    let ink = [220, 220, 230, 255];
    for x in (0..map.width).step_by(5) {
        font::draw_text(&mut img, (margin + x * tw + 1) as i32, 4, &x.to_string(), 1, ink);
    }
    for y in (0..map.height).step_by(5) {
        let t = y.to_string();
        let w = font::text_width(&t, 1);
        font::draw_text(&mut img, (margin as i32 - w as i32 - 2).max(0), (margin + y * th + 1) as i32, &t, 1, ink);
    }
    img
}

// ---- diff -----------------------------------------------------------------

/// Changed cells between two maps of the same size: count and bbox.
pub fn cell_diff(before: &TileMap, after: &TileMap) -> (usize, Option<Rect>) {
    if (before.width, before.height) != (after.width, after.height) {
        return (after.cells.len(), Some(Rect::new(0, 0, after.width, after.height)));
    }
    let mut n = 0;
    let mut bb: Option<Rect> = None;
    for y in 0..after.height as i32 {
        for x in 0..after.width as i32 {
            if before.get(x, y) != after.get(x, y) {
                n += 1;
                let r = Rect::new(x, y, 1, 1);
                bb = Some(bb.map_or(r, |b| b.union(&r)));
            }
        }
    }
    (n, bb)
}

/// "N cells changed, x a..b y c..d" with the rows before and after.
pub fn describe_diff(before: &TileMap, after: &TileMap, max_rows: u32) -> String {
    let (n, bb) = cell_diff(before, after);
    let Some(bb) = bb else { return "no cells changed".into() };
    let mut s = format!("{n} cell(s) changed, {}\n", bb.describe());
    if bb.h > max_rows {
        s += &format!("({} rows, too many to print; use get_tilemap)\n", bb.h);
        return s;
    }
    s += "before:\n";
    s += &crate::diff::rows_block(|x, y| before.get(x, y), bb);
    s += "after:\n";
    s += &crate::diff::rows_block(|x, y| after.get(x, y), bb);
    s
}

// ---- world.txt ------------------------------------------------------------

/// Legend pairs in a comment line: "; .  Gras        ,  Gras (Variante)"
/// → [('.', "Gras"), (',', "Gras (Variante)")]. Tokens are separated by
/// two or more spaces; a single character followed by a longer token is
/// a pair.
pub fn legend_pairs(comment: &str) -> Vec<(u8, String)> {
    let body = comment.trim_start_matches(';').trim();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut spaces = 0;
    for c in body.chars() {
        if c == ' ' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 && !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        } else if spaces == 1 && !cur.is_empty() {
            cur.push(' ');
        }
        spaces = 0;
        cur.push(c);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        let t = tokens[i].as_bytes();
        if t.len() == 1 && is_valid_key(t[0]) && tokens[i + 1].chars().count() > 1 {
            out.push((t[0], tokens[i + 1].clone()));
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Read a `world.txt`: map rows, comment lines kept verbatim, legend from
/// the comments with tiles from [`SANDBOX_TILES`]. Characters without a
/// tile stay in the legend as placeholders and are reported. `tileset` is
/// stored as given (relative to the map); object documents are expected
/// next to the tileset.
pub fn import_world(text: &str, tileset: &str, tile_size: (u32, u32)) -> Result<(TileMap, Vec<String>), String> {
    let mut rows: Vec<&str> = Vec::new();
    let mut lines: Vec<WorldLine> = Vec::new();
    let final_newline = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(text);
    for line in body.split('\n') {
        if line.starts_with(';') || line.trim().is_empty() {
            lines.push(WorldLine { at: rows.len(), text: line.to_string() });
        } else {
            if !line.is_ascii() {
                return Err(format!("map row {}: only ASCII characters are allowed", rows.len()));
            }
            rows.push(line);
        }
    }
    if rows.is_empty() {
        return Err("no map rows (lines that are not comments)".into());
    }
    let width = rows.iter().map(|r| r.len()).max().unwrap() as u32;
    let mut map = TileMap::new(tileset, tile_size.0, tile_size.1, width, rows.len() as u32)?;
    map.world_lines = lines;
    map.world_final_newline = final_newline;
    let mut notes = Vec::new();
    let tile_of = |k: u8| SANDBOX_TILES.iter().find(|(c, _, _)| *c as u32 == k as u32);
    let object_dir = Path::new(tileset).parent().map(Path::to_path_buf).unwrap_or_default();
    let add = |map: &mut TileMap, key: u8, name: String| {
        if map.entry(key).is_some() || key == EMPTY {
            return;
        }
        let (tile, object) = match tile_of(key) {
            Some((_, t, o)) => (Some(t.to_string()), o.map(|o| object_dir.join(o).to_string_lossy().into_owned())),
            None => (None, None),
        };
        map.legend.push(LegendEntry { key, tile, object, name });
    };
    for l in map.world_lines.clone() {
        for (k, name) in legend_pairs(&l.text) {
            add(&mut map, k, name);
        }
    }
    for (y, r) in rows.iter().enumerate() {
        for (x, k) in r.bytes().enumerate() {
            if !is_valid_key(k) && k != EMPTY {
                return Err(format!("map row {y}, x {x}: character {:?} cannot be a map key", k as char));
            }
            if k != EMPTY && map.entry(k).is_none() {
                notes.push(format!("{:?} (first at x {x}, y {y}) is not in the legend comments", k as char));
                add(&mut map, k, String::new());
            }
            map.cells[y * width as usize + x] = k;
        }
    }
    for e in &map.legend {
        if e.tile.is_none() && e.object.is_none() {
            notes.push(format!(
                "{:?} has no tile in SANDBOX_TILES: shown as a placeholder, kept as it is",
                e.key as char
            ));
        }
    }
    Ok((map, notes))
}

/// Write the map as `world.txt`: the kept comment lines at their places,
/// rows without trailing empty cells. Returns the text and warnings
/// (keys the comments of the file do not explain).
pub fn export_world(map: &TileMap) -> (String, Vec<String>) {
    let rows: Vec<String> = map.rows().into_iter().map(|r| r.trim_end_matches(EMPTY as char).to_string()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut li = 0;
    for (y, r) in rows.iter().enumerate() {
        while li < map.world_lines.len() && map.world_lines[li].at <= y {
            out.push(map.world_lines[li].text.clone());
            li += 1;
        }
        out.push(r.clone());
    }
    out.extend(map.world_lines[li..].iter().map(|l| l.text.clone()));
    let mut text = out.join("\n");
    if map.world_final_newline {
        text.push('\n');
    }
    let explained: Vec<u8> = map.world_lines.iter().flat_map(|l| legend_pairs(&l.text)).map(|(k, _)| k).collect();
    let mut warnings = Vec::new();
    let mut seen = Vec::new();
    for &k in &map.cells {
        if k != EMPTY && !explained.contains(&k) && !seen.contains(&k) {
            seen.push(k);
        }
    }
    if !seen.is_empty() {
        let list: Vec<String> = seen.iter().map(|k| format!("{:?}", *k as char)).collect();
        warnings.push(format!(
            "keys not explained by the world.txt legend comments: {} (the game draws plain grass there)",
            list.join(", ")
        ));
    }
    if rows.iter().any(|r| r.contains(EMPTY as char)) {
        warnings.push("empty cells inside rows are written as spaces".into());
    }
    (text, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: &str =
        "; Karte\n; .  Gras        ,  Gras (Variante)   ~  Wasser\n; T  Baum   Q  Quelle\n.,~T\n..~Z\n\n; Ende\n";

    #[test]
    fn world_roundtrip_is_exact() {
        let (map, notes) = import_world(WORLD, "art/tiles32.pxe.json", (32, 32)).unwrap();
        assert_eq!((map.width, map.height), (4, 2));
        assert_eq!(map.entry(b'.').unwrap().tile.as_deref(), Some("grass"));
        assert_eq!(map.entry(b'~').unwrap().name, "Wasser");
        assert_eq!(map.entry(b'T').unwrap().object.as_deref(), Some("art/tree64.pxe.json"));
        // Q (in the comments) and Z (only in the map) have no tile: kept, reported
        assert!(map.entry(b'Q').unwrap().tile.is_none());
        assert!(map.entry(b'Z').is_some());
        assert!(notes.iter().any(|n| n.contains("'Z'")));
        let (text, warnings) = export_world(&map);
        assert_eq!(text, WORLD);
        assert!(warnings[0].contains("'Z'"), "{warnings:?}");
        // and through the file format
        let back = from_json(&to_json(&map)).unwrap();
        assert_eq!(back, map);
        assert_eq!(export_world(&back).0, WORLD);
    }

    #[test]
    fn legend_comment_pairs() {
        let p = legend_pairs("; ^  Anhöhe (Plateau, Felswand davor entsteht automatisch)   H  Treppe (direkt)");
        assert_eq!(
            p,
            vec![
                (b'^', "Anhöhe (Plateau, Felswand davor entsteht automatisch)".into()),
                (b'H', "Treppe (direkt)".into())
            ]
        );
        assert!(legend_pairs("; Weltkarte: ein Zeichen = eine Kachel. Zeilen mit ; sind Kommentare.").is_empty());
        assert!(legend_pairs("; Deko (Blumen, Steine) wird pro Feld fest ausgewürfelt.").is_empty());
    }

    #[test]
    fn edit_resize_fill_diff() {
        let mut m = TileMap::new("t.pxe.json", 2, 2, 4, 3).unwrap();
        let g = m.key_for_tile("grass").unwrap();
        assert_eq!(g, b'0');
        assert_eq!(m.key_for_tile("grass").unwrap(), g);
        let before = m.clone();
        assert_eq!(m.fill_rect(Rect::new(0, 0, 4, 3), g), 12);
        let w = m.key_for_tile("water").unwrap();
        m.set(1, 1, w);
        m.set(2, 1, w);
        assert_eq!(m.flood_fill(1, 1, b'0'), 2);
        assert_eq!(m.flood_fill(0, 0, w), 12);
        let (n, bb) = cell_diff(&before, &m);
        assert_eq!((n, bb), (12, Some(Rect::new(0, 0, 4, 3))));
        assert!(describe_diff(&before, &m, 40).contains("after:"));
        m.resize(5, 2, EMPTY).unwrap();
        assert_eq!(m.rows(), vec!["1111 ", "1111 "]);
        assert!(m.resize(0, 2, EMPTY).is_err());
        assert_eq!(m.find_key("water", None).unwrap(), w);
        assert!(m.find_key("lava", None).unwrap_err().contains("legend"));
    }

    #[test]
    fn rejects_bad_files() {
        let ok = r#"{"format":"pixel-editor-tilemap","version":1,"tileset":"t.pxe.json","width":2,"height":1,
            "legend":[{"key":"a","tile":"grass"}],"rows":["a "]}"#;
        assert!(from_json(ok).is_ok());
        assert!(from_json(&ok.replace("\"a \"", "\"ab\"")).unwrap_err().contains("not in the legend"));
        assert!(from_json(&ok.replace("\"a \"", "\"a\"")).unwrap_err().contains("width"));
        assert!(from_json(&ok.replace("\"version\":1", "\"version\":9")).unwrap_err().contains("newer"));
        assert!(from_json(&ok.replace("\"key\":\"a\"", "\"key\":\";\"")).is_err());
    }

    #[test]
    fn render_tiles_objects_placeholders() {
        let mut ts = Document::new(2, 2, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        ts.frames[0].name = "grass".into();
        ts.set_rows(0, 0, 0, 0, &["55".into(), "55".into()]).unwrap();
        let mut obj = Document::new(2, 3, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        obj.set_rows(0, 0, 0, 0, &["11".into(), "11".into(), "11".into()]).unwrap();
        let mut m = TileMap::new("t", 2, 2, 3, 2).unwrap();
        let g = m.key_for_tile("grass").unwrap();
        m.fill_rect(Rect::new(0, 0, 3, 2), g);
        m.legend.push(LegendEntry {
            key: b'T',
            tile: Some("grass".into()),
            object: Some("o".into()),
            name: "Baum".into(),
        });
        m.legend.push(LegendEntry { key: b'?', tile: None, object: None, name: String::new() });
        m.set(1, 1, b'T');
        m.set(2, 0, b'?');
        let mut assets = Assets { tileset: Tileset::from_document(&ts), ..Default::default() };
        assets.objects.insert(b'T', Object::from_document(&obj));
        let img = render(&m, &assets);
        assert_eq!((img.width, img.height), (6, 4));
        let grass = ts.color_of(b'5').unwrap().0;
        let trunk = obj.color_of(b'1').unwrap().0;
        assert_eq!(img.get(0, 0), grass);
        // the 2x3 object stands on cell (1, 1): bottom centre at (3, 4) -> x 2..4, y 1..4
        assert_eq!(img.get(2, 1), trunk);
        assert_eq!(img.get(3, 3), trunk);
        assert_eq!(img.get(2, 0), grass);
        assert_ne!(img.get(4, 0), grass, "placeholder");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_to_map(Path::new("/a/maps/w.tilemap.json"), Path::new("/a/art/t.pxe.json")),
            "../art/t.pxe.json"
        );
        assert_eq!(relative_to_map(Path::new("/a/w.tilemap.json"), Path::new("/a/t.pxe.json")), "t.pxe.json");
        assert_eq!(stem(Path::new("/a/w.tilemap.json")), "w");
        let here = std::env::current_dir().unwrap();
        assert_eq!(relative_to_map(Path::new("w.tilemap.json"), &here.join("art/t.pxe.json")), "art/t.pxe.json");
        assert_eq!(relative_to_map(Path::new("maps/../w.tilemap.json"), Path::new("art/t.pxe.json")), "art/t.pxe.json");
    }
}
