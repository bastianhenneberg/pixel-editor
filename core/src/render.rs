//! Rendering documents to RGBA buffers and PNG files.

use std::path::Path;

use crate::color::blend_over;
use crate::document::Document;

/// A simple RGBA8 image buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Image { width, height, pixels: vec![0; (width * height * 4) as usize] }
    }

    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut img = Image::new(width, height);
        for px in img.pixels.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&rgba);
        }
        img
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    pub fn put(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 4) as usize;
            self.pixels[i..i + 4].copy_from_slice(&rgba);
        }
    }

    pub fn blend(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x < self.width && y < self.height {
            let d = self.get(x, y);
            self.put(x, y, blend_over(d, rgba, 1.0));
        }
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: [u8; 4]) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                self.put(xx, yy, rgba);
            }
        }
    }

    /// Copy `src` into self at (x, y) without blending.
    pub fn paste(&mut self, src: &Image, x: u32, y: u32) {
        for sy in 0..src.height {
            for sx in 0..src.width {
                self.put(x + sx, y + sy, src.get(sx, sy));
            }
        }
    }

    /// Alpha-composite `src` onto self at (x, y).
    pub fn overlay(&mut self, src: &Image, x: u32, y: u32) {
        for sy in 0..src.height {
            for sx in 0..src.width {
                let s = src.get(sx, sy);
                if s[3] > 0 {
                    self.blend(x + sx, y + sy, s);
                }
            }
        }
    }

    /// Nearest-neighbour integer upscale.
    pub fn scaled(&self, scale: u32) -> Image {
        let scale = scale.max(1);
        if scale == 1 {
            return self.clone();
        }
        let mut out = Image::new(self.width * scale, self.height * scale);
        for y in 0..out.height {
            for x in 0..out.width {
                out.put(x, y, self.get(x / scale, y / scale));
            }
        }
        out
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let bytes = self.encode_png()?;
        crate::io::write_atomic(path, &bytes)
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().map_err(|e| e.to_string())?;
            w.write_image_data(&self.pixels).map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    pub fn load_png(path: &Path) -> Result<Image, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
        let mut reader = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        let pixels = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::GrayscaleAlpha => {
                buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            other => return Err(format!("{}: unsupported PNG color type {other:?}", path.display())),
        };
        Ok(Image { width: w, height: h, pixels })
    }
}

/// Composite one layer's cel (with its opacity) onto `img`.
fn draw_layer(doc: &Document, img: &mut Image, layer: usize, frame: usize, opacity: f32) {
    let table = doc.color_table();
    for (i, k) in doc.cel(layer, frame).iter().enumerate() {
        if let Some(c) = table[*k as usize] {
            let j = i * 4;
            let d: [u8; 4] = img.pixels[j..j + 4].try_into().unwrap();
            img.pixels[j..j + 4].copy_from_slice(&blend_over(d, c.0, opacity));
        }
    }
}

/// All visible layers of a frame, bottom to top.
pub fn composite(doc: &Document, frame: usize) -> Image {
    let mut img = Image::new(doc.width, doc.height);
    for (li, l) in doc.layers.iter().enumerate() {
        if l.visible && l.opacity > 0.0 {
            draw_layer(doc, &mut img, li, frame, l.opacity);
        }
    }
    img
}

/// A single layer's cel at full opacity.
pub fn layer_image(doc: &Document, layer: usize, frame: usize) -> Image {
    let mut img = Image::new(doc.width, doc.height);
    draw_layer(doc, &mut img, layer, frame, 1.0);
    img
}

pub const CHECKER: [[u8; 4]; 2] = [[58, 58, 66, 255], [72, 72, 82, 255]];
const BG: [u8; 4] = [30, 30, 36, 255];
const GRID: [u8; 4] = [255, 255, 255, 28];
const GRID8: [u8; 4] = [255, 255, 255, 90];
const TEXT: [u8; 4] = [230, 230, 240, 255];
const DIM: [u8; 4] = [140, 140, 160, 255];
const TEXT_SCALE: u32 = 2;
/// Seam ticks of a tiled preview.
const SEAM: [u8; 4] = [255, 120, 200, 255];

