//! Color edits over an area: replace one color by another, dither patterns
//! for fills and the automatic 1-px outline.
//!
//! Each edit is planned first (`*_points` returns the pixels that would
//! change, so a dialog can show the count) and applied with `apply`.
//! An area is a rect of the canvas with an optional selection mask.

use crate::document::{Document, Rect, TRANSPARENT};

/// The pixels an edit may read and change: a rect, optionally masked.
#[derive(Clone, Copy, Debug)]
pub struct Area<'a> {
    pub rect: Rect,
    /// One flag per pixel of `rect`, row by row (None = all of it).
    pub mask: Option<&'a [bool]>,
}

impl<'a> Area<'a> {
    pub fn canvas(doc: &Document) -> Area<'static> {
        Area { rect: Rect::new(0, 0, doc.width, doc.height), mask: None }
    }

    pub fn rect(rect: Rect) -> Area<'a> {
        Area { rect, mask: None }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        let r = self.rect;
        r.contains(x, y) && self.mask.is_none_or(|m| m[((y - r.y) as u32 * r.w + (x - r.x) as u32) as usize])
    }

    /// The canvas pixels of the area, row by row.
    fn points(&self, doc: &Document) -> impl Iterator<Item = (i32, i32)> + '_ {
        let r = self.rect;
        let (w, h) = (doc.width as i32, doc.height as i32);
        (r.y.max(0)..(r.y + r.h as i32).min(h))
            .flat_map(move |y| (r.x.max(0)..(r.x + r.w as i32).min(w)).map(move |x| (x, y)))
            .filter(move |&(x, y)| self.contains(x, y))
    }
}

/// A planned edit: the pixels to set and how many were left alone because
/// they lie in a locked region.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub pixels: Vec<(i32, i32, u8)>,
    pub protected: usize,
}

impl Plan {
    fn push(&mut self, doc: &Document, layer: usize, frame: usize, (x, y): (i32, i32), key: u8, locks: bool) {
        if doc.get(layer, frame, x, y) == key {
            return;
        }
        if locks && doc.locked_region_at(layer, frame, x, y).is_some() {
            self.protected += 1;
        } else {
            self.pixels.push((x, y, key));
        }
    }

    /// Write the planned pixels into cel (layer, frame); returns the count.
    pub fn apply(&self, doc: &mut Document, layer: usize, frame: usize) -> usize {
        self.pixels.iter().filter(|&&(x, y, k)| doc.set(layer, frame, x, y, k)).count()
    }
}

/// Every pixel of color `from` in the area becomes `to`. With
/// `respect_locks`, pixels in locked regions stay (counted as protected).
pub fn replace_points(
    doc: &Document,
    layer: usize,
    frame: usize,
    area: Area,
    from: u8,
    to: u8,
    respect_locks: bool,
) -> Plan {
    let mut plan = Plan::default();
    if from == to {
        return plan;
    }
    for p in area.points(doc) {
        if doc.get(layer, frame, p.0, p.1) == from {
            plan.push(doc, layer, frame, p, to, respect_locks);
        }
    }
    plan
}

// ---- dither ---------------------------------------------------------------

/// The ordered 4x4 Bayer matrix: a pixel gets the first color when its
/// entry is below the level (1..=15 of 16).
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// A two-color fill pattern. It is aligned to the canvas (pixel (x, y)
/// always gets the same color), and every period divides 4, so fills next
/// to each other and tiles of 4·n pixels continue without a seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Checkerboard, half and half.
    Checker,
    /// A quarter first color, spread evenly.
    Quarter,
    /// Three quarters first color.
    ThreeQuarters,
    /// Bayer 4x4 at level 1..=15: level/16 first color.
    Bayer(u8),
    /// Horizontal lines, every other row first color.
    LinesH,
    /// Diagonal lines (rising to the right), every fourth diagonal.
    LinesD,
}

impl Pattern {
    /// The patterns offered in menus (Bayer in a few steps).
    pub const MENU: [Pattern; 9] = [
        Pattern::Checker,
        Pattern::Quarter,
        Pattern::ThreeQuarters,
        Pattern::Bayer(2),
        Pattern::Bayer(6),
        Pattern::Bayer(10),
        Pattern::Bayer(14),
        Pattern::LinesH,
        Pattern::LinesD,
    ];

