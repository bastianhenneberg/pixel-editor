//! Parts library: a reviewed piece of a cel (a sword at one angle, a hand)
//! saved under a name and placed again later instead of being redrawn.
//!
//! A part is one JSON file `<name>.json`: its rows of palette keys ('.' =
//! transparent, kept as transparent) and, for every key it uses, the color
//! and name it had. Placing it into a document maps the colors by value,
//! so a document with other keys for the same colors gets the right ones.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::document::{Document, Rect, TRANSPARENT};
use crate::Rgba;

/// File extension of a part in a library folder.
pub const EXTENSION: &str = ".json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartColor {
    pub key: String,
    /// `#rrggbb` or `#rrggbbaa`.
    pub color: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rows: Vec<String>,
    pub colors: Vec<PartColor>,
    /// Where it was taken from ("hero/walk.pxe.json layer body frame 2 x,y,w,h").
    #[serde(default)]
    pub source: String,
}

/// Part names: lowercase letters, digits, '-' and '_' (1..64 characters).
pub fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid part name {name:?}: use lowercase letters, digits, '-' and '_' (e.g. sword_45)"))
    }
}

impl Part {
    /// The pixels of one cel inside `rect` (which must lie on the canvas).
    pub fn from_cel(
        doc: &Document,
        layer: usize,
        frame: usize,
        rect: Rect,
        name: &str,
        source: String,
    ) -> Result<Part, String> {
        check_name(name)?;
        if rect.x < 0
            || rect.y < 0
            || rect.x + rect.w as i32 > doc.width as i32
            || rect.y + rect.h as i32 > doc.height as i32
        {
            return Err(format!("{} reaches outside the {}x{} canvas", rect.describe(), doc.width, doc.height));
        }
        let mut used: Vec<u8> = Vec::new();
        let rows: Vec<String> = (rect.y..rect.y + rect.h as i32)
            .map(|y| {
                (rect.x..rect.x + rect.w as i32)
                    .map(|x| {
                        let k = doc.get(layer, frame, x, y);
                        if k != TRANSPARENT && !used.contains(&k) {
                            used.push(k);
                        }
                        k as char
                    })
                    .collect()
            })
            .collect();
        if used.is_empty() {
            return Err(format!("{} of that cel is empty; nothing to save", rect.describe()));
        }
        used.sort();
        let colors = used
            .iter()
            .map(|&k| {
                let e = doc.palette_entry(k).ok_or(format!("key {:?} is not in the palette", k as char))?;
                Ok(PartColor { key: (k as char).to_string(), color: e.color.to_hex(), name: e.name.clone() })
            })
            .collect::<Result<_, String>>()?;
        Ok(Part { name: name.to_string(), width: rect.w, height: rect.h, rows, colors, source })
    }

    pub fn load(path: &Path) -> Result<Part, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let p: Part = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if p.rows.len() != p.height as usize || p.rows.iter().any(|r| r.len() != p.width as usize) {
            return Err(format!("{}: rows do not match {}x{}", path.display(), p.width, p.height));
        }
        Ok(p)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::io::write_atomic(path, format!("{text}\n").as_bytes())
    }

    /// The target key for each of the part's keys: `explicit` first, then
    /// the same key if it has the same color, then any key with that color.
    /// Errors list the colors the target palette lacks. The notes say which
    /// keys changed.
    pub fn key_map(
        &self,
        doc: &Document,
        explicit: &BTreeMap<u8, u8>,
    ) -> Result<(BTreeMap<u8, u8>, Vec<String>), String> {
        let (mut map, mut notes, mut missing) = (BTreeMap::new(), Vec::new(), Vec::new());
        for c in &self.colors {
            let k = *c.key.as_bytes().first().ok_or("part color without key")?;
            let rgba = Rgba::parse(&c.color)?;
            let label = if c.name.is_empty() {
                format!("'{}' {}", c.key, c.color)
            } else {
                format!("'{}' {} ({})", c.key, c.color, c.name)
            };
            let target = if let Some(&t) = explicit.get(&k) {
                doc.check_key(t)?;
                Some(t)
            } else if doc.color_of(k) == Some(rgba) {
                Some(k)
            } else {
                doc.palette.iter().find(|e| e.color == rgba).map(|e| e.key)
            };
            match target {
                Some(t) => {
                    if t != k {
                        notes.push(format!("{label} -> '{}'", t as char));
                    }
                    map.insert(k, t);
                }
                None => missing.push(label),
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "the target palette has no color for {} (add it with set_palette_color, or pass map {{part key: \
                 target key}}); nothing was written",
                missing.join(", ")
            ));
        }
        Ok((map, notes))
    }

    /// The part's pixels as (dx, dy, key), mirrored if asked; transparent
    /// pixels included as TRANSPARENT.
    pub fn pixels(&self, flip_h: bool, flip_v: bool) -> Vec<(i32, i32, u8)> {
        let (w, h) = (self.width as i32, self.height as i32);
        let mut out = Vec::new();
        for (y, row) in self.rows.iter().enumerate() {
            for (x, k) in row.bytes().enumerate() {
                let (x, y) = (x as i32, y as i32);
                let dx = if flip_h { w - 1 - x } else { x };
                let dy = if flip_v { h - 1 - y } else { y };
                out.push((dx, dy, k));
            }
        }
        out
    }
}

