//! Exporters. All targets share one sheet layout (`build_sheet`).

mod gif;
mod godot;
mod unity;

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::document::Document;
use crate::render::{composite, Image};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    PngSheet,
    Godot,
    Unity,
    Gif,
    Aseprite,
}

impl Target {
    pub const ALL: [Target; 5] = [Target::PngSheet, Target::Godot, Target::Unity, Target::Gif, Target::Aseprite];

    pub fn parse(s: &str) -> Result<Target, String> {
        match s {
            "png_sheet" | "sheet" | "png" => Ok(Target::PngSheet),
            "godot" => Ok(Target::Godot),
            "unity" => Ok(Target::Unity),
            "gif" => Ok(Target::Gif),
            "aseprite" | "ase" => Ok(Target::Aseprite),
            _ => Err(format!("unknown export target {s:?} (png_sheet, godot, unity, gif, aseprite)")),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Target::PngSheet => "png_sheet",
            Target::Godot => "godot",
            Target::Unity => "unity",
            Target::Gif => "gif",
            Target::Aseprite => "aseprite",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Integer upscale. Default: 1 for sheets, 4 for GIFs.
    pub scale: Option<u32>,
    /// Sheet columns. Default: all frames in one row, at most 16 per row.
    pub columns: Option<u32>,
    /// Godot: `res://` directory the sheet PNG will live in. If unset, the
    /// `.tres` references the PNG by a path relative to itself.
    pub godot_res_dir: Option<String>,
    /// Unity: pixels per unit for the sprites.
    pub pixels_per_unit: u32,
    /// Aseprite: color mode of the written file.
    pub color_mode: crate::aseprite::ColorMode,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            scale: None,
            columns: None,
            godot_res_dir: None,
            pixels_per_unit: 32,
            color_mode: Default::default(),
        }
    }
}

/// One frame's rectangle on the sheet.
#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    pub frame: usize,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub ms: u32,
}

/// An animation as exported: frames in playback order with durations.
#[derive(Clone, Debug)]
pub struct Anim {
    pub name: String,
    pub steps: Vec<(usize, u32)>,
    pub looping: bool,
}

pub struct Sheet {
    pub image: Image,
    pub frame_w: u32,
    pub frame_h: u32,
    pub cells: Vec<Cell>,
    pub anims: Vec<Anim>,
}

/// Animations from tags (ping-pong expanded). Without tags, one
/// looping "default" animation over all frames.
pub fn anims(doc: &Document) -> Vec<Anim> {
    if doc.tags.is_empty() {
        return vec![Anim {
            name: "default".into(),
            steps: doc.frames.iter().enumerate().map(|(i, f)| (i, f.duration_ms)).collect(),
            looping: true,
        }];
    }
    doc.tags
        .iter()
        .map(|t| Anim {
            name: t.name.clone(),
            steps: t.sequence().into_iter().map(|f| (f, doc.frames[f].duration_ms)).collect(),
            looping: t.looping,
        })
        .collect()
}

/// Frame names made unique for use as JSON keys.
fn unique_frame_names(doc: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in &doc.frames {
        let mut name = f.name.clone();
        let mut i = 2;
        while out.contains(&name) {
            name = format!("{}_{i}", f.name);
            i += 1;
        }
        out.push(name);
    }
    out
}

/// Lay the frames out on a sheet. Frames that render identically (e.g.
/// linked frames) share one region, so the atlas holds each image once.
pub fn build_sheet(doc: &Document, scale: u32, columns: Option<u32>) -> Sheet {
    let scale = scale.clamp(1, 64);
    let mut unique: Vec<Image> = Vec::new();
    let slot: Vec<usize> = (0..doc.frames.len())
        .map(|i| {
            let img = composite(doc, i);
            unique.iter().position(|u| *u == img).unwrap_or_else(|| {
                unique.push(img);
                unique.len() - 1
            })
        })
        .collect();
    let n = unique.len() as u32;
    let cols = columns.unwrap_or(16).clamp(1, n.max(1));
    let rows = n.div_ceil(cols);
    let (fw, fh) = (doc.width * scale, doc.height * scale);
    let mut image = Image::new(fw * cols, fh * rows);
    for (i, img) in unique.iter().enumerate() {
        image.paste(&img.scaled(scale), (i as u32 % cols) * fw, (i as u32 / cols) * fh);
    }
    let names = unique_frame_names(doc);
    let cells = doc
        .frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let s = slot[i] as u32;
            Cell {
                name: names[i].clone(),
                frame: i,
                x: (s % cols) * fw,
                y: (s / cols) * fh,
                w: fw,
                h: fh,
                ms: f.duration_ms,
            }
        })
        .collect();
    Sheet { image, frame_w: fw, frame_h: fh, cells, anims: anims(doc) }
}

/// Sheet JSON in the same shape as pixelart-studio's `*_sheet.json`.
pub fn sheet_json(sheet: &Sheet, image_name: &str) -> Value {
    let mut frames = Map::new();
    for c in &sheet.cells {
        frames.insert(c.name.clone(), json!({"x": c.x, "y": c.y, "w": c.w, "h": c.h, "ms": c.ms}));
    }
    let mut anims = Map::new();
    for a in &sheet.anims {
        let steps: Vec<Value> =
            a.steps.iter().map(|(f, ms)| json!({"frame": sheet.cells[*f].name, "ms": ms})).collect();
        anims.insert(a.name.clone(), json!({"steps": steps, "loop": a.looping}));
    }
    json!({
        "image": image_name,
        "frameWidth": sheet.frame_w,
        "frameHeight": sheet.frame_h,
        "frames": frames,
        "anims": anims,
    })
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    crate::io::write_atomic(path, text.as_bytes())
}

