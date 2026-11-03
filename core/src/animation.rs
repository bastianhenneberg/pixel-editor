//! Animation review for Claude, who only sees still images: a tag in its
//! real play order as a filmstrip (plus the loop transition), an onion-skin
//! row, a difference row, an optional tracked area with its centre path,
//! traced points with their dotted paths (see `trace`), and a numeric
//! analysis with warnings (ground line, jumps, stray pixels, duplicate
//! frames, paths that leave their arc).

use crate::document::{Document, Rect, TRANSPARENT};
use crate::draw;
use crate::font::{draw_text, fit_text, text_height, text_width};
use crate::render::{composite, Image, CHECKER};

/// An area given by the caller: a named region (its rect follows the
/// region's keys per frame) or a fixed rectangle.
#[derive(Clone, Debug, PartialEq)]
pub enum Area {
    Region(String),
    Rect(Rect),
}

impl Area {
    /// "name" or "x,y,w,h".
    pub fn parse(s: &str) -> Result<Area, String> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let n: Result<Vec<i32>, _> = parts.iter().map(|p| p.parse::<i32>()).collect();
            if let Ok(n) = n {
                if n[2] < 1 || n[3] < 1 {
                    return Err(format!("rect {s:?}: w and h must be >= 1"));
                }
                return Ok(Area::Rect(Rect::new(n[0], n[1], n[2] as u32, n[3] as u32)));
            }
        }
        Ok(Area::Region(s.to_string()))
    }

    /// Is this a marker region (a point)?
    pub fn is_marker(&self, doc: &Document) -> bool {
        match self {
            Area::Region(n) => doc.regions.iter().any(|r| &r.name == n && r.is_marker()),
            Area::Rect(_) => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Area::Region(n) => n.clone(),
            Area::Rect(r) => r.describe(),
        }
    }

    /// The area's rect on each frame of `seq` (None where a region is absent).
    pub fn rects(&self, doc: &Document, seq: &[usize]) -> Result<Vec<Option<Rect>>, String> {
        match self {
            Area::Rect(r) => Ok(vec![Some(*r); seq.len()]),
            Area::Region(name) => {
                let r = &doc.regions[doc.find_region(name)?];
                Ok(seq.iter().map(|f| r.rect_at(*f)).collect())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnimOptions {
    /// Screen pixels per art pixel; None = automatic (cells ~200 px).
    pub zoom: Option<u32>,
    /// Show only this area, enlarged.
    pub area: Option<Area>,
    /// Mark the opaque pixels inside this area per step and plot their centre.
    pub track: Option<Area>,
    /// Silhouette edge moves larger than this (px) are reported as jumps.
    pub jump_threshold: u32,
    /// Points to follow from step to step (hands, feet, head).
    pub trace: Vec<crate::trace::Trace>,
    /// Traced points moving more than this (px) in one step are reported.
    pub trace_threshold: u32,
}

impl Default for AnimOptions {
    fn default() -> Self {
        AnimOptions { zoom: None, area: None, track: None, jump_threshold: 2, trace: Vec::new(), trace_threshold: 3 }
    }
}

/// Bounding box of opaque pixels as inclusive edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Edges {
    fn of(img: &Image, within: Option<Rect>) -> Option<Edges> {
        let r = within.unwrap_or(Rect::new(0, 0, img.width, img.height));
        let mut e: Option<Edges> = None;
        for y in r.y.max(0)..(r.y + r.h as i32).min(img.height as i32) {
            for x in r.x.max(0)..(r.x + r.w as i32).min(img.width as i32) {
                if img.get(x as u32, y as u32)[3] > 0 {
                    e = Some(match e {
                        None => Edges { left: x, top: y, right: x, bottom: y },
                        Some(e) => Edges {
                            left: e.left.min(x),
                            top: e.top.min(y),
                            right: e.right.max(x),
                            bottom: e.bottom.max(y),
                        },
                    });
                }
            }
        }
        e
    }

    pub fn center(&self) -> (f32, f32) {
        ((self.left + self.right) as f32 / 2.0, (self.top + self.bottom) as f32 / 2.0)
    }

    pub fn describe(&self) -> String {
        format!("x {}..{} y {}..{}", self.left, self.right, self.top, self.bottom)
    }
}

#[derive(Clone, Debug)]
pub struct Step {
    pub frame: usize,
    pub ms: u32,
    /// Pixels that differ from the previous step (None for the first step
    /// of a tag that does not loop).
    pub changed: Option<usize>,
    /// If the whole sprite moved: the offset vs the previous step that
    /// explains most pixels, and the changes left after aligning by it.
    pub aligned: Option<((i32, i32), usize)>,
    pub silhouette: Option<Edges>,
    /// Pixels without a 4-neighbour of the same key (composite keys).
    pub orphans: Vec<(i32, i32)>,
    /// Pixels without any opaque 4-neighbour.
    pub isolated: Vec<(i32, i32)>,
    pub track: Option<Edges>,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub tag: String,
    pub direction: String,
    pub looping: bool,
    pub steps: Vec<Step>,
    /// Changed pixels from the last step back to the first (loops only).
    pub loop_changed: Option<usize>,
    pub total_ms: u32,
    pub warnings: Vec<String>,
    pub track_label: Option<String>,
    pub traces: Vec<crate::trace::TracePath>,
}

fn changed(a: &Image, b: &Image) -> usize {
    a.pixels.as_chunks::<4>().0.iter().zip(b.pixels.as_chunks::<4>().0).filter(|(p, q)| p != q).count()
}

/// Pixel of `img` at (x, y), transparent outside.
fn at(img: &Image, x: i32, y: i32) -> [u8; 4] {
    if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
        [0, 0, 0, 0]
    } else {
        img.get(x as u32, y as u32)
    }
}

/// Changes between `prev` moved by (dx, dy) and `cur`.
fn changed_shifted(prev: &Image, cur: &Image, (dx, dy): (i32, i32)) -> usize {
    let mut n = 0;
    for y in 0..cur.height as i32 {
        for x in 0..cur.width as i32 {
            if at(prev, x - dx, y - dy) != at(cur, x, y) {
                n += 1;
            }
        }
    }
    n
}

/// Largest offset searched when aligning steps.
const SHIFT: i32 = 5;

/// The offset (within ±SHIFT px) under which most pixels match the previous
/// step (e.g. a body bob), if it is not (0, 0) and explains at least 20%
/// more pixels than no offset.
fn best_shift(prev: &Image, cur: &Image, raw: usize) -> Option<((i32, i32), usize)> {
    let mut best = ((0, 0), raw);
    for dy in -SHIFT..=SHIFT {
        for dx in -SHIFT..=SHIFT {
            if (dx, dy) != (0, 0) {
                let n = changed_shifted(prev, cur, (dx, dy));
                if n < best.1 {
                    best = ((dx, dy), n);
                }
            }
        }
    }
    (best.0 != (0, 0) && best.1 * 10 < raw * 8).then_some(best)
}

/// Topmost visible key per pixel.
fn key_map(doc: &Document, frame: usize) -> Vec<u8> {
    let (w, h) = (doc.width as i32, doc.height as i32);
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| doc.top_key(frame, x, y)).collect()
}

