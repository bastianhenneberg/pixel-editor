//! Mirroring and rotating: pixel blocks, cel areas (with a selection mask),
//! whole cels, region rects and a mirrored copy of a document.
//!
//! A block of w x h pixels is turned around its own centre. Rotating a
//! non-square block by 90° gives an h x w block on the same centre (rounded
//! to whole pixels); what lands outside the canvas is cut off and counted.

use crate::document::{Document, Rect, TRANSPARENT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Mirror left/right.
    FlipH,
    /// Mirror top/bottom.
    FlipV,
    /// Rotate 90° clockwise.
    Rot90,
    Rot180,
    /// Rotate 90° counter-clockwise.
    Rot270,
}

impl Op {
    pub const ALL: [Op; 5] = [Op::FlipH, Op::FlipV, Op::Rot90, Op::Rot270, Op::Rot180];

    pub fn id(self) -> &'static str {
        match self {
            Op::FlipH => "flip_h",
            Op::FlipV => "flip_v",
            Op::Rot90 => "rot90",
            Op::Rot180 => "rot180",
            Op::Rot270 => "rot270",
        }
    }

    pub fn parse(s: &str) -> Result<Op, String> {
        Op::ALL
            .into_iter()
            .find(|o| o.id() == s)
            .ok_or_else(|| format!("unknown op {s:?} (flip_h, flip_v, rot90 = clockwise, rot180, rot270)"))
    }

    /// German label for the GUI and history.
    pub fn label(self) -> &'static str {
        match self {
            Op::FlipH => "Horizontal spiegeln",
            Op::FlipV => "Vertikal spiegeln",
            Op::Rot90 => "90° rechts drehen",
            Op::Rot180 => "180° drehen",
            Op::Rot270 => "90° links drehen",
        }
    }

    /// Does the op swap width and height?
    pub fn swaps(self) -> bool {
        matches!(self, Op::Rot90 | Op::Rot270)
    }

    /// Size of a w x h block after the op.
    pub fn size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Where local pixel (u, v) of a w x h block ends up.
    pub fn map(self, u: i32, v: i32, w: i32, h: i32) -> (i32, i32) {
        match self {
            Op::FlipH => (w - 1 - u, v),
            Op::FlipV => (u, h - 1 - v),
            Op::Rot180 => (w - 1 - u, h - 1 - v),
            Op::Rot90 => (h - 1 - v, u),
            Op::Rot270 => (v, w - 1 - u),
        }
    }
}

/// A w x h block (row by row) after the op; its size is `op.size(w, h)`.
pub fn transform_block<T: Copy + Default>(src: &[T], w: u32, h: u32, op: Op) -> Vec<T> {
    let (nw, _) = op.size(w, h);
    let mut out = vec![T::default(); src.len()];
    for (i, v) in src.iter().enumerate() {
        let (x, y) = op.map((i as u32 % w) as i32, (i as u32 / w) as i32, w as i32, h as i32);
        out[(y as u32 * nw + x as u32) as usize] = *v;
    }
    out
}

/// The box a rect's content occupies after the op: same centre, rounded
/// so that a quarter turn right followed by one left gives the same box.
pub fn transformed_rect(r: Rect, op: Op) -> Rect {
    let d = r.w as i32 - r.h as i32;
    match op {
        Op::Rot90 => Rect::new(r.x + d.div_euclid(2), r.y + (-d).div_euclid(2), r.h, r.w),
        Op::Rot270 => Rect::new(r.x - (-d).div_euclid(2), r.y - d.div_euclid(2), r.h, r.w),
        _ => r,
    }
}

/// Where `inner` (a rect inside `outer`) lies after `outer` is transformed.
pub fn map_rect_in(inner: Rect, outer: Rect, op: Op) -> Rect {
    let (w, h) = (outer.w as i32, outer.h as i32);
    let a = op.map(inner.x - outer.x, inner.y - outer.y, w, h);
    let b = op.map(inner.x - outer.x + inner.w as i32 - 1, inner.y - outer.y + inner.h as i32 - 1, w, h);
    let dest = transformed_rect(outer, op);
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let (nw, nh) = op.size(inner.w, inner.h);
    Rect::new(dest.x + x0, dest.y + y0, nw, nh)
}

/// The part of `r` on a w x h canvas, if any.
pub fn clip_rect(r: Rect, w: u32, h: u32) -> Option<Rect> {
    let (x0, y0) = (r.x.max(0), r.y.max(0));
    let (x1, y1) = ((r.x + r.w as i32).min(w as i32), (r.y + r.h as i32).min(h as i32));
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// What a transform did besides moving pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Non-transparent pixels that ended up outside the canvas.
    pub clipped: usize,
    /// Pixel changes refused because they lie in a locked region.
    pub protected: usize,
}

