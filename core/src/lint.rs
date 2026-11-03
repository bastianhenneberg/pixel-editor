//! Pixel-art craft checks on one key grid (a layer's cel or the composite
//! of a frame), for the `lint` tool. Read-only; every finding has
//! coordinates.
//!
//! - orphans: pixels without a 4-neighbour of the same key (see
//!   `animation::stray_in`), except those with a diagonal neighbour of their
//!   key (steps of a diagonal line); single highlights such as eyes or
//!   glints are often deliberate.
//! - doubles: L-corners on 1-px lines. Only "thin" shapes are looked at:
//!   8-connected areas of one key with at least 4 pixels and no 2x2 block
//!   of that key, so filled areas and thick strokes are never flagged. In
//!   such a line, a pixel p is a double when exactly two of its
//!   4-neighbours have its key and they are perpendicular (an L), every
//!   other same-key neighbour of p touches one of the two (removing p keeps
//!   the line connected), and the two arms do not both continue straight
//!   away from p (that is a real right-angle corner, e.g. of a box). One
//!   stair step often holds two such corners next to each other; they are
//!   reported as one double, either pixel can go.
//! - jaggies: uneven steps along thin lines. The line is cut into simple
//!   chains (at junctions, after removing one pixel of each double) and
//!   each chain into segments whose moves stay in one octant (one straight
//!   and one diagonal direction). The pixel runs along the straight
//!   direction are its steps; the first and last run of a segment are
//!   partial and ignored, at least 3 inner runs are needed. A segment is
//!   fine when its inner runs take at most two neighbouring values (a
//!   straight line of any slope, e.g. 2-1-2-1 or 3-3-2), only grow or only
//!   shrink (a curve), or change by at most 1 from run to run; anything
//!   else (2-1-3, 3-1-3, 1-4-1) is reported. Limits: lines drawn in more
//!   than one key (anti-aliasing, selective outlines) are split into
//!   several lines, short lines are not judged, and an intended sharp kink
//!   inside one octant is reported too.
//! - ramps: colors that are in no ramp of the document (only when it has
//!   ramps); outline and accent colors often are on purpose.
//!
//! No banding check: a cheap heuristic (parallel bands of shades along an
//! outline) gave too many false hits on small sprites, so it is left out.

use crate::animation::{coords, stray_in, Points};
use crate::document::{Ramp, TRANSPARENT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Orphans,
    Doubles,
    Jaggies,
    Ramps,
}

impl Check {
    pub const ALL: [Check; 4] = [Check::Orphans, Check::Doubles, Check::Jaggies, Check::Ramps];

    pub fn id(self) -> &'static str {
        match self {
            Check::Orphans => "orphans",
            Check::Doubles => "doubles",
            Check::Jaggies => "jaggies",
            Check::Ramps => "ramps",
        }
    }

    pub fn parse(s: &str) -> Result<Check, String> {
        Check::ALL
            .into_iter()
            .find(|c| c.id() == s)
            .ok_or_else(|| format!("unknown check {s:?} ({})", Check::ALL.map(Check::id).join(", ")))
    }
}

/// An L-corner on a line: the pixel(s) of which one can go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Double {
    pub key: u8,
    pub pixels: Vec<(i32, i32)>,
}

/// A line segment with uneven steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jaggy {
    pub key: u8,
    pub from: (i32, i32),
    pub to: (i32, i32),
    /// Pixel runs along the straight direction, first and last included.
    pub runs: Vec<u32>,
    /// 'x' for runs along rows, 'y' along columns.
    pub axis: char,
}

/// A color in no ramp: key, pixel count and the first pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffRamp {
    pub key: u8,
    pub count: usize,
    pub first: (i32, i32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Findings {
    pub orphans: Points,
    pub doubles: Vec<Double>,
    pub jaggies: Vec<Jaggy>,
    pub off_ramp: Vec<OffRamp>,
}

impl Findings {
    pub fn is_empty(&self) -> bool {
        self.orphans.is_empty() && self.doubles.is_empty() && self.jaggies.is_empty() && self.off_ramp.is_empty()
    }
}

const N8: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

struct Grid<'a> {
    keys: &'a [u8],
    w: i32,
    h: i32,
}

