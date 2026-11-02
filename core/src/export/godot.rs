//! Godot 4 `SpriteFrames` text resource (`.tres`, format=3).
//!
//! Godot plays a frame for `duration / speed` seconds. Per animation we pick
//! a base duration (see `base_ms`): `speed = 1000 / base_ms` FPS and each
//! frame gets `duration = ms / base_ms`, which preserves the exact per-frame
//! milliseconds while keeping the numbers readable in the Godot editor.

use std::fmt::Write;

use super::{fnv64, Sheet};

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Base duration for an animation: the most common frame duration if it
/// divides all others, else their GCD (if >= 10 ms), so that speed and
/// durations stay short exact numbers.
fn base_ms(steps: &[(usize, u32)]) -> u32 {
    let mut best = (0usize, 100u32);
    for (_, ms) in steps {
        let count = steps.iter().filter(|(_, m)| m == ms).count();
        if count > best.0 || (count == best.0 && *ms < best.1) {
            best = (count, *ms);
        }
    }
    let common = best.1.max(1);
    if steps.iter().all(|(_, ms)| ms % common == 0) {
        return common;
    }
    let g = steps.iter().fold(0, |g, (_, ms)| gcd(g, *ms));
    if g >= 10 {
        g
    } else {
        common
    }
}

/// Godot string literal escaping.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn sprite_frames(sheet: &Sheet, png_name: &str, res_dir: Option<&str>) -> String {
    let path = match res_dir {
        Some(dir) => format!("{}/{png_name}", dir.trim_end_matches('/')),
        None => png_name.to_string(),
    };
    let tex_id = format!("1_{:05x}", fnv64(png_name) & 0xfffff);
    let mut out = String::new();
    writeln!(out, "[gd_resource type=\"SpriteFrames\" format=3]\n").unwrap();
    writeln!(out, "[ext_resource type=\"Texture2D\" path={} id=\"{tex_id}\"]\n", quote(&path)).unwrap();
    for c in &sheet.cells {
        writeln!(out, "[sub_resource type=\"AtlasTexture\" id=\"AtlasTexture_{}\"]", c.frame).unwrap();
        writeln!(out, "atlas = ExtResource(\"{tex_id}\")").unwrap();
        writeln!(out, "region = Rect2({}, {}, {}, {})\n", c.x, c.y, c.w, c.h).unwrap();
    }
    let anims: Vec<String> = sheet
        .anims
        .iter()
        .map(|a| {
            let base = base_ms(&a.steps);
            let frames: Vec<String> = a
                .steps
                .iter()
                .map(|(f, ms)| {
                    format!(
                        "{{\n\"duration\": {:?},\n\"texture\": SubResource(\"AtlasTexture_{f}\")\n}}",
                        *ms as f64 / base as f64
                    )
                })
                .collect();
            format!(
                "{{\n\"frames\": [{}],\n\"loop\": {},\n\"name\": &{},\n\"speed\": {:?}\n}}",
                frames.join(", "),
                a.looping,
                quote(&a.name),
                1000.0 / base as f64
            )
        })
        .collect();
    writeln!(out, "[resource]\nanimations = [{}]", anims.join(", ")).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{build_sheet, tests::sample};

    #[test]
    fn tres_structure() {
        let s = build_sheet(&sample(), 1, None);
        let t = sprite_frames(&s, "hero.png", Some("res://art/"));
        assert!(t.starts_with("[gd_resource type=\"SpriteFrames\" format=3]"));
        assert!(t.contains("path=\"res://art/hero.png\""));
        assert!(t.contains("region = Rect2(8, 0, 4, 4)"));
        assert!(t.contains("\"name\": &\"idle\""));
        assert!(t.contains("\"speed\": 10.0"));
        // hit: 100,100,250,100 -> base gcd 50 ms = 20 FPS, the 250 ms frame lasts 5 units
        assert!(t.contains("\"speed\": 20.0"));
        assert!(t.contains("\"duration\": 5.0"));
        assert!(t.contains("\"loop\": false"));
    }

    #[test]
    fn base_duration() {
        assert_eq!(base_ms(&[(0, 120), (1, 120), (2, 240)]), 120);
        assert_eq!(base_ms(&[(0, 200), (1, 100)]), 100);
        assert_eq!(base_ms(&[(0, 120), (1, 200)]), 40);
        assert_eq!(base_ms(&[(0, 101), (1, 103)]), 101);
    }
}
