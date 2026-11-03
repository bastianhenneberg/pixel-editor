//! Path tracing for the animation review: where a point is on each step of
//! a tag (a marker region's point, or the topmost pixel of a palette key),
//! the step between steps, and warnings where the path does not move on an
//! arc: single steps against the motion on one axis (back and forth), more
//! than two direction reversals (consecutive moves over 90 degrees apart; a
//! pendulum swing turns twice per loop, an arc or circle never), and steps
//! longer than a threshold. Standing still is not a move, and wiggles made
//! only of 1-px moves (a head bob) are not judged: a pixel is the smallest
//! step, so a small arc can look like that.

use crate::animation::Area;
use crate::document::{Document, TRANSPARENT};
use crate::draw;
use crate::render::Image;

/// What to follow.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceSource {
    /// A marker: its point per step. Another region: the topmost opaque
    /// pixel inside its rect.
    Region(String),
    /// The topmost pixel of this key (composite), optionally only inside an area.
    Key { key: u8, area: Option<Area> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub source: TraceSource,
}

impl Trace {
    pub fn label(&self, doc: &Document) -> String {
        match &self.source {
            TraceSource::Region(n) => n.clone(),
            TraceSource::Key { key, area } => match area {
                Some(a) => format!("key {} in {}", doc.key_label(*key), a.label()),
                None => format!("key {}", doc.key_label(*key)),
            },
        }
    }
}

/// A traced point over the steps of a tag.
#[derive(Clone, Debug, PartialEq)]
pub struct TracePath {
    pub label: String,
    /// Position per step (None where the region is absent or the key not drawn).
    pub points: Vec<Option<(i32, i32)>>,
    pub looping: bool,
    pub warnings: Vec<String>,
}

/// Topmost pixel among `pixels`: the smallest y, and the middle of that
/// row's pixels (so a flat top reads as one point, not its left end).
fn topmost(pixels: impl Iterator<Item = (i32, i32)>) -> Option<(i32, i32)> {
    let mut best: Option<(i32, i32, i32)> = None; // y, min x, max x
    for (x, y) in pixels {
        best = match best {
            Some((by, x0, x1)) if y == by => Some((by, x0.min(x), x1.max(x))),
            Some((by, ..)) if y > by => best,
            _ => Some((y, x, x)),
        };
    }
    best.map(|(y, x0, x1)| ((x0 + x1).div_euclid(2), y))
}

/// The traced point on each frame of `seq`.
pub fn positions(doc: &Document, seq: &[usize], t: &Trace) -> Result<Vec<Option<(i32, i32)>>, String> {
    let (w, h) = (doc.width as i32, doc.height as i32);
    let all = move || (0..h).flat_map(move |y| (0..w).map(move |x| (x, y)));
    match &t.source {
        TraceSource::Region(name) => {
            let r = &doc.regions[doc.find_region(name)?];
            Ok(seq
                .iter()
                .map(|&f| {
                    let rc = r.rect_at(f)?;
                    if r.is_marker() {
                        return Some((rc.x, rc.y));
                    }
                    topmost(all().filter(|&(x, y)| rc.contains(x, y) && doc.top_key(f, x, y) != TRANSPARENT))
                })
                .collect())
        }
        TraceSource::Key { key, area } => {
            let rects = match area {
                Some(a) => a.rects(doc, seq)?,
                None => vec![None; seq.len()],
            };
            Ok(seq
                .iter()
                .zip(rects)
                .map(|(&f, rc)| {
                    if area.is_some() && rc.is_none() {
                        return None;
                    }
                    topmost(all().filter(|&(x, y)| rc.is_none_or(|r| r.contains(x, y)) && doc.top_key(f, x, y) == *key))
                })
                .collect())
        }
    }
}

fn sign(v: i32) -> i32 {
    v.signum()
}