impl Outcome {
    pub fn add(&mut self, o: Outcome) {
        self.clipped += o.clipped;
        self.protected += o.protected;
    }
}

/// The result of transforming an area: where the pixels went.
pub struct Moved {
    pub outcome: Outcome,
    /// The box the content occupies now (may leave the canvas).
    pub rect: Rect,
    /// Which pixels of `rect` belong to the content (None = all).
    pub mask: Option<Vec<bool>>,
}

/// Transform the pixels of cel (layer, frame) inside `area` (only those
/// with `mask` set, if given) around the area's centre. Vacated pixels
/// become transparent; selected transparent pixels are carried along too.
/// With `respect_locks`, pixels in locked regions keep their value.
/// Linked frames share the cel and see the change.
pub fn transform_area(
    doc: &mut Document,
    layer: usize,
    frame: usize,
    area: Rect,
    mask: Option<&[bool]>,
    op: Op,
    respect_locks: bool,
) -> Moved {
    let (w, h) = (area.w, area.h);
    let n = (w * h) as usize;
    let sel = |i: usize| mask.is_none_or(|m| m[i]);
    let at = |i: usize| (area.x + (i as u32 % w) as i32, area.y + (i as u32 / w) as i32);
    let src: Vec<u8> =
        (0..n).map(|i| if sel(i) { doc.get(layer, frame, at(i).0, at(i).1) } else { TRANSPARENT }).collect();
    let src_mask: Vec<bool> = (0..n).map(sel).collect();
    let dest = transformed_rect(area, op);
    let pixels = transform_block(&src, w, h, op);
    let dmask = transform_block(&src_mask, w, h, op);
    let mut out = Outcome::default();
    let locked = |d: &Document, x: i32, y: i32| respect_locks && d.locked_region_at(layer, frame, x, y).is_some();
    // the new content per canvas pixel, so vacated pixels are cleared once
    let mut new = std::collections::HashMap::new();
    for (i, k) in pixels.iter().enumerate() {
        if !dmask[i] {
            continue;
        }
        let (x, y) = (dest.x + (i as u32 % dest.w) as i32, dest.y + (i as u32 / dest.w) as i32);
        if doc.in_bounds(x, y) {
            new.insert((x, y), *k);
        } else if *k != TRANSPARENT {
            out.clipped += 1;
        }
    }
    let before = doc.clone();
    for i in (0..n).filter(|i| sel(*i)) {
        let (x, y) = at(i);
        if doc.in_bounds(x, y) && !new.contains_key(&(x, y)) {
            if locked(&before, x, y) {
                out.protected += (before.get(layer, frame, x, y) != TRANSPARENT) as usize;
            } else {
                doc.set(layer, frame, x, y, TRANSPARENT);
            }
        }
    }
    for (&(x, y), &k) in &new {
        if before.get(layer, frame, x, y) == k {
            continue;
        }
        if locked(&before, x, y) {
            out.protected += 1;
        } else {
            doc.set(layer, frame, x, y, k);
        }
    }
    let mask = (!dmask.iter().all(|b| *b)).then_some(dmask);
    Moved { outcome: out, rect: dest, mask }
}

/// Transform a whole cel around the canvas centre.
pub fn transform_cel(doc: &mut Document, layer: usize, frame: usize, op: Op, respect_locks: bool) -> Outcome {
    let canvas = Rect::new(0, 0, doc.width, doc.height);
    transform_area(doc, layer, frame, canvas, None, op, respect_locks).outcome
}

/// The distinct cels (layer, first frame of the link group) shown on
/// `frames` of `layers`: each must be transformed exactly once.
pub fn unique_cels(doc: &Document, layers: &[usize], frames: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &l in layers {
        for &f in frames {
            let src = doc.layers[l].link_source(f);
            if !out.contains(&(l, src)) {
                out.push((l, src));
            }
        }
    }
    out
}

/// Move region rects on `frames` along with a whole-canvas transform.
/// With `layer`, only regions restricted to that layer follow (the other
/// layers did not move). Locked regions keep their rect; their names are
/// returned. A rect that leaves the canvas is cut to it (or hidden).
pub fn transform_regions(
    doc: &mut Document,
    frames: &[usize],
    layer: Option<usize>,
    op: Op,
    respect_locks: bool,
) -> Vec<String> {
    let canvas = Rect::new(0, 0, doc.width, doc.height);
    let n = doc.frames.len();
    let layer_name = layer.map(|l| doc.layers[l].name.clone());
    let mut skipped = Vec::new();
    for r in &mut doc.regions {
        if layer_name.is_some() && r.layer != layer_name {
            continue;
        }
        if !frames.iter().any(|f| r.rect_at(*f).is_some()) {
            continue;
        }
        if respect_locks && r.locked {
            skipped.push(r.name.clone());
            continue;
        }
        let mut rects: Vec<Option<Rect>> = (0..n).map(|f| r.rect_at(f)).collect();
        for &f in frames {
            rects[f] = rects[f].and_then(|rc| clip_rect(map_rect_in(rc, canvas, op), canvas.w, canvas.h));
        }
        r.keys =
            rects.into_iter().enumerate().map(|(frame, rect)| crate::document::RegionKey { frame, rect }).collect();
        r.normalize();
    }
    skipped
}

