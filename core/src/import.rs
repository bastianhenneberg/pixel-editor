//! Import a PNG (optionally a sheet of equal tiles) as a new document.
//! Colors are taken exactly; each distinct opaque color gets a palette key.

use std::path::Path;

use crate::color::Rgba;
use crate::document::{Document, PaletteEntry, KEY_ORDER, TRANSPARENT};
use crate::render::Image;

/// Pixels with alpha 0 become transparent; every tile becomes one frame
/// (left to right, top to bottom).
pub fn from_image(img: &Image, tile: Option<(u32, u32)>, ms: u32) -> Result<Document, String> {
    let (tw, th) = tile.unwrap_or((img.width, img.height));
    if tw == 0 || th == 0 || tw > img.width || th > img.height {
        return Err(format!("tile {tw}x{th} does not fit image {}x{}", img.width, img.height));
    }
    let keys: Vec<u8> = KEY_ORDER.bytes().collect();
    let mut palette: Vec<PaletteEntry> = Vec::new();
    let mut doc = Document::new(tw, th, Vec::new())?;
    let mut first = true;
    for ty in (0..=img.height - th).step_by(th as usize) {
        for tx in (0..=img.width - tw).step_by(tw as usize) {
            let mut px = Vec::with_capacity((tw * th) as usize);
            for y in 0..th {
                for x in 0..tw {
                    let c = img.get(tx + x, ty + y);
                    if c[3] == 0 {
                        px.push(TRANSPARENT);
                        continue;
                    }
                    let key = match palette.iter().find(|p| p.color.0 == c) {
                        Some(p) => p.key,
                        None => {
                            let k = *keys.get(palette.len()).ok_or_else(|| {
                                format!("image has more than {} colors; reduce colors first", keys.len())
                            })?;
                            palette.push(PaletteEntry { key: k, color: Rgba(c), name: String::new() });
                            k
                        }
                    };
                    px.push(key);
                }
            }
            let f = if first { 0 } else { doc.add_frame(doc.frames.len(), None, false)? };
            first = false;
            doc.frames[f].duration_ms = ms.max(1);
            *doc.cel_mut(0, f) = px;
        }
    }
    doc.palette = palette;
    doc.validate()?;
    Ok(doc)
}

pub fn import_png(path: &Path, tile: Option<(u32, u32)>, ms: u32) -> Result<Document, String> {
    from_image(&Image::load_png(path)?, tile, ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::composite;

    #[test]
    fn sheet_to_frames_roundtrip() {
        let mut img = Image::new(4, 2);
        img.put(0, 0, [255, 0, 0, 255]);
        img.put(3, 1, [0, 0, 255, 128]);
        let d = from_image(&img, Some((2, 2)), 80).unwrap();
        assert_eq!(d.frames.len(), 2);
        assert_eq!(d.palette.len(), 2);
        assert_eq!(composite(&d, 1).get(1, 1), [0, 0, 255, 128]);
        assert_eq!(d.rows(0, 0), vec!["0.", ".."]);
        assert_eq!(d.frames[1].duration_ms, 80);
    }
}
