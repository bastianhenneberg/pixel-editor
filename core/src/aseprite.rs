//! `.aseprite` / `.ase` import and export, implemented from the public file
//! format specification (docs/ase-file-specs.md in the Aseprite repository).
//!
//! Export writes Indexed (default) or RGBA color mode: one layer chunk per
//! layer, zlib-compressed cels cropped to their content, linked cels, tags
//! with direction/repeat/color, and the palette. Palette entry names carry
//! our palette keys, so a round trip keeps the keys.
//!
//! Import reads RGBA, Grayscale and Indexed files: normal layers (groups are
//! flattened, their visibility is inherited), frames with durations, raw
//! and compressed cels, linked cels, tags and palettes. Things the document
//! model has no place for (blend modes, cel opacity, z-index, tilemaps) are
//! reported as warnings.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::color::Rgba;
use crate::document::{
    Direction, Document, Frame, PaletteEntry, Rect, Region, RegionKey, RegionKind, Tag, KEY_ORDER, TRANSPARENT,
};

const MAGIC: u16 = 0xA5E0;
const FRAME_MAGIC: u16 = 0xF1FA;
const CHUNK_OLD_PALETTE: u16 = 0x0004;
const CHUNK_OLD_PALETTE_64: u16 = 0x0011;
const CHUNK_LAYER: u16 = 0x2004;
const CHUNK_CEL: u16 = 0x2005;
const CHUNK_COLOR_PROFILE: u16 = 0x2007;
const CHUNK_TAGS: u16 = 0x2018;
const CHUNK_PALETTE: u16 = 0x2019;
const CHUNK_USER_DATA: u16 = 0x2020;
const CHUNK_SLICE: u16 = 0x2022;
/// Prefix of slice user-data text carrying region settings we add.
const REGION_META: &str = "pixel-editor:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// 8 bpp, index 0 = transparent, palette entries 1.. = our palette.
    #[default]
    Indexed,
    /// 32 bpp RGBA, palette included for reference.
    Rgba,
}

impl ColorMode {
    pub fn parse(s: &str) -> Result<ColorMode, String> {
        match s {
            "indexed" => Ok(ColorMode::Indexed),
            "rgba" | "rgb" => Ok(ColorMode::Rgba),
            _ => Err(format!("unknown color mode {s:?} (indexed, rgba)")),
        }
    }
}

pub fn is_aseprite_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("aseprite") || e.eq_ignore_ascii_case("ase"))
}

// ---- writing ----------------------------------------------------------

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn zeros(&mut self, n: usize) {
        self.0.extend(std::iter::repeat_n(0u8, n));
    }
    fn string(&mut self, s: &str) {
        self.u16(s.len() as u16);
        self.0.extend_from_slice(s.as_bytes());
    }
}

