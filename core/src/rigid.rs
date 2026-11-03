//! Rigid parts (sword, shield, helmet): check that a part of one layer stays
//! pixel-identical across frames and only moves, and move a layer's pixels
//! frame by frame (e.g. along the body bob).

use crate::document::{Document, Rect, TRANSPARENT};

/// A pixel where a frame differs from the shifted reference part, in the
/// frame's coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelDiff {
    pub x: i32,
    pub y: i32,
    /// The reference part's key there ('.' for a pixel the part does not have).
    pub expected: u8,
    pub found: u8,
}

/// The best match of the reference part on one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub frame: usize,
    pub dx: i32,
    pub dy: i32,
    /// Empty = the part is rigid on this frame.
    pub diffs: Vec<PixelDiff>,
}

/// The part to follow on the reference frame: `area` (a region or rect),
/// or the opaque bounding box of the whole cel.
pub fn part_area(doc: &Document, layer: usize, reference: usize, area: Option<Rect>) -> Result<Rect, String> {
    let r = match area {
        Some(r) => r,
        None => {
            let (x, y, w, h) = doc
                .cel_bounds(layer, reference)
                .ok_or_else(|| format!("layer {:?} is empty on the reference frame", doc.layers[layer].name))?;
            Rect::new(x as i32, y as i32, w, h)
        }
    };
    let opaque = cells(r).any(|(x, y)| doc.get(layer, reference, x, y) != TRANSPARENT);
    if !opaque {
        return Err(format!("the part {} on the reference frame has no pixels", r.describe()));
    }
    Ok(r)
}

/// Find the shift (within `max_shift` of `centre`) at which `frame` shows
/// the part `area` of the reference frame best. Every pixel of the area
/// counts, transparent ones too; with `whole_cel`, opaque pixels of the
/// frame outside the shifted area count as differences as well (the layer
/// holds only the part). Ties go to the shift nearest `centre`.
#[allow(clippy::too_many_arguments)]
pub fn match_part(
    doc: &Document,
    layer: usize,
    reference: usize,
    area: Rect,
    whole_cel: bool,
    frame: usize,
    centre: (i32, i32),
    max_shift: i32,
) -> Match {
    let opaque_total = if whole_cel { opaque_in(doc, layer, frame, canvas(doc)) } else { 0 };
    let score = |dx: i32, dy: i32| {
        let inside = cells(area)
            .filter(|&(x, y)| doc.get(layer, reference, x, y) != doc.get(layer, frame, x + dx, y + dy))
            .count();
        let extra = if whole_cel { opaque_total - opaque_in(doc, layer, frame, area.translate(dx, dy)) } else { 0 };
        inside + extra
    };
    let mut best = (usize::MAX, i32::MAX, centre);
    for dy in centre.1 - max_shift..=centre.1 + max_shift {
        for dx in centre.0 - max_shift..=centre.0 + max_shift {
            let dist = (dx - centre.0).abs() + (dy - centre.1).abs();
            let s = score(dx, dy);
            if (s, dist) < (best.0, best.1) {
                best = (s, dist, (dx, dy));
            }
        }
    }
    let (dx, dy) = best.2;
    let mut diffs: Vec<PixelDiff> = cells(area)
        .map(|(x, y)| PixelDiff {
            x: x + dx,
            y: y + dy,
            expected: doc.get(layer, reference, x, y),
            found: doc.get(layer, frame, x + dx, y + dy),
        })
        .filter(|d| d.expected != d.found)
        .collect();
    if whole_cel {
        let moved = area.translate(dx, dy);
        diffs.extend(
            cells(canvas(doc))
                .filter(|&(x, y)| !moved.contains(x, y) && doc.get(layer, frame, x, y) != TRANSPARENT)
                .map(|(x, y)| PixelDiff { x, y, expected: TRANSPARENT, found: doc.get(layer, frame, x, y) }),
        );
        diffs.sort_by_key(|d| (d.y, d.x));
    }
    Match { frame, dx, dy, diffs }
}

