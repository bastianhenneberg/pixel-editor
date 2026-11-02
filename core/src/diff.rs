//! Compact, verifiable descriptions of what changed between two documents:
//! per cel the number of changed pixels, their bounding box and the rows of
//! that box before and after; plus a structural summary for snapshots.

use crate::document::{Document, Rect};

#[derive(Clone, Debug, PartialEq)]
pub struct CelDiff {
    pub layer: usize,
    pub frame: usize,
    pub count: usize,
    pub bbox: Option<Rect>,
}

/// Pixel differences of one cel (both documents must have that cel).
pub fn cel_diff(before: &Document, after: &Document, layer: usize, frame: usize) -> CelDiff {
    let w = after.width as usize;
    let (a, b) = (before.cel(layer, frame), after.cel(layer, frame));
    let mut count = 0;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, (p, q)) in a.iter().zip(b.iter()).enumerate() {
        if p != q {
            count += 1;
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    let bbox = (count > 0).then(|| Rect::new(x0 as i32, y0 as i32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32));
    CelDiff { layer, frame, count, bbox }
}

/// Cels whose pixels differ. Documents must have the same layer/frame layout.
pub fn changed_cels(before: &Document, after: &Document) -> Vec<CelDiff> {
    let mut out = Vec::new();
    for l in 0..after.layers.len().min(before.layers.len()) {
        for f in 0..after.frames.len().min(before.frames.len()) {
            if std::sync::Arc::ptr_eq(before.layers[l].cel(f), after.layers[l].cel(f)) {
                continue;
            }
            let d = cel_diff(before, after, l, f);
            if d.count > 0 {
                out.push(d);
            }
        }
    }
    out
}

/// Rows of keys inside `r` with absolute rulers, as used by the MCP tools.
pub fn rows_block(key: impl Fn(i32, i32) -> u8, r: Rect) -> String {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w as i32, r.y + r.h as i32);
    let label_w = (y1 - 1).max(0).to_string().len();
    let pad = " ".repeat(label_w + 1);
    let mut s = String::new();
    let digits = (x1 - 1).max(0).to_string().len();
    for place in (0..digits).rev() {
        let p = 10i32.pow(place as u32);
        let line: String =
            (x0..x1).map(|x| if place > 0 && x < p { ' ' } else { char::from(b'0' + (x / p % 10) as u8) }).collect();
        s += &format!("{pad}{line}\n");
    }
    for y in y0..y1 {
        let row: String = (x0..x1).map(|x| key(x, y) as char).collect();
        s += &format!("{y:>label_w$} {row}\n");
    }
    s
}

/// "N pixels changed in <cel>, x a..b y c..d" plus before/after rows of the
/// changed area (`max_rows` limits the output for big changes).
pub fn describe_cel_diff(before: &Document, after: &Document, d: &CelDiff, max_rows: u32) -> String {
    let head = format!(
        "{} pixels changed in layer {} {:?}, frame {}",
        d.count,
        d.layer,
        after.layers[d.layer].name,
        after.frame_label_long(d.frame)
    );
    let Some(bb) = d.bbox else { return format!("{head}: nothing changed") };
    let mut s = format!("{head}, {}\n", bb.describe());
    if bb.h > max_rows {
        s += &format!("(area has {} rows; use get_rows to read it)\n", bb.h);
        return s;
    }
    let (l, f) = (d.layer, d.frame);
    s += "before:\n";
    s += &rows_block(|x, y| before.get(l, f, x, y), bb);
    s += "after:\n";
    s += &rows_block(|x, y| after.get(l, f, x, y), bb);
    s
}

