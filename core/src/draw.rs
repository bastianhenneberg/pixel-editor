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
}