/// Move the pixels of cel (layer, frame) inside `area` (None = the whole
/// cel) by (dx, dy), like move_region: the area is cleared, then its opaque
/// pixels are written at the offset (they may leave the area). Returns the
/// number of opaque pixels cut off at the canvas edge. Linked frames share
/// the cel and see the change.
pub fn shift_area(doc: &mut Document, layer: usize, frame: usize, area: Option<Rect>, dx: i32, dy: i32) -> usize {
    let area = area.unwrap_or(canvas(doc));
    let block: Vec<(i32, i32, u8)> = cells(area).map(|(x, y)| (x, y, doc.get(layer, frame, x, y))).collect();
    for &(x, y, _) in &block {
        doc.set(layer, frame, x, y, TRANSPARENT);
    }
    let mut cut = 0;
    for &(x, y, k) in block.iter().filter(|b| b.2 != TRANSPARENT) {
        if doc.in_bounds(x + dx, y + dy) {
            doc.set(layer, frame, x + dx, y + dy, k);
        } else {
            cut += 1;
        }
    }
    cut
}

fn canvas(doc: &Document) -> Rect {
    Rect::new(0, 0, doc.width, doc.height)
}

fn cells(r: Rect) -> impl Iterator<Item = (i32, i32)> {
    (r.y..r.y + r.h as i32).flat_map(move |y| (r.x..r.x + r.w as i32).map(move |x| (x, y)))
}

fn opaque_in(doc: &Document, layer: usize, frame: usize, r: Rect) -> usize {
    cells(r).filter(|&(x, y)| doc.get(layer, frame, x, y) != TRANSPARENT).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One layer, one frame per entry of `frames` (rows each).
    fn doc(frames: &[&[&str]]) -> Document {
        let (w, h) = (frames[0][0].len() as u32, frames[0].len() as u32);
        let mut d = Document::new(w, h, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        for (f, rows) in frames.iter().enumerate() {
            if f > 0 {
                d.add_frame(f, None, false).unwrap();
            }
            let rows: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
            d.set_rows(0, f, 0, 0, &rows).unwrap();
        }
        d
    }

    #[test]
    fn finds_the_offset_and_the_changed_pixel() {
        let d = doc(&[&["12..", "3...", "...."], &["....", "12..", "3..."], &["....", ".12.", ".4.."]]);
        let area = part_area(&d, 0, 0, None).unwrap();
        assert_eq!(area, Rect::new(0, 0, 2, 2));
        let m = match_part(&d, 0, 0, area, true, 1, (0, 0), 2);
        assert_eq!((m.dx, m.dy, m.diffs.len()), (0, 1, 0));
        let m = match_part(&d, 0, 0, area, true, 2, (0, 0), 2);
        assert_eq!((m.dx, m.dy), (1, 1));
        assert_eq!(m.diffs, [PixelDiff { x: 1, y: 2, expected: b'3', found: b'4' }]);
        // the frame itself matches at (0, 0)
        assert_eq!(match_part(&d, 0, 0, area, true, 0, (0, 0), 2).diffs, []);
    }

    #[test]
    fn whole_cel_counts_extra_pixels_and_an_empty_part_is_refused() {
        let d = doc(&[&["1...", "...."], &["1...", "...5"]]);
        let area = part_area(&d, 0, 0, None).unwrap();
        let m = match_part(&d, 0, 0, area, true, 1, (0, 0), 1);
        assert_eq!(m.diffs, [PixelDiff { x: 3, y: 1, expected: TRANSPARENT, found: b'5' }]);
        // a rect around the part alone does not see the extra pixel
        assert_eq!(match_part(&d, 0, 0, Rect::new(0, 0, 2, 1), false, 1, (0, 0), 1).diffs, []);
        assert!(part_area(&d, 0, 0, Some(Rect::new(2, 0, 2, 2))).is_err());
    }

    #[test]
    fn shift_moves_clears_and_cuts() {
        let mut d = doc(&[&["12..", "3...", "...."]]);
        assert_eq!(shift_area(&mut d, 0, 0, None, 1, 1), 0);
        assert_eq!(d.rows(0, 0), ["....", ".12.", ".3.."]);
        assert_eq!(shift_area(&mut d, 0, 0, None, 0, 1), 1, "the bottom row falls off");
        assert_eq!(d.rows(0, 0), ["....", "....", ".12."]);
        // an area: only its pixels move, transparent ones keep what is under them
        let mut d = doc(&[&["1.5.", "...."]]);
        assert_eq!(shift_area(&mut d, 0, 0, Some(Rect::new(0, 0, 2, 1)), 2, 1), 0);
        assert_eq!(d.rows(0, 0), ["..5.", "..1."]);
    }
}