    pub fn id(self) -> String {
        match self {
            Pattern::Checker => "checker".into(),
            Pattern::Quarter => "25".into(),
            Pattern::ThreeQuarters => "75".into(),
            Pattern::Bayer(n) => format!("bayer{n}"),
            Pattern::LinesH => "lines_h".into(),
            Pattern::LinesD => "lines_d".into(),
        }
    }

    pub fn parse(s: &str) -> Result<Pattern, String> {
        let p = match s {
            "checker" | "50" => Pattern::Checker,
            "25" => Pattern::Quarter,
            "75" => Pattern::ThreeQuarters,
            "lines_h" => Pattern::LinesH,
            "lines_d" => Pattern::LinesD,
            _ => match s.strip_prefix("bayer").and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=15) => Pattern::Bayer(n),
                _ => {
                    return Err(format!(
                        "unknown pattern {s:?} (checker, 25, 75, bayer1..bayer15 = n/16 first color, lines_h, lines_d)"
                    ))
                }
            },
        };
        Ok(p)
    }

    /// German label for the GUI.
    pub fn label(self) -> String {
        match self {
            Pattern::Checker => "Schachbrett 50 %".into(),
            Pattern::Quarter => "25 %".into(),
            Pattern::ThreeQuarters => "75 %".into(),
            Pattern::Bayer(n) => format!("Bayer 4×4 · {n}/16"),
            Pattern::LinesH => "Linien horizontal".into(),
            Pattern::LinesD => "Linien diagonal".into(),
        }
    }

    /// Does canvas pixel (x, y) get the first color?
    pub fn first(self, x: i32, y: i32) -> bool {
        let (x4, y4) = (x.rem_euclid(4), y.rem_euclid(4));
        match self {
            Pattern::Checker => (x + y).rem_euclid(2) == 0,
            // every other row, shifted by two: an even, staggered grid
            Pattern::Quarter => (x4 + 2 * y4) % 4 == 0,
            Pattern::ThreeQuarters => (x4 + 2 * y4) % 4 != 0,
            Pattern::Bayer(n) => BAYER4[y4 as usize][x4 as usize] < n,
            Pattern::LinesH => y.rem_euclid(2) == 0,
            Pattern::LinesD => (x4 + y4) % 4 == 3,
        }
    }

    pub fn pick(self, x: i32, y: i32, a: u8, b: u8) -> u8 {
        if self.first(x, y) {
            a
        } else {
            b
        }
    }
}

/// The pixels a dither fill of `points` sets (colors `a` and `b` by the
/// pattern); points outside the canvas are dropped.
pub fn dither_points(
    doc: &Document,
    layer: usize,
    frame: usize,
    points: &[(i32, i32)],
    pattern: Pattern,
    a: u8,
    b: u8,
) -> Plan {
    let mut plan = Plan::default();
    let mut seen = std::collections::HashSet::new();
    for &(x, y) in points {
        if doc.in_bounds(x, y) && seen.insert((x, y)) {
            plan.push(doc, layer, frame, (x, y), pattern.pick(x, y, a, b), false);
        }
    }
    plan
}

// ---- outline --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineMode {
    /// Around the shape, on the transparent pixels touching it.
    Outside,
    /// On the shape's own edge pixels.
    Inside,
}