/// The parts in a library folder, sorted by name (unreadable files as errors).
pub fn list(dir: &Path) -> Vec<(String, Result<Part, String>)> {
    let mut out: Vec<(String, Result<Part, String>)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| {
            let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            (name, Part::load(&p))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// File of part `name` in library folder `dir`.
pub fn path_in(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{EXTENSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaletteEntry;

    #[test]
    fn save_load_and_map_colors_by_value() {
        let mut d = Document::new(6, 6, Document::builtin_palette("pico8").unwrap()).unwrap();
        d.set_rows(0, 0, 1, 1, &["8c".into(), ".8".into()]).unwrap();
        let p = Part::from_cel(&d, 0, 0, Rect::new(1, 1, 2, 2), "blade", "test".into()).unwrap();
        assert_eq!(p.rows, vec!["8c", ".8"]);
        assert_eq!(p.colors.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), vec!["8", "c"]);
        let dir = std::env::temp_dir().join(format!("pxe-parts-{}", std::process::id()));
        let f = path_in(&dir, "blade");
        p.save(&f).unwrap();
        assert_eq!(Part::load(&f).unwrap(), p);
        assert_eq!(list(&dir).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
        // another palette: same colors under other keys, one missing
        let red = d.color_of(b'8').unwrap();
        let blue = d.color_of(b'c').unwrap();
        let pal = |entries: &[(u8, Rgba)]| -> Vec<PaletteEntry> {
            entries.iter().map(|&(key, color)| PaletteEntry { key, color, name: String::new() }).collect()
        };
        let other = Document::new(4, 4, pal(&[(b'a', blue), (b'b', red)])).unwrap();
        let (map, notes) = p.key_map(&other, &BTreeMap::new()).unwrap();
        assert_eq!(map, BTreeMap::from([(b'8', b'b'), (b'c', b'a')]));
        assert_eq!(notes.len(), 2, "{notes:?}");
        let lacking = Document::new(4, 4, pal(&[(b'a', blue)])).unwrap();
        let e = p.key_map(&lacking, &BTreeMap::new()).unwrap_err();
        assert!(e.contains(&format!("'8' {}", red.to_hex())) && e.contains("set_palette_color"), "{e}");
        let (map, _) = p.key_map(&lacking, &BTreeMap::from([(b'8', b'a')])).unwrap();
        assert_eq!(map[&b'8'], b'a', "explicit map wins");
        // flips
        let px = p.pixels(true, false);
        assert!(px.contains(&(1, 0, b'8')) && px.contains(&(0, 1, b'8')) && px.contains(&(1, 1, TRANSPARENT)));
    }

    #[test]
    fn names_and_bounds() {
        assert!(check_name("sword_45-a").is_ok());
        for bad in ["", "Sword", "a b", "../x", "x.json"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        let d = Document::new(4, 4, Document::builtin_palette("pico8").unwrap()).unwrap();
        assert!(Part::from_cel(&d, 0, 0, Rect::new(2, 2, 4, 4), "x", String::new()).unwrap_err().contains("outside"));
        assert!(Part::from_cel(&d, 0, 0, Rect::new(0, 0, 2, 2), "x", String::new()).unwrap_err().contains("empty"));
    }
}