pub type Points = Vec<(i32, i32)>;

/// (orphans, isolated) pixels of a frame's composite keys.
fn stray_pixels(doc: &Document, frame: usize) -> (Points, Points) {
    stray_in(&key_map(doc, frame), doc.width as i32, doc.height as i32)
}

/// (orphans, isolated) pixels of a `w` x `h` key grid: orphans have no
/// 4-neighbour of the same key, isolated ones no opaque 4-neighbour.
pub fn stray_in(keys: &[u8], w: i32, h: i32) -> (Points, Points) {
    let at = |x: i32, y: i32| if x < 0 || y < 0 || x >= w || y >= h { TRANSPARENT } else { keys[(y * w + x) as usize] };
    let (mut orphans, mut isolated) = (Vec::new(), Vec::new());
    for y in 0..h {
        for x in 0..w {
            let k = at(x, y);
            if k == TRANSPARENT {
                continue;
            }
            let n = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)];
            if !n.contains(&k) {
                orphans.push((x, y));
            }
            if n.iter().all(|k| *k == TRANSPARENT) {
                isolated.push((x, y));
            }
        }
    }
    (orphans, isolated)
}

fn signed(v: i32) -> String {
    if v > 0 {
        format!("+{v}")
    } else {
        v.to_string()
    }
}

pub fn coords(list: &[(i32, i32)], max: usize) -> String {
    let mut s: Vec<String> = list.iter().take(max).map(|(x, y)| format!("({x},{y})")).collect();
    if list.len() > max {
        s.push(format!("… {} more", list.len() - max));
    }
    s.join(" ")
}