#[derive(Clone, Debug)]
pub struct PreviewOptions {
    /// Screen pixels per art pixel.
    pub zoom: u32,
    pub grid: bool,
    /// Only this layer (at full opacity) instead of the composite.
    pub layer: Option<usize>,
    /// Draw named regions as labelled outlines.
    pub regions: bool,
    /// Show each frame repeated 3x3, as a seamless tile would appear; the
    /// rulers count inside the tile, ticks mark the seams, regions are
    /// drawn on the middle copy.
    pub tiled: bool,
    /// Ways to look at each frame, side by side (empty = normal only).
    pub views: Vec<View>,
}

/// A way of looking at the art in the preview only (pixels and exports stay
/// as they are): the silhouette checks that the shape reads, grayscale that
/// the values hold without hue, mirrored shows proportion errors the eye
/// has got used to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Normal,
    /// Every opaque pixel one dark color on a light background.
    Silhouette,
    /// Luminance only.
    Grayscale,
    /// Flipped horizontally; rulers and regions still name the real pixels.
    Mirrored,
}

/// Silhouette view: the shape and its background.
const SIL_INK: [u8; 4] = [28, 28, 36, 255];
const SIL_BG: [u8; 4] = [226, 224, 214, 255];

impl View {
    pub const ALL: [View; 4] = [View::Normal, View::Silhouette, View::Grayscale, View::Mirrored];

    pub fn id(self) -> &'static str {
        match self {
            View::Normal => "normal",
            View::Silhouette => "silhouette",
            View::Grayscale => "grayscale",
            View::Mirrored => "mirrored",
        }
    }

    pub fn parse(s: &str) -> Result<View, String> {
        View::ALL
            .into_iter()
            .find(|v| v.id() == s)
            .ok_or_else(|| format!("unknown view {s:?} ({})", View::ALL.map(View::id).join(", ")))
    }

    /// The frame image as this view shows it.
    pub fn apply(self, img: &Image) -> Image {
        let mut out = img.clone();
        match self {
            View::Normal => {}
            View::Silhouette => {
                for px in out.pixels.as_chunks_mut::<4>().0 {
                    if px[3] > 0 {
                        px.copy_from_slice(&SIL_INK);
                    }
                }
            }
            View::Grayscale => {
                for px in out.pixels.as_chunks_mut::<4>().0 {
                    // Rec. 709 luma of the sRGB values
                    let l = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32).round() as u8;
                    px[..3].copy_from_slice(&[l, l, l]);
                }
            }
            View::Mirrored => {
                for y in 0..img.height {
                    for x in 0..img.width {
                        out.put(img.width - 1 - x, y, img.get(x, y));
                    }
                }
            }
        }
        out
    }
}

/// A marker on the art pixel whose top-left screen position is (x0, y0) at
/// `zoom`: four arms around the pixel, leaving the pixel itself visible.
/// `clip` = (x0, y0, x1, y1) screen bounds.
pub fn crosshair(out: &mut Image, x0: i32, y0: i32, zoom: u32, c: [u8; 4], clip: (i32, i32, i32, i32)) {
    let z = zoom as i32;
    let arm = z.max(6);
    let t = if zoom >= 8 { 2 } else { 1 };
    let (cx, cy) = (x0 + z / 2, y0 + z / 2);
    let mut put = |x: i32, y: i32| {
        if x >= clip.0 && y >= clip.1 && x < clip.2 && y < clip.3 {
            out.put(x as u32, y as u32, c);
        }
    };
    for d in 1..=arm {
        for w in 0..t {
            let o = w - t / 2;
            put(x0 - d, cy + o); // left of the pixel
            put(x0 + z - 1 + d, cy + o); // right
            put(cx + o, y0 - d); // above
            put(cx + o, y0 + z - 1 + d); // below
        }
    }
}

/// Distinct overlay colors for tags/regions without their own color.
pub fn auto_color(i: usize) -> [u8; 4] {
    const C: [[u8; 4]; 6] = [
        [70, 150, 230, 255],
        [230, 120, 60, 255],
        [90, 190, 110, 255],
        [200, 90, 200, 255],
        [220, 190, 60, 255],
        [70, 190, 190, 255],
    ];
    C[i % C.len()]
}

impl PreviewOptions {
    /// Zoom that makes a frame roughly 384-512 px on its longer side.
    pub fn auto_zoom(doc: &Document) -> u32 {
        (512 / doc.width.max(doc.height)).clamp(1, 32)
    }

    /// Zoom for a tiled (3x3) preview: about 768 px on the longer side.
    pub fn auto_zoom_tiled(doc: &Document) -> u32 {
        (768 / (3 * doc.width.max(doc.height))).clamp(1, 32)
    }
}