/// Follow `t` through `seq` and judge the path.
pub fn trace(doc: &Document, seq: &[usize], looping: bool, t: &Trace, threshold: u32) -> Result<TracePath, String> {
    let points = positions(doc, seq, t)?;
    let label = t.label(doc);
    let n = points.len();
    let mut warnings = Vec::new();
    let missing: Vec<usize> = (0..n).filter(|i| points[*i].is_none()).collect();
    if !missing.is_empty() {
        warnings.push(format!("{label}: not found on step(s) {missing:?}"));
    }
    // moves into each step (into step 0 only when looping)
    let moves: Vec<(usize, (i32, i32))> = (0..n)
        .filter_map(|i| {
            let p = if i > 0 {
                i - 1
            } else if looping && n > 1 {
                n - 1
            } else {
                return None;
            };
            let (a, b) = (points[p]?, points[i]?);
            Some((i, (b.0 - a.0, b.1 - a.1)))
        })
        .collect();
    for &(i, (dx, dy)) in &moves {
        if dx.unsigned_abs().max(dy.unsigned_abs()) > threshold {
            warnings.push(format!(
                "{label}: step {i} moves ({dx}, {dy}), more than {threshold} px at once (reads as a pop; add an \
                 in-between or check the position)"
            ));
        }
    }
    // per axis: the signs of the moves that move on it
    let cyclic = looping && moves.len() == n;
    for (axis, name) in [(0, "x"), (1, "y")] {
        // (step, move on this axis) for the moves that move on it
        let s: Vec<(usize, i32)> =
            moves.iter().map(|&(i, d)| (i, if axis == 0 { d.0 } else { d.1 })).filter(|(_, v)| *v != 0).collect();
        if s.len() < 3 {
            continue;
        }
        let m = s.len();
        let at = |j: isize| s[j.rem_euclid(m as isize) as usize];
        let against = |j: isize| sign(at(j).1) != sign(at(j - 1).1) && sign(at(j).1) != sign(at(j + 1).1);
        // 1-px wiggles are the smallest step there is (e.g. a head bob), not judged
        let big = |j: isize| at(j - 1).1.abs().max(at(j).1.abs()).max(at(j + 1).1.abs()) >= 2;
        let range: Vec<isize> = if cyclic { (0..m as isize).collect() } else { (1..m as isize - 1).collect() };
        // one step against the motion on both sides: back and forth
        let jitter: Vec<usize> = range.iter().filter(|&&j| against(j) && big(j)).map(|&j| at(j).0).collect();
        if !jitter.is_empty() {
            warnings.push(format!(
                "{label}: {name} goes back and forth at step(s) {jitter:?} (one step against the motion; an arc \
                 keeps its direction between the turning points)"
            ));
        }
    }
    // direction reversals: consecutive moves more than 90 degrees apart
    let moving: Vec<(usize, (i32, i32))> = moves.iter().copied().filter(|(_, d)| *d != (0, 0)).collect();
    let m = moving.len();
    if m >= 3 {
        let pairs = if cyclic { m } else { m - 1 };
        let turns: Vec<usize> = (0..pairs)
            .filter(|&j| {
                let (a, b) = (moving[j].1, moving[(j + 1) % m].1);
                let size = |d: (i32, i32)| d.0.abs().max(d.1.abs());
                a.0 * b.0 + a.1 * b.1 < 0 && size(a).max(size(b)) >= 2
            })
            .map(|j| moving[(j + 1) % m].0)
            .collect();
        if turns.len() > 2 {
            warnings.push(format!(
                "{label}: reverses direction {} times{} at step(s) {turns:?} (a swing turns at most twice, an arc \
                 or circle not at all)",
                turns.len(),
                if cyclic { " per loop" } else { "" }
            ));
        }
    }
    Ok(TracePath { label, points, looping, warnings })
}

/// "hand: 0:(3,4) 1:(5,3) +2,-1 | ..." per step with the move into it.
pub fn describe(p: &TracePath) -> String {
    let n = p.points.len();
    let mut parts = Vec::new();
    for (i, pt) in p.points.iter().enumerate() {
        let prev = if i > 0 {
            Some(i - 1)
        } else if p.looping && n > 1 {
            Some(n - 1)
        } else {
            None
        };
        parts.push(match (pt, prev.and_then(|j| p.points[j])) {
            (Some((x, y)), Some((px, py))) => format!("{i}:({x},{y}) {:+},{:+}", x - px, y - py),
            (Some((x, y)), None) => format!("{i}:({x},{y})"),
            (None, _) => format!("{i}:-"),
        });
    }
    format!("trace {}: {}", p.label, parts.join(" | "))
}

/// Overlay colors per trace (distinct from the track box and path).
pub const COLORS: [[u8; 4]; 4] = [[255, 90, 200, 255], [120, 255, 120, 255], [255, 160, 40, 255], [160, 140, 255, 255]];