/// Analyse a tag's play order.
pub fn analyze(doc: &Document, tag: usize, opts: &AnimOptions) -> Result<Analysis, String> {
    let t = doc.tags.get(tag).ok_or("no such tag")?;
    let seq = t.sequence();
    let looping = t.looping && seq.len() > 1;
    let images: Vec<Image> = seq.iter().map(|f| composite(doc, *f)).collect();
    let track_rects = match &opts.track {
        Some(a) => Some(a.rects(doc, &seq)?),
        None => None,
    };
    let mut steps = Vec::new();
    for (i, &f) in seq.iter().enumerate() {
        let prev = if i > 0 {
            Some(i - 1)
        } else if looping {
            Some(seq.len() - 1)
        } else {
            None
        };
        let (orphans, isolated) = stray_pixels(doc, f);
        let raw = prev.map(|p| changed(&images[p], &images[i]));
        steps.push(Step {
            frame: f,
            ms: doc.frames[f].duration_ms,
            changed: raw,
            aligned: prev.zip(raw).and_then(|(p, r)| best_shift(&images[p], &images[i], r)),
            silhouette: Edges::of(&images[i], None),
            orphans,
            isolated,
            track: match (&opts.track, track_rects.as_ref()) {
                // a marker is an anchor: its point per step, not the pixels under it
                (Some(t), Some(r)) if t.is_marker(doc) => {
                    r[i].map(|rc| Edges { left: rc.x, top: rc.y, right: rc.x, bottom: rc.y })
                }
                (_, Some(r)) => r[i].and_then(|rc| Edges::of(&images[i], Some(rc))),
                _ => None,
            },
        });
    }
    let loop_changed = looping.then(|| changed(&images[seq.len() - 1], &images[0]));
    let total_ms = steps.iter().map(|s| s.ms).sum();

    // ---- warnings
    let mut warnings = Vec::new();
    let label = |i: usize| format!("step {i} ({})", doc.frames[seq[i]].name);
    let grounds: Vec<Option<i32>> = steps.iter().map(|s| s.silhouette.map(|e| e.bottom)).collect();
    let distinct: Vec<i32> = {
        let mut g: Vec<i32> = grounds.iter().flatten().copied().collect();
        g.sort();
        g.dedup();
        g
    };
    if distinct.len() > 1 {
        let list: Vec<String> = grounds
            .iter()
            .enumerate()
            .map(|(i, g)| format!("{i}:{}", g.map_or("-".into(), |g| g.to_string())))
            .collect();
        warnings.push(format!(
            "ground line (lowest opaque row) changes between steps: {} (if the feet should stay on the ground, they leave it; if a weapon or effect reaches below the feet, check the feet with track instead)",
            list.join(" ")
        ));
    }
    let n = steps.len();
    for i in 0..n {
        let prev = if i > 0 {
            Some(i - 1)
        } else if looping {
            Some(n - 1)
        } else {
            None
        };
        let Some(p) = prev else { continue };
        let (a, b) = (steps[p].silhouette, steps[i].silhouette);
        if let (Some(a), Some(b)) = (a, b) {
            let d = [
                ("top", b.top - a.top),
                ("bottom", b.bottom - a.bottom),
                ("left", b.left - a.left),
                ("right", b.right - a.right),
            ];
            let jumps: Vec<String> = d
                .iter()
                .filter(|(_, v)| v.unsigned_abs() > opts.jump_threshold)
                .map(|(k, v)| format!("{k} {}", signed(*v)))
                .collect();
            if !jumps.is_empty() {
                let what = if i == 0 { format!("loop {} -> {}", label(p), label(i)) } else { label(i) };
                warnings.push(format!(
                    "{what}: silhouette jumps {} px (threshold {})",
                    jumps.join(", "),
                    opts.jump_threshold
                ));
            }
        }
        if steps[i].changed == Some(0) {
            let what = if i == 0 { "loop: first step".to_string() } else { label(i) };
            warnings.push(format!("{what} is identical to the step before it (wasted frame or missing change)"));
        }
    }
    for (i, s) in steps.iter().enumerate() {
        if s.silhouette.is_none() {
            warnings.push(format!("{}: frame is empty", label(i)));
        }
        if !s.isolated.is_empty() {
            warnings.push(format!(
                "{}: {} isolated pixel(s) with no opaque neighbour (stray?): {}",
                label(i),
                s.isolated.len(),
                coords(&s.isolated, 8)
            ));
        }
    }
    // orphans that are new in a step are suspicious; ones that persist are
    // probably deliberate highlights
    for i in 0..n {
        let prev = if i > 0 {
            Some(i - 1)
        } else if looping {
            Some(n - 1)
        } else {
            None
        };
        // compare at the position the previous step moved to (whole-sprite bob)
        let (sx, sy) = steps[i].aligned.map_or((0, 0), |a| a.0);
        let new: Vec<(i32, i32)> = match prev {
            Some(p) => steps[i]
                .orphans
                .iter()
                .filter(|(x, y)| !steps[p].orphans.contains(&(x - sx, y - sy)))
                .copied()
                .collect(),
            None => Vec::new(),
        };
        if !new.is_empty() {
            warnings.push(format!(
                "{}: {} new orphan pixel(s) (no 4-neighbour of the same key, not present before): {}",
                label(i),
                new.len(),
                coords(&new, 8)
            ));
        }
    }
    let traces = opts
        .trace
        .iter()
        .map(|tr| crate::trace::trace(doc, &seq, looping, tr, opts.trace_threshold))
        .collect::<Result<Vec<_>, _>>()?;
    warnings.extend(traces.iter().flat_map(|p| p.warnings.iter().cloned()));
    Ok(Analysis {
        traces,
        tag: t.name.clone(),
        direction: t.direction.id().to_string(),
        looping,
        steps,
        loop_changed,
        total_ms,
        warnings,
        track_label: opts.track.as_ref().map(Area::label),
    })
}