/// A copy of `doc` with every frame transformed around the canvas centre:
/// tags, durations, linked cels, locks and regions are kept, region rects
/// and markers are transformed too. Layers in `keep` (e.g. a weapon that
/// stays on its side) are copied untransformed, and so are the regions and
/// markers restricted to them; regions on all layers (origin) follow the
/// transformed body. Returns the copy and the number of pixels cut off at
/// the canvas edge.
pub fn mirror_document(doc: &Document, op: Op, keep: &[usize]) -> (Document, usize) {
    let mut d = doc.clone();
    let layers: Vec<usize> = (0..d.layers.len()).filter(|l| !keep.contains(l)).collect();
    let frames: Vec<usize> = (0..d.frames.len()).collect();
    let mut clipped = 0;
    for (l, f) in unique_cels(&d, &layers, &frames) {
        clipped += transform_cel(&mut d, l, f, op, false).clipped;
    }
    transform_regions(&mut d, &frames, None, op, false);
    let kept: Vec<&str> = keep.iter().map(|l| doc.layers[*l].name.as_str()).collect();
    for (r, old) in d.regions.iter_mut().zip(&doc.regions) {
        if old.layer.as_deref().is_some_and(|n| kept.contains(&n)) {
            *r = old.clone();
        }
    }
    (d, clipped)
}