/// `img` repeated `nx` times side by side and `ny` times on top of each other.
pub fn tiled(img: &Image, nx: u32, ny: u32) -> Image {
    let mut out = Image::new(img.width * nx, img.height * ny);
    for ty in 0..ny {
        for tx in 0..nx {
            out.paste(img, tx * img.width, ty * img.height);
        }
    }
    out
}

/// Inspection preview, modelled on pixelart-studio's preview: per frame a
/// label (index, name, duration, link/tag info), coordinate rulers, the
/// zoomed frame on a checkerboard with a pixel grid (stronger every 8 px),
/// and 1x/2x thumbnails for a readability check. Below, the palette with
/// each swatch's key. Everything is drawn with nearest-neighbour scaling and
/// a bitmap font, so the image contains no smoothed pixels.
pub fn preview(doc: &Document, frames: &[usize], opts: &PreviewOptions) -> Image {
    use crate::font::{draw_text, fit_text, text_height, text_width};
    let zoom = opts.zoom.clamp(1, 64);
    let ts = TEXT_SCALE;
    // displayed size in art pixels: the frame, or 3x3 copies of it
    let rep = if opts.tiled { 3 } else { 1 };
    let (dw, dh) = (doc.width * rep, doc.height * rep);
    let (tw, th) = (dw * zoom, dh * zoom);
    let pad = 12;
    // label lines: name, info, and a region legend if there are regions
    let label_lines = if opts.regions && !doc.regions.is_empty() { 3 } else { 2 };
    let label_h = label_lines * (text_height(ts) + 3) + 5;
    let digits = (doc.width.max(doc.height).saturating_sub(1)).to_string().len() as u32;
    let ruler_top = text_height(ts) + 4;
    let ruler_left = text_width(&"0".repeat(digits as usize), ts) + 6;
    // label every `step` pixels, so numbers never overlap
    let num_w = text_width(&"0".repeat(digits as usize), ts) + 4;
    let step = [1u32, 2, 4, 8, 16, 32, 64].into_iter().find(|s| s * zoom >= num_w).unwrap_or(64);
    let thumbs_w = dw * 2 + 8;
    let views: Vec<View> = if opts.views.is_empty() { vec![View::Normal] } else { opts.views.clone() };
    // one cell per frame and view, the views of a frame side by side
    let cells: Vec<(usize, View)> = frames.iter().flat_map(|&f| views.iter().map(move |&v| (f, v))).collect();
    // two label lines per cell: "index name" and "ms [tags] LINK [view]"
    let labels: Vec<(String, String)> = cells
        .iter()
        .map(|&(f, view)| {
            let fr = &doc.frames[f];
            let mut info = format!("{}ms", fr.duration_ms);
            let tags: Vec<&str> = doc.tags.iter().filter(|t| t.contains(f)).map(|t| t.name.as_str()).collect();
            if !tags.is_empty() {
                info += &format!(" [{}]", tags.join(","));
            }
            if doc.layers.iter().any(|l| l.link_source(f) != f) {
                info += " LINK";
            }
            if views.len() > 1 || view != View::Normal {
                info += &format!(" {}", view.id().to_uppercase());
            }
            (format!("{f} {}", fr.name), info)
        })
        .collect();
    let min_cell = ruler_left + tw + 8 + thumbs_w + pad;
    let widest = labels.iter().map(|(a, b)| text_width(a, ts).max(text_width(b, ts))).max().unwrap_or(0) + pad;
    let cell_w = min_cell.max(widest.min(min_cell * 2));
    let cell_h = label_h + ruler_top + th.max(dh * 3 + 6) + pad;
    let n = cells.len().max(1) as u32;
    // keep the views of one frame in one row
    let nv = views.len() as u32;
    let cols = if nv > 1 { ((1800 / cell_w) / nv).max(1) * nv } else { 1800 / cell_w }.clamp(1, n);
    let rows = n.div_ceil(cols);
    let sw = 22;
    let pal_h = if doc.palette.is_empty() { 0 } else { sw + 6 + text_height(ts) + pad };
    let pal_w = pad + (doc.palette.len() as u32 + 1) * (sw + 6);
    let width = (pad + cols * cell_w).max(pal_w);
    let height = pad + rows * cell_h + pal_h;
    let mut out = Image::filled(width, height, BG);

    let render_frame = |f: usize| {
        let img = match opts.layer {
            Some(l) => layer_image(doc, l, f),
            None => composite(doc, f),
        };
        if opts.tiled {
            tiled(&img, rep, rep)
        } else {
            img
        }
    };
    for (i, &(f, view)) in cells.iter().enumerate() {
        let mirrored = view == View::Mirrored;
        let ox = pad + (i as u32 % cols) * cell_w;
        let oy = pad + (i as u32 / cols) * cell_h;
        let (name, info) = &labels[i];
        draw_text(&mut out, ox as i32, oy as i32, &fit_text(name, ts, cell_w - pad), ts, TEXT);
        let y2 = (oy + text_height(ts) + 3) as i32;
        draw_text(&mut out, ox as i32, y2, &fit_text(info, ts, cell_w - pad), ts, DIM);
        let gx = ox + ruler_left;
        let gy = oy + label_h + ruler_top;
        // rulers: numbers centred on their pixel column/row
        for x in (0..dw).filter(|x| (x % doc.width).is_multiple_of(step)) {
            // mirrored: the ruler names the real column shown at this place
            let real = if mirrored { doc.width - 1 - x % doc.width } else { x % doc.width };
            let t = real.to_string();
            let cx = gx as i32 + (x * zoom + zoom / 2) as i32 - text_width(&t, ts) as i32 / 2;
            draw_text(&mut out, cx, (gy - ruler_top) as i32, &t, ts, if x % 8 == 0 { TEXT } else { DIM });
        }
        for y in (0..dh).filter(|y| (y % doc.height).is_multiple_of(step)) {
            let t = (y % doc.height).to_string();
            let cy = gy as i32 + (y * zoom + zoom / 2) as i32 - text_height(ts) as i32 / 2;
            let cx = (gx - 4 - text_width(&t, ts)) as i32;
            draw_text(&mut out, cx, cy, &t, ts, if y % 8 == 0 { TEXT } else { DIM });
        }
        let cell = (zoom / 2).max(2);
        let ground = |x: u32, y: u32| {
            if view == View::Silhouette {
                SIL_BG
            } else {
                CHECKER[((x / cell + y / cell) % 2) as usize]
            }
        };
        for y in 0..th {
            for x in 0..tw {
                out.put(gx + x, gy + y, ground(x, y));
            }
        }
        let img = view.apply(&render_frame(f));
        out.overlay(&img.scaled(zoom), gx, gy);
        if opts.tiled {
            // seam ticks between the rulers and the image, the art stays untouched
            for i in 1..rep {
                let sx = gx + i * doc.width * zoom;
                let sy = gy + i * doc.height * zoom;
                out.fill_rect(sx - 1, gy - 4, 3, 4, SEAM);
                out.fill_rect(gx - 4, sy - 1, 4, 3, SEAM);
            }
        }
        if opts.grid && zoom >= 4 {
            for x in 0..=dw {
                let c = if x % 8 == 0 { GRID8 } else { GRID };
                for y in 0..th {
                    out.blend((gx + x * zoom).min(gx + tw - 1), gy + y, c);
                }
            }
            for y in 0..=dh {
                let c = if y % 8 == 0 { GRID8 } else { GRID };
                for x in 0..tw {
                    out.blend(gx + x, (gy + y * zoom).min(gy + th - 1), c);
                }
            }
        }
        if opts.regions {
            for (ri, r) in doc.regions.iter().enumerate() {
                let Some(rc) = r.rect_at(f) else { continue };
                if opts.layer.is_some_and(|l| !r.applies_to_layer(doc, l)) {
                    continue;
                }
                let c = r.color.map_or(auto_color(ri), |c| [c.0[0], c.0[1], c.0[2], 255]);
                let t = if zoom >= 6 { 2 } else { 1 };
                // tiled: on the middle copy
                let (mx, my) = if opts.tiled { (doc.width * zoom, doc.height * zoom) } else { (0, 0) };
                // mirrored: the region where its pixels are shown
                let rx = if mirrored { doc.width as i32 - rc.x - rc.w as i32 } else { rc.x };
                let (x0, y0) = ((gx + mx) as i32 + rx * zoom as i32, (gy + my) as i32 + rc.y * zoom as i32);
                let clip = (gx as i32, gy as i32, (gx + tw) as i32, (gy + th) as i32);
                if r.is_marker() {
                    crosshair(&mut out, x0, y0, zoom, c, clip);
                    continue;
                }
                let (x1, y1) = (x0 + (rc.w * zoom) as i32, y0 + (rc.h * zoom) as i32);
                let clipped =
                    |x: i32, y: i32| x >= gx as i32 && y >= gy as i32 && x < (gx + tw) as i32 && y < (gy + th) as i32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let edge = x < x0 + t || x >= x1 - t || y < y0 + t || y >= y1 - t;
                        if edge && clipped(x, y) && (!r.locked || (x + y) / 3 % 2 == 0) {
                            out.put(x as u32, y as u32, c);
                        }
                    }
                }
            }
            // legend instead of labels on the art, so no art pixel is hidden
            let mut lx = ox as i32;
            let ly = (oy + 2 * (text_height(ts) + 3)) as i32;
            for (ri, r) in doc.regions.iter().enumerate() {
                if r.rect_at(f).is_none() || opts.layer.is_some_and(|l| !r.applies_to_layer(doc, l)) {
                    continue;
                }
                let c = r.color.map_or(auto_color(ri), |c| [c.0[0], c.0[1], c.0[2], 255]);
                let mut label = if r.is_marker() { format!("+{}", r.name) } else { r.name.clone() };
                if r.locked {
                    label += "(locked)";
                }
                if lx as u32 + text_width(&label, ts) > ox + cell_w - pad {
                    draw_text(&mut out, lx, ly, "~", ts, c);
                    break;
                }
                draw_text(&mut out, lx, ly, &label, ts, c);
                lx += (text_width(&label, ts) + 8) as i32;
            }
        }
        // actual-size thumbnails: does it read at 1x and 2x?
        let tx = gx + tw + 8;
        if view == View::Silhouette {
            out.fill_rect(tx, gy, dw, dh, SIL_BG);
            out.fill_rect(tx, gy + dh + 6, dw * 2, dh * 2, SIL_BG);
        }
        out.overlay(&img, tx, gy);
        out.overlay(&img.scaled(2), tx, gy + dh + 6);
    }

    if !doc.palette.is_empty() {
        let py = height - pal_h;
        let entries = std::iter::once(None).chain(doc.palette.iter().map(Some));
        for (i, p) in entries.enumerate() {
            let x = pad + i as u32 * (sw + 6);
            out.fill_rect(x, py, sw, sw, [200, 200, 210, 255]);
            let inner = sw - 2;
            for yy in 0..inner {
                for xx in 0..inner {
                    out.put(x + 1 + xx, py + 1 + yy, CHECKER[(((xx / 5) + (yy / 5)) % 2) as usize]);
                }
            }
            let key = match p {
                Some(p) => {
                    for yy in 0..inner {
                        for xx in 0..inner {
                            out.blend(x + 1 + xx, py + 1 + yy, p.color.0);
                        }
                    }
                    (p.key as char).to_string()
                }
                None => ".".to_string(),
            };
            let kx = x as i32 + (sw as i32 - text_width(&key, ts) as i32) / 2;
            draw_text(&mut out, kx, (py + sw + 4) as i32, &key, ts, TEXT);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_respects_visibility_and_opacity() {
        let mut d = Document::new(2, 1, Document::builtin_palette("mono").unwrap()).unwrap();
        d.set(0, 0, 0, 0, b'0');
        let top = d.add_layer("top", 1);
        d.set(top, 0, 0, 0, b'1');
        d.set(top, 0, 1, 0, b'1');
        let img = composite(&d, 0);
        assert_eq!(img.get(0, 0), [255, 255, 255, 255]);
        d.layers[top].opacity = 0.5;
        let img = composite(&d, 0);
        assert_eq!(img.get(0, 0), [128, 128, 128, 255]);
        assert_eq!(img.get(1, 0)[3], 128);
        d.layers[top].visible = false;
        assert_eq!(composite(&d, 0).get(1, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn png_roundtrip() {
        let mut img = Image::new(3, 2);
        img.put(1, 1, [1, 2, 3, 4]);
        let dir = std::env::temp_dir().join(format!("pxe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.png");
        img.save_png(&p).unwrap();
        assert_eq!(Image::load_png(&p).unwrap(), img);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preview_has_exact_colors() {
        let mut d = Document::new(4, 4, Document::builtin_palette("pico8").unwrap()).unwrap();
        d.set_rows(0, 0, 0, 0, &["8c".into(), "c8".into()]).unwrap();
        let zoom = 6;
        let img = preview(
            &d,
            &[0],
            &PreviewOptions { zoom, grid: false, layer: None, regions: false, tiled: false, views: vec![] },
        );
        // find the zoomed frame: the first run of zoom x zoom pixels of color '8'
        let red = d.color_of(b'8').unwrap().0;
        let (mut found, w) = (None, img.width);
        'scan: for y in 0..img.height - zoom {
            for x in 0..w - zoom {
                if img.get(x, y) == red {
                    found = Some((x, y));
                    break 'scan;
                }
            }
        }
        let (x0, y0) = found.expect("frame drawn");
        for (ax, ay, k) in [(0, 0, b'8'), (1, 0, b'c'), (0, 1, b'c'), (1, 1, b'8')] {
            for dy in 0..zoom {
                for dx in 0..zoom {
                    assert_eq!(img.get(x0 + ax * zoom + dx, y0 + ay * zoom + dy), d.color_of(k).unwrap().0);
                }
            }
        }
    }

    #[test]
    fn markers_are_crosshairs_not_boxes() {
        let mut d = Document::new(8, 8, Document::builtin_palette("pico8").unwrap()).unwrap();
        d.set(0, 0, 4, 4, b'8');
        let mut m = crate::document::Region::marker("origin", 4, 4);
        m.color = Some(crate::Rgba([0, 255, 0, 255]));
        d.regions.push(m);
        let zoom = 8;
        let img = preview(
            &d,
            &[0],
            &PreviewOptions { zoom, grid: false, layer: None, regions: true, tiled: false, views: vec![] },
        );
        // the marked art pixel keeps its color; green arms around it
        let red = d.color_of(b'8').unwrap().0;
        // leftmost red pixel = the zoomed frame (thumbnails are to its right)
        let mut px = None;
        'scan: for x in 0..img.width {
            for y in 0..img.height {
                if img.get(x, y) == red {
                    px = Some((x, y));
                    break 'scan;
                }
            }
        }
        let (x0, y0) = px.expect("marked pixel drawn");
        assert_eq!(img.get(x0 + 3, y0 + 3), red, "pixel under the marker untouched");
        assert_eq!(img.get(x0 - 2, y0 + 4), [0, 255, 0, 255], "left arm");
        assert_eq!(img.get(x0 + 4, y0 + zoom + 1), [0, 255, 0, 255], "lower arm");
        assert_ne!(img.get(x0 - 2, y0 - 2), [0, 255, 0, 255], "no box corner");
    }

    #[test]
    fn tiled_preview_repeats_the_frame_3x3() {
        let mut d = Document::new(4, 3, Document::builtin_palette("pico8").unwrap()).unwrap();
        d.set(0, 0, 3, 2, b'8');
        let t = tiled(&composite(&d, 0), 3, 3);
        assert_eq!((t.width, t.height), (12, 9));
        let red = d.color_of(b'8').unwrap().0;
        for (x, y) in [(3, 2), (7, 2), (11, 8), (3, 5)] {
            assert_eq!(t.get(x, y), red, "({x}, {y})");
        }
        assert_eq!(t.get(4, 2), [0, 0, 0, 0]);
        let zoom = 5;
        let opts = PreviewOptions { zoom, grid: false, layer: None, regions: false, tiled: true, views: vec![] };
        let plain = preview(&d, &[0], &PreviewOptions { tiled: false, ..opts.clone() });
        let img = preview(&d, &[0], &opts);
        assert!(img.height > plain.height);
        // the red pixel appears 9 times zoomed: count red blocks exactly zoom wide
        let corners = (1..img.height - zoom)
            .flat_map(|y| (1..img.width - zoom).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                img.get(x, y) == red
                    && img.get(x - 1, y) != red
                    && img.get(x, y - 1) != red
                    && img.get(x + zoom - 1, y + zoom - 1) == red
                    && img.get(x + zoom, y) != red
            })
            .count();
        assert_eq!(corners, 9);
    }

    #[test]
    fn scaled() {
        let mut img = Image::new(2, 1);
        img.put(1, 0, [9, 9, 9, 255]);
        let s = img.scaled(3);
        assert_eq!((s.width, s.height), (6, 3));
        assert_eq!(s.get(5, 2), [9, 9, 9, 255]);
        assert_eq!(s.get(2, 2), [0, 0, 0, 0]);
    }
}