/// Text summary for the reply.
pub fn summary(doc: &Document, a: &Analysis) -> String {
    let mut s = format!(
        "{}: {} steps, {} ms total, {}, {}\n",
        a.tag,
        a.steps.len(),
        a.total_ms,
        a.direction,
        if a.looping { "loops" } else { "plays once" }
    );
    for (i, st) in a.steps.iter().enumerate() {
        let name = &doc.frames[st.frame].name;
        let ch = match st.changed {
            Some(c) if i == 0 => format!("Δ {c} px vs last step (loop)"),
            Some(c) => format!("Δ {c} px"),
            None => "start".into(),
        };
        let ch = match st.aligned {
            Some(((dx, dy), n)) => {
                format!(
                    "{ch} (most pixels match the previous step shifted by ({}, {}); {n} px change after aligning)",
                    signed(dx),
                    signed(dy)
                )
            }
            None => ch,
        };
        let prev = if i > 0 {
            Some(i - 1)
        } else if a.looping {
            Some(a.steps.len() - 1)
        } else {
            None
        };
        let shift = match (prev.and_then(|p| a.steps[p].silhouette), st.silhouette) {
            (Some(p), Some(c)) => format!(
                ", shift top {} bottom {} left {} right {}",
                signed(c.top - p.top),
                signed(c.bottom - p.bottom),
                signed(c.left - p.left),
                signed(c.right - p.right)
            ),
            _ => String::new(),
        };
        let sil = st.silhouette.map_or("empty".to_string(), |e| e.describe());
        s += &format!("step {i} {name} {}ms: {ch}{shift}; silhouette {sil}\n", st.ms);
    }
    if let Some(c) = a.loop_changed {
        s += &format!("loop {} -> 0: Δ {c} px\n", a.steps.len() - 1);
    }
    if let Some(l) = &a.track_label {
        let path: Vec<String> = a
            .steps
            .iter()
            .enumerate()
            .map(|(i, st)| match st.track {
                Some(e) => {
                    let (cx, cy) = e.center();
                    format!("{i}:({cx},{cy}) bottom {}", e.bottom)
                }
                None => format!("{i}:-"),
            })
            .collect();
        s += &format!("track {l}: centre per step {}\n", path.join(" | "));
    }
    for p in &a.traces {
        s += &format!("{}\n", crate::trace::describe(p));
    }
    if a.warnings.is_empty() {
        s += "warnings: none\n";
    } else {
        s += "warnings:\n";
        for w in &a.warnings {
            s += &format!("- {w}\n");
        }
    }
    s
}

// ---- rendering ------------------------------------------------------------

const BG: [u8; 4] = [30, 30, 36, 255];
const ONION_BG: [u8; 4] = [44, 44, 52, 255];
const TEXT: [u8; 4] = [230, 230, 240, 255];
const DIM: [u8; 4] = [140, 140, 160, 255];
const DIFF: [u8; 4] = [255, 40, 220, 255];
const GROUND: [u8; 4] = [255, 70, 70, 255];
const TRACK: [u8; 4] = [60, 230, 255, 255];
const PATH: [u8; 4] = [255, 230, 60, 255];
const TS: u32 = 2;

/// "moved 0,-1 chg 57" when the step is a whole-sprite move.
fn aligned_note(st: &Step) -> String {
    match st.aligned {
        Some(((dx, dy), n)) => format!("moved {},{} chg {n}", signed(dx), signed(dy)),
        None => String::new(),
    }
}

fn mix(c: [u8; 4], t: [u8; 3], amount: f32) -> [u8; 4] {
    let m = |a: u8, b: u8| (a as f32 * (1.0 - amount) + b as f32 * amount).round() as u8;
    [m(c[0], t[0]), m(c[1], t[1]), m(c[2], t[2]), c[3]]
}

fn line(img: &mut Image, a: (i32, i32), b: (i32, i32), c: [u8; 4]) {
    for (x, y) in draw::line(a.0, a.1, b.0, b.1) {
        if x >= 0 && y >= 0 {
            img.put(x as u32, y as u32, c);
        }
    }
}