impl Grid<'_> {
    fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            TRANSPARENT
        } else {
            self.keys[(y * self.w + x) as usize]
        }
    }

    fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }

    /// 8-connected areas of one key that are 1 px thin (no 2x2 block) and
    /// at least 4 pixels big: the lines.
    fn thin_lines(&self) -> Vec<(u8, Vec<(i32, i32)>)> {
        let mut seen = vec![false; self.keys.len()];
        let mut out = Vec::new();
        for y in 0..self.h {
            for x in 0..self.w {
                let k = self.at(x, y);
                if k == TRANSPARENT || seen[self.idx(x, y)] {
                    continue;
                }
                let mut comp = vec![(x, y)];
                seen[self.idx(x, y)] = true;
                let mut i = 0;
                while i < comp.len() {
                    let (cx, cy) = comp[i];
                    for (dx, dy) in N8 {
                        let (nx, ny) = (cx + dx, cy + dy);
                        if self.at(nx, ny) == k && !seen[self.idx(nx, ny)] {
                            seen[self.idx(nx, ny)] = true;
                            comp.push((nx, ny));
                        }
                    }
                    i += 1;
                }
                let block = comp.iter().any(|&(cx, cy)| {
                    self.at(cx + 1, cy) == k && self.at(cx, cy + 1) == k && self.at(cx + 1, cy + 1) == k
                });
                if comp.len() >= 4 && !block {
                    out.push((k, comp));
                }
            }
        }
        out
    }
}