/// Human summary of all differences between two documents (any layout):
/// size, palette, layers/frames/tags/regions, then pixel changes per cel
/// for layers matched by name and frames matched by index.
pub fn summary(old: &Document, new: &Document) -> Vec<String> {
    let mut out = Vec::new();
    if (old.width, old.height) != (new.width, new.height) {
        out.push(format!("size {}x{} -> {}x{}", old.width, old.height, new.width, new.height));
        return out;
    }
    for p in &new.palette {
        match old.palette_entry(p.key) {
            None => out.push(format!("palette: added {} {}", p.label(), p.color)),
            Some(o) if o.color != p.color || o.name != p.name => {
                out.push(format!("palette: {} {} -> {} {}", o.label(), o.color, p.label(), p.color))
            }
            _ => {}
        }
    }
    for o in &old.palette {
        if new.palette_entry(o.key).is_none() {
            out.push(format!("palette: removed {}", o.label()));
        }
    }
    if old.ramps != new.ramps {
        let rs = |d: &Document| d.ramps.iter().map(|r| format!("{} {}", r.name, r.keys_str())).collect::<Vec<_>>();
        out.push(format!("ramps {:?} -> {:?}", rs(old), rs(new)));
    }
    let names = |d: &Document| d.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    if names(old) != names(new) {
        out.push(format!("layers {:?} -> {:?}", names(old), names(new)));
    }
    for l in &new.layers {
        if let Some(o) = old.layers.iter().find(|o| o.name == l.name) {
            if (o.visible, o.opacity, o.locked) != (l.visible, l.opacity, l.locked) {
                out.push(format!(
                    "layer {:?}: visible {} -> {}, opacity {:.2} -> {:.2}, locked {} -> {}",
                    l.name, o.visible, l.visible, o.opacity, l.opacity, o.locked, l.locked
                ));
            }
        }
    }
    if old.frames != new.frames {
        let fr = |d: &Document| d.frames.iter().map(|f| format!("{}:{}ms", f.name, f.duration_ms)).collect::<Vec<_>>();
        out.push(format!("frames [{}] -> [{}]", fr(old).join(" "), fr(new).join(" ")));
    }
    if old.tags != new.tags {
        let tg = |d: &Document| {
            d.tags.iter().map(|t| format!("{} {}..{} {}", t.name, t.from, t.to, t.direction.id())).collect::<Vec<_>>()
        };
        out.push(format!("tags [{}] -> [{}]", tg(old).join(", "), tg(new).join(", ")));
    }
    if old.regions != new.regions {
        let rg = |d: &Document| d.regions.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        out.push(format!("regions changed: {:?} -> {:?}", rg(old), rg(new)));
    }
    let w = new.width as usize;
    for (li, l) in new.layers.iter().enumerate() {
        let Some(oi) = old.layers.iter().position(|o| o.name == l.name) else { continue };
        for f in 0..new.frames.len().min(old.frames.len()) {
            let (a, b) = (old.cel(oi, f), new.cel(li, f));
            let changed: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
            if changed.is_empty() {
                continue;
            }
            let (x0, x1) = (changed.iter().map(|i| i % w).min().unwrap(), changed.iter().map(|i| i % w).max().unwrap());
            let (y0, y1) = (changed.iter().map(|i| i / w).min().unwrap(), changed.iter().map(|i| i / w).max().unwrap());
            let r = Rect::new(x0 as i32, y0 as i32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
            out.push(format!(
                "pixels: layer {:?} frame {}: {} changed, {}",
                l.name,
                new.frame_label(f),
                changed.len(),
                r.describe()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_counts_and_bbox() {
        let a = Document::new(6, 4, Document::builtin_palette("pico8").unwrap()).unwrap();
        let mut b = a.clone();
        b.set(0, 0, 2, 1, b'8');
        b.set(0, 0, 4, 2, b'8');
        let d = cel_diff(&a, &b, 0, 0);
        assert_eq!((d.count, d.bbox), (2, Some(Rect::new(2, 1, 3, 2))));
        assert_eq!(changed_cels(&a, &b), vec![d.clone()]);
        let text = describe_cel_diff(&a, &b, &d, 64);
        assert!(text.contains("before:\n  234\n1 ...\n2 ...\nafter:\n  234\n1 8..\n2 ..8\n"), "{text}");
        let s = summary(&a, &b);
        assert_eq!(s.len(), 1);
        assert!(s[0].contains("2 changed"), "{s:?}");
    }
}
