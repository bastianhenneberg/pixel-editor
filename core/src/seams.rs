//! Seam check for tiles: does a frame continue across its own edges when it
//! is repeated? The rightmost column meets the leftmost one, the bottom row
//! the top row. Each pair is compared by color; a pair that differs more
//! than `threshold` is a hard edge. Hard edges are judged against the tile's
//! own interior: a tile full of contrast may have contrast at the seam too,
//! a calm tile may not.

use crate::document::Document;
use crate::render::{self, Image};

/// Default color distance (0..1) from which a pair counts as a hard edge.
pub const DEFAULT_THRESHOLD: f32 = 0.25;

/// One pair of edge pixels that meet in the tiled image.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgePair {
    /// Row (left/right seam) or column (top/bottom seam).
    pub pos: u32,
    /// Keys on both sides: right/bottom edge pixel first, then left/top.
    pub keys: (u8, u8),
    pub distance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seam {
    /// Pixel pairs along the seam.
    pub pairs: u32,
    /// Pairs whose colors differ at all.
    pub differing: u32,
    /// Pairs at or above the threshold.
    pub hard: Vec<EdgePair>,
    /// Share of hard edges between neighbours inside the tile, in the same
    /// direction (columns next to each other for the left/right seam).
    pub interior_rate: f32,
}

impl Seam {
    pub fn hard_rate(&self) -> f32 {
        self.hard.len() as f32 / self.pairs.max(1) as f32
    }