fn chunk(kind: u16, data: W) -> Vec<u8> {
    let mut out = W(Vec::with_capacity(data.0.len() + 6));
    out.u32(data.0.len() as u32 + 6);
    out.u16(kind);
    out.0.extend(data.0);
    out.0
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

pub fn write(doc: &Document, mode: ColorMode) -> Result<Vec<u8>, String> {
    write_with(doc, mode, true)
}

/// `key_names`: store palette keys as palette entry names.
fn write_with(doc: &Document, mode: ColorMode, key_names: bool) -> Result<Vec<u8>, String> {
    if doc.width > u16::MAX as u32 || doc.height > u16::MAX as u32 || doc.frames.len() > u16::MAX as usize {
        return Err("document too large for .aseprite".into());
    }
    let table = doc.color_table();
    // key -> palette index (indexed mode: 0 is transparent)
    let mut index = [0u8; 256];
    for (i, p) in doc.palette.iter().enumerate() {
        index[p.key as usize] = (i + 1) as u8;
    }

    let mut frames_out = Vec::new();
    for (fi, frame) in doc.frames.iter().enumerate() {
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        if fi == 0 {
            let mut d = W(Vec::new());
            d.u16(1); // sRGB
            d.u16(0);
            d.u32(0);
            d.zeros(8);
            chunks.push(chunk(CHUNK_COLOR_PROFILE, d));

            // palette entries as written to the file
            let mut entries: Vec<(Rgba, Option<String>)> = Vec::new();
            if mode == ColorMode::Indexed {
                entries.push((Rgba::TRANSPARENT, None));
            }
            entries.extend(doc.palette.iter().map(|p| (p.color, key_names.then(|| p.label()))));
            if entries.is_empty() {
                entries.push((Rgba::TRANSPARENT, None));
            }
            if entries.iter().all(|(c, _)| c.a() == 255 || c == &Rgba::TRANSPARENT) && entries.len() <= 256 {
                let mut d = W(Vec::new());
                d.u16(1);
                d.u8(0);
                d.u8(if entries.len() == 256 { 0 } else { entries.len() as u8 });
                for (c, _) in &entries {
                    d.u8(c.0[0]);
                    d.u8(c.0[1]);
                    d.u8(c.0[2]);
                }
                chunks.push(chunk(CHUNK_OLD_PALETTE, d));
            }
            let mut d = W(Vec::new());
            d.u32(entries.len() as u32);
            d.u32(0);
            d.u32(entries.len() as u32 - 1);
            d.zeros(8);
            for (c, name) in &entries {
                d.u16(if name.is_some() { 1 } else { 0 });
                d.0.extend_from_slice(&c.0);
                if let Some(n) = name {
                    d.string(n);
                }
            }
            chunks.push(chunk(CHUNK_PALETTE, d));

            for l in &doc.layers {
                let mut d = W(Vec::new());
                // 1 = visible, 2 = editable (our "locked" is the opposite)
                d.u16(u16::from(l.visible) | if l.locked { 0 } else { 2 });
                d.u16(0); // normal layer
                d.u16(0); // child level
                d.u16(0);
                d.u16(0);
                d.u16(0); // blend mode normal
                d.u8((l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
                d.zeros(3);
                d.string(&l.name);
                chunks.push(chunk(CHUNK_LAYER, d));
            }

            // regions as slices; layer/lock go into the slice's user-data text
            for r in &doc.regions {
                let mut d = W(Vec::new());
                // markers: slice with a pivot at the point (flag 2)
                let marker = r.is_marker();
                d.u32(r.keys.len() as u32);
                d.u32(if marker { 2 } else { 0 });
                d.u32(0);
                d.string(&r.name);
                for k in &r.keys {
                    let rc = k.rect.unwrap_or(Rect::new(0, 0, 0, 0));
                    d.u32(k.frame as u32);
                    d.u32(rc.x as u32);
                    d.u32(rc.y as u32);
                    d.u32(rc.w);
                    d.u32(rc.h);
                    if marker {
                        d.u32(0); // pivot x, relative to the slice origin
                        d.u32(0); // pivot y
                    }
                }
                chunks.push(chunk(CHUNK_SLICE, d));
                let meta = serde_json::json!({"layer": r.layer, "locked": r.locked, "kind": r.kind.id()});
                let mut d = W(Vec::new());
                d.u32(1 | if r.color.is_some() { 2 } else { 0 });
                d.string(&format!("{REGION_META}{meta}"));
                if let Some(c) = r.color {
                    d.0.extend_from_slice(&c.0);
                }
                chunks.push(chunk(CHUNK_USER_DATA, d));
            }

            if !doc.tags.is_empty() {
                let mut d = W(Vec::new());
                d.u16(doc.tags.len() as u16);
                d.zeros(8);
                for t in &doc.tags {
                    d.u16(t.from as u16);
                    d.u16(t.to as u16);
                    d.u8(match t.direction {
                        Direction::Forward => 0,
                        Direction::Reverse => 1,
                        Direction::PingPong => 2,
                        Direction::PingPongReverse => 3,
                    });
                    d.u16(if t.looping { 0 } else { 1 });
                    d.zeros(6);
                    let c = t.color.unwrap_or(Rgba::TRANSPARENT).0;
                    d.0.extend_from_slice(&c[..3]);
                    d.u8(0);
                    d.string(&t.name);
                }
                chunks.push(chunk(CHUNK_TAGS, d));
                // one user data chunk per tag, in order; carries the tag color
                for t in &doc.tags {
                    let mut d = W(Vec::new());
                    match t.color {
                        Some(c) => {
                            d.u32(2);
                            d.0.extend_from_slice(&c.0);
                        }
                        None => d.u32(0),
                    }
                    chunks.push(chunk(CHUNK_USER_DATA, d));
                }
            }
        }

        for (li, l) in doc.layers.iter().enumerate() {
            let source = l.link_source(fi);
            let mut d = W(Vec::new());
            d.u16(li as u16);
            if source != fi {
                d.i16(0);
                d.i16(0);
                d.u8(255);
                d.u16(1); // linked cel
                d.i16(0);
                d.zeros(5);
                d.u16(source as u16);
                chunks.push(chunk(CHUNK_CEL, d));
                continue;
            }
            let Some((x, y, w, h)) = doc.cel_bounds(li, fi) else { continue };
            let cel = doc.cel(li, fi);
            let mut raw = Vec::with_capacity((w * h * 4) as usize);
            for yy in y..y + h {
                for xx in x..x + w {
                    let k = cel[(yy * doc.width + xx) as usize];
                    match mode {
                        ColorMode::Indexed => raw.push(if k == TRANSPARENT { 0 } else { index[k as usize] }),
                        ColorMode::Rgba => raw.extend_from_slice(&table[k as usize].unwrap_or(Rgba::TRANSPARENT).0),
                    }
                }
            }
            d.i16(x as i16);
            d.i16(y as i16);
            d.u8(255);
            d.u16(2); // compressed image
            d.i16(0);
            d.zeros(5);
            d.u16(w as u16);
            d.u16(h as u16);
            d.0.extend(zlib(&raw));
            chunks.push(chunk(CHUNK_CEL, d));
        }

        let body: Vec<u8> = chunks.concat();
        let mut fh = W(Vec::with_capacity(16 + body.len()));
        fh.u32(16 + body.len() as u32);
        fh.u16(FRAME_MAGIC);
        fh.u16(chunks.len().min(0xFFFF) as u16);
        fh.u16(frame.duration_ms.min(u16::MAX as u32) as u16);
        fh.zeros(2);
        fh.u32(chunks.len() as u32);
        fh.0.extend(body);
        frames_out.push(fh.0);
    }

    let body: Vec<u8> = frames_out.concat();
    let colors = doc.palette.len() + usize::from(mode == ColorMode::Indexed);
    let mut h = W(Vec::with_capacity(128 + body.len()));
    h.u32(128 + body.len() as u32);
    h.u16(MAGIC);
    h.u16(doc.frames.len() as u16);
    h.u16(doc.width as u16);
    h.u16(doc.height as u16);
    h.u16(match mode {
        ColorMode::Indexed => 8,
        ColorMode::Rgba => 32,
    });
    h.u32(1); // layer opacity is valid
    h.u16(doc.frames[0].duration_ms.min(u16::MAX as u32) as u16);
    h.u32(0);
    h.u32(0);
    h.u8(0); // transparent index
    h.zeros(3);
    h.u16(colors.min(256) as u16);
    h.u8(1);
    h.u8(1);
    h.i16(0);
    h.i16(0);
    h.u16(0);
    h.u16(0);
    h.zeros(84);
    debug_assert_eq!(h.0.len(), 128);
    h.0.extend(body);
    Ok(h.0)
}

pub fn save(doc: &Document, path: &Path, mode: ColorMode) -> Result<(), String> {
    crate::io::write_atomic(path, &write(doc, mode)?)
}

// ---- reading ----------------------------------------------------------

struct R<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| format!("unexpected end of data at byte {} (wanted {n} more bytes)", self.pos))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16, String> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.take(n).map(|_| ())
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

struct AseLayer {
    name: String,
    kind: u16,
    child_level: u16,
    visible: bool,
    editable: bool,
    background: bool,
    blend: u16,
    opacity: u8,
}

enum CelData {
    Image { x: i32, y: i32, w: u32, h: u32, pixels: Vec<u8> },
    Link(usize),
}

struct AseCel {
    layer: usize,
    frame: usize,
    data: CelData,
}

struct AseTag {
    from: usize,
    to: usize,
    direction: u8,
    repeat: u16,
    rgb: [u8; 3],
    color: Option<Rgba>,
    name: String,
}

/// Parse an .aseprite file. Returns the document and warnings about
/// features that could not be represented.
pub fn read(bytes: &[u8]) -> Result<(Document, Vec<String>), String> {
    let mut r = R { data: bytes, pos: 0 };
    let mut warnings: Vec<String> = Vec::new();
    let _file_size = r.u32()?;
    if r.u16()? != MAGIC {
        return Err("not an .aseprite file (bad magic number)".into());
    }
    let frame_count = r.u16()? as usize;
    let width = r.u16()? as u32;
    let height = r.u16()? as u32;
    let depth = r.u16()?;
    let flags = r.u32()?;
    let speed = r.u16()? as u32;
    r.skip(8)?;
    let transparent_index = r.u8()?;
    r.skip(3)?;
    let _colors = r.u16()?;
    r.skip(128 - r.pos)?;
    let bpp = match depth {
        32 => 4,
        16 => 2,
        8 => 1,
        _ => return Err(format!("unsupported color depth {depth} bpp")),
    };
    if frame_count == 0 {
        return Err("file has no frames".into());
    }

    let mut layers: Vec<AseLayer> = Vec::new();
    let mut cels: Vec<AseCel> = Vec::new();
    let mut tags: Vec<AseTag> = Vec::new();
    let mut durations = Vec::new();
    let mut palette: Vec<(Rgba, Option<String>)> = Vec::new();
    let mut old_palette: Vec<Rgba> = Vec::new();
    let mut has_new_palette = false;
    let mut slices: Vec<Region> = Vec::new();
    let mut slice_pivots: Vec<Vec<(i32, i32)>> = Vec::new();

    for fi in 0..frame_count {
        let frame_start = r.pos;
        let frame_bytes = r.u32()? as usize;
        if r.u16()? != FRAME_MAGIC {
            return Err(format!("frame {fi}: bad frame magic at byte {frame_start}"));
        }
        let old_chunks = r.u16()? as usize;
        let duration = r.u16()? as u32;
        r.skip(2)?;
        let new_chunks = r.u32()? as usize;
        let chunk_count = if new_chunks == 0 { old_chunks } else { new_chunks };
        durations.push(if duration > 0 { duration } else { speed.max(1) });
        // user data chunks directly after a tags chunk belong to the tags
        let mut tag_user_data: Option<usize> = None;
        let mut slice_user_data = false;
        for _ in 0..chunk_count {
            let start = r.pos;
            let size = r.u32()? as usize;
            let kind = r.u16()?;
            if size < 6 {
                return Err(format!("frame {fi}: invalid chunk size {size} at byte {start}"));
            }
            let mut c = R { data: r.take(size - 6)?, pos: 0 };
            let ctx = |e: String| format!("frame {fi}, chunk 0x{kind:04x} at byte {start}: {e}");
            match kind {
                CHUNK_LAYER => {
                    let lflags = c.u16().map_err(ctx)?;
                    let ltype = c.u16().map_err(ctx)?;
                    let child_level = c.u16().map_err(ctx)?;
                    c.skip(4).map_err(ctx)?;
                    let blend = c.u16().map_err(ctx)?;
                    let opacity = c.u8().map_err(ctx)?;
                    c.skip(3).map_err(ctx)?;
                    let name = c.string().map_err(ctx)?;
                    layers.push(AseLayer {
                        name,
                        kind: ltype,
                        child_level,
                        visible: lflags & 1 != 0,
                        editable: lflags & 2 != 0,
                        background: lflags & 8 != 0,
                        blend,
                        opacity: if flags & 1 != 0 { opacity } else { 255 },
                    });
                }
                CHUNK_CEL => {
                    let layer = c.u16().map_err(ctx)? as usize;
                    let x = c.i16().map_err(ctx)? as i32;
                    let y = c.i16().map_err(ctx)? as i32;
                    let opacity = c.u8().map_err(ctx)?;
                    let ctype = c.u16().map_err(ctx)?;
                    let z = c.i16().map_err(ctx)?;
                    c.skip(5).map_err(ctx)?;
                    if opacity != 255 {
                        warnings.push(format!("frame {fi}, layer {layer}: cel opacity {opacity} ignored"));
                    }
                    if z != 0 {
                        warnings.push(format!("frame {fi}, layer {layer}: cel z-index {z} ignored"));
                    }
                    let data = match ctype {
                        0 | 2 => {
                            let w = c.u16().map_err(ctx)? as u32;
                            let h = c.u16().map_err(ctx)? as u32;
                            let n = (w * h) as usize * bpp;
                            let pixels = if ctype == 0 {
                                c.take(n).map_err(ctx)?.to_vec()
                            } else {
                                let mut out = Vec::with_capacity(n);
                                flate2::read::ZlibDecoder::new(&c.data[c.pos..])
                                    .read_to_end(&mut out)
                                    .map_err(|e| ctx(format!("zlib: {e}")))?;
                                out
                            };
                            if pixels.len() < n {
                                return Err(ctx(format!("cel has {} bytes, expected {n}", pixels.len())));
                            }
                            CelData::Image { x, y, w, h, pixels }
                        }
                        1 => CelData::Link(c.u16().map_err(ctx)? as usize),
                        3 => {
                            warnings.push(format!("frame {fi}, layer {layer}: tilemap cel skipped"));
                            continue;
                        }
                        other => {
                            warnings.push(format!("frame {fi}, layer {layer}: unknown cel type {other} skipped"));
                            continue;
                        }
                    };
                    cels.push(AseCel { layer, frame: fi, data });
                }
                CHUNK_TAGS => {
                    let n = c.u16().map_err(ctx)? as usize;
                    c.skip(8).map_err(ctx)?;
                    for _ in 0..n {
                        let from = c.u16().map_err(ctx)? as usize;
                        let to = c.u16().map_err(ctx)? as usize;
                        let direction = c.u8().map_err(ctx)?;
                        let repeat = c.u16().map_err(ctx)?;
                        c.skip(6).map_err(ctx)?;
                        let rgb: [u8; 3] = c.take(3).map_err(ctx)?.try_into().unwrap();
                        c.skip(1).map_err(ctx)?;
                        let name = c.string().map_err(ctx)?;
                        tags.push(AseTag { from, to, direction, repeat, rgb, color: None, name });
                    }
                    tag_user_data = Some(tags.len() - n);
                    continue;
                }
                CHUNK_SLICE => {
                    let nkeys = c.u32().map_err(ctx)? as usize;
                    let sflags = c.u32().map_err(ctx)?;
                    c.skip(4).map_err(ctx)?;
                    let name = c.string().map_err(ctx)?;
                    let mut keys = Vec::new();
                    let mut pivots = Vec::new();
                    for _ in 0..nkeys {
                        let frame = c.u32().map_err(ctx)? as usize;
                        let x = c.u32().map_err(ctx)? as i32;
                        let y = c.u32().map_err(ctx)? as i32;
                        let w = c.u32().map_err(ctx)?;
                        let h = c.u32().map_err(ctx)?;
                        if sflags & 1 != 0 {
                            c.skip(16).map_err(ctx)?;
                        }
                        if sflags & 2 != 0 {
                            pivots.push((c.u32().map_err(ctx)? as i32, c.u32().map_err(ctx)? as i32));
                        }
                        keys.push(RegionKey { frame, rect: (w > 0 && h > 0).then(|| Rect::new(x, y, w, h)) });
                    }
                    if sflags & 1 != 0 {
                        warnings.push(format!("slice {name:?}: 9-patch data ignored"));
                    }
                    slices.push(Region {
                        name,
                        kind: Default::default(),
                        keys,
                        layer: None,
                        locked: false,
                        color: None,
                    });
                    slice_pivots.push(pivots);
                    slice_user_data = true;
                    tag_user_data = None;
                    continue;
                }
                CHUNK_USER_DATA if slice_user_data => {
                    let r = slices.last_mut().unwrap();
                    let uflags = c.u32().map_err(ctx)?;
                    if uflags & 1 != 0 {
                        let text = c.string().map_err(ctx)?;
                        if let Some(meta) = text.strip_prefix(REGION_META) {
                            let v: serde_json::Value = serde_json::from_str(meta).unwrap_or_default();
                            r.layer = v["layer"].as_str().map(str::to_string);
                            r.locked = v["locked"].as_bool().unwrap_or(false);
                            if let Some(k) = v["kind"].as_str() {
                                r.kind = RegionKind::parse(k).unwrap_or_default();
                            }
                        }
                    }
                    if uflags & 2 != 0 {
                        let rgba: [u8; 4] = c.take(4).map_err(ctx)?.try_into().unwrap();
                        r.color = Some(Rgba(rgba));
                    }
                    slice_user_data = false;
                    continue;
                }
                CHUNK_USER_DATA => {
                    if let Some(t) = tag_user_data.filter(|t| *t < tags.len()) {
                        let uflags = c.u32().map_err(ctx)?;
                        if uflags & 1 != 0 {
                            c.string().map_err(ctx)?;
                        }
                        if uflags & 2 != 0 {
                            let rgba: [u8; 4] = c.take(4).map_err(ctx)?.try_into().unwrap();
                            tags[t].color = Some(Rgba(rgba));
                        }
                        tag_user_data = Some(t + 1);
                        continue;
                    }
                }
                CHUNK_PALETTE => {
                    has_new_palette = true;
                    let size = c.u32().map_err(ctx)? as usize;
                    let first = c.u32().map_err(ctx)? as usize;
                    let last = c.u32().map_err(ctx)? as usize;
                    c.skip(8).map_err(ctx)?;
                    if size > 65536 || last < first || last >= size.max(1) {
                        return Err(ctx(format!("bad palette range {first}..{last} of {size}")));
                    }
                    palette.resize(size, (Rgba([0, 0, 0, 255]), None));
                    for entry in &mut palette[first..=last] {
                        let eflags = c.u16().map_err(ctx)?;
                        let rgba: [u8; 4] = c.take(4).map_err(ctx)?.try_into().unwrap();
                        let name = if eflags & 1 != 0 { Some(c.string().map_err(ctx)?) } else { None };
                        *entry = (Rgba(rgba), name);
                    }
                }
                CHUNK_OLD_PALETTE | CHUNK_OLD_PALETTE_64 => {
                    let packets = c.u16().map_err(ctx)?;
                    let mut i = 0usize;
                    for _ in 0..packets {
                        i += c.u8().map_err(ctx)? as usize;
                        let n = match c.u8().map_err(ctx)? {
                            0 => 256,
                            n => n as usize,
                        };
                        for _ in 0..n {
                            let mut rgb: [u8; 3] = c.take(3).map_err(ctx)?.try_into().unwrap();
                            if kind == CHUNK_OLD_PALETTE_64 {
                                rgb = rgb.map(|v| (v as u32 * 255 / 63) as u8);
                            }
                            if old_palette.len() <= i {
                                old_palette.resize(i + 1, Rgba([0, 0, 0, 255]));
                            }
                            old_palette[i] = Rgba([rgb[0], rgb[1], rgb[2], 255]);
                            i += 1;
                        }
                    }
                }
                _ => {}
            }
            tag_user_data = None;
            slice_user_data = false;
        }
        r.pos = frame_start + frame_bytes; // tolerate unknown trailing data
    }
    if !has_new_palette {
        palette = old_palette.into_iter().map(|c| (c, None)).collect();
    }

    // ---- layers: keep image layers, flatten groups --------------------
    let mut keep: Vec<Option<usize>> = Vec::new(); // file layer index -> our layer
    let mut out_layers: Vec<(String, bool, f32, bool, bool)> = Vec::new(); // name, visible, opacity, background, locked
    let mut group_visible: Vec<bool> = Vec::new(); // by child level
    for l in &layers {
        let level = l.child_level as usize;
        group_visible.truncate(level);
        let parents_visible = group_visible.iter().all(|v| *v);
        match l.kind {
            0 => {
                if l.blend != 0 {
                    warnings.push(format!("layer {:?}: blend mode {} rendered as normal", l.name, l.blend));
                }
                keep.push(Some(out_layers.len()));
                out_layers.push((
                    l.name.clone(),
                    l.visible && parents_visible,
                    l.opacity as f32 / 255.0,
                    l.background,
                    !l.editable,
                ));
            }
            1 => {
                keep.push(None);
                while group_visible.len() < level {
                    group_visible.push(true);
                }
                group_visible.push(l.visible && parents_visible);
            }
            _ => {
                warnings.push(format!("layer {:?}: tilemap layers are not supported, skipped", l.name));
                keep.push(None);
            }
        }
    }
    if out_layers.is_empty() {
        return Err("file has no image layers".into());
    }

    // ---- colors -> palette keys ---------------------------------------
    let pixel_rgba = |p: &[u8], background: bool| -> Rgba {
        match bpp {
            4 => Rgba([p[0], p[1], p[2], p[3]]),
            2 => Rgba([p[0], p[0], p[0], p[1]]),
            _ => {
                let i = p[0];
                if i == transparent_index && !background {
                    Rgba::TRANSPARENT
                } else {
                    match palette.get(i as usize) {
                        Some((c, _)) => *c,
                        None => Rgba::TRANSPARENT,
                    }
                }
            }
        }
    };
    // which palette entries / colors are actually used
    let mut used: Vec<Rgba> = Vec::new();
    for c in &cels {
        if let (CelData::Image { pixels, .. }, Some(Some(li))) = (&c.data, keep.get(c.layer)) {
            let bg = out_layers[*li].3;
            for p in pixels.chunks_exact(bpp) {
                let col = pixel_rgba(p, bg);
                if col.a() > 0 && !used.contains(&col) {
                    used.push(col);
                }
            }
        }
    }
    let skip_index = |i: usize| bpp == 1 && i == transparent_index as usize;
    let file_entries: Vec<(Rgba, Option<String>)> =
        palette.iter().enumerate().filter(|(i, _)| !skip_index(*i)).map(|(_, e)| e.clone()).collect();
    // our files name entries "<key>" or "<key> <name>"
    let split = |n: &Option<String>| -> Option<(u8, String)> {
        let n = n.as_deref()?;
        let b = n.as_bytes();
        let ok = !b.is_empty() && crate::document::is_valid_key(b[0]) && (b.len() == 1 || b[1] == b' ');
        ok.then(|| (b[0], n.get(2..).unwrap_or("").to_string()))
    };
    let named: Vec<Option<(u8, String)>> = file_entries.iter().map(|(_, n)| split(n)).collect();
    let keys_from_names = !file_entries.is_empty() && named.iter().all(Option::is_some) && {
        let mut k: Vec<u8> = named.iter().map(|n| n.as_ref().unwrap().0).collect();
        k.sort();
        k.dedup();
        k.len() == file_entries.len()
    };
    let max_keys = KEY_ORDER.len();
    let mut pal: Vec<PaletteEntry> = Vec::new();
    if keys_from_names {
        for ((c, _), n) in file_entries.iter().zip(&named) {
            let (key, name) = n.clone().unwrap();
            pal.push(PaletteEntry { key, color: *c, name });
        }
    } else {
        // keep the file's palette order; drop unused entries if it is too big
        let take_all =
            file_entries.len() + used.iter().filter(|u| !file_entries.iter().any(|(c, _)| c == *u)).count() <= max_keys;
        if !take_all && !file_entries.is_empty() {
            warnings.push(format!(
                "palette has {} entries; only the {} used colors were kept",
                file_entries.len(),
                used.len()
            ));
        }
        let mut colors: Vec<Rgba> = Vec::new();
        for (c, _) in &file_entries {
            if (take_all || used.contains(c)) && !colors.contains(c) && c.a() > 0 {
                colors.push(*c);
            }
        }
        for u in &used {
            if !colors.contains(u) {
                colors.push(*u);
            }
        }
        if colors.len() > max_keys {
            return Err(format!("image uses {} colors; at most {max_keys} are supported", colors.len()));
        }
        let keys = KEY_ORDER.as_bytes();
        pal = colors
            .into_iter()
            .enumerate()
            .map(|(i, color)| PaletteEntry { key: keys[i], color, name: String::new() })
            .collect();
    }
    for u in &used {
        if !pal.iter().any(|p| p.color == *u) {
            let key = KEY_ORDER.bytes().find(|k| !pal.iter().any(|p| p.key == *k)).ok_or("too many colors")?;
            pal.push(PaletteEntry { key, color: *u, name: String::new() });
        }
    }
    let key_of = |c: Rgba| -> u8 {
        if c.a() == 0 {
            TRANSPARENT
        } else {
            pal.iter().find(|p| p.color == c).map_or(TRANSPARENT, |p| p.key)
        }
    };

    // ---- build the document -------------------------------------------
    let mut doc = Document::new(width, height, pal.clone())?;
    doc.frames = durations
        .iter()
        .enumerate()
        .map(|(i, ms)| Frame { name: format!("f{i}"), duration_ms: *ms, locked: false })
        .collect();
    doc.layers.clear();
    for (name, visible, opacity, _, locked) in &out_layers {
        let i = doc.add_layer(name, doc.layers.len());
        doc.layers[i].name = name.clone();
        doc.layers[i].visible = *visible;
        doc.layers[i].opacity = *opacity;
        doc.layers[i].locked = *locked;
    }
    let mut links = Vec::new();
    for c in &cels {
        let Some(Some(li)) = keep.get(c.layer).copied() else {
            if keep.get(c.layer).is_none() {
                warnings.push(format!("frame {}: cel refers to missing layer {}", c.frame, c.layer));
            }
            continue;
        };
        match &c.data {
            CelData::Link(target) if *target < frame_count => links.push((li, c.frame, *target)),
            CelData::Link(target) => warnings.push(format!("frame {}: link to missing frame {target}", c.frame)),
            CelData::Image { x, y, w, h, pixels } => {
                let bg = out_layers[li].3;
                let mut px = vec![TRANSPARENT; doc.pixel_count()];
                for cy in 0..*h as i32 {
                    for cx in 0..*w as i32 {
                        let (dx, dy) = (x + cx, y + cy);
                        if doc.in_bounds(dx, dy) {
                            let i = (cy * *w as i32 + cx) as usize * bpp;
                            px[(dy as u32 * width + dx as u32) as usize] = key_of(pixel_rgba(&pixels[i..i + bpp], bg));
                        }
                    }
                }
                doc.layers[li].set_cel(c.frame, Arc::new(px));
            }
        }
    }
    for (li, f, target) in links {
        doc.layers[li].link(f, target);
    }
    for t in tags {
        if t.from > t.to || t.to >= frame_count {
            warnings.push(format!("tag {:?}: invalid range {}..{} skipped", t.name, t.from, t.to));
            continue;
        }
        if t.repeat > 1 {
            warnings.push(format!("tag {:?}: repeat {} stored as \"play once\"", t.name, t.repeat));
        }
        let color = t.color.or((t.rgb != [0, 0, 0]).then(|| Rgba([t.rgb[0], t.rgb[1], t.rgb[2], 255])));
        doc.tags.push(Tag {
            name: t.name,
            from: t.from,
            to: t.to,
            direction: match t.direction {
                1 => Direction::Reverse,
                2 => Direction::PingPong,
                3 => Direction::PingPongReverse,
                _ => Direction::Forward,
            },
            looping: t.repeat == 0,
            color,
        });
    }
    for (mut r, pivots) in slices.into_iter().zip(slice_pivots) {
        if r.is_marker() {
            // the point is at the pivot
            for (k, (px, py)) in r.keys.iter_mut().zip(pivots) {
                if let Some(rc) = &mut k.rect {
                    *rc = Rect::new(rc.x + px, rc.y + py, 1, 1);
                }
            }
        } else if !pivots.is_empty() {
            warnings.push(format!("slice {:?}: pivot ignored (not a marker)", r.name));
        }
        r.keys.retain(|k| k.frame < frame_count);
        r.normalize();
        if r.keys.is_empty() || doc.regions.iter().any(|o| o.name == r.name) {
            warnings.push(format!("slice {:?} skipped (empty or duplicate name)", r.name));
            continue;
        }
        doc.regions.push(r);
    }
    doc.validate()?;
    Ok((doc, warnings))
}

pub fn load(path: &Path) -> Result<(Document, Vec<String>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Document {
        let mut d = Document::new(6, 4, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        d.set_palette_color(b'g', Rgba::parse("#ff000080").unwrap()).unwrap();
        d.palette[2].name = "crust dark".into();
        d.set_rows(0, 0, 1, 1, &["22".into(), "2c".into()]).unwrap();
        let top = d.add_layer("Glanz", 1);
        d.set(top, 0, 5, 3, b'g');
        d.layers[top].opacity = 0.6;
        d.add_frame(1, Some(0), true).unwrap(); // linked
        d.add_frame(2, Some(0), false).unwrap();
        d.set(0, 2, 0, 0, b'9');
        d.frames[0].duration_ms = 120;
        d.frames[2].duration_ms = 250;
        d.layers[top].visible = false;
        d.layers[0].locked = true;
        let mut face = Region::new("face", Rect::new(1, 1, 2, 2));
        face.set_rect_for(1, 1, Some(Rect::new(2, 1, 2, 2)), 3);
        face.layer = Some("Glanz".into());
        face.locked = true;
        face.color = Some(Rgba::parse("#ff00ff").unwrap());
        d.regions.push(face);
        let mut hand = Region::marker("hand", 4, 2);
        hand.set_rect_for(2, 2, Some(Rect::new(5, 1, 1, 1)), 3);
        hand.locked = true;
        d.regions.push(hand);
        let mut gone = Region::new("spark", Rect::new(0, 0, 1, 1));
        gone.set_rect_for(1, 2, None, 3);
        d.regions.push(gone);
        let mut t = Tag::new("idle", 0, 1);
        t.color = Some(Rgba::parse("#36b5ff").unwrap());
        d.set_tag(t).unwrap();
        let mut t = Tag::new("hit", 1, 2);
        t.direction = Direction::PingPongReverse;
        t.looping = false;
        d.set_tag(t).unwrap();
        d
    }

    fn round(v: f32) -> f32 {
        (v * 255.0).round() / 255.0
    }

    #[test]
    fn roundtrip_both_modes() {
        for mode in [ColorMode::Indexed, ColorMode::Rgba] {
            let d = sample();
            let bytes = write(&d, mode).unwrap();
            let (back, warnings) = read(&bytes).unwrap();
            assert!(warnings.is_empty(), "{mode:?}: {warnings:?}");
            let mut expect = d.clone();
            for l in &mut expect.layers {
                l.opacity = round(l.opacity);
            }
            assert_eq!(back.palette, expect.palette, "{mode:?}: palette keys survive via entry names");
            assert_eq!(back.layers, expect.layers, "{mode:?}");
            assert_eq!(back.tags, expect.tags, "{mode:?}");
            assert_eq!(back.regions, expect.regions, "{mode:?}: regions <-> slices");
            assert_eq!(back.palette[2].name, "crust dark");
            assert!(back.layers[0].locked);
            for f in 0..3 {
                assert_eq!(crate::render::composite(&back, f), crate::render::composite(&d, f), "identical pixels");
            }
            assert_eq!(back.frames.iter().map(|f| f.duration_ms).collect::<Vec<_>>(), vec![120, 100, 250]);
            assert!(back.layers[0].is_linked(1));
        }
    }

    /// Cross-check with an independent parser (the `asefile` crate).
    #[test]
    fn readable_by_asefile() {
        let mut d = sample();
        // asefile 0.3 predates "pingpong reverse" (Aseprite 1.3) and rejects it
        d.tags[1].direction = Direction::PingPong;
        for mode in [ColorMode::Indexed, ColorMode::Rgba] {
            let bytes = write(&d, mode).unwrap();
            let ase = asefile::AsepriteFile::read(&bytes[..]).unwrap_or_else(|e| panic!("{mode:?}: {e}"));
            assert_eq!((ase.width(), ase.height(), ase.num_frames(), ase.num_layers()), (6, 4, 3, 2));
            assert_eq!(ase.frame(0).duration(), 120);
            assert_eq!(ase.frame(2).duration(), 250);
            assert_eq!(ase.layer(1).name(), "Glanz");
            assert!(!ase.layer(1).is_visible());
            assert_eq!(ase.num_tags(), 2);
            let hit = ase.tag(1);
            assert_eq!((hit.name(), hit.from_frame(), hit.to_frame()), ("hit", 1, 2));
            assert_eq!(hit.animation_direction(), asefile::AnimationDirection::PingPong);
            // pixels of a single layer, exact colors
            let img = ase.frame(2).layer(0).image();
            let red = d.color_of(b'2').unwrap().0;
            assert_eq!(img.get_pixel(1, 1).0, red, "{mode:?}");
            assert_eq!(img.get_pixel(0, 0).0, d.color_of(b'9').unwrap().0);
            assert_eq!(img.get_pixel(3, 3).0[3], 0, "transparent stays transparent");
            let linked = ase.frame(1).layer(0).image();
            assert_eq!(linked.get_pixel(2, 2).0, d.color_of(b'c').unwrap().0, "linked cel resolves");
            if mode == ColorMode::Indexed {
                assert!(ase.is_indexed_color());
                assert_eq!(ase.palette().unwrap().num_colors(), 18);
            }
        }
    }

    #[test]
    fn rejects_garbage_with_position() {
        assert!(read(b"nope").unwrap_err().contains("unexpected end"));
        let mut bytes = write(&sample(), ColorMode::Rgba).unwrap();
        bytes.truncate(200);
        let e = read(&bytes).unwrap_err();
        assert!(e.contains("byte"), "{e}");
    }

    #[test]
    fn foreign_palette_without_names() {
        // keys come from palette order when entries carry no key names
        let mut d = sample();
        d.palette.iter_mut().enumerate().for_each(|(i, p)| p.key = KEY_ORDER.as_bytes()[i + 30]);
        for (li, f) in [(0, 0), (0, 2), (1, 0)] {
            let rows: Vec<String> = sample().rows(li, f);
            let mapped: Vec<String> = rows
                .iter()
                .map(|r| {
                    r.bytes()
                        .map(|k| {
                            if k == TRANSPARENT {
                                '.'
                            } else {
                                let i = sample().palette.iter().position(|p| p.key == k).unwrap();
                                KEY_ORDER.as_bytes()[i + 30] as char
                            }
                        })
                        .collect()
                })
                .collect();
            d.layers[li].set_cel(f, Arc::new(mapped.concat().into_bytes()));
        }
        for mode in [ColorMode::Indexed, ColorMode::Rgba] {
            let (back, warnings) = read(&write_with(&d, mode, false).unwrap()).unwrap();
            assert!(warnings.is_empty());
            let keys: String = back.palette.iter().map(|p| p.key as char).collect();
            assert_eq!(keys, &KEY_ORDER[..17], "{mode:?}");
            assert_eq!(
                back.palette.iter().map(|p| p.color).collect::<Vec<_>>(),
                d.palette.iter().map(|p| p.color).collect::<Vec<_>>()
            );
            assert_eq!(crate::render::composite(&back, 2), crate::render::composite(&d, 2), "{mode:?}");
        }
    }
}
