//! Shape rasterisation. Functions return point lists so the GUI can preview
//! a shape before committing it with `Document::set_points`.

use crate::document::Document;

/// Bresenham line, endpoints inclusive.
pub fn line(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    let mut out = Vec::with_capacity((dx - dy + 1) as usize);
    loop {
        out.push((x, y));
        if x == x1 && y == y1 {
            return out;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Pixel-perfect freehand path (as in Aseprite): drops the corner pixel of
/// every "L" (a pixel whose neighbours before and after touch it straight
/// but each other only diagonally), so strokes stay one pixel thin.
/// Repeated points are dropped too. Expects an 8-connected path, e.g.
/// consecutive `line` segments.
pub fn pixel_perfect(path: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(path.len());
    for &p in path {
        if out.last() == Some(&p) {
            continue;
        }
        if let [.., a, c] = out[..] {
            let straight = |q: (i32, i32)| (q.0 == c.0) != (q.1 == c.1) && (q.0 - c.0).abs() + (q.1 - c.1).abs() == 1;
            if straight(a) && straight(p) && a.0 != p.0 && a.1 != p.1 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Shade: step each pixel `steps` places along its color ramp (positive =
/// lighter, see `Document::shade_key`). Pixels whose color is in no ramp
/// stay; a point listed twice is shaded once. Returns the changed count.
pub fn shade(doc: &mut Document, layer: usize, frame: usize, points: &[(i32, i32)], steps: i32) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut changed = 0;
    for &(x, y) in points {
        if !doc.in_bounds(x, y) || !seen.insert((x, y)) {
            continue;
        }
        if let Some(k) = doc.shade_key(doc.get(layer, frame, x, y), steps) {
            if doc.set(layer, frame, x, y, k) {
                changed += 1;
            }
        }
    }
    changed
}

fn ordered(a: i32, b: i32) -> (i32, i32) {
    (a.min(b), a.max(b))
}

/// Rectangle spanned by two corners (inclusive, any order).
pub fn rect(x0: i32, y0: i32, x1: i32, y1: i32, filled: bool) -> Vec<(i32, i32)> {
    let (l, r) = ordered(x0, x1);
    let (t, b) = ordered(y0, y1);
    let mut out = Vec::new();
    for y in t..=b {
        for x in l..=r {
            if filled || x == l || x == r || y == t || y == b {
                out.push((x, y));
            }
        }
    }
    out
}

/// Ellipse inscribed in the box spanned by two corners (inclusive).
/// The outline is the set of inside pixels that touch the outside
/// (4-neighbourhood), which gives a clean 1px pixel-art border.
pub fn ellipse_in_box(x0: i32, y0: i32, x1: i32, y1: i32, filled: bool) -> Vec<(i32, i32)> {
    let (l, r) = ordered(x0, x1);
    let (t, b) = ordered(y0, y1);
    let cx = (l + r + 1) as f64 / 2.0;
    let cy = (t + b + 1) as f64 / 2.0;
    let a = (r - l + 1) as f64 / 2.0;
    let bb = (b - t + 1) as f64 / 2.0;
    let inside = |x: i32, y: i32| -> bool {
        let nx = (x as f64 + 0.5 - cx) / a;
        let ny = (y as f64 + 0.5 - cy) / bb;
        nx * nx + ny * ny <= 1.0 + 1e-9
    };
    let mut out = Vec::new();
    for y in t..=b {
        for x in l..=r {
            if inside(x, y)
                && (filled || !inside(x - 1, y) || !inside(x + 1, y) || !inside(x, y - 1) || !inside(x, y + 1))
            {
                out.push((x, y));
            }
        }
    }
    out
}

/// Ellipse by centre and radii (pixels from the centre pixel to the edge).
pub fn ellipse(cx: i32, cy: i32, rx: i32, ry: i32, filled: bool) -> Vec<(i32, i32)> {
    let (rx, ry) = (rx.abs(), ry.abs());
    ellipse_in_box(cx - rx, cy - ry, cx + rx, cy + ry, filled)
}

/// Pixels of the 4-connected region of equal keys around (x, y) in one cel.
pub fn flood_region(doc: &Document, layer: usize, frame: usize, x: i32, y: i32) -> Vec<(i32, i32)> {
    flood_region_wrapped(doc, layer, frame, x, y, false, false)
}

/// Like `flood_region`, but the region continues across the left/right
/// (`wrap_x`) and top/bottom (`wrap_y`) edges, as in a seamless tile.
pub fn flood_region_wrapped(
    doc: &Document,
    layer: usize,
    frame: usize,
    x: i32,
    y: i32,
    wrap_x: bool,
    wrap_y: bool,
) -> Vec<(i32, i32)> {
    if !doc.in_bounds(x, y) {
        return Vec::new();
    }
    let (w, h) = (doc.width as i32, doc.height as i32);
    let cel = doc.cel(layer, frame);
    let target = cel[(y * w + x) as usize];
    let mut seen = vec![false; cel.len()];
    let mut stack = vec![(x, y)];
    let mut out = Vec::new();
    seen[(y * w + x) as usize] = true;
    while let Some((px, py)) = stack.pop() {
        out.push((px, py));
        for (nx, ny) in [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)] {
            let nx = if wrap_x { nx.rem_euclid(w) } else { nx };
            let ny = if wrap_y { ny.rem_euclid(h) } else { ny };
            if nx >= 0 && ny >= 0 && nx < w && ny < h {
                let i = (ny * w + nx) as usize;
                if !seen[i] && cel[i] == target {
                    seen[i] = true;
                    stack.push((nx, ny));
                }
            }
        }
    }
    out
}

/// Flood fill; returns the number of changed pixels.
pub fn fill(doc: &mut Document, layer: usize, frame: usize, x: i32, y: i32, key: u8) -> usize {
    let region = flood_region(doc, layer, frame, x, y);
    doc.set_points(layer, frame, &region, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endpoints_and_count() {
        let l = line(0, 0, 5, 2);
        assert_eq!(l.first(), Some(&(0, 0)));
        assert_eq!(l.last(), Some(&(5, 2)));
        assert_eq!(l.len(), 6);
        assert_eq!(line(3, 3, 3, 3), vec![(3, 3)]);
        assert_eq!(line(2, 0, 0, 0), vec![(2, 0), (1, 0), (0, 0)]);
    }

    /// A freehand path: consecutive Bresenham segments through `pts`.
    fn path(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
        let mut out = vec![pts[0]];
        for w in pts.windows(2) {
            out.extend(line(w[0].0, w[0].1, w[1].0, w[1].1).into_iter().skip(1));
        }
        out
    }

    /// No pixel of the result has both a horizontal and a vertical neighbour
    /// in the stroke where those two touch diagonally (no "L" corners).
    fn has_l_corner(pts: &[(i32, i32)]) -> bool {
        pts.windows(3).any(|w| {
            let (a, c, b) = (w[0], w[1], w[2]);
            (a.0 - c.0).abs() + (a.1 - c.1).abs() == 1
                && (b.0 - c.0).abs() + (b.1 - c.1).abs() == 1
                && a.0 != b.0
                && a.1 != b.1
        })
    }

    #[test]
    fn pixel_perfect_diagonal() {
        // a mouse that moves right, then down, then right ... (a staircase of Ls)
        let raw = path(&[(0, 0), (1, 0), (1, 1), (2, 1), (2, 2), (3, 2), (3, 3)]);
        assert!(has_l_corner(&raw));
        assert_eq!(pixel_perfect(&raw), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
        // a clean diagonal is kept as it is
        let diag = line(0, 0, 5, 5);
        assert_eq!(pixel_perfect(&diag), diag);
    }

    #[test]
    fn pixel_perfect_staircase() {
        // steps of two pixels: only the inner corner of each step goes
        let raw = path(&[(0, 0), (2, 0), (2, 2), (4, 2), (4, 4)]);
        let pp = pixel_perfect(&raw);
        assert!(!has_l_corner(&pp));
        assert_eq!(pp, vec![(0, 0), (1, 0), (2, 1), (3, 2), (4, 3), (4, 4)]);
        // straight lines and repeated points stay
        assert_eq!(pixel_perfect(&[(0, 0), (1, 0), (1, 0), (2, 0)]), vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn pixel_perfect_curve() {
        // a quarter circle drawn by a wobbly hand: every L corner goes,
        // the curve stays connected (8-neighbourhood) and keeps its ends
        let raw = path(&[
            (0, 8),
            (1, 8),
            (1, 7),
            (2, 7),
            (3, 6),
            (3, 5),
            (4, 5),
            (5, 4),
            (5, 3),
            (6, 3),
            (6, 2),
            (7, 1),
            (8, 1),
            (8, 0),
        ]);
        let pp = pixel_perfect(&raw);
        assert!(has_l_corner(&raw) && !has_l_corner(&pp));
        assert!(pp.len() < raw.len());
        assert_eq!((pp[0], *pp.last().unwrap()), ((0, 8), (8, 0)));
        assert!(pp.windows(2).all(|w| (w[0].0 - w[1].0).abs() <= 1 && (w[0].1 - w[1].1).abs() <= 1));
    }

    #[test]
    fn shade_steps_along_the_ramp_once() {
        use crate::document::Ramp;
        let mut d = Document::new(4, 1, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        d.set_ramps(vec![Ramp::new("crust", b"389a")]).unwrap();
        d.set_rows(0, 0, 0, 0, &["38a5".to_string()]).unwrap();
        let row = [(0, 0), (1, 0), (2, 0), (3, 0), (1, 0)];
        assert_eq!(shade(&mut d, 0, 0, &row, 1), 2, "a stays at the light end, 5 is in no ramp");
        assert_eq!(d.rows(0, 0), vec!["89a5"]);
        assert_eq!(shade(&mut d, 0, 0, &row, -2), 3);
        assert_eq!(d.rows(0, 0), vec!["3385"]);
    }

    #[test]
    fn rect_outline_and_filled() {
        assert_eq!(rect(0, 0, 2, 2, true).len(), 9);
        assert_eq!(rect(2, 2, 0, 0, false).len(), 8);
    }

    #[test]
    fn ellipse_symmetric() {
        let pts = ellipse(5, 5, 3, 2, false);
        for (x, y) in &pts {
            assert!(pts.contains(&(10 - x, *y)));
            assert!(pts.contains(&(*x, 10 - y)));
        }
        assert!(pts.contains(&(2, 5)) && pts.contains(&(8, 5)));
        assert!(pts.contains(&(5, 3)) && pts.contains(&(5, 7)));
        assert!(!pts.contains(&(5, 5)));
        let filled = ellipse(5, 5, 3, 2, true);
        assert!(filled.contains(&(5, 5)) && filled.len() > pts.len());
    }

    #[test]
    fn fill_bounded_region() {
        let mut d = Document::new(5, 5, Document::builtin_palette("mono").unwrap()).unwrap();
        let ring = rect(1, 1, 3, 3, false);
        d.set_points(0, 0, &ring, b'0');
        assert_eq!(fill(&mut d, 0, 0, 2, 2, b'1'), 1);
        assert_eq!(fill(&mut d, 0, 0, 0, 0, b'1'), 16);
        assert_eq!(d.get(0, 0, 1, 1), b'0');
    }

    #[test]
    fn wrapped_fill_crosses_the_edges() {
        let mut d = Document::new(6, 4, Document::builtin_palette("mono").unwrap()).unwrap();
        // a wall in column 2..3 splits the cel; wrapped, column 0 and 5 touch
        d.set_points(0, 0, &rect(2, 0, 3, 3, true), b'0');
        assert_eq!(flood_region(&d, 0, 0, 0, 0).len(), 8);
        assert_eq!(flood_region_wrapped(&d, 0, 0, 0, 0, true, false).len(), 16);
        assert_eq!(flood_region_wrapped(&d, 0, 0, 0, 0, false, true).len(), 8);
        let pts = flood_region_wrapped(&d, 0, 0, 5, 3, true, true);
        assert!(pts.contains(&(0, 0)) && pts.contains(&(4, 1)) && !pts.contains(&(2, 1)));
    }
}
