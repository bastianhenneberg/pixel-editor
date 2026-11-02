//! The in-memory document: palette, layers, frames, cels and tags.
//!
//! Pixels are stored as palette keys (single ASCII bytes), exactly like the
//! rows in the file format. `.` is always transparent.
//!
//! Each layer keeps a pool of cel buffers and a frame -> pool index map.
//! Frames that share a pool index are *linked cels* (as in Aseprite):
//! editing one edits all of them. Buffers are `Arc`-shared, so cloning a
//! document (undo snapshots) and unlinked copies are cheap; writes go
//! through `Arc::make_mut` (copy on write).

use std::sync::Arc;

use crate::color::Rgba;
use crate::palettes;

pub const TRANSPARENT: u8 = b'.';
/// In row strings passed to `set_rows`, `-` means "leave pixel unchanged".
pub const SKIP: u8 = b'-';
pub const FORMAT_VERSION: u32 = 1;
pub const MAX_SIZE: u32 = 1024;

/// Order in which new palette keys are handed out.
pub const KEY_ORDER: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!$%&*+=?@^~<>/|#:_,()[]{}'`";

pub type Cel = Arc<Vec<u8>>;

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    pub key: u8,
    pub color: Rgba,
    /// Human-readable name ("crust_dark"); empty = unnamed.
    pub name: String,
}

impl PaletteEntry {
    /// "9 harness" or just "9".
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            (self.key as char).to_string()
        } else {
            format!("{} {}", self.key as char, self.name)
        }
    }
}

/// A color ramp: palette keys ordered from dark to light (e.g. crust
/// `3 8 9 a`). The shading tool steps a pixel along its ramp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ramp {
    pub name: String,
    pub keys: Vec<u8>,
}

impl Ramp {
    pub fn new(name: &str, keys: &[u8]) -> Ramp {
        Ramp { name: name.to_string(), keys: keys.to_vec() }
    }

    /// The keys as a string, "389a".
    pub fn keys_str(&self) -> String {
        self.keys.iter().map(|k| *k as char).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    /// Locked layers refuse all pixel edits (GUI and MCP).
    pub locked: bool,
    /// frame index -> index into `pool`
    frames: Vec<usize>,
    pool: Vec<Cel>,
}

impl Layer {
    fn new(name: String, frame_count: usize, empty: &Cel) -> Self {
        Layer {
            name,
            visible: true,
            opacity: 1.0,
            locked: false,
            frames: (0..frame_count).collect(),
            pool: vec![empty.clone(); frame_count],
        }
    }

    pub fn cel(&self, frame: usize) -> &Cel {
        &self.pool[self.frames[frame]]
    }

    /// Mutable pixels of a frame's cel; linked frames see the change too.
    pub fn cel_mut(&mut self, frame: usize) -> &mut Vec<u8> {
        Arc::make_mut(&mut self.pool[self.frames[frame]])
    }

    /// Replace the content of a frame's cel (and of all frames linked to it).
    pub fn set_cel(&mut self, frame: usize, data: Cel) {
        self.pool[self.frames[frame]] = data;
    }

    /// All frames sharing this frame's cel (including itself), ascending.
    pub fn linked_frames(&self, frame: usize) -> Vec<usize> {
        let id = self.frames[frame];
        (0..self.frames.len()).filter(|f| self.frames[*f] == id).collect()
    }

    pub fn is_linked(&self, frame: usize) -> bool {
        let id = self.frames[frame];
        self.frames.iter().filter(|i| **i == id).count() > 1
    }

    /// First frame of the link group this frame belongs to.
    pub fn link_source(&self, frame: usize) -> usize {
        let id = self.frames[frame];
        self.frames.iter().position(|i| *i == id).unwrap()
    }

    /// Make `frame` share the cel of `target`.
    pub fn link(&mut self, frame: usize, target: usize) {
        self.frames[frame] = self.frames[target];
        self.compact();
    }

    /// Give `frame` its own copy of its cel.
    pub fn unlink(&mut self, frame: usize) {
        if self.is_linked(frame) {
            self.pool.push(self.cel(frame).clone());
            self.frames[frame] = self.pool.len() - 1;
        }
    }

    fn insert_frame(&mut self, index: usize, cel: Result<Cel, usize>) {
        let id = match cel {
            Ok(data) => {
                self.pool.push(data);
                self.pool.len() - 1
            }
            Err(linked_to) => self.frames[linked_to],
        };
        self.frames.insert(index, id);
    }

    fn remove_frame(&mut self, index: usize) {
        self.frames.remove(index);
        self.compact();
    }

    fn move_frame(&mut self, from: usize, to: usize) {
        let id = self.frames.remove(from);
        self.frames.insert(to, id);
    }

    /// Drop pool entries no frame refers to (keeps first-use order).
    fn compact(&mut self) {
        let mut map = vec![usize::MAX; self.pool.len()];
        let mut pool = Vec::new();
        for id in &mut self.frames {
            if map[*id] == usize::MAX {
                map[*id] = pool.len();
                pool.push(self.pool[*id].clone());
            }
            *id = map[*id];
        }
        self.pool = pool;
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

impl PartialEq for Layer {
    /// Equal content and equal link structure; pool layout does not matter.
    fn eq(&self, o: &Self) -> bool {
        self.name == o.name
            && self.visible == o.visible
            && self.opacity == o.opacity
            && self.locked == o.locked
            && self.frames.len() == o.frames.len()
            && (0..self.frames.len())
                .all(|f| self.link_source(f) == o.link_source(f) && self.cel(f)[..] == o.cel(f)[..])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Unique name; "f0", "f1", ... unless set.
    pub name: String,
    pub duration_ms: u32,
    /// Locked frames refuse all pixel edits on every layer.
    pub locked: bool,
}

/// Rectangle in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w as i32 && y < self.y + self.h as i32
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }

    /// Smallest rect containing both.
    pub fn union(&self, o: &Rect) -> Rect {
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        let x1 = (self.x + self.w as i32).max(o.x + o.w as i32);
        let y1 = (self.y + self.h as i32).max(o.y + o.h as i32);
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn describe(&self) -> String {
        format!(
            "x {}..{} y {}..{} ({}x{})",
            self.x,
            self.x + self.w as i32 - 1,
            self.y,
            self.y + self.h as i32 - 1,
            self.w,
            self.h
        )
    }
}

/// From `frame` on, a region has this rect (or is hidden, `rect: None`)
/// until the next key. Same model as Aseprite slice keys.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionKey {
    pub frame: usize,
    pub rect: Option<Rect>,
}

/// What a region is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RegionKind {
    /// A part of the sprite; `locked` protects its pixels and its definition.
    #[default]
    Area,
    /// A point (the rect's top-left, e.g. an origin or hand anchor);
    /// `locked` protects only its definition, never the pixels under it.
    Marker,
}

impl RegionKind {
    pub fn id(self) -> &'static str {
        match self {
            RegionKind::Area => "area",
            RegionKind::Marker => "marker",
        }
    }