/// The L-corner pixels of one thin line.
fn corners(g: &Grid, k: u8, line: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let is = |x: i32, y: i32| g.at(x, y) == k;
    let mut out = Vec::new();
    for &(x, y) in line {
        let four: Vec<(i32, i32)> =
            [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().filter(|d| is(x + d.0, y + d.1)).collect();
        if four.len() != 2 || four[0].0 * four[1].0 + four[0].1 * four[1].1 != 0 {
            continue; // not two perpendicular arms
        }
        let (u, v) = (four[0], four[1]);
        let (a, b) = ((x + u.0, y + u.1), (x + v.0, y + v.1));
        let touches = |p: (i32, i32), q: (i32, i32)| (p.0 - q.0).abs() <= 1 && (p.1 - q.1).abs() <= 1;
        let others_kept = N8
            .iter()
            .map(|d| (x + d.0, y + d.1))
            .filter(|&n| is(n.0, n.1) && n != a && n != b)
            .all(|n| touches(n, a) || touches(n, b));
        let corner = is(x + 2 * u.0, y + 2 * u.1) && is(x + 2 * v.0, y + 2 * v.1);
        if others_kept && !corner {
            out.push((x, y));
        }
    }
    out
}

/// Simple chains of a line without the given pixels, split at junctions;
/// each as pixels in walking order.
fn chains(line: &[(i32, i32)], removed: &[(i32, i32)]) -> Vec<Vec<(i32, i32)>> {
    use std::collections::HashSet;
    let all: HashSet<(i32, i32)> = line.iter().copied().filter(|p| !removed.contains(p)).collect();
    let nbrs = |p: (i32, i32), set: &HashSet<(i32, i32)>| -> Vec<(i32, i32)> {
        N8.iter().map(|d| (p.0 + d.0, p.1 + d.1)).filter(|n| set.contains(n)).collect()
    };
    let simple: HashSet<(i32, i32)> = all.iter().copied().filter(|p| nbrs(*p, &all).len() <= 2).collect();
    let mut left: Vec<(i32, i32)> = line.iter().copied().filter(|p| simple.contains(p)).collect();
    let mut done: HashSet<(i32, i32)> = HashSet::new();
    let mut out = Vec::new();
    // start at chain ends first, so open chains are walked end to end
    left.sort_by_key(|p| nbrs(*p, &simple).len());
    for start in left {
        if done.contains(&start) {
            continue;
        }
        let mut chain = vec![start];
        done.insert(start);
        let mut cur = start;
        while let Some(next) = nbrs(cur, &simple).into_iter().find(|n| !done.contains(n)) {
            chain.push(next);
            done.insert(next);
            cur = next;
        }
        out.push(chain);
    }
    out
}

/// Is a run sequence a clean line or curve (see the module doc)?
fn even_steps(inner: &[u32]) -> bool {
    let (lo, hi) = (*inner.iter().min().unwrap(), *inner.iter().max().unwrap());
    let up = inner.windows(2).all(|w| w[0] <= w[1]);
    let down = inner.windows(2).all(|w| w[0] >= w[1]);
    let smooth = inner.windows(2).all(|w| w[0].abs_diff(w[1]) <= 1);
    hi - lo <= 1 || up || down || smooth
}

/// Uneven segments of one chain.
fn jaggies_of(k: u8, chain: &[(i32, i32)]) -> Vec<Jaggy> {
    let mut out = Vec::new();
    let moves: Vec<(i32, i32)> = chain.windows(2).map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1)).collect();
    let mut start = 0;
    while start < moves.len() {
        // the longest run of moves in one octant
        let mut axial: Option<(i32, i32)> = None;
        let mut diag: Option<(i32, i32)> = None;
        let mut end = start;
        while end < moves.len() {
            let m = moves[end];
            let is_diag = m.0 != 0 && m.1 != 0;
            let fits = if is_diag {
                diag.is_none_or(|d| d == m) && axial.is_none_or(|a| a.0 == 0 && a.1 == m.1 || a.1 == 0 && a.0 == m.0)
            } else {
                axial.is_none_or(|a| a == m) && diag.is_none_or(|d| m.0 == 0 && m.1 == d.1 || m.1 == 0 && m.0 == d.0)
            };
            if !fits {
                break;
            }
            if is_diag {
                diag = Some(m);
            } else {
                axial = Some(m);
            }
            end += 1;
        }
        if let (Some(a), Some(_)) = (axial, diag) {
            let mut runs = Vec::new();
            let mut cur = 1;
            for m in &moves[start..end] {
                if *m == a {
                    cur += 1;
                } else {
                    runs.push(cur);
                    cur = 1;
                }
            }
            runs.push(cur);
            if runs.len() >= 5 && !even_steps(&runs[1..runs.len() - 1]) {
                out.push(Jaggy {
                    key: k,
                    from: chain[start],
                    to: chain[end],
                    runs,
                    axis: if a.1 == 0 { 'x' } else { 'y' },
                });
            }
        }
        start = end.max(start + 1);
    }
    out
}

/// Run the checks on a `w` x `h` key grid. `ramps` is only used by the
/// ramps check (and skipped when empty).
pub fn lint(keys: &[u8], w: u32, h: u32, checks: &[Check], ramps: &[Ramp]) -> Findings {
    let g = Grid { keys, w: w as i32, h: h as i32 };
    let mut f = Findings::default();
    if checks.contains(&Check::Orphans) {
        let diagonal = |&(x, y): &(i32, i32)| {
            let k = g.at(x, y);
            [(-1, -1), (1, -1), (-1, 1), (1, 1)].iter().any(|(dx, dy)| g.at(x + dx, y + dy) == k)
        };
        f.orphans = stray_in(keys, g.w, g.h).0.into_iter().filter(|p| !diagonal(p)).collect();
    }
    if checks.contains(&Check::Doubles) || checks.contains(&Check::Jaggies) {
        for (k, line) in g.thin_lines() {
            // corners next to each other belong to one stair step
            let mut groups: Vec<Vec<(i32, i32)>> = Vec::new();
            for c in corners(&g, k, &line) {
                match groups.iter_mut().find(|gr| gr.iter().any(|p| (p.0 - c.0).abs() + (p.1 - c.1).abs() == 1)) {
                    Some(gr) => gr.push(c),
                    None => groups.push(vec![c]),
                }
            }
            if checks.contains(&Check::Jaggies) {
                let removed: Vec<(i32, i32)> = groups.iter().map(|gr| gr[0]).collect();
                for chain in chains(&line, &removed) {
                    f.jaggies.extend(jaggies_of(k, &chain));
                }
            }
            if checks.contains(&Check::Doubles) {
                f.doubles.extend(groups.into_iter().map(|pixels| Double { key: k, pixels }));
            }
        }
        f.doubles.sort_by_key(|d| (d.pixels[0].1, d.pixels[0].0));
        f.jaggies.sort_by_key(|j| (j.from.1, j.from.0));
    }
    if checks.contains(&Check::Ramps) && !ramps.is_empty() {
        for y in 0..g.h {
            for x in 0..g.w {
                let k = g.at(x, y);
                if k == TRANSPARENT || ramps.iter().any(|r| r.keys.contains(&k)) {
                    continue;
                }
                match f.off_ramp.iter_mut().find(|o| o.key == k) {
                    Some(o) => o.count += 1,
                    None => f.off_ramp.push(OffRamp { key: k, count: 1, first: (x, y) }),
                }
            }
        }
    }
    f
}