    /// Clearly more hard edges at the seam than inside the tile.
    pub fn stands_out(&self) -> bool {
        !self.hard.is_empty() && self.hard_rate() > self.interior_rate * 2.0 + 0.05
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeamReport {
    /// Rightmost column against the leftmost (tiling side by side).
    pub left_right: Seam,
    /// Bottom row against the top row (tiling on top of each other).
    pub top_bottom: Seam,
    pub threshold: f32,
}

/// Color distance 0..1 of two RGBA colors (premultiplied, so two
/// transparent pixels are equal whatever their RGB).
pub fn distance(a: [u8; 4], b: [u8; 4]) -> f32 {
    let pm = |c: [u8; 4], i: usize| c[i] as f32 * c[3] as f32 / 255.0;
    let d: f32 = (0..3).map(|i| (pm(a, i) - pm(b, i)).powi(2)).sum::<f32>() + (a[3] as f32 - b[3] as f32).powi(2);
    d.sqrt() / 510.0
}

/// Compare the edges of `frame` (the visible composite, or only `layer`).
pub fn check(doc: &Document, frame: usize, layer: Option<usize>, threshold: f32) -> SeamReport {
    let img: Image = match layer {
        Some(l) => render::layer_image(doc, l, frame),
        None => render::composite(doc, frame),
    };
    let key = |x: u32, y: u32| match layer {
        Some(l) => doc.get(l, frame, x as i32, y as i32),
        None => doc.top_key(frame, x as i32, y as i32),
    };
    let (w, h) = (img.width, img.height);
    // pairs (a, b) of neighbours inside the tile, and across the seam
    let seam = |pairs: Vec<((u32, u32), (u32, u32))>, interior: Vec<((u32, u32), (u32, u32))>, along_y: bool| {
        let hard_inside =
            interior.iter().filter(|(a, b)| distance(img.get(a.0, a.1), img.get(b.0, b.1)) >= threshold).count();
        let mut s = Seam {
            pairs: pairs.len() as u32,
            differing: 0,
            hard: Vec::new(),
            interior_rate: hard_inside as f32 / interior.len().max(1) as f32,
        };
        for (a, b) in pairs {
            let (ca, cb) = (img.get(a.0, a.1), img.get(b.0, b.1));
            if ca != cb {
                s.differing += 1;
            }
            let d = distance(ca, cb);
            if d >= threshold {
                let pos = if along_y { a.1 } else { a.0 };
                s.hard.push(EdgePair { pos, keys: (key(a.0, a.1), key(b.0, b.1)), distance: d });
            }
        }
        s
    };
    let lr_pairs = (0..h).map(|y| ((w - 1, y), (0, y))).collect();
    let lr_inside = (0..h).flat_map(|y| (0..w.saturating_sub(1)).map(move |x| ((x, y), (x + 1, y)))).collect();
    let tb_pairs = (0..w).map(|x| ((x, h - 1), (x, 0))).collect();
    let tb_inside = (0..h.saturating_sub(1)).flat_map(|y| (0..w).map(move |x| ((x, y), (x, y + 1)))).collect();
    SeamReport { left_right: seam(lr_pairs, lr_inside, true), top_bottom: seam(tb_pairs, tb_inside, false), threshold }
}

impl SeamReport {
    /// Text for people and Claude: per seam a verdict, then the hard edges
    /// as runs ("y 4..7: g grass | d dirt").
    pub fn describe(&self, doc: &Document) -> String {
        let mut out = String::new();
        for (name, axis, s) in [
            ("left/right seam (x=last | x=0)", "y", &self.left_right),
            ("top/bottom seam (y=last | y=0)", "x", &self.top_bottom),
        ] {
            let verdict = if s.differing == 0 {
                "seamless"
            } else if s.hard.is_empty() {
                "no hard edges"
            } else if s.stands_out() {
                "VISIBLE SEAM"
            } else {
                "no more contrast than inside the tile"
            };
            out += &format!(
                "{name}: {verdict}. {} of {} pairs differ, {} hard ({:.0}%), interior hard edges {:.0}%\n",
                s.differing,
                s.pairs,
                s.hard.len(),
                s.hard_rate() * 100.0,
                s.interior_rate * 100.0
            );
            // consecutive positions with the same keys form one run
            let mut runs: Vec<(u32, u32, (u8, u8), f32)> = Vec::new();
            for p in &s.hard {
                match runs.last_mut() {
                    Some(r) if r.1 + 1 == p.pos && r.2 == p.keys => {
                        r.1 = p.pos;
                        r.3 = r.3.max(p.distance);
                    }
                    _ => runs.push((p.pos, p.pos, p.keys, p.distance)),
                }
            }
            for (a, b, (ka, kb), d) in runs {
                let at = if a == b { format!("{axis} {a}") } else { format!("{axis} {a}..{b}") };
                out += &format!("  {at}: {} | {} (distance {d:.2})\n", doc.key_label(ka), doc.key_label(kb));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(rows: &[&str]) -> Document {
        let mut d = Document::new(rows[0].len() as u32, rows.len() as u32, Document::builtin_palette("pico8").unwrap())
            .unwrap();
        d.set_rows(0, 0, 0, 0, &rows.iter().map(|r| r.to_string()).collect::<Vec<_>>()).unwrap();
        d
    }

    #[test]
    fn calm_tile_with_a_break_at_the_edge() {
        // green everywhere, but the right column is white: a hard left/right seam
        let d = doc(&["bbb7", "bbb7", "bbb7", "bbbb"]);
        let r = check(&d, 0, None, DEFAULT_THRESHOLD);
        assert_eq!(r.left_right.pairs, 4);
        assert_eq!(r.left_right.hard.iter().map(|p| p.pos).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(r.left_right.hard[0].keys, (b'7', b'b'));
        assert!(r.left_right.stands_out());
        // top/bottom: column 3 meets 7 (row 3: b) vs 7 (row 0) -> one hard pair
        assert_eq!(r.top_bottom.hard.len(), 1);
        let text = r.describe(&d);
        assert!(text.contains("left/right seam (x=last | x=0): VISIBLE SEAM"), "{text}");
        assert!(text.contains("y 0..2: 7"), "{text}");
    }

    #[test]
    fn seamless_and_busy_tiles() {
        let d = doc(&["b8b8", "8b8b"]);
        let r = check(&d, 0, None, DEFAULT_THRESHOLD);
        assert!(r.top_bottom.hard.len() == 4 && !r.top_bottom.stands_out(), "a checker pattern is busy everywhere");
        let flat = doc(&["bbbb", "bbbb"]);
        let r = check(&flat, 0, None, DEFAULT_THRESHOLD);
        assert!(r.left_right.hard.is_empty() && r.top_bottom.hard.is_empty());
        assert!(r.describe(&flat).contains("seamless"));
        assert_eq!(distance([1, 2, 3, 0], [200, 0, 0, 0]), 0.0);
    }
}