    pub fn parse(s: &str) -> Result<RegionKind, String> {
        match s {
            "area" => Ok(RegionKind::Area),
            "marker" => Ok(RegionKind::Marker),
            _ => Err(format!("unknown region kind {s:?} (area, marker)")),
        }
    }
}

/// A named rectangle ("left_shoe", "face") that the user and Claude use to
/// talk about and edit exactly one part of the sprite, or a named point
/// (marker).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: String,
    pub kind: RegionKind,
    /// Sorted by frame, at least one key.
    pub keys: Vec<RegionKey>,
    /// Restrict the region to one layer (by name); None = all layers.
    pub layer: Option<String>,
    /// Locked regions protect their pixels from every edit.
    pub locked: bool,
    pub color: Option<Rgba>,
}

impl Region {
    /// A region with one rect on all frames.
    pub fn new(name: &str, rect: Rect) -> Region {
        Region {
            name: name.into(),
            kind: RegionKind::Area,
            keys: vec![RegionKey { frame: 0, rect: Some(rect) }],
            layer: None,
            locked: false,
            color: None,
        }
    }

    /// A marker at one point on all frames (stored as a 1x1 rect).
    pub fn marker(name: &str, x: i32, y: i32) -> Region {
        Region { kind: RegionKind::Marker, ..Region::new(name, Rect::new(x, y, 1, 1)) }
    }

    pub fn is_marker(&self) -> bool {
        self.kind == RegionKind::Marker
    }

    /// A marker's point on a frame (the rect's top-left).
    pub fn point_at(&self, frame: usize) -> Option<(i32, i32)> {
        self.rect_at(frame).map(|r| (r.x, r.y))
    }

    /// Does `locked` protect the pixels inside this region?
    pub fn protects_pixels(&self) -> bool {
        self.locked && self.kind == RegionKind::Area
    }

    /// The region's rect on a frame (None = not present there).
    pub fn rect_at(&self, frame: usize) -> Option<Rect> {
        self.keys.iter().rev().find(|k| k.frame <= frame).and_then(|k| k.rect)
    }

    /// Does the region apply to this layer (index) of `doc`?
    pub fn applies_to_layer(&self, doc: &Document, layer: usize) -> bool {
        self.layer.as_ref().is_none_or(|n| doc.layers[layer].name == *n)
    }

    /// Set the rect for exactly the frames `from..=to`, keeping other frames.
    pub fn set_rect_for(&mut self, from: usize, to: usize, rect: Option<Rect>, frame_count: usize) {
        let after = self.rect_at(to + 1);
        let before_from: Vec<RegionKey> = self.keys.iter().filter(|k| k.frame < from).cloned().collect();
        let after_to: Vec<RegionKey> = self.keys.iter().filter(|k| k.frame > to + 1).cloned().collect();
        let mut keys = before_from;
        keys.push(RegionKey { frame: from, rect });
        if to + 1 < frame_count && !self.keys.iter().any(|k| k.frame == to + 1) {
            keys.push(RegionKey { frame: to + 1, rect: after });
        } else if let Some(k) = self.keys.iter().find(|k| k.frame == to + 1) {
            keys.push(k.clone());
        }
        keys.extend(after_to);
        self.keys = keys;
        self.normalize();
    }

    /// Sort keys and drop redundant ones (same rect as the previous key).
    pub fn normalize(&mut self) {
        self.keys.sort_by_key(|k| k.frame);
        self.keys.dedup_by(|b, a| a.frame == b.frame);
        let mut out: Vec<RegionKey> = Vec::new();
        for k in self.keys.drain(..) {
            if out.last().is_some_and(|p| p.rect == k.rect) {
                continue;
            }
            if out.is_empty() && k.rect.is_none() {
                continue;
            }
            out.push(k);
        }
        self.keys = out;
    }

    fn frame_inserted(&mut self, index: usize) {
        for k in &mut self.keys {
            if k.frame >= index && k.frame > 0 {
                k.frame += 1;
            }
        }
    }

    fn frame_removed(&mut self, index: usize) {
        for k in &mut self.keys {
            if k.frame > index {
                k.frame -= 1;
            }
        }
        self.normalize();
    }
}

/// Playback direction of a tag (the same four modes as Aseprite).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Forward,
    Reverse,
    /// 0 1 2 1
    PingPong,
    /// 2 1 0 1
    PingPongReverse,
}

impl Direction {
    pub const ALL: [Direction; 4] =
        [Direction::Forward, Direction::Reverse, Direction::PingPong, Direction::PingPongReverse];

    pub fn id(self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Reverse => "reverse",
            Direction::PingPong => "pingpong",
            Direction::PingPongReverse => "pingpong_reverse",
        }
    }

    pub fn parse(s: &str) -> Result<Direction, String> {
        Direction::ALL
            .into_iter()
            .find(|d| d.id() == s)
            .ok_or_else(|| format!("unknown direction {s:?} (forward, reverse, pingpong, pingpong_reverse)"))
    }
}

/// An animation: an inclusive frame range played in a direction.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Tag {
    pub name: String,
    pub from: usize,
    pub to: usize,
    pub direction: Direction,
    pub looping: bool,
    /// Display color (timeline); None = automatic.
    pub color: Option<Rgba>,
}

impl Tag {
    pub fn new(name: &str, from: usize, to: usize) -> Tag {
        Tag {
            name: name.to_string(),
            from: from.min(to),
            to: from.max(to),
            direction: Direction::Forward,
            looping: true,
            color: None,
        }
    }

    pub fn contains(&self, frame: usize) -> bool {
        (self.from..=self.to).contains(&frame)
    }

