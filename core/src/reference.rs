//! The reference ("Referenzebene"): a guide picture (PNG/JPG or another
//! document) laid under or over the canvas. It is only ever drawn by the
//! editor; exports, previews and pixel tools never see it.

use std::path::{Path, PathBuf};

use crate::document::{Document, RefSource, Reference};
use crate::render::{composite, Image};

/// The file a reference points to (relative to the document's folder).
pub fn resolve(doc_path: Option<&Path>, r: &Reference) -> PathBuf {
    let p = Path::new(r.source.path());
    match doc_path.and_then(Path::parent) {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p.to_path_buf(),
    }
}

/// The reference's picture at its own size; for a document the frame with
/// index `frame` (its last frame when it has fewer).
pub fn load(doc_path: Option<&Path>, r: &Reference, frame: usize) -> Result<Image, String> {
    let path = resolve(doc_path, r);
    match &r.source {
        RefSource::Image(_) => load_image(&path),
        RefSource::Document(_) => {
            let doc = crate::io::load(&path)?;
            Ok(composite(&doc, frame.min(doc.frames.len() - 1)))
        }
    }
}

/// A PNG or JPEG file as RGBA.
pub fn load_image(path: &Path) -> Result<Image, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "jpg" || ext == "jpeg" {
        use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), opts);
        let pixels = dec.decode().map_err(|e| format!("{}: {e:?}", path.display()))?;
        let info = dec.info().ok_or_else(|| format!("{}: no JPEG header", path.display()))?;
        let (width, height) = (info.width as u32, info.height as u32);
        if pixels.len() != (width * height * 4) as usize {
            return Err(format!("{}: unexpected JPEG data", path.display()));
        }
        return Ok(Image { width, height, pixels });
    }
    Image::load_png(path)
}

/// Is `path` something a reference can show (PNG/JPG or a document)?
pub fn source_for(path: &str) -> Result<RefSource, String> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Ok(RefSource::Image(path.to_string()))
    } else if crate::io::is_document_path(Path::new(path)) {
        Ok(RefSource::Document(path.to_string()))
    } else {
        Err(format!("{path:?}: a reference is a PNG/JPG image or a .pxe.json/.aseprite document"))
    }
}

/// Lay the reference picture onto `out` (canvas pixels times `zoom`) under
/// or over `sprite`: nearest neighbour, with its opacity. Used for
/// screenshots and tests; the GUI draws it as a texture.
pub fn compose(doc: &Document, sprite: &Image, picture: &Image, r: &Reference) -> Image {
    let mut out = Image::new(doc.width, doc.height);
    let draw_ref = |out: &mut Image| {
        if !r.visible {
            return;
        }
        for y in 0..doc.height {
            for x in 0..doc.width {
                // centre of the canvas pixel, in reference pixels
                let rx = ((x as f32 + 0.5 - r.x) / r.scale).floor();
                let ry = ((y as f32 + 0.5 - r.y) / r.scale).floor();
                if rx < 0.0 || ry < 0.0 || rx >= picture.width as f32 || ry >= picture.height as f32 {
                    continue;
                }
                let c = picture.get(rx as u32, ry as u32);
                let d = out.get(x, y);
                out.put(x, y, crate::color::blend_over(d, c, r.opacity));
            }
        }
    };
    if !r.above {
        draw_ref(&mut out);
    }
    out.overlay(sprite, 0, 0);
    if r.above {
        draw_ref(&mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn source_kinds() {
        assert_eq!(source_for("a/b.PNG").unwrap(), RefSource::Image("a/b.PNG".into()));
        assert_eq!(source_for("x.jpeg").unwrap().kind(), "image");
        assert_eq!(source_for("walk.pxe.json").unwrap().kind(), "document");
        assert!(source_for("notes.txt").is_err());
    }

    #[test]
    fn loads_documents_and_images_relative_to_the_document() {
        let dir = std::env::temp_dir().join(format!("pxe-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut other = Document::new(2, 1, Document::builtin_palette("pico8").unwrap()).unwrap();
        other.set(0, 0, 1, 0, b'8');
        crate::io::save(&other, &dir.join("other.pxe.json")).unwrap();
        let mut png = Image::new(3, 2);
        png.put(2, 1, [1, 2, 3, 255]);
        png.save_png(&dir.join("ref.png")).unwrap();
        let me = dir.join("me.pxe.json");
        let r = Reference::new(source_for("other.pxe.json").unwrap());
        let img = load(Some(&me), &r, 5).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.get(1, 0), other.color_of(b'8').unwrap().0);
        let img = load(Some(&me), &Reference::new(source_for("ref.png").unwrap()), 0).unwrap();
        assert_eq!(img.get(2, 1), [1, 2, 3, 255]);
        assert!(load(Some(&me), &Reference::new(source_for("gone.png").unwrap()), 0).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compose_scales_offsets_and_blends() {
        let d = Document::new(4, 4, Document::builtin_palette("pico8").unwrap()).unwrap();
        let mut pic = Image::new(2, 2);
        pic.put(0, 0, [255, 0, 0, 255]);
        let mut r = Reference::new(RefSource::Image("p.png".into()));
        r.x = 1.0;
        r.y = 1.0;
        r.scale = 2.0;
        r.opacity = 0.5;
        let sprite = Image::new(4, 4);
        let out = compose(&d, &sprite, &pic, &r);
        // the red reference pixel covers canvas 1..3 x 1..3, at half opacity
        assert_eq!(out.get(1, 1)[3], 128);
        assert_eq!(out.get(2, 2)[0], 255);
        assert_eq!(out.get(0, 0), [0, 0, 0, 0]);
        assert_eq!(out.get(3, 3), [0, 0, 0, 0]);
        r.visible = false;
        assert_eq!(compose(&d, &sprite, &pic, &r).get(1, 1), [0, 0, 0, 0]);
    }
}