/// Draw every trace's whole path dotted into one filmstrip cell and mark
/// the point of `step`. `screen` maps an art pixel to the screen position
/// of its centre; `clip` = (x0, y0, x1, y1) of the cell.
pub fn draw_paths(
    out: &mut Image,
    paths: &[TracePath],
    step: usize,
    zoom: u32,
    screen: impl Fn(i32, i32) -> (i32, i32),
    clip: (i32, i32, i32, i32),
) {
    let inside = |x: i32, y: i32| x >= clip.0 && y >= clip.1 && x < clip.2 && y < clip.3;
    for (ti, p) in paths.iter().enumerate() {
        let c = COLORS[ti % COLORS.len()];
        let pts: Vec<(i32, i32)> = p.points.iter().flatten().map(|&(x, y)| screen(x, y)).collect();
        let mut segs: Vec<((i32, i32), (i32, i32))> = pts.windows(2).map(|w| (w[0], w[1])).collect();
        if p.looping && pts.len() > 2 {
            segs.push((pts[pts.len() - 1], pts[0]));
        }
        for (a, b) in segs {
            for (k, (x, y)) in draw::line(a.0, a.1, b.0, b.1).into_iter().enumerate() {
                if k % 4 < 2 && inside(x, y) {
                    out.put(x as u32, y as u32, c);
                }
            }
        }
        // every step's point as a small dot, this step's as a ring
        let r = (zoom as i32 / 2).max(2);
        for (i, pt) in p.points.iter().enumerate() {
            let Some(&(x, y)) = pt.as_ref() else { continue };
            let (cx, cy) = screen(x, y);
            if i == step {
                for d in -r..=r {
                    for (px, py) in [(cx + d, cy - r), (cx + d, cy + r), (cx - r, cy + d), (cx + r, cy + d)] {
                        if inside(px, py) {
                            out.put(px as u32, py as u32, c);
                        }
                    }
                }
            } else {
                for (px, py) in [(cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)] {
                    if inside(px, py) {
                        out.put(px as u32, py as u32, c);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Rect, Region, Tag};

    /// 12x12, one frame per point: a pixel of key '8' at each position.
    fn path_doc(points: &[(i32, i32)], looping: bool) -> Document {
        let mut d = Document::new(12, 12, Document::builtin_palette("pico8").unwrap()).unwrap();
        for (i, &(x, y)) in points.iter().enumerate() {
            if i > 0 {
                d.add_frame(i, None, false).unwrap();
            }
            d.set(0, i, x, y, b'8');
            d.set(0, i, 0, 11, b'c'); // something else lower down
        }
        let mut t = Tag::new("swing", 0, points.len() - 1);
        t.looping = looping;
        d.set_tag(t).unwrap();
        d
    }

    fn run(d: &Document, t: &Trace, threshold: u32) -> TracePath {
        let seq = d.tags[0].sequence();
        trace(d, &seq, d.tags[0].looping, t, threshold).unwrap()
    }

    #[test]
    fn a_swing_on_an_arc_is_clean() {
        // forward over the top and back the same way: turns twice per loop
        let pts = [(2, 6), (4, 4), (6, 3), (8, 4), (10, 6), (8, 4), (6, 3), (4, 4)];
        let d = path_doc(&pts, true);
        let p = run(&d, &Trace { source: TraceSource::Key { key: b'8', area: None } }, 3);
        assert_eq!(p.points, pts.iter().map(|&p| Some(p)).collect::<Vec<_>>());
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let s = describe(&p);
        assert!(s.starts_with("trace key 8"), "{s}");
        assert!(s.contains("0:(2,6) -2,+2 | 1:(4,4) +2,-2"), "{s}");
    }

    #[test]
    fn back_and_forth_reversals_and_jumps_are_flagged() {
        // x: + + - + + (one step back), and a 5 px jump
        let pts = [(1, 5), (3, 5), (5, 5), (3, 5), (5, 5), (10, 5)];
        let d = path_doc(&pts, false);
        let p = run(&d, &Trace { source: TraceSource::Key { key: b'8', area: None } }, 3);
        let w = p.warnings.join("\n");
        assert!(w.contains("x goes back and forth at step(s) [3]"), "{w}");
        assert!(w.contains("step 5 moves (5, 0), more than 3 px"), "{w}");
        // a 1-px bob (up, down, up, down) is not judged
        let bob = [(4, 5), (4, 4), (4, 5), (4, 4)];
        let p = run(&path_doc(&bob, true), &Trace { source: TraceSource::Key { key: b'8', area: None } }, 3);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // a zig-zag reverses more often than a swing
        let zig = [(1, 2), (3, 2), (2, 2), (4, 2), (3, 2), (5, 2)];
        let p = run(&path_doc(&zig, false), &Trace { source: TraceSource::Key { key: b'8', area: None } }, 3);
        assert!(p.warnings.iter().any(|w| w.contains("reverses direction 4 times")), "{:?}", p.warnings);
    }

    #[test]
    fn markers_regions_and_flat_tops() {
        let mut d = path_doc(&[(5, 5), (6, 4)], true);
        let mut hand = Region::marker("hand", 1, 1);
        hand.set_rect_for(1, 1, Some(Rect::new(2, 2, 1, 1)), 2);
        d.regions.push(hand);
        d.regions.push(Region::new("low", Rect::new(0, 8, 12, 4)));
        d.set_rows(0, 0, 3, 9, &["888".into()]).unwrap();
        let seq = d.tags[0].sequence();
        let m = positions(&d, &seq, &Trace { source: TraceSource::Region("hand".into()) }).unwrap();
        assert_eq!(m, vec![Some((1, 1)), Some((2, 2))], "a marker's point");
        let r = positions(&d, &seq, &Trace { source: TraceSource::Region("low".into()) }).unwrap();
        assert_eq!(r, vec![Some((4, 9)), Some((0, 11))], "topmost opaque pixel in the region, middle of a flat top");
        let k = positions(
            &d,
            &seq,
            &Trace { source: TraceSource::Key { key: b'8', area: Some(Area::Rect(Rect::new(0, 0, 12, 7))) } },
        )
        .unwrap();
        assert_eq!(k, vec![Some((5, 5)), Some((6, 4))], "only inside the area");
        assert!(positions(&d, &seq, &Trace { source: TraceSource::Region("nose".into()) }).is_err());
    }
}