    /// Frame indices in playback order for one pass.
    pub fn sequence(&self) -> Vec<usize> {
        let fwd: Vec<usize> = (self.from..=self.to).collect();
        let rev: Vec<usize> = fwd.iter().rev().copied().collect();
        let inner = |v: &[usize]| -> Vec<usize> {
            if v.len() > 2 {
                v[1..v.len() - 1].iter().rev().copied().collect()
            } else {
                Vec::new()
            }
        };
        match self.direction {
            Direction::Forward => fwd,
            Direction::Reverse => rev,
            Direction::PingPong => {
                let back = inner(&fwd);
                [fwd, back].concat()
            }
            Direction::PingPongReverse => {
                let back = inner(&rev);
                [rev, back].concat()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub palette: Vec<PaletteEntry>,
    pub layers: Vec<Layer>,
    pub frames: Vec<Frame>,
    pub tags: Vec<Tag>,
    pub regions: Vec<Region>,
    /// Color ramps for shading (see `Ramp`).
    pub ramps: Vec<Ramp>,
}

pub fn is_valid_key(key: u8) -> bool {
    key.is_ascii_graphic() && !matches!(key, b'.' | b'-' | b';' | b'"' | b'\\')
}

impl Document {
    /// New document with one layer, one frame and the given palette.
    pub fn new(width: u32, height: u32, palette: Vec<PaletteEntry>) -> Result<Self, String> {
        if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
            return Err(format!("size must be 1..={MAX_SIZE}, got {width}x{height}"));
        }
        let mut doc = Document {
            width,
            height,
            palette,
            layers: Vec::new(),
            frames: vec![Frame { name: "f0".into(), duration_ms: 100, locked: false }],
            tags: Vec::new(),
            regions: Vec::new(),
            ramps: Vec::new(),
        };
        doc.add_layer("Ebene 1", 0);
        Ok(doc)
    }

    pub fn builtin_palette(name: &str) -> Result<Vec<PaletteEntry>, String> {
        let p = palettes::get(name)
            .ok_or_else(|| format!("unknown palette {name:?}, available: {}", palettes::names().join(", ")))?;
        Ok(p.iter()
            .map(|(k, c)| PaletteEntry { key: *k as u8, color: Rgba::parse(c).unwrap(), name: String::new() })
            .collect())
    }

    pub fn pixel_count(&self) -> usize {
        (self.width * self.height) as usize
    }

    pub fn empty_cel(&self) -> Cel {
        Arc::new(vec![TRANSPARENT; self.pixel_count()])
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    // ---- palette ------------------------------------------------------

    pub fn color_of(&self, key: u8) -> Option<Rgba> {
        if key == TRANSPARENT {
            return None;
        }
        self.palette.iter().find(|p| p.key == key).map(|p| p.color)
    }

    /// Lookup table key -> color (None = transparent/unknown).
    pub fn color_table(&self) -> [Option<Rgba>; 256] {
        let mut t = [None; 256];
        for p in &self.palette {
            t[p.key as usize] = Some(p.color);
        }
        t
    }

    pub fn has_key(&self, key: u8) -> bool {
        key == TRANSPARENT || self.palette.iter().any(|p| p.key == key)
    }

    pub fn palette_keys(&self) -> String {
        self.palette.iter().map(|p| p.key as char).collect()
    }

    pub fn check_key(&self, key: u8) -> Result<(), String> {
        if self.has_key(key) {
            Ok(())
        } else {
            Err(format!(
                "key {:?} is not in the palette (valid keys: \".{}\"; add colors with set_palette_color)",
                key as char,
                self.palette_keys()
            ))
        }
    }

    pub fn next_free_key(&self) -> Option<u8> {
        KEY_ORDER.bytes().find(|k| !self.palette.iter().any(|p| p.key == *k))
    }

    /// Add or change a palette color. Returns true if a new entry was added.
    pub fn set_palette_color(&mut self, key: u8, color: Rgba) -> Result<bool, String> {
        if !is_valid_key(key) {
            return Err(format!("invalid palette key {:?} (printable ASCII except . - ; \" \\)", key as char));
        }
        if let Some(p) = self.palette.iter_mut().find(|p| p.key == key) {
            p.color = color;
            Ok(false)
        } else {
            self.palette.push(PaletteEntry { key, color, name: String::new() });
            Ok(true)
        }
    }

    pub fn key_in_use(&self, key: u8) -> bool {
        self.layers.iter().any(|l| l.pool.iter().any(|c| c.contains(&key)))
    }

    /// Remove a palette entry; pixels using it become transparent, ramps
    /// lose the key (a ramp left with fewer than two keys is removed).
    pub fn remove_palette_color(&mut self, key: u8) {
        self.palette.retain(|p| p.key != key);
        for r in &mut self.ramps {
            r.keys.retain(|k| *k != key);
        }
        self.ramps.retain(|r| r.keys.len() >= 2);
        for layer in &mut self.layers {
            for cel in &mut layer.pool {
                if cel.contains(&key) {
                    for v in Arc::make_mut(cel).iter_mut() {
                        if *v == key {
                            *v = TRANSPARENT;
                        }
                    }
                }
            }
        }
    }

    // ---- pixels -------------------------------------------------------

    pub fn cel(&self, layer: usize, frame: usize) -> &[u8] {
        self.layers[layer].cel(frame)
    }

    pub fn cel_mut(&mut self, layer: usize, frame: usize) -> &mut Vec<u8> {
        self.layers[layer].cel_mut(frame)
    }

    pub fn cel_is_empty(&self, layer: usize, frame: usize) -> bool {
        self.cel(layer, frame).iter().all(|k| *k == TRANSPARENT)
    }

    pub fn get(&self, layer: usize, frame: usize, x: i32, y: i32) -> u8 {
        if !self.in_bounds(x, y) {
            return TRANSPARENT;
        }
        self.cel(layer, frame)[(y as u32 * self.width + x as u32) as usize]
    }

    /// Set one pixel; out-of-bounds writes are ignored. Returns true if changed.
    pub fn set(&mut self, layer: usize, frame: usize, x: i32, y: i32, key: u8) -> bool {
        if !self.in_bounds(x, y) {
            return false;
        }
        let i = (y as u32 * self.width + x as u32) as usize;
        if self.cel(layer, frame)[i] == key {
            return false;
        }
        self.cel_mut(layer, frame)[i] = key;
        true
    }

    /// Set many pixels to one key. Returns the number changed.
    pub fn set_points(&mut self, layer: usize, frame: usize, points: &[(i32, i32)], key: u8) -> usize {
        points.iter().filter(|(x, y)| self.set(layer, frame, *x, *y, key)).count()
    }

    /// Write rows of keys with the top-left at (x, y). `-` skips a pixel.
    /// Keys are validated before anything is written; the error names the
    /// row and column of the first bad key.
    pub fn set_rows(&mut self, layer: usize, frame: usize, x: i32, y: i32, rows: &[String]) -> Result<usize, String> {
        for (r, row) in rows.iter().enumerate() {
            for (c, k) in row.bytes().enumerate() {
                if k != SKIP && !self.has_key(k) {
                    return Err(format!(
                        "rows[{r}][{c}] (x={}, y={}): {}",
                        x + c as i32,
                        y + r as i32,
                        self.check_key(k).unwrap_err()
                    ));
                }
            }
            if !row.is_ascii() {
                return Err(format!("rows[{r}] contains non-ASCII characters"));
            }
        }
        let mut changed = 0;
        for (dy, row) in rows.iter().enumerate() {
            for (dx, k) in row.bytes().enumerate() {
                if k != SKIP && self.set(layer, frame, x + dx as i32, y + dy as i32, k) {
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// The cel as row strings.
    pub fn rows(&self, layer: usize, frame: usize) -> Vec<String> {
        self.cel(layer, frame).chunks(self.width as usize).map(|r| String::from_utf8_lossy(r).into_owned()).collect()
    }

    /// Topmost visible non-transparent key at a pixel.
    pub fn top_key(&self, frame: usize, x: i32, y: i32) -> u8 {
        for l in (0..self.layers.len()).rev() {
            if self.layers[l].visible {
                let k = self.get(l, frame, x, y);
                if k != TRANSPARENT {
                    return k;
                }
            }
        }
        TRANSPARENT
    }

    /// Bounding box (x, y, w, h) of the non-transparent pixels of a cel.
    pub fn cel_bounds(&self, layer: usize, frame: usize) -> Option<(u32, u32, u32, u32)> {
        let (w, cel) = (self.width as usize, self.cel(layer, frame));
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for (i, k) in cel.iter().enumerate() {
            if *k != TRANSPARENT {
                let (x, y) = (i % w, i / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        (x0 != usize::MAX).then(|| (x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
    }

    // ---- layers -------------------------------------------------------

    pub fn unique_layer_name(&self, base: &str) -> String {
        unique_name(base, |n| self.layers.iter().any(|l| l.name == n))
    }

    /// Insert an empty layer at `index` (0 = bottom). Returns its index.
    pub fn add_layer(&mut self, name: &str, index: usize) -> usize {
        let name = self.unique_layer_name(name);
        let empty = self.empty_cel();
        let index = index.min(self.layers.len());
        self.layers.insert(index, Layer::new(name, self.frames.len(), &empty));
        index
    }

    pub fn delete_layer(&mut self, index: usize) -> Result<(), String> {
        if self.layers.len() <= 1 {
            return Err("cannot delete the last layer".into());
        }
        self.check_layer(index)?;
        if self.layers[index].locked {
            return Err(format!("layer {index} {:?} is locked", self.layers[index].name));
        }
        let name = self.layers.remove(index).name;
        if !self.layers.iter().any(|l| l.name == name) {
            for r in &mut self.regions {
                if r.layer.as_deref() == Some(&name) {
                    r.layer = None;
                }
            }
        }
        Ok(())
    }

    pub fn move_layer(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.check_layer(from)?;
        self.check_layer(to)?;
        let l = self.layers.remove(from);
        self.layers.insert(to, l);
        Ok(())
    }

    pub fn check_layer(&self, index: usize) -> Result<(), String> {
        if index < self.layers.len() {
            Ok(())
        } else {
            Err(format!("layer index {index} out of range: {}", self.layer_list()))
        }
    }

    fn layer_list(&self) -> String {
        let names: Vec<String> = self.layers.iter().enumerate().map(|(i, l)| format!("{i} {:?}", l.name)).collect();
        format!("layers are [{}] (0 = bottom)", names.join(", "))
    }

    /// Resolve a layer by exact name or numeric index.
    pub fn find_layer(&self, name_or_index: &str) -> Result<usize, String> {
        if let Some(i) = self.layers.iter().position(|l| l.name == name_or_index) {
            return Ok(i);
        }
        match name_or_index.parse::<usize>() {
            Ok(i) => self.check_layer(i).map(|_| i),
            Err(_) => Err(format!("no layer {name_or_index:?}; {}", self.layer_list())),
        }
    }

    // ---- frames -------------------------------------------------------

    pub fn unique_frame_name(&self, base: &str) -> String {
        unique_name(base, |n| self.frames.iter().any(|f| f.name == n))
    }

    fn next_frame_name(&self) -> String {
        (0..).map(|i| format!("f{i}")).find(|n| !self.frames.iter().any(|f| &f.name == n)).unwrap()
    }

    /// Insert a frame at `index`: empty, a copy of another frame, or (with
    /// `linked`) sharing the other frame's cels on every layer. Tags that
    /// span the insert position grow by one frame. Returns the new index.
    pub fn add_frame(&mut self, index: usize, copy_from: Option<usize>, linked: bool) -> Result<usize, String> {
        if let Some(src) = copy_from {
            self.check_frame(src)?;
        }
        let index = index.min(self.frames.len());
        let duration_ms = copy_from
            .map(|s| self.frames[s].duration_ms)
            .unwrap_or_else(|| self.frames.last().map_or(100, |f| f.duration_ms));
        let name = self.next_frame_name();
        let empty = self.empty_cel();
        for layer in &mut self.layers {
            let cel = match copy_from {
                Some(src) if linked => Err(src),
                Some(src) => Ok(layer.cel(src).clone()),
                None => Ok(empty.clone()),
            };
            layer.insert_frame(index, cel);
        }
        self.frames.insert(index, Frame { name, duration_ms, locked: false });
        for r in &mut self.regions {
            r.frame_inserted(index);
        }
        for tag in &mut self.tags {
            if tag.from >= index {
                tag.from += 1;
            }
            if tag.to >= index {
                tag.to += 1;
            }
        }
        Ok(index)
    }

    /// Delete a frame. Tags shrink; a tag that only had this frame is removed.
    pub fn delete_frame(&mut self, index: usize) -> Result<(), String> {
        if self.frames.len() <= 1 {
            return Err("cannot delete the last frame".into());
        }
        self.check_frame(index)?;
        if self.frames[index].locked {
            return Err(format!("frame {} is locked", self.frame_label_long(index)));
        }
        self.frames.remove(index);
        for layer in &mut self.layers {
            layer.remove_frame(index);
        }
        for r in &mut self.regions {
            r.frame_removed(index);
        }
        self.tags.retain(|t| !(t.from == index && t.to == index));
        for tag in &mut self.tags {
            if tag.from > index {
                tag.from -= 1;
            }
            if tag.to >= index {
                tag.to -= 1;
            }
        }
        Ok(())
    }

    /// Move a frame (with its cels). Tags are positional and stay as they are.
    pub fn move_frame(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.check_frame(from)?;
        self.check_frame(to)?;
        if from == to {
            return Ok(());
        }
        let f = self.frames.remove(from);
        self.frames.insert(to, f);
        for layer in &mut self.layers {
            layer.move_frame(from, to);
        }
        Ok(())
    }

    pub fn check_frame(&self, index: usize) -> Result<(), String> {
        if index < self.frames.len() {
            Ok(())
        } else {
            Err(format!("frame index {index} out of range: {}", self.frame_list()))
        }
    }

    fn frame_list(&self) -> String {
        let names: Vec<String> = self.frames.iter().enumerate().map(|(i, f)| format!("{i} {:?}", f.name)).collect();
        format!("frames are [{}]", names.join(", "))
    }

    /// Resolve a frame by unique name, index, or "<tag>/<n>" (n-th frame of
    /// the tag's range, counting from 0).
    pub fn find_frame(&self, name_or_index: &str) -> Result<usize, String> {
        if let Some(i) = self.frames.iter().position(|f| f.name == name_or_index) {
            return Ok(i);
        }
        if let Some((tag, n)) = name_or_index.rsplit_once('/') {
            let t = self.tags.iter().find(|t| t.name == tag).ok_or_else(|| {
                format!(
                    "no tag {tag:?} in {name_or_index:?}; tags are [{}]",
                    self.tags.iter().map(|t| format!("{:?}", t.name)).collect::<Vec<_>>().join(", ")
                )
            })?;
            let n: usize = n.parse().map_err(|_| format!("{name_or_index:?}: {n:?} is not a frame number"))?;
            let len = t.to - t.from + 1;
            return if n < len {
                Ok(t.from + n)
            } else {
                Err(format!("tag {tag:?} has {len} frames ({tag}/0..{tag}/{}), not {n}", len - 1))
            };
        }
        match name_or_index.parse::<usize>() {
            Ok(i) => self.check_frame(i).map(|_| i),
            Err(_) => Err(format!("no frame {name_or_index:?}; {}", self.frame_list())),
        }
    }

    // ---- tags ---------------------------------------------------------

    /// Create or replace a tag by name.
    pub fn set_tag(&mut self, tag: Tag) -> Result<(), String> {
        if tag.name.trim().is_empty() {
            return Err("tag name must not be empty".into());
        }
        if tag.from > tag.to {
            return Err(format!("tag {:?}: from ({}) is after to ({})", tag.name, tag.from, tag.to));
        }
        self.check_frame(tag.to)?;
        match self.tags.iter_mut().find(|t| t.name == tag.name) {
            Some(t) => *t = tag,
            None => self.tags.push(tag),
        }
        Ok(())
    }

    /// Playback sequence for a tag, or all frames when `tag` is None.
    pub fn sequence(&self, tag: Option<usize>) -> Vec<usize> {
        match tag.and_then(|t| self.tags.get(t)) {
            Some(t) => t.sequence(),
            None => (0..self.frames.len()).collect(),
        }
    }

    /// Human address of a frame: "walk_down/2" (first tag containing it) or
    /// its name.
    pub fn frame_label(&self, frame: usize) -> String {
        match self.tags.iter().find(|t| t.contains(frame)) {
            Some(t) => format!("{}/{}", t.name, frame - t.from),
            None => self.frames[frame].name.clone(),
        }
    }

    /// "walk_down/2 (f7)": tag address plus the unique frame name.
    pub fn frame_label_long(&self, frame: usize) -> String {
        let short = self.frame_label(frame);
        if short == self.frames[frame].name {
            format!("{frame} {short:?}")
        } else {
            format!("{short} ({:?}, index {frame})", self.frames[frame].name)
        }
    }

    // ---- ramps ---------------------------------------------------------

    /// The first ramp that contains `key` and the key's position in it.
    pub fn ramp_of(&self, key: u8) -> Option<(usize, usize)> {
        self.ramps.iter().enumerate().find_map(|(ri, r)| r.keys.iter().position(|k| *k == key).map(|i| (ri, i)))
    }

    /// The key `steps` places further along the ramp of `key` (positive =
    /// lighter, negative = darker), stopping at the ends. None if `key` is
    /// in no ramp (transparent never is).
    pub fn shade_key(&self, key: u8, steps: i32) -> Option<u8> {
        let (ri, i) = self.ramp_of(key)?;
        let keys = &self.ramps[ri].keys;
        let j = (i as i64 + steps as i64).clamp(0, keys.len() as i64 - 1) as usize;
        Some(keys[j])
    }

    /// Check a ramp against the palette and the other ramps (`except` = the
    /// index of the ramp it replaces).
    pub fn check_ramp(&self, ramp: &Ramp, except: Option<usize>) -> Result<(), String> {
        if ramp.name.trim().is_empty() {
            return Err("ramp name must not be empty".into());
        }
        if self.ramps.iter().enumerate().any(|(i, r)| Some(i) != except && r.name == ramp.name) {
            return Err(format!("ramp name {:?} is used twice", ramp.name));
        }
        if ramp.keys.len() < 2 {
            return Err(format!("ramp {:?} needs at least two colors (dark to light)", ramp.name));
        }
        for (i, k) in ramp.keys.iter().enumerate() {
            if *k == TRANSPARENT || !self.has_key(*k) {
                return Err(format!(
                    "ramp {:?}: key {:?} is not in the palette (valid keys: \"{}\")",
                    ramp.name,
                    *k as char,
                    self.palette_keys()
                ));
            }
            if ramp.keys[..i].contains(k) {
                return Err(format!("ramp {:?}: key {:?} appears twice", ramp.name, *k as char));
            }
        }
        Ok(())
    }

    /// Replace all ramps (checked first; nothing changes on an error).
    pub fn set_ramps(&mut self, ramps: Vec<Ramp>) -> Result<(), String> {
        let old = std::mem::replace(&mut self.ramps, ramps);
        let checked = self.ramps.iter().enumerate().try_for_each(|(i, r)| self.check_ramp(r, Some(i)));
        if checked.is_err() {
            self.ramps = old;
        }
        checked
    }

    /// "crust 3>8>9>a" with color names where there are some.
    pub fn ramp_label(&self, ramp: &Ramp) -> String {
        let keys: Vec<String> = ramp.keys.iter().map(|k| self.key_label(*k)).collect();
        format!("{}: {}", ramp.name, keys.join(" > "))
    }

    pub fn palette_entry(&self, key: u8) -> Option<&PaletteEntry> {
        self.palette.iter().find(|p| p.key == key)
    }

    /// "9 harness", "." for transparent.
    pub fn key_label(&self, key: u8) -> String {
        match self.palette_entry(key) {
            Some(p) => p.label(),
            None => (key as char).to_string(),
        }
    }

    /// Resolve a palette key by its key character or its name.
    pub fn find_key(&self, key_or_name: &str) -> Result<u8, String> {
        if let [k] = key_or_name.as_bytes() {
            self.check_key(*k)?;
            return Ok(*k);
        }
        self.palette.iter().find(|p| p.name == key_or_name).map(|p| p.key).ok_or_else(|| {
            format!(
                "no palette color {key_or_name:?}; use a key or a name: {}",
                self.palette.iter().map(|p| p.label()).collect::<Vec<_>>().join(", ")
            )
        })
    }

    /// Rename a layer and keep region references pointing at it.
    pub fn rename_layer(&mut self, layer: usize, name: &str) {
        let old = std::mem::replace(&mut self.layers[layer].name, name.to_string());
        for r in &mut self.regions {
            if r.layer.as_deref() == Some(&old) {
                r.layer = Some(name.to_string());
            }
        }
    }

    pub fn find_region(&self, name: &str) -> Result<usize, String> {
        self.regions.iter().position(|r| r.name == name).ok_or_else(|| {
            format!(
                "no region {name:?}; regions are [{}]",
                self.regions.iter().map(|r| format!("{:?}", r.name)).collect::<Vec<_>>().join(", ")
            )
        })
    }

    /// Why a pixel edit on (layer, frame) is not allowed, if it is not.
    pub fn edit_blocked(&self, layer: usize, frame: usize) -> Option<String> {
        if self.layers[layer].locked {
            return Some(format!("layer {layer} {:?} is locked", self.layers[layer].name));
        }
        for f in self.layers[layer].linked_frames(frame) {
            if self.frames[f].locked {
                return Some(format!("frame {} is locked", self.frame_label_long(f)));
            }
        }
        None
    }

    /// Locked *area* regions covering (x, y) on this layer/frame (locked
    /// markers do not protect pixels).
    pub fn locked_region_at(&self, layer: usize, frame: usize, x: i32, y: i32) -> Option<&Region> {
        self.regions.iter().find(|r| {
            r.protects_pixels()
                && r.applies_to_layer(self, layer)
                && r.rect_at(frame).is_some_and(|rc| rc.contains(x, y))
        })
    }

    /// Undo every change in cel (layer, frame) relative to `before` that is
    /// not allowed by `allowed(x, y)`. Returns how many pixels were reverted.
    pub fn revert_outside(
        &mut self,
        before: &Document,
        layer: usize,
        frame: usize,
        allowed: impl Fn(i32, i32) -> bool,
    ) -> usize {
        let w = self.width as usize;
        let old = before.cel(layer, frame);
        let bad: Vec<usize> = self
            .cel(layer, frame)
            .iter()
            .zip(old.iter())
            .enumerate()
            .filter(|(i, (a, b))| a != b && !allowed((i % w) as i32, (i / w) as i32))
            .map(|(i, _)| i)
            .collect();
        if !bad.is_empty() {
            let cel = self.cel_mut(layer, frame);
            for i in &bad {
                cel[*i] = old[*i];
            }
        }
        bad.len()
    }

    /// Animation from an arbitrary frame order (e.g. 0 1 0 2): Aseprite-style
    /// tags are ranges, so the sequence is appended as new frames linked to
    /// the listed ones, and the tag covers them. A list that already is a
    /// contiguous ascending range just becomes a tag over it.
    pub fn sequence_tag(&mut self, name: &str, frames: &[usize]) -> Result<(usize, usize), String> {
        if frames.is_empty() {
            return Err("frame list is empty".into());
        }
        for f in frames {
            self.check_frame(*f)?;
        }
        let (from, to) = if frames.windows(2).all(|w| w[1] == w[0] + 1) {
            (frames[0], frames[frames.len() - 1])
        } else {
            let start = self.frames.len();
            for f in frames {
                let i = self.add_frame(self.frames.len(), Some(*f), true)?;
                self.frames[i].name = self.unique_frame_name(&format!("{name}_{}", i - start));
            }
            (start, self.frames.len() - 1)
        };
        let mut tag = self.tags.iter().find(|t| t.name == name).cloned().unwrap_or_else(|| Tag::new(name, from, to));
        (tag.from, tag.to) = (from, to);
        self.set_tag(tag)?;
        Ok((from, to))
    }

    /// Check internal consistency (used after loading and after MCP edits).
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > MAX_SIZE || self.height > MAX_SIZE {
            return Err(format!("bad size {}x{}", self.width, self.height));
        }
        if self.layers.is_empty() || self.frames.is_empty() {
            return Err("document needs at least one layer and one frame".into());
        }
        for (i, p) in self.palette.iter().enumerate() {
            if !is_valid_key(p.key) {
                return Err(format!("palette entry {i}: invalid key {:?}", p.key as char));
            }
            if self.palette[..i].iter().any(|q| q.key == p.key) {
                return Err(format!("palette key {:?} defined twice", p.key as char));
            }
        }
        let table = self.color_table();
        for l in &self.layers {
            if l.frames.len() != self.frames.len() {
                return Err(format!("layer {:?} has {} cels for {} frames", l.name, l.frames.len(), self.frames.len()));
            }
            for f in 0..l.frames.len() {
                let c = l.cel(f);
                if c.len() != self.pixel_count() {
                    return Err(format!("layer {:?} frame {f}: wrong pixel count", l.name));
                }
                if let Some(k) = c.iter().find(|k| **k != TRANSPARENT && table[**k as usize].is_none()) {
                    return Err(format!("layer {:?} frame {f}: key {:?} is not in the palette", l.name, *k as char));
                }
            }
        }
        for (i, f) in self.frames.iter().enumerate() {
            if self.frames[..i].iter().any(|o| o.name == f.name) {
                return Err(format!("frame name {:?} is used twice (frames need unique names)", f.name));
            }
        }
        for (i, r) in self.regions.iter().enumerate() {
            if r.name.is_empty() || self.regions[..i].iter().any(|o| o.name == r.name) {
                return Err(format!("region name {:?} is empty or used twice", r.name));
            }
            if r.keys.is_empty() || r.keys.iter().any(|k| k.frame >= self.frames.len()) {
                return Err(format!("region {:?}: keys must exist and lie within the frames", r.name));
            }
        }
        for (i, r) in self.ramps.iter().enumerate() {
            self.check_ramp(r, Some(i))?;
        }
        for t in &self.tags {
            if t.from > t.to || t.to >= self.frames.len() {
                return Err(format!(
                    "tag {:?}: range {}..{} does not fit {} frames",
                    t.name,
                    t.from,
                    t.to,
                    self.frames.len()
                ));
            }
        }
        Ok(())
    }
}

fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::new(4, 3, Document::builtin_palette("pico8").unwrap()).unwrap()
    }

    #[test]
    fn ramps_shade_and_follow_the_palette() {
        let mut d = doc();
        assert!(d.set_ramps(vec![Ramp::new("a", b"12"), Ramp::new("a", b"34")]).unwrap_err().contains("twice"));
        assert!(d.set_ramps(vec![Ramp::new("a", b"121")]).unwrap_err().contains("twice"));
        assert!(d.set_ramps(vec![Ramp::new("a", b"1.")]).is_err());
        assert!(d.ramps.is_empty(), "a refused set changes nothing");
        d.set_ramps(vec![Ramp::new("crust", b"1234"), Ramp::new("x", b"56")]).unwrap();
        assert_eq!(d.shade_key(b'2', 1), Some(b'3'));
        assert_eq!(d.shade_key(b'2', -5), Some(b'1'), "stops at the dark end");
        assert_eq!(d.shade_key(b'4', 1), Some(b'4'));
        assert_eq!((d.shade_key(b'7', 1), d.shade_key(TRANSPARENT, 1)), (None, None));
        d.remove_palette_color(b'2');
        assert_eq!(d.ramps[0].keys, b"134");
        d.remove_palette_color(b'5');
        assert_eq!(d.ramps.len(), 1, "a ramp with one color left is dropped");
        d.validate().unwrap();
        assert_eq!(d.ramp_label(&d.ramps[0]), "crust: 1 > 3 > 4");
    }

    #[test]
    fn new_document() {
        let d = doc();
        assert_eq!(d.layers.len(), 1);
        assert_eq!(d.frames.len(), 1);
        assert_eq!(d.rows(0, 0), vec!["....", "....", "...."]);
        d.validate().unwrap();
    }

    #[test]
    fn set_rows_skip_and_validate() {
        let mut d = doc();
        assert_eq!(d.set_rows(0, 0, 1, 1, &["8-8".into()]).unwrap(), 2);
        assert_eq!(d.rows(0, 0)[1], ".8.8");
        let e = d.set_rows(0, 0, 1, 0, &["88".into(), "8z".into()]).unwrap_err();
        assert!(e.starts_with("rows[1][1] (x=2, y=1)"), "{e}");
        // out of bounds is clipped, not an error
        d.set_rows(0, 0, 3, 2, &["77".into(), "77".into()]).unwrap();
        assert_eq!(d.rows(0, 0)[2], "...7");
        assert_eq!(d.cel_bounds(0, 0), Some((1, 1, 3, 2)));
    }

    #[test]
    fn copied_frames_are_independent() {
        let mut d = doc();
        d.set(0, 0, 0, 0, b'1');
        d.add_frame(1, Some(0), false).unwrap();
        d.add_frame(2, None, false).unwrap();
        assert_eq!(d.get(0, 1, 0, 0), b'1');
        assert_eq!(d.get(0, 2, 0, 0), TRANSPARENT);
        // copy shares the buffer until written (copy on write)
        assert!(Arc::ptr_eq(d.layers[0].cel(0), d.layers[0].cel(1)));
        d.set(0, 1, 1, 1, b'2');
        assert_eq!(d.get(0, 0, 1, 1), TRANSPARENT);
        assert!(!d.layers[0].is_linked(1));
    }

    #[test]
    fn linked_cels() {
        let mut d = doc();
        d.add_frame(1, Some(0), true).unwrap();
        d.add_frame(2, Some(0), false).unwrap();
        assert!(d.layers[0].is_linked(0) && d.layers[0].is_linked(1));
        assert_eq!(d.layers[0].linked_frames(1), vec![0, 1]);
        d.set(0, 1, 2, 2, b'3');
        assert_eq!(d.get(0, 0, 2, 2), b'3', "editing a linked cel edits all");
        assert_eq!(d.get(0, 2, 2, 2), TRANSPARENT);
        d.move_frame(0, 2).unwrap();
        assert_eq!(d.layers[0].linked_frames(0), vec![0, 2]);
        d.layers[0].unlink(2);
        d.set(0, 2, 0, 0, b'4');
        assert_eq!(d.get(0, 0, 0, 0), TRANSPARENT);
        d.layers[0].link(1, 0);
        assert_eq!(d.get(0, 1, 2, 2), b'3');
        d.delete_frame(0).unwrap();
        assert!(!d.layers[0].is_linked(0));
        d.validate().unwrap();
    }

    #[test]
    fn tags_follow_inserts_and_deletes() {
        let mut d = doc();
        for i in 1..5 {
            d.add_frame(i, None, false).unwrap();
        }
        d.set_tag(Tag::new("walk", 1, 3)).unwrap();
        d.add_frame(0, None, false).unwrap();
        assert_eq!((d.tags[0].from, d.tags[0].to), (2, 4));
        d.add_frame(3, None, false).unwrap(); // inside the range: grows
        assert_eq!((d.tags[0].from, d.tags[0].to), (2, 5));
        d.delete_frame(2).unwrap();
        assert_eq!((d.tags[0].from, d.tags[0].to), (2, 4));
        d.set_tag(Tag::new("one", 0, 0)).unwrap();
        d.delete_frame(0).unwrap();
        assert!(d.tags.iter().all(|t| t.name != "one"));
        assert!(d.set_tag(Tag::new("bad", 0, 99)).is_err());
        d.validate().unwrap();
    }

    #[test]
    fn directions() {
        let mut t = Tag::new("b", 0, 3);
        assert_eq!(t.sequence(), vec![0, 1, 2, 3]);
        t.direction = Direction::Reverse;
        assert_eq!(t.sequence(), vec![3, 2, 1, 0]);
        t.direction = Direction::PingPong;
        assert_eq!(t.sequence(), vec![0, 1, 2, 3, 2, 1]);
        t.direction = Direction::PingPongReverse;
        assert_eq!(t.sequence(), vec![3, 2, 1, 0, 1, 2]);
        assert_eq!(Tag::new("x", 2, 3).sequence(), vec![2, 3]);
        assert_eq!(Direction::parse("pingpong").unwrap(), Direction::PingPong);
    }

    #[test]
    fn frame_addressing_and_labels() {
        let mut d = doc();
        for i in 1..6 {
            d.add_frame(i, None, false).unwrap();
        }
        d.set_tag(Tag::new("walk_down", 2, 4)).unwrap();
        assert_eq!(d.find_frame("walk_down/0").unwrap(), 2);
        assert_eq!(d.find_frame("walk_down/2").unwrap(), 4);
        assert!(d.find_frame("walk_down/3").unwrap_err().contains("walk_down/0..walk_down/2"));
        assert!(d.find_frame("run/0").unwrap_err().contains("\"walk_down\""));
        assert_eq!(d.frame_label(3), "walk_down/1");
        assert_eq!(d.frame_label(0), "f0");
        d.palette[9].name = "harness".into();
        assert_eq!(d.find_key("harness").unwrap(), b'9');
        assert_eq!(d.key_label(b'9'), "9 harness");
        assert!(d.find_key("nope").unwrap_err().contains("9 harness"));
    }

    #[test]
    fn regions_follow_frames() {
        let mut d = doc();
        for i in 1..4 {
            d.add_frame(i, None, false).unwrap();
        }
        let mut r = Region::new("shoe", Rect::new(0, 1, 2, 1));
        r.set_rect_for(2, 2, Some(Rect::new(1, 1, 2, 1)), 4);
        assert_eq!(r.rect_at(1), Some(Rect::new(0, 1, 2, 1)));
        assert_eq!(r.rect_at(2), Some(Rect::new(1, 1, 2, 1)));
        assert_eq!(r.rect_at(3), Some(Rect::new(0, 1, 2, 1)), "only frame 2 changed");
        d.regions.push(r);
        d.add_frame(0, None, false).unwrap();
        assert_eq!(d.regions[0].rect_at(3), Some(Rect::new(1, 1, 2, 1)), "keys move with inserted frames");
        d.delete_frame(1).unwrap();
        assert_eq!(d.regions[0].rect_at(2), Some(Rect::new(1, 1, 2, 1)));
        let mut once = Region::new("flash", Rect::new(0, 0, 1, 1));
        once.set_rect_for(0, 0, Some(Rect::new(0, 0, 1, 1)), 4);
        once.set_rect_for(1, 3, None, 4);
        assert_eq!(once.rect_at(1), None);
        d.regions.push(once);
        d.validate().unwrap();
    }

    #[test]
    fn locks_and_revert_outside() {
        let mut d = doc();
        d.layers[0].locked = true;
        assert!(d.edit_blocked(0, 0).unwrap().contains("locked"));
        assert!(d.delete_layer(0).is_err());
        d.layers[0].locked = false;
        let before = d.clone();
        d.set_rows(0, 0, 0, 0, &["1111".into(), "1111".into()]).unwrap();
        let clip = Rect::new(1, 0, 2, 1);
        assert_eq!(d.revert_outside(&before, 0, 0, |x, y| clip.contains(x, y)), 6);
        assert_eq!(d.rows(0, 0)[0], ".11.");
        assert_eq!(d.rows(0, 0)[1], "....");
        let mut r = Region::new("face", Rect::new(0, 0, 2, 2));
        r.locked = true;
        d.regions.push(r);
        assert!(d.locked_region_at(0, 0, 1, 1).is_some());
        assert!(d.locked_region_at(0, 0, 2, 1).is_none());
    }

    #[test]
    fn locked_markers_do_not_protect_pixels() {
        let mut d = doc();
        let mut m = Region::marker("origin", 1, 1);
        m.locked = true;
        d.regions.push(m);
        assert!(d.locked_region_at(0, 0, 1, 1).is_none(), "marker: pixels stay editable");
        let mut a = Region::new("face", Rect::new(1, 1, 1, 1));
        a.locked = true;
        d.regions.push(a);
        assert_eq!(d.locked_region_at(0, 0, 1, 1).unwrap().name, "face", "area: pixels protected");
        assert_eq!(d.regions[0].point_at(0), Some((1, 1)));
        assert_eq!(RegionKind::parse("marker").unwrap(), RegionKind::Marker);
    }

    #[test]
    fn sequence_tag_uses_linked_frames() {
        let mut d = doc();
        for i in 1..3 {
            d.add_frame(i, None, false).unwrap();
        }
        d.set(0, 1, 0, 0, b'8');
        let (from, to) = d.sequence_tag("idle", &[0, 1, 0, 2]).unwrap();
        assert_eq!((from, to), (3, 6));
        assert_eq!(d.layers[0].link_source(4), 1);
        assert_eq!(d.layers[0].link_source(5), 0);
        assert_eq!(d.get(0, 4, 0, 0), b'8');
        assert_eq!(d.sequence_tag("plain", &[1, 2]).unwrap(), (1, 2), "ranges stay ranges");
        d.validate().unwrap();
    }

    #[test]
    fn layers() {
        let mut d = doc();
        let top = d.add_layer("Ebene 1", 1);
        assert_eq!(d.layers[top].name, "Ebene 1 2");
        d.set(0, 0, 0, 0, b'1');
        d.set(top, 0, 0, 0, b'2');
        assert_eq!(d.top_key(0, 0, 0), b'2');
        d.layers[top].visible = false;
        assert_eq!(d.top_key(0, 0, 0), b'1');
        d.move_layer(1, 0).unwrap();
        assert_eq!(d.layers[1].name, "Ebene 1");
        d.delete_layer(0).unwrap();
        assert!(d.delete_layer(0).is_err());
        assert_eq!(d.find_layer("Ebene 1").unwrap(), 0);
        assert_eq!(d.find_layer("0").unwrap(), 0);
        assert!(d.find_layer("nope").unwrap_err().contains("0 \"Ebene 1\""));
    }

    #[test]
    fn palette_ops() {
        let mut d = doc();
        let k = d.next_free_key().unwrap();
        assert_eq!(k, b'g');
        assert!(d.set_palette_color(k, Rgba::parse("#123456").unwrap()).unwrap());
        assert!(!d.set_palette_color(k, Rgba::parse("#654321").unwrap()).unwrap());
        assert!(d.set_palette_color(b'.', Rgba::default()).is_err());
        d.set(0, 0, 0, 0, k);
        assert!(d.key_in_use(k));
        d.remove_palette_color(k);
        assert_eq!(d.get(0, 0, 0, 0), TRANSPARENT);
        d.validate().unwrap();
    }
}
