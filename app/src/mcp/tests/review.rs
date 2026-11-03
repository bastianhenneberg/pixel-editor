//! Tests of the review tools (preview views, lint, render_animation trace)
//! and the parts library.

use super::*;

#[test]
fn preview_views_side_by_side_and_errors() {
    let (s, dir) = server("views");
    ok(&s, "create_document", json!({"name": "a", "width": 6, "height": 6}));
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 0, "x": 0, "y": 0, "rows": ["22", "2"]}));
    let plain = ok(&s, "render_preview", json!({"path": "a", "frame": 0, "inline": false}));
    let one = ok(&s, "render_preview", json!({"path": "a", "frame": 0, "view": "silhouette", "inline": false}));
    assert!(one.contains("preview/a_f0_silhouette.png") && one.contains("preview only"), "{one}");
    let all = ok(
        &s,
        "render_preview",
        json!({"path": "a", "frame": 0, "view": ["normal", "silhouette", "grayscale", "mirrored"], "inline": false}),
    );
    assert!(all.contains("a_f0_normal_silhouette_grayscale_mirrored.png"), "{all}");
    let width = |t: &str| -> u32 {
        let i = t.find(" px").unwrap();
        t[..i].rsplit('(').next().unwrap().split('x').next().unwrap().parse().unwrap()
    };
    // auto zoom 32: four 6 px frames side by side are wider than the single preview
    assert!(width(&all) >= 4 * 6 * 32 && width(&plain) < 4 * 6 * 32, "four cells side by side: {all} / {plain}");
    assert!(err(&s, "render_preview", json!({"path": "a", "view": "sepia"})).contains("unknown view \"sepia\""));
    // the document is untouched
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[0], "22....");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lint_reports_line_problems_and_passes_clean_shapes() {
    let (s, dir) = server("lint");
    ok(&s, "create_document", json!({"name": "a", "width": 16, "height": 10}));
    // frame 0: a filled rectangle and a clean 2:1 line
    ok(
        &s,
        "draw_rect",
        json!({"path": "a", "layer": 0, "frame": 0, "x": 0, "y": 0, "w": 5, "h": 4, "key": "2", "filled": true}),
    );
    ok(
        &s,
        "set_rows",
        json!({"path": "a", "layer": 0, "frame": 0, "x": 6, "y": 0, "rows": [
        "88", "..88", "....88", "......88", "........88"]}),
    );
    // frame 1: a line with an uneven step (2-2-1-3-2-2), a double and an orphan
    ok(&s, "add_frame", json!({"path": "a"}));
    ok(
        &s,
        "set_rows",
        json!({"path": "a", "layer": 0, "frame": 1, "x": 0, "y": 0, "rows": [
        "88", "..88", "....8", ".....888", "........88", "..........88"]}),
    );
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 1, "x": 12, "y": 7, "rows": ["c...", "cccc"]}));
    ok(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 1, "pixels": [{"x": 15, "y": 0, "key": "5"}]}));
    let t = ok(&s, "lint", json!({"path": "a", "frame": 0}));
    assert!(
        t.starts_with("lint a.pxe.json frame f0, composite: 0 orphans, 0 doubles, 0 jaggy lines, ramps: none defined"),
        "{t}"
    );
    assert!(t.contains("clean: f0"), "{t}");
    let t = ok(&s, "lint", json!({"path": "a"}));
    assert!(t.lines().next().unwrap().contains("all frames, composite: 1 orphans, 1 doubles, 1 jaggy lines"), "{t}");
    assert!(t.contains("frame 1 \"f1\""), "{t}");
    assert!(t.contains("runs along x 2-2-1-3-2-2"), "{t}");
    assert!(t.contains("(12,8)"), "the hook at the start of the c line: {t}");
    assert!(t.contains("orphans (no neighbour of the same key, diagonal steps aside): (15,0)"), "{t}");
    // checks and areas narrow it down
    let t = ok(&s, "lint", json!({"path": "a", "frame": 1, "checks": ["orphans"]}));
    assert!(t.lines().next().unwrap().ends_with(": 1 orphans"), "{t}");
    let t = ok(
        &s,
        "lint",
        json!({"path": "a", "frame": 1, "rect": {"x": 12, "y": 0, "w": 4, "h": 10}, "checks": ["jaggies", "doubles"]}),
    );
    assert!(t.lines().next().unwrap().ends_with("in x 12..15 y 0..9 (4x10): 0 jaggy lines, 1 doubles"), "{t}");
    // ramps: the line color is outside the ramp
    ok(&s, "set_ramps", json!({"path": "a", "ramps": [{"name": "red", "keys": "25"}]}));
    let t = ok(&s, "lint", json!({"path": "a", "frame": 1, "checks": ["ramps"]}));
    assert!(t.contains("2 off-ramp colors") && t.contains("colors in no ramp: 8"), "{t}");
    assert!(err(&s, "lint", json!({"path": "a", "checks": ["banding"]})).contains("unknown check"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn render_animation_traces_markers_and_keys() {
    let (s, dir) = server("trace");
    ok(&s, "create_document", json!({"name": "a", "width": 12, "height": 12}));
    // a hand pixel (key 4) that swings over the top and back
    let pts = [(2, 6), (4, 4), (6, 3), (8, 4), (10, 6), (8, 4), (6, 3), (4, 4)];
    for (i, (x, y)) in pts.iter().enumerate() {
        if i > 0 {
            ok(&s, "add_frame", json!({"path": "a"}));
        }
        ok(
            &s,
            "set_pixels",
            json!({"path": "a", "layer": 0, "frame": i, "pixels": [{"x": x, "y": y, "key": "4"}, {"x": 0, "y": 11, "key": "2"}]}),
        );
    }
    ok(&s, "set_tag", json!({"path": "a", "name": "swing", "from": 0, "to": 7}));
    let t = ok(&s, "render_animation", json!({"path": "a", "tag": "swing", "trace": [{"key": "4"}], "inline": false}));
    assert!(t.contains("trace key 4") && t.contains("1:(4,4) +2,-2"), "{t}");
    assert!(!t.contains("reverses direction") && !t.contains("back and forth"), "a clean swing: {t}");
    // a marker that wobbles: one step back in x
    ok(&s, "set_region", json!({"path": "a", "name": "hand", "kind": "marker", "x": 1, "y": 1}));
    for (f, x) in [(1, 3), (2, 5), (3, 3), (4, 5), (5, 7), (6, 9), (7, 11)] {
        ok(&s, "set_region", json!({"path": "a", "name": "hand", "kind": "marker", "x": x, "y": 1, "frame": f}));
    }
    let t = ok(
        &s,
        "render_animation",
        json!({"path": "a", "tag": "swing", "trace": ["hand"], "trace_threshold": 4, "inline": false}),
    );
    assert!(t.contains("trace hand: 0:(1,1) -10,+0 | 1:(3,1) +2,+0"), "{t}");
    assert!(t.contains("hand: step 0 moves (-10, 0), more than 4 px"), "{t}");
    assert!(t.contains("hand: x goes back and forth at step(s) [0, 3]"), "{t}");
    assert!(t.contains("hand: reverses direction 4 times per loop"), "{t}");
    assert!(t.contains("Traces: each cell shows the whole path dotted"), "{t}");
    let e = err(&s, "render_animation", json!({"path": "a", "tag": "swing", "trace": ["nose"]}));
    assert!(e.contains("trace[0]") && e.contains("no region \"nose\""), "{e}");
    assert!(err(&s, "render_animation", json!({"path": "a", "tag": "swing", "trace": [{"key": "."}]}))
        .contains("transparent"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn parts_round_trip_with_key_remapping_locks_and_dry_run() {
    let (mut s, dir) = server("parts");
    let personal = dir.join("personal-parts");
    s.personal_parts = Some(personal.clone());
    // source: sweetie16; a 3x2 sword with a transparent corner
    ok(&s, "create_document", json!({"name": "src", "width": 8, "height": 8}));
    ok(&s, "set_rows", json!({"path": "src", "layer": 0, "frame": 0, "x": 2, "y": 3, "rows": ["c9.", "999"]}));
    ok(&s, "set_region", json!({"path": "src", "name": "sword", "x": 2, "y": 3, "w": 3, "h": 2}));
    let t = ok(&s, "save_part", json!({"path": "src", "layer": 0, "frame": 0, "region": "sword", "name": "sword_0"}));
    assert!(t.contains("saved part \"sword_0\" (3x2") && t.contains(".pxe-parts/sword_0.json"), "{t}");
    assert!(dir.join(".pxe-parts/sword_0.json").exists());
    assert!(err(&s, "save_part", json!({"path": "src", "layer": 0, "frame": 0, "region": "sword", "name": "sword_0"}))
        .contains("already exists"));
    assert!(err(&s, "save_part", json!({"path": "src", "layer": 0, "frame": 0, "region": "sword", "name": "Sword 0"}))
        .contains("invalid part name"));
    ok(
        &s,
        "save_part",
        json!({"path": "src", "layer": 0, "frame": 0, "rect": {"x": 2, "y": 3, "w": 2, "h": 1}, "name": "hilt", "library": "personal"}),
    );
    assert!(personal.join("hilt.json").exists());
    let list = ok(&s, "list_parts", json!({}));
    assert!(
        list.contains("project library") && list.contains("- sword_0 3x2, keys 9c") && list.contains("    c9."),
        "{list}"
    );
    assert!(list.contains("personal library") && list.contains("- hilt 2x1"), "{list}");
    // target: the same colors under other keys, and one color missing
    let sweetie = Document::builtin_palette("sweetie16").unwrap();
    let color = |k: u8| sweetie.iter().find(|e| e.key == k).unwrap().color.to_hex();
    ok(
        &s,
        "create_document",
        json!({"name": "dst", "width": 8, "height": 8, "palette": [
        {"key": "a", "color": color(b'9')}, {"key": "b", "color": color(b'c')}, {"key": "z", "color": "#123456"}]}),
    );
    ok(&s, "set_rows", json!({"path": "dst", "layer": 0, "frame": 0, "x": 0, "y": 0, "rows": ["zzzz", "zzzz"]}));
    let dry = ok(
        &s,
        "place_part",
        json!({"path": "dst", "layer": 0, "frame": 0, "name": "sword_0", "x": 0, "y": 0, "dry_run": true}),
    );
    assert!(
        dry.contains("DRY RUN")
            && dry.contains("colors mapped by value")
            && dry.contains("'9'")
            && dry.contains("-> 'a'"),
        "{dry}"
    );
    assert_eq!(load(&dir, "dst.pxe.json").rows(0, 0)[0], "zzzz....", "dry run writes nothing");
    let t = ok(&s, "place_part", json!({"path": "dst", "layer": 0, "frame": 0, "name": "sword_0", "x": 0, "y": 0}));
    assert!(t.contains("recorded as \"Claude: place_part sword_0 f0 Ebene 1 at (0, 0)\""), "{t}");
    let d = load(&dir, "dst.pxe.json");
    assert_eq!(d.rows(0, 0)[..2], ["bazz....".to_string(), "aaaz....".to_string()], "over keeps the cel under '.'");
    // replace + flip, into two frames
    ok(&s, "add_frame", json!({"path": "dst", "copy_from": 0}));
    ok(
        &s,
        "place_part",
        json!({"path": "dst", "layer": 0, "frames": [0, 1], "name": "sword_0", "x": 4, "y": 0, "mode": "replace", "flip_h": true}),
    );
    let d = load(&dir, "dst.pxe.json");
    for f in 0..2 {
        assert_eq!(d.rows(0, f)[..2], ["bazz.ab.".to_string(), "aaazaaa.".to_string()], "frame {f}");
    }
    let u = ok(&s, "undo", json!({"path": "dst"}));
    assert!(u.contains("restored the state after") && u.contains("frame f1: 5 changed"), "{u}");
    assert_eq!(load(&dir, "dst.pxe.json").rows(0, 1)[0], "bazz....", "undo reverts both frames");
    // a palette without the color: error, nothing written; an explicit map helps
    ok(
        &s,
        "create_document",
        json!({"name": "other", "width": 4, "height": 4, "palette": [{"key": "a", "color": color(b'9')}]}),
    );
    let e = err(&s, "place_part", json!({"path": "other", "layer": 0, "frame": 0, "name": "sword_0", "x": 0, "y": 0}));
    assert!(e.contains(&format!("'c' {}", color(b'c'))) && e.contains("nothing was written"), "{e}");
    ok(
        &s,
        "place_part",
        json!({"path": "other", "layer": 0, "frame": 0, "name": "sword_0", "x": 0, "y": 0, "map": {"c": "a"}}),
    );
    assert_eq!(load(&dir, "other.pxe.json").rows(0, 0)[0], "aa..");
    // locks refuse
    ok(&s, "set_layer", json!({"path": "dst", "layer": 0, "locked": true}));
    let e = err(&s, "place_part", json!({"path": "dst", "layer": 0, "frame": 0, "name": "hilt", "x": 0, "y": 4}));
    assert!(e.contains("is locked") && e.contains("nothing was written"), "{e}");
    ok(&s, "set_layer", json!({"path": "dst", "layer": 0, "locked": false}));
    ok(&s, "set_region", json!({"path": "dst", "name": "face", "x": 0, "y": 4, "w": 2, "h": 2, "locked": true}));
    let e = err(&s, "place_part", json!({"path": "dst", "layer": 0, "frame": 0, "name": "hilt", "x": 0, "y": 4}));
    assert!(e.contains("locked region \"face\""), "{e}");
    assert!(err(&s, "place_part", json!({"path": "dst", "layer": 0, "frame": 0, "name": "axe", "x": 0, "y": 0}))
        .contains("no part \"axe\" in the project or personal libraries"));
    std::fs::remove_dir_all(&dir).unwrap();
}