/// "3 orphans, 1 double, 0 jaggies, 2 off-ramp colors" for the checks run.
pub fn counts(f: &Findings, checks: &[Check], has_ramps: bool) -> String {
    let mut parts = Vec::new();
    for c in checks {
        parts.push(match c {
            Check::Orphans => format!("{} orphans", f.orphans.len()),
            Check::Doubles => format!("{} doubles", f.doubles.len()),
            Check::Jaggies => format!("{} jaggy lines", f.jaggies.len()),
            Check::Ramps if has_ramps => format!("{} off-ramp colors", f.off_ramp.len()),
            Check::Ramps => "ramps: none defined".to_string(),
        });
    }
    parts.join(", ")
}

/// The findings grouped by kind, at most `max` entries per kind.
pub fn describe(f: &Findings, label: impl Fn(u8) -> String, max: usize) -> String {
    let mut s = String::new();
    let more = |n: usize| if n > max { format!(" … {} more", n - max) } else { String::new() };
    if !f.orphans.is_empty() {
        s += &format!("  orphans (no neighbour of the same key, diagonal steps aside): {}\n", coords(&f.orphans, max));
    }
    if !f.doubles.is_empty() {
        let list: Vec<String> = f
            .doubles
            .iter()
            .take(max)
            .map(|d| {
                let px: Vec<String> = d.pixels.iter().map(|(x, y)| format!("({x},{y})")).collect();
                format!("{} {}", label(d.key), px.join("/"))
            })
            .collect();
        s += &format!(
            "  doubles (L-corner on a 1-px line; remove the pixel, or one of the pair): {}{}\n",
            list.join("; "),
            more(f.doubles.len())
        );
    }
    if !f.jaggies.is_empty() {
        let list: Vec<String> = f
            .jaggies
            .iter()
            .take(max)
            .map(|j| {
                let runs: Vec<String> = j.runs.iter().map(u32::to_string).collect();
                format!(
                    "{} ({},{})->({},{}) runs along {} {}",
                    label(j.key),
                    j.from.0,
                    j.from.1,
                    j.to.0,
                    j.to.1,
                    j.axis,
                    runs.join("-")
                )
            })
            .collect();
        s += &format!(
            "  jaggies (uneven steps; ends are partial and not judged): {}{}\n",
            list.join("; "),
            more(f.jaggies.len())
        );
    }
    if !f.off_ramp.is_empty() {
        let list: Vec<String> = f
            .off_ramp
            .iter()
            .take(max)
            .map(|o| format!("{} {} px from ({},{})", label(o.key), o.count, o.first.0, o.first.1))
            .collect();
        s += &format!("  colors in no ramp: {}{}\n", list.join("; "), more(f.off_ramp.len()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> (Vec<u8>, u32, u32) {
        let w = rows[0].len() as u32;
        (rows.iter().flat_map(|r| r.bytes()).collect(), w, rows.len() as u32)
    }

    fn run(rows: &[&str]) -> Findings {
        let (k, w, h) = grid(rows);
        lint(&k, w, h, &Check::ALL, &[])
    }

    #[test]
    fn a_filled_rectangle_and_a_box_outline_are_clean() {
        let f = run(&["..........", ".88888888.", ".88888888.", ".88888888.", ".........."]);
        assert!(f.is_empty(), "{f:?}");
        let f = run(&["........", ".888888.", ".8....8.", ".8....8.", ".888888.", "........"]);
        assert!(f.is_empty(), "box corners are real corners: {f:?}");
    }

    #[test]
    fn a_clean_2_1_line_and_a_curve_are_clean() {
        let f = run(&["88..........", "..88........", "....88......", "......88....", "........88..", "..........88"]);
        assert!(f.is_empty(), "{f:?}");
        // 1-1-2-3-4: a curve flattening out
        let f = run(&["8...........", ".8..........", "..88........", "....888.....", ".......8888."]);
        assert!(f.jaggies.is_empty() && f.doubles.is_empty(), "{f:?}");
    }

    #[test]
    fn doubles_are_l_corners_reported_once() {
        // a 2:1 line with one stair step drawn as an L
        let f = run(&["88..........", ".888........", "...888......", "......88...."]);
        assert_eq!(f.doubles.len(), 2, "{:?}", f.doubles);
        assert_eq!(f.doubles[0].pixels, vec![(1, 0), (1, 1)], "one stair step, either pixel");
        // a hook at the start of a line
        let f = run(&["8.....", "88888.", "......"]);
        assert_eq!(f.doubles, vec![Double { key: b'8', pixels: vec![(0, 1)] }]);
    }

    #[test]
    fn uneven_steps_are_jaggies() {
        // runs 2-2-1-3-2-2: the 1 and 3 break the 2:1 line
        let f = run(&[
            "88.............",
            "..88...........",
            "....8..........",
            ".....888.......",
            "........88.....",
            "..........88...",
        ]);
        assert_eq!(f.jaggies.len(), 1, "{f:?}");
        let j = &f.jaggies[0];
        assert_eq!((j.runs.clone(), j.axis, j.from, j.to), (vec![2, 2, 1, 3, 2, 2], 'x', (0, 0), (11, 5)));
        let text = describe(&f, |k| (k as char).to_string(), 8);
        assert!(text.contains("8 (0,0)->(11,5) runs along x 2-2-1-3-2-2"), "{text}");
        // a vertical one
        let rows: Vec<String> = [0, 0, 1, 1, 2, 3, 3, 3, 4, 4, 5, 5]
            .iter()
            .map(|&x| {
                let mut r = vec![b'.'; 7];
                r[x] = b'8';
                String::from_utf8(r).unwrap()
            })
            .collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let f = run(&rows);
        assert_eq!(
            f.jaggies.iter().map(|j| (j.axis, j.runs.clone())).collect::<Vec<_>>(),
            vec![('y', vec![2, 2, 1, 3, 2, 2])]
        );
    }

    #[test]
    fn orphans_and_ramps() {
        let (k, w, h) = grid(&["8..", "..9", "99c", "..9"]);
        let ramps = [Ramp::new("skin", b"89")];
        let f = lint(&k, w, h, &Check::ALL, &ramps);
        // (2,1) has no 4-neighbour of its key but continues a diagonal line
        assert_eq!(f.orphans, vec![(0, 0), (2, 2)]);
        assert_eq!(f.off_ramp, vec![OffRamp { key: b'c', count: 1, first: (2, 2) }]);
        assert_eq!(counts(&f, &[Check::Orphans, Check::Ramps], true), "2 orphans, 1 off-ramp colors");
        assert!(lint(&k, w, h, &[Check::Ramps], &[]).off_ramp.is_empty(), "no ramps, no check");
        assert!(Check::parse("banding").is_err());
    }
}