/// Export `doc` into `out_dir` using `stem` as the base file name.
/// Returns the written files.
pub fn export(
    doc: &Document,
    target: Target,
    out_dir: &Path,
    stem: &str,
    opts: &ExportOptions,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    match target {
        Target::Gif => gif::export(doc, out_dir, stem, opts.scale.unwrap_or(4)),
        Target::Aseprite => {
            let p = out_dir.join(format!("{stem}.aseprite"));
            crate::aseprite::save(doc, &p, opts.color_mode)?;
            Ok(vec![p])
        }
        _ => {
            let sheet = build_sheet(doc, opts.scale.unwrap_or(1), opts.columns);
            let png_name = format!("{stem}.png");
            let png_path = out_dir.join(&png_name);
            sheet.image.save_png(&png_path)?;
            let mut files = vec![png_path];
            match target {
                Target::PngSheet => {
                    let p = out_dir.join(format!("{stem}.json"));
                    write_text(&p, &serde_json::to_string_pretty(&sheet_json(&sheet, &png_name)).unwrap())?;
                    files.push(p);
                }
                Target::Godot => {
                    let p = out_dir.join(format!("{stem}.tres"));
                    write_text(&p, &godot::sprite_frames(&sheet, &png_name, opts.godot_res_dir.as_deref()))?;
                    files.push(p);
                }
                Target::Unity => {
                    let names = unity::sprite_names(doc, &sheet);
                    let meta = out_dir.join(format!("{png_name}.meta"));
                    write_text(&meta, &unity::texture_meta(&sheet, &names, stem, opts.pixels_per_unit))?;
                    let p = out_dir.join(format!("{stem}.anim.json"));
                    write_text(
                        &p,
                        &serde_json::to_string_pretty(&unity::anim_json(&sheet, &names, &png_name)).unwrap(),
                    )?;
                    files.extend([meta, p]);
                }
                Target::Gif | Target::Aseprite => unreachable!(),
            }
            Ok(files)
        }
    }
}

/// Deterministic 64-bit FNV-1a, used for stable Unity GUIDs/IDs and Godot ids.
pub(crate) fn fnv64(data: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Tag;

    pub(crate) fn sample() -> Document {
        let mut d = Document::new(4, 4, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        d.set_rows(0, 0, 0, 0, &["2222".into(), "2..2".into()]).unwrap();
        d.add_frame(1, Some(0), false).unwrap();
        d.set(0, 1, 0, 3, b'c');
        d.add_frame(2, None, false).unwrap();
        d.set(0, 2, 1, 1, b'9');
        d.frames[0].duration_ms = 100;
        d.frames[1].duration_ms = 100;
        d.frames[2].duration_ms = 250;
        d.set_tag(Tag::new("idle", 0, 1)).unwrap();
        let mut hit = Tag::new("hit", 0, 2);
        hit.looping = false;
        hit.direction = crate::document::Direction::PingPong;
        d.set_tag(hit).unwrap();
        d
    }

    #[test]
    fn sheet_layout_and_json() {
        let d = sample();
        let s = build_sheet(&d, 2, Some(2));
        assert_eq!((s.image.width, s.image.height), (16, 16));
        assert_eq!((s.cells[2].x, s.cells[2].y), (0, 8));
        assert_eq!(s.image.get(0, 0), d.color_of(b'2').unwrap().0);
        let j = sheet_json(&s, "x.png");
        assert_eq!(j["frameWidth"], 8);
        assert_eq!(j["frames"]["f2"]["ms"], 250);
        let hit = j["anims"]["hit"]["steps"].as_array().unwrap();
        let names: Vec<_> = hit.iter().map(|s| s["frame"].as_str().unwrap()).collect();
        assert_eq!(names, ["f0", "f1", "f2", "f1"]);
        assert_eq!(j["anims"]["hit"]["loop"], false);
    }

    #[test]
    fn identical_frames_share_a_region() {
        let mut d = sample();
        d.add_frame(3, Some(0), true).unwrap();
        let s = build_sheet(&d, 1, None);
        assert_eq!(s.image.width, 4 * 3, "4 frames, 3 distinct images");
        assert_eq!((s.cells[3].x, s.cells[3].y), (s.cells[0].x, s.cells[0].y));
    }

    #[test]
    fn default_anim_without_tags() {
        let mut d = sample();
        d.tags.clear();
        let a = anims(&d);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].steps.len(), 3);
    }

    #[test]
    fn export_all_targets() {
        let d = sample();
        let dir = std::env::temp_dir().join(format!("pxe-export-{}", std::process::id()));
        for t in Target::ALL {
            let files = export(&d, t, &dir, "hero", &ExportOptions::default()).unwrap();
            for f in &files {
                assert!(f.exists(), "{t:?}: {} missing", f.display());
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