fn outline(img: &mut Image, x0: i32, y0: i32, x1: i32, y1: i32, c: [u8; 4]) {
    line(img, (x0, y0), (x1, y0), c);
    line(img, (x0, y1), (x1, y1), c);
    line(img, (x0, y0), (x0, y1), c);
    line(img, (x1, y0), (x1, y1), c);
}

/// Render the review strip. Layout: bands of up to N cells; each band has a
/// header (step, frame name, ms, Δ) and three rows: frames (with ground
/// reference line, track box and centre path), onion skin, difference.
pub fn render(doc: &Document, tag: usize, a: &Analysis, opts: &AnimOptions) -> Result<Image, String> {
    let seq: Vec<usize> = a.steps.iter().map(|s| s.frame).collect();
    // crop: the area (union over the steps) or the whole canvas
    let crop = match &opts.area {
        None => Rect::new(0, 0, doc.width, doc.height),
        Some(area) => {
            let mut rects: Vec<Rect> = area.rects(doc, &seq)?.into_iter().flatten().collect();
            if area.is_marker(doc) {
                // a point: show its surroundings
                rects = rects.into_iter().map(|r| Rect::new(r.x - 12, r.y - 12, 25, 25)).collect();
            }
            let r =
                rects.iter().skip(1).fold(*rects.first().ok_or("area does not exist on any step")?, |u, r| u.union(r));
            let x0 = r.x.max(0);
            let y0 = r.y.max(0);
            let x1 = (r.x + r.w as i32).min(doc.width as i32);
            let y1 = (r.y + r.h as i32).min(doc.height as i32);
            if x1 <= x0 || y1 <= y0 {
                return Err("area lies outside the canvas".into());
            }
            Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
        }
    };
    let zoom = opts.zoom.unwrap_or((200 / crop.w.max(crop.h)).clamp(1, 16)).clamp(1, 32);
    let (cw, ch) = (crop.w * zoom, crop.h * zoom);
    let pad = 10;
    let label_w = text_width("frames", TS) + 14;
    let head_h = 3 * (text_height(TS) + 3) + 4;
    // cells: every step, plus the first step again after a loop
    let mut cells: Vec<(usize, bool)> = (0..a.steps.len()).map(|i| (i, false)).collect();
    if a.looping {
        cells.push((0, true));
    }
    let per_band = ((1900 - label_w) / (cw + pad)).clamp(1, cells.len() as u32) as usize;
    let bands = cells.len().div_ceil(per_band);
    let band_h = head_h + 3 * (ch + pad) + pad;
    let title_h = text_height(TS) + 12;
    let width = label_w + per_band as u32 * (cw + pad) + pad;
    let height = title_h + bands as u32 * band_h;
    let mut out = Image::filled(width, height, BG);
    let title = format!(
        "{} | {} steps | {} ms | {} | {}{}",
        a.tag,
        a.steps.len(),
        a.total_ms,
        a.direction,
        if a.looping { "loop" } else { "once" },
        opts.area.as_ref().map(|r| format!(" | area {}", r.label())).unwrap_or_default()
    );
    draw_text(&mut out, pad as i32, 6, &fit_text(&title, TS, width - 2 * pad), TS, TEXT);

    let images: Vec<Image> = seq.iter().map(|f| composite(doc, *f)).collect();
    let track_rects = match &opts.track {
        Some(t) => Some(t.rects(doc, &seq)?),
        None => None,
    };
    let ground_ref = a.steps.first().and_then(|s| s.silhouette).map(|e| e.bottom);
    let n = a.steps.len();
    let _ = tag;

    for band in 0..bands {
        let by = title_h + band as u32 * band_h;
        let rows_y = [by + head_h, by + head_h + ch + pad, by + head_h + 2 * (ch + pad)];
        for (r, name) in ["frames", "onion", "diff"].iter().enumerate() {
            draw_text(&mut out, pad as i32, (rows_y[r] + ch / 2) as i32 - 5, name, TS, DIM);
        }
        let mut path: Vec<(i32, i32)> = Vec::new();
        for (col, &(i, is_loop)) in cells.iter().skip(band * per_band).take(per_band).enumerate() {
            let x0 = label_w + col as u32 * (cw + pad);
            let st = &a.steps[i];
            let fname = &doc.frames[st.frame].name;
            let (l1, l2) = if is_loop {
                (format!("loop -> 0 {fname}"), format!("chg {}", a.loop_changed.unwrap_or(0)))
            } else {
                let d = match st.changed {
                    Some(c) => format!(" chg {c}"),
                    None => String::new(),
                };
                (format!("{i} {fname}"), format!("{}ms{d}", st.ms))
            };
            let l3 = aligned_note(st);
            let col_text = if is_loop { DIFF } else { TEXT };
            draw_text(&mut out, x0 as i32, by as i32, &fit_text(&l1, TS, cw), TS, col_text);
            draw_text(&mut out, x0 as i32, (by + text_height(TS) + 3) as i32, &fit_text(&l2, TS, cw), TS, DIM);
            draw_text(&mut out, x0 as i32, (by + 2 * (text_height(TS) + 3)) as i32, &fit_text(&l3, TS, cw), TS, PATH);

            let img = &images[i];
            let prev = if is_loop {
                Some(n - 1)
            } else if i > 0 {
                Some(i - 1)
            } else if a.looping {
                Some(n - 1)
            } else {
                None
            };
            let px = |x: i32, y: i32| img.get(x as u32, y as u32);
            // row 1: frame on checkerboard
            let fy = rows_y[0];
            let cell = (zoom / 2).max(2);
            for y in 0..ch {
                for x in 0..cw {
                    out.put(x0 + x, fy + y, CHECKER[((x / cell + y / cell) % 2) as usize]);
                }
            }
            for y in 0..crop.h {
                for x in 0..crop.w {
                    let c = px(crop.x + x as i32, crop.y + y as i32);
                    if c[3] > 0 {
                        for dy in 0..zoom {
                            for dx in 0..zoom {
                                out.blend(x0 + x * zoom + dx, fy + y * zoom + dy, c);
                            }
                        }
                    }
                }
            }
            if zoom >= 6 {
                for gx in (8 - crop.x.rem_euclid(8)) as u32..crop.w {
                    if (crop.x + gx as i32) % 8 == 0 {
                        for y in 0..ch {
                            out.blend(x0 + gx * zoom, fy + y, [255, 255, 255, 50]);
                        }
                    }
                }
            }
            // ground reference: the first step's lowest opaque row
            if let Some(g) = ground_ref {
                let gy = g + 1 - crop.y;
                if gy > 0 && gy <= crop.h as i32 {
                    let yy = (fy + gy as u32 * zoom).min(fy + ch - 1);
                    for x in 0..cw {
                        if (x / 3) % 2 == 0 {
                            out.put(x0 + x, yy, GROUND);
                        }
                    }
                }
            }
            // track box and centre
            if let (Some(tr), Some(e)) = (&track_rects, st.track) {
                let _ = tr;
                let sx = |x: i32| x0 as i32 + (x - crop.x) * zoom as i32;
                let sy = |y: i32| fy as i32 + (y - crop.y) * zoom as i32;
                let anchor = opts.track.as_ref().is_some_and(|t| t.is_marker(doc));
                if anchor {
                    let clip = (x0 as i32, fy as i32, (x0 + cw) as i32, (fy + ch) as i32);
                    crate::render::crosshair(&mut out, sx(e.left), sy(e.top), zoom, TRACK, clip);
                } else {
                    outline(&mut out, sx(e.left), sy(e.top), sx(e.right + 1) - 1, sy(e.bottom + 1) - 1, TRACK);
                }
                let (cx, cy) = e.center();
                let c = (
                    x0 as i32 + ((cx + 0.5 - crop.x as f32) * zoom as f32) as i32,
                    fy as i32 + ((cy + 0.5 - crop.y as f32) * zoom as f32) as i32,
                );
                for d in -3..=3 {
                    out.put((c.0 + d) as u32, c.1 as u32, PATH);
                    out.put(c.0 as u32, (c.1 + d) as u32, PATH);
                }
                path.push(c);
            }
            if !a.traces.is_empty() {
                let screen = |x: i32, y: i32| {
                    let z = zoom as i32;
                    (x0 as i32 + (x - crop.x) * z + z / 2, fy as i32 + (y - crop.y) * z + z / 2)
                };
                let clip = (x0 as i32, fy as i32, (x0 + cw) as i32, (fy + ch) as i32);
                crate::trace::draw_paths(&mut out, &a.traces, i, zoom, screen, clip);
            }

            // row 2: onion skin: previous step cool at ~35%, current warm
            let oy = rows_y[1];
            out.fill_rect(x0, oy, cw, ch, ONION_BG);
            let put_zoomed = |out: &mut Image, x: u32, y: u32, c: [u8; 4], top: u32| {
                for dy in 0..zoom {
                    for dx in 0..zoom {
                        out.blend(x0 + x * zoom + dx, top + y * zoom + dy, c);
                    }
                }
            };
            if let Some(p) = prev {
                for y in 0..crop.h {
                    for x in 0..crop.w {
                        let c = images[p].get((crop.x + x as i32) as u32, (crop.y + y as i32) as u32);
                        if c[3] > 0 {
                            let mut t = mix(c, [70, 150, 255], 0.65);
                            t[3] = (c[3] as f32 * 0.35) as u8;
                            put_zoomed(&mut out, x, y, t, oy);
                        }
                    }
                }
            }
            for y in 0..crop.h {
                for x in 0..crop.w {
                    let c = px(crop.x + x as i32, crop.y + y as i32);
                    if c[3] > 0 {
                        let mut t = mix(c, [255, 150, 60], 0.45);
                        t[3] = (c[3] as f32 * 0.8) as u8;
                        put_zoomed(&mut out, x, y, t, oy);
                    }
                }
            }

            // row 3: difference to the previous step over a dimmed frame;
            // if the whole sprite moved, against the moved previous step
            let shift = st.aligned.map_or((0, 0), |a| a.0);
            let dy0 = rows_y[2];
            out.fill_rect(x0, dy0, cw, ch, ONION_BG);
            for y in 0..crop.h {
                for x in 0..crop.w {
                    let (sx, sy) = (crop.x + x as i32, crop.y + y as i32);
                    let c = px(sx, sy);
                    let differs = prev.is_some_and(|p| at(&images[p], sx - shift.0, sy - shift.1) != c);
                    let col = if differs {
                        DIFF
                    } else if c[3] > 0 {
                        let m = mix(c, [44, 44, 52], 0.7);
                        [m[0], m[1], m[2], 255]
                    } else {
                        continue;
                    };
                    put_zoomed(&mut out, x, y, col, dy0);
                }
            }
        }
        // centre path across the cells of this band
        for w in path.windows(2) {
            line(&mut out, w[0], w[1], PATH);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Direction, Region, Tag};

    /// 8x8, a 2x2 block that walks right, with a stray pixel in step 2.
    fn walker(direction: Direction, looping: bool) -> Document {
        let mut d = Document::new(8, 8, Document::builtin_palette("pico8").unwrap()).unwrap();
        for i in 0..3 {
            if i > 0 {
                d.add_frame(i, None, false).unwrap();
            }
            d.set_rows(0, i, i as i32, 5, &["88".into(), "88".into()]).unwrap();
        }
        d.set(0, 2, 7, 0, b'c'); // stray pixel
        let mut t = Tag::new("walk", 0, 2);
        t.direction = direction;
        t.looping = looping;
        d.set_tag(t).unwrap();
        d
    }

    #[test]
    fn analysis_in_play_order_with_warnings() {
        let d = walker(Direction::PingPong, true);
        let a = analyze(&d, 0, &AnimOptions::default()).unwrap();
        let frames: Vec<usize> = a.steps.iter().map(|s| s.frame).collect();
        assert_eq!(frames, vec![0, 1, 2, 1], "pingpong expanded");
        assert_eq!(a.total_ms, 400);
        assert_eq!(a.steps[1].changed, Some(4), "block moved 1 px: 2 pixels left, 2 entered");
        assert_eq!(a.steps[0].changed, a.loop_changed, "first step compares with the last (loop)");
        assert_eq!(a.steps[2].silhouette, Some(Edges { left: 2, top: 0, right: 7, bottom: 6 }));
        let w = a.warnings.join("\n");
        assert!(w.contains("isolated pixel") && w.contains("(7,0)"), "{w}");
        assert!(w.contains("silhouette jumps") && w.contains("top -5"), "{w}");
        assert!(!w.contains("ground line"), "feet stay on row 6: {w}");
        let s = summary(&d, &a);
        assert!(s.contains("walk: 4 steps, 400 ms total, pingpong, loops"), "{s}");
        assert!(
            s.contains("step 1 f1 100ms: Δ 4 px (most pixels match the previous step shifted by (+1, 0); 0 px change after aligning), shift top 0 bottom 0 left +1 right +1"),
            "{s}"
        );
    }

    #[test]
    fn ground_line_identical_frames_and_track() {
        let mut d = walker(Direction::Forward, false);
        d.set(0, 2, 7, 0, TRANSPARENT);
        // step 1 lifts the block one row: ground changes
        d.set_rows(0, 1, 0, 4, &[".88.....".into(), ".88.....".into(), "........".into()]).unwrap();
        d.add_frame(3, Some(2), false).unwrap();
        d.tags[0].to = 3;
        d.regions.push(Region::new("feet", Rect::new(0, 4, 8, 4)));
        let opts = AnimOptions { track: Some(Area::Region("feet".into())), ..Default::default() };
        let a = analyze(&d, 0, &opts).unwrap();
        let w = a.warnings.join("\n");
        assert!(w.contains("ground line") && w.contains("0:6 1:5 2:6 3:6"), "{w}");
        assert!(w.contains("step 3 (f3) is identical"), "{w}");
        assert_eq!(a.steps[0].changed, None, "no loop: nothing before the first step");
        assert_eq!(a.steps[1].track, Some(Edges { left: 1, top: 4, right: 2, bottom: 5 }));
        let s = summary(&d, &a);
        assert!(s.contains("track feet: centre per step 0:(0.5,5.5) bottom 6 | 1:(1.5,4.5) bottom 5"), "{s}");
    }

    #[test]
    fn render_layout_loop_cell_and_diff_pixels() {
        // three steps; step 1 only adds the pixel (2,5), so nothing is "moved"
        let mut d = walker(Direction::Forward, true);
        d.set(0, 2, 7, 0, TRANSPARENT);
        d.set_rows(0, 1, 0, 5, &["888.....".into(), "88......".into()]).unwrap();
        let opts = AnimOptions { zoom: Some(4), ..Default::default() };
        let a = analyze(&d, 0, &opts).unwrap();
        let img = render(&d, 0, &a, &opts).unwrap();
        // 3 steps + loop cell
        let label_w = text_width("frames", TS) + 14;
        assert_eq!(img.width, label_w + 4 * (32 + 10) + 10);
        // diff row of step 1: the pixel at (1,5) is new -> highlight
        let head_h = 3 * (text_height(TS) + 3) + 4;
        let title_h = text_height(TS) + 12;
        let diff_y = title_h + head_h + 2 * (32 + 10);
        let x0 = label_w + (32 + 10);
        assert_eq!(img.get(x0 + 2 * 4 + 1, diff_y + 5 * 4 + 1), DIFF, "(2,5) entered in step 1");
        assert_ne!(img.get(x0 + 4 + 1, diff_y + 5 * 4 + 1), DIFF, "(1,5) unchanged");
        // area crop enlarges only that part
        let crop = AnimOptions { area: Some(Area::Rect(Rect::new(0, 4, 4, 4))), ..Default::default() };
        let small = render(&d, 0, &analyze(&d, 0, &crop).unwrap(), &crop).unwrap();
        assert!(small.height < 900 && small.width > img.width, "auto zoom enlarges the 4x4 area");
    }

    #[test]
    fn whole_sprite_bob_is_aligned() {
        let mut d = Document::new(10, 10, Document::builtin_palette("pico8").unwrap()).unwrap();
        d.set_rows(0, 0, 2, 2, &["8888".into(), "8c88".into(), "8888".into()]).unwrap();
        d.add_frame(1, Some(0), false).unwrap();
        // frame 1: everything one row up, plus one real change
        let rows = d.rows(0, 0);
        d.set_rows(0, 1, 0, 0, &rows[1..]).unwrap();
        d.set_rows(0, 1, 0, 9, &["..........".into()]).unwrap();
        d.set(0, 1, 5, 1, b'9');
        d.set_tag(Tag::new("bob", 0, 1)).unwrap();
        let a = analyze(&d, 0, &AnimOptions::default()).unwrap();
        assert_eq!(a.steps[1].aligned, Some(((0, -1), 1)), "moved up 1, one real change");
        let s = summary(&d, &a);
        assert!(
            s.contains("most pixels match the previous step shifted by (0, -1); 1 px change after aligning"),
            "{s}"
        );
        assert!(!a.warnings.iter().any(|w| w.contains("orphan pixel(s)") && w.contains("(3,2)")), "{:?}", a.warnings);
    }

    #[test]
    fn marker_track_is_an_anchor_path() {
        let mut d = walker(Direction::Forward, true);
        let mut hand = Region::marker("hand", 1, 4);
        hand.set_rect_for(1, 1, Some(Rect::new(2, 3, 1, 1)), 3);
        d.regions.push(hand);
        let opts = AnimOptions { track: Some(Area::Region("hand".into())), ..Default::default() };
        let a = analyze(&d, 0, &opts).unwrap();
        let pts: Vec<(i32, i32)> = a.steps.iter().map(|s| s.track.map(|e| (e.left, e.top)).unwrap()).collect();
        assert_eq!(pts, vec![(1, 4), (2, 3), (1, 4)], "the marker's point per step, not the pixels");
        let crop = AnimOptions { area: Some(Area::Region("hand".into())), ..opts };
        assert!(render(&d, 0, &analyze(&d, 0, &crop).unwrap(), &crop).is_ok(), "marker crop has a margin");
    }

    #[test]
    fn area_parse() {
        assert_eq!(Area::parse("1, 2,3,4").unwrap(), Area::Rect(Rect::new(1, 2, 3, 4)));
        assert_eq!(Area::parse("left_shoe").unwrap(), Area::Region("left_shoe".into()));
        assert!(Area::parse("0,0,0,4").is_err());
    }
}