impl OutlineMode {
    pub fn id(self) -> &'static str {
        match self {
            OutlineMode::Outside => "outside",
            OutlineMode::Inside => "inside",
        }
    }

    pub fn parse(s: &str) -> Result<OutlineMode, String> {
        match s {
            "outside" => Ok(OutlineMode::Outside),
            "inside" => Ok(OutlineMode::Inside),
            _ => Err(format!("unknown mode {s:?} (outside, inside)")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutlineMode::Outside => "außen",
            OutlineMode::Inside => "innen",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outline {
    pub color: u8,
    pub mode: OutlineMode,
    /// 8-neighbourhood (diagonals touch too) instead of 4.
    pub eight: bool,
    /// Outside only: only transparent pixels get the outline, colored
    /// ones (e.g. next to a selection) stay. Inside it has no meaning,
    /// as the shape's pixels are colored by definition, and is ignored.
    pub only_transparent: bool,
}

/// A 1-px outline around the non-transparent pixels of the area (the
/// shape) in cel (layer, frame). Outside: every pixel not in the shape
/// that touches it (4 or 8 neighbours) gets the color; it may lie just
/// outside the area. Inside: every shape pixel that touches a pixel
/// outside the shape (or the canvas edge) gets it.
pub fn outline_points(doc: &Document, layer: usize, frame: usize, area: Area, o: Outline, respect_locks: bool) -> Plan {
    let (w, h) = (doc.width as i32, doc.height as i32);
    let mut shape = vec![false; (w * h) as usize];
    for (x, y) in area.points(doc) {
        shape[(y * w + x) as usize] = doc.get(layer, frame, x, y) != TRANSPARENT;
    }
    let in_shape = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && shape[(y * w + x) as usize];
    const N4: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    const N8: [(i32, i32); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    let near: &[(i32, i32)] = if o.eight { &N8 } else { &N4 };
    let mut plan = Plan::default();
    for y in 0..h {
        for x in 0..w {
            let edge = match o.mode {
                OutlineMode::Outside => !in_shape(x, y) && near.iter().any(|(dx, dy)| in_shape(x + dx, y + dy)),
                OutlineMode::Inside => in_shape(x, y) && near.iter().any(|(dx, dy)| !in_shape(x + dx, y + dy)),
            };
            let keep =
                o.only_transparent && o.mode == OutlineMode::Outside && doc.get(layer, frame, x, y) != TRANSPARENT;
            if edge && !keep {
                plan.push(doc, layer, frame, (x, y), o.color, respect_locks);
            }
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Region;

    fn doc(rows: &[&str]) -> Document {
        let mut d =
            Document::new(rows[0].len() as u32, rows.len() as u32, Document::builtin_palette("sweetie16").unwrap())
                .unwrap();
        let rows: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        d.set_rows(0, 0, 0, 0, &rows).unwrap();
        d
    }

    fn run(d: &mut Document, plan: Plan) -> Vec<String> {
        plan.apply(d, 0, 0);
        d.rows(0, 0)
    }

    #[test]
    fn replace_in_area_mask_and_locks() {
        let mut d = doc(&["1121", "1.11", "2111"]);
        let all = Area::canvas(&d);
        let plan = replace_points(&d, 0, 0, all, b'1', b'3', true);
        assert_eq!(plan.pixels.len(), 9);
        assert_eq!(run(&mut d.clone(), plan), ["3323", "3.33", "2333"]);
        // a masked 2x2 box: only the two selected pixels
        let mask = [true, false, false, true];
        let area = Area { rect: Rect::new(0, 0, 2, 2), mask: Some(&mask) };
        let plan = replace_points(&d, 0, 0, area, b'1', b'3', true);
        assert_eq!(run(&mut d.clone(), plan), ["3121", "1.11", "2111"]);
        // a locked region keeps its pixels and counts them
        let mut lock = Region::new("lock", Rect::new(2, 0, 2, 1));
        lock.locked = true;
        d.regions.push(lock);
        let plan = replace_points(&d, 0, 0, all, b'1', b'3', true);
        assert_eq!(plan.protected, 1);
        assert_eq!(run(&mut d, plan), ["3321", "3.33", "2333"]);
        // the same color is no edit; transparent can be replaced too
        assert!(replace_points(&d, 0, 0, all, b'3', b'3', true).pixels.is_empty());
        assert_eq!(replace_points(&d, 0, 0, all, TRANSPARENT, b'5', true).pixels, [(1, 1, b'5')]);
    }

    #[test]
    fn patterns_parse_and_have_their_share() {
        for p in Pattern::MENU {
            assert_eq!(Pattern::parse(&p.id()), Ok(p));
        }
        assert!(Pattern::parse("bayer16").is_err() && Pattern::parse("stripes").is_err());
        let share =
            |p: Pattern| (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter(|&(x, y)| p.first(x, y)).count();
        assert_eq!(share(Pattern::Checker), 8);
        assert_eq!(share(Pattern::Quarter), 4);
        assert_eq!(share(Pattern::ThreeQuarters), 12);
        assert_eq!(share(Pattern::LinesH), 8);
        assert_eq!(share(Pattern::LinesD), 4);
        for n in 1..=15 {
            assert_eq!(share(Pattern::Bayer(n)), n as usize);
        }
        // 25 %: no two first-color pixels touch, not even diagonally
        let q = |x, y| Pattern::Quarter.first(x, y);
        for y in 0..8 {
            for x in 0..8 {
                if q(x, y) {
                    assert!(!q(x + 1, y) && !q(x, y + 1) && !q(x + 1, y + 1) && !q(x - 1, y + 1));
                }
            }
        }
    }

    #[test]
    fn dither_is_aligned_to_the_canvas_and_repeats_every_4_pixels() {
        for p in Pattern::MENU {
            for y in -8..8 {
                for x in -8..8 {
                    assert_eq!(p.first(x, y), p.first(x + 4, y), "{p:?}");
                    assert_eq!(p.first(x, y), p.first(x, y + 4), "{p:?}");
                }
            }
        }
        // two fills side by side give one continuous pattern
        let mut d = doc(&["......", "......"]);
        let left: Vec<(i32, i32)> = (0..2).flat_map(|y| (0..3).map(move |x| (x, y))).collect();
        let right: Vec<(i32, i32)> = (0..2).flat_map(|y| (3..6).map(move |x| (x, y))).collect();
        dither_points(&d, 0, 0, &left, Pattern::Checker, b'1', b'2').apply(&mut d, 0, 0);
        dither_points(&d, 0, 0, &right, Pattern::Checker, b'1', b'2').apply(&mut d, 0, 0);
        assert_eq!(d.rows(0, 0), ["121212", "212121"]);
        // the second color may be transparent; off-canvas points are dropped
        let plan = dither_points(&d, 0, 0, &[(0, 0), (1, 0), (9, 9)], Pattern::Checker, b'3', TRANSPARENT);
        assert_eq!(plan.pixels, [(0, 0, b'3'), (1, 0, TRANSPARENT)]);
    }

    #[test]
    fn outline_outside_4_and_8() {
        let d = doc(&[".....", ".....", "..1..", ".....", "....."]);
        let o = Outline { color: b'0', mode: OutlineMode::Outside, eight: false, only_transparent: false };
        let plan = outline_points(&d, 0, 0, Area::canvas(&d), o, true);
        assert_eq!(run(&mut d.clone(), plan), [".....", "..0..", ".010.", "..0..", "....."]);
        let plan = outline_points(&d, 0, 0, Area::canvas(&d), Outline { eight: true, ..o }, true);
        assert_eq!(run(&mut d.clone(), plan), [".....", ".000.", ".010.", ".000.", "....."]);
    }

    #[test]
    fn outline_inside_and_canvas_edge() {
        let d = doc(&["111.", "111.", "111.", "...."]);
        let o = Outline { color: b'0', mode: OutlineMode::Inside, eight: false, only_transparent: false };
        let plan = outline_points(&d, 0, 0, Area::canvas(&d), o, true);
        // the canvas edge counts as outside, so only the centre stays
        assert_eq!(run(&mut d.clone(), plan), ["000.", "010.", "000.", "...."]);
        let plan = outline_points(&d, 0, 0, Area::canvas(&d), Outline { only_transparent: true, ..o }, true);
        assert_eq!(plan.pixels.len(), 8, "only_transparent means nothing inside");
    }

    #[test]
    fn outline_of_a_selection_and_only_transparent() {
        // shape: the 1 in the selected left half; the 2 to its right is
        // not selected and gets the outline unless only_transparent
        let d = doc(&["....", ".12.", "...."]);
        let sel = Area::rect(Rect::new(0, 0, 2, 3));
        let o = Outline { color: b'0', mode: OutlineMode::Outside, eight: false, only_transparent: false };
        let plan = outline_points(&d, 0, 0, sel, o, true);
        assert_eq!(run(&mut d.clone(), plan), [".0..", "010.", ".0.."]);
        let plan = outline_points(&d, 0, 0, sel, Outline { only_transparent: true, ..o }, true);
        assert_eq!(run(&mut d.clone(), plan), [".0..", "012.", ".0.."]);
        // locked pixels stay and are counted
        let mut d = d;
        let mut lock = Region::new("lock", Rect::new(0, 1, 1, 1));
        lock.locked = true;
        d.regions.push(lock);
        let plan = outline_points(&d, 0, 0, sel, o, true);
        assert_eq!(plan.protected, 1);
        assert_eq!(run(&mut d, plan), [".0..", ".10.", ".0.."]);
    }
}