/// "walk_right" -> "walk_left" and back; otherwise `<name>_mirrored`.
pub fn mirrored_name(stem: &str) -> String {
    for (a, b) in [("right", "left"), ("left", "right"), ("Right", "Left"), ("Left", "Right")] {
        if let Some(i) = stem.rfind(a) {
            return format!("{}{b}{}", &stem[..i], &stem[i + a.len()..]);
        }
    }
    if let Some(base) = stem.strip_suffix("_side") {
        return format!("{base}_left");
    }
    format!("{stem}_mirrored")
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

    fn rows(d: &Document) -> Vec<String> {
        d.rows(0, 0)
    }

    #[test]
    fn blocks_flip_and_rotate() {
        // 3 x 2: 1 2 3 / 4 5 6
        let b = [1, 2, 3, 4, 5, 6];
        assert_eq!(transform_block(&b, 3, 2, Op::FlipH), [3, 2, 1, 6, 5, 4]);
        assert_eq!(transform_block(&b, 3, 2, Op::FlipV), [4, 5, 6, 1, 2, 3]);
        assert_eq!(transform_block(&b, 3, 2, Op::Rot180), [6, 5, 4, 3, 2, 1]);
        // clockwise: 2 x 3: 4 1 / 5 2 / 6 3
        assert_eq!(transform_block(&b, 3, 2, Op::Rot90), [4, 1, 5, 2, 6, 3]);
        assert_eq!(transform_block(&b, 3, 2, Op::Rot270), [3, 6, 2, 5, 1, 4]);
        let back = transform_block(&transform_block(&b, 3, 2, Op::Rot90), 2, 3, Op::Rot270);
        assert_eq!(back, b);
        for op in Op::ALL {
            assert_eq!(Op::parse(op.id()), Ok(op));
        }
        assert!(Op::parse("rotate").is_err());
    }

    #[test]
    fn rects_keep_their_centre_and_round_trip() {
        let r = Rect::new(2, 3, 5, 2);
        let a = transformed_rect(r, Op::Rot90);
        assert_eq!((a.w, a.h), (2, 5));
        assert_eq!(transformed_rect(a, Op::Rot270), r);
        assert_eq!(transformed_rect(Rect::new(0, 0, 4, 4), Op::Rot90), Rect::new(0, 0, 4, 4));
        // a 1-px marker in an 8-wide canvas mirrors to 7 - x
        let canvas = Rect::new(0, 0, 8, 6);
        assert_eq!(map_rect_in(Rect::new(1, 2, 1, 1), canvas, Op::FlipH), Rect::new(6, 2, 1, 1));
        assert_eq!(map_rect_in(Rect::new(1, 2, 3, 1), canvas, Op::FlipV), Rect::new(1, 3, 3, 1));
        assert_eq!(mirrored_name("walk_right"), "walk_left");
        assert_eq!(mirrored_name("walk_side"), "walk_left");
        assert_eq!(mirrored_name("idle"), "idle_mirrored");
    }

    #[test]
    fn cel_flip_and_rotation_cuts_at_the_canvas() {
        let mut d = doc(&["12..", "3...", "...."]);
        let o = transform_cel(&mut d, 0, 0, Op::FlipH, true);
        assert_eq!(rows(&d), ["..21", "...3", "...."]);
        assert_eq!(o, Outcome::default());
        // 4 x 3 canvas turned by 90°: a 3 x 4 box on the same centre, the
        // top row of it (y = -1) falls off the canvas
        let mut d = doc(&["1..2", "....", "3..4"]);
        let o = transform_cel(&mut d, 0, 0, Op::Rot90, true);
        assert_eq!(o.clipped, 2, "{:?}", rows(&d));
        assert_eq!(rows(&d), ["....", "....", "4.2."]);
    }

    #[test]
    fn masked_area_moves_only_selected_pixels_and_respects_locks() {
        let mut d = doc(&["12.", "34.", "..."]);
        // only 1 and 4 selected in the 2 x 2 box
        let m = [true, false, false, true];
        let r = transform_area(&mut d, 0, 0, Rect::new(0, 0, 2, 2), Some(&m), Op::FlipH, true);
        // 1 and 4 swap sides and cover the unselected 2 and 3
        assert_eq!(rows(&d), [".1.", "4..", "..."]);
        assert_eq!(r.mask, Some(vec![false, true, true, false]));
        // a locked region keeps its pixels
        let mut d = doc(&["12", ".."]);
        let mut lock = Region::new("lock", Rect::new(1, 0, 1, 1));
        lock.locked = true;
        d.regions.push(lock);
        let o = transform_cel(&mut d, 0, 0, Op::FlipH, true).protected;
        assert_eq!(rows(&d), ["22", ".."]);
        assert_eq!(o, 1);
    }

    #[test]
    fn mirror_document_keeps_structure_and_mirrors_regions() {
        let mut d = doc(&["1...", "2..."]);
        d.add_frame(1, Some(0), true).unwrap();
        d.add_frame(2, None, false).unwrap();
        d.set(0, 2, 1, 1, b'3');
        d.frames[1].duration_ms = 250;
        d.set_tag(crate::Tag::new("walk", 0, 2)).unwrap();
        let mut origin = Region::marker("origin", 1, 1);
        origin.locked = true;
        d.regions.push(origin);
        let mut hand = Region::new("hand", Rect::new(0, 0, 2, 1));
        hand.set_rect_for(2, 2, Some(Rect::new(1, 1, 1, 1)), 3);
        d.regions.push(hand);
        let (m, clipped) = mirror_document(&d, Op::FlipH, &[]);
        assert_eq!(clipped, 0);
        assert_eq!(m.rows(0, 0), ["...1", "...2"]);
        assert!(m.layers[0].is_linked(1), "links stay");
        assert_eq!(m.rows(0, 1), m.rows(0, 0));
        assert_eq!(m.rows(0, 2), ["....", "..3."]);
        assert_eq!((m.frames[1].duration_ms, &m.tags, m.frames.len()), (250, &d.tags, 3));
        assert_eq!(m.regions[0].point_at(0), Some((2, 1)));
        assert!(m.regions[0].locked);
        assert_eq!(m.regions[1].rect_at(0), Some(Rect::new(2, 0, 2, 1)));
        assert_eq!(m.regions[1].rect_at(2), Some(Rect::new(2, 1, 1, 1)));
        // mirroring twice gives the original back
        assert_eq!(mirror_document(&m, Op::FlipH, &[]).0.layers, d.layers);
        m.validate().unwrap();
    }

    #[test]
    fn mirror_document_keeps_layers_on_their_side() {
        let mut d = doc(&["1...", "2..."]);
        d.add_layer("sword", 1);
        d.set_rows(1, 0, 0, 0, &["..3.".to_string()]).unwrap();
        let mut grip = Region::marker("grip", 2, 0);
        grip.layer = Some("sword".into());
        d.regions.push(grip);
        d.regions.push(Region::marker("origin", 0, 1));
        let (m, _) = mirror_document(&d, Op::FlipH, &[1]);
        assert_eq!(m.rows(0, 0), ["...1", "...2"], "body mirrored");
        assert_eq!(m.rows(1, 0), d.rows(1, 0), "sword stays");
        assert_eq!(m.regions[0].point_at(0), Some((2, 0)), "the sword's marker stays");
        assert_eq!(m.regions[1].point_at(0), Some((3, 1)), "origin follows the body");
        m.validate().unwrap();
    }
}
