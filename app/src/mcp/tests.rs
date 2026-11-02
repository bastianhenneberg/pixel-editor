use super::*;

fn call(s: &Server, name: &str, args: Value) -> (bool, String) {
    let r = s
        .handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}),
        )
        .unwrap();
    let res = &r["result"];
    (res["isError"] == false, res["content"][0]["text"].as_str().unwrap_or("").to_string())
}

fn ok(s: &Server, name: &str, args: Value) -> String {
    let (ok, text) = call(s, name, args);
    assert!(ok, "{name} failed: {text}");
    text
}

fn err(s: &Server, name: &str, args: Value) -> String {
    let (ok, text) = call(s, name, args);
    assert!(!ok, "{name} should fail: {text}");
    text
}

/// A server after `initialize` from Claude Code (author "Claude").
fn claude_server(dir: &Path) -> Server {
    let s = Server::new(dir.canonicalize().unwrap());
    s.handle(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"clientInfo": {"name": "claude-code", "version": "2.1"}}}));
    s
}

fn server(tag: &str) -> (Server, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pxe-mcp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (claude_server(&dir), dir)
}

fn load(dir: &Path, name: &str) -> Document {
    io::load(&dir.join(name)).unwrap()
}

#[test]
fn protocol_and_tools() {
    let (s, dir) = server("proto");
    let init = s
        .handle(
            &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
        )
        .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    let instructions = init["result"]["instructions"].as_str().unwrap();
    for part in [
        ".pxe-instructions.md",
        "linked cels",
        "pace_ms",
        "get_user_view",
        "CONSISTENCY RULES: every pixel write needs an explicit target",
        "mirror_document(src, dst)",
        "transform (op flip_h",
        "tiled: true",
        "check_seams",
        "set_ramps",
        "shade(pixels|rect|region",
        "move_frames(frames, to)",
        "set_reference",
        "never exported",
    ] {
        assert!(instructions.contains(part), "instructions lack {part:?}: {instructions}");
    }
    for game in ["48x48", "48x36", "slime", "104x56", "toast_warrior", "## Project instructions"] {
        assert!(!instructions.contains(game), "built-in instructions name {game:?}: {instructions}");
    }
    assert!(instructions.contains("the AI assistant (you;"), "{instructions}");
    assert_eq!(instructions.matches("Claude").count(), 1, "only as an example author name");
    assert!(instructions.contains("'KI zeigen'"), "{instructions}");
    assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    let tools = s.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for required in [
        "list_documents",
        "create_document",
        "get_document",
        "get_rows",
        "set_pixels",
        "set_rows",
        "draw_line",
        "draw_rect",
        "draw_ellipse",
        "fill",
        "add_layer",
        "add_frame",
        "set_frame_duration",
        "set_tag",
        "set_palette_color",
        "render_preview",
        "export",
        "link_cel",
        "save_as",
        "list_regions",
        "set_region",
        "delete_region",
        "copy_region",
        "move_region",
        "history",
        "undo",
        "snapshot",
        "list_snapshots",
        "diff_snapshot",
        "restore_snapshot",
        "get_user_view",
        "transform",
        "mirror_document",
        "check_seams",
        "set_ramps",
        "shade",
        "replace_color",
        "outline",
        "move_frames",
        "set_reference",
        "set_layer",
    ] {
        assert!(names.contains(&required), "missing tool {required}");
    }
    ok(&s, "create_document", json!({"name": "hero", "width": 8, "height": 8}));
    assert!(err(&s, "create_document", json!({"name": "hero", "width": 8, "height": 8})).contains("already exists"));
    ok(&s, "set_rows", json!({"path": "hero", "layer": 0, "frame": 0, "x": 1, "y": 1, "rows": ["22", "-2"]}));
    ok(&s, "add_frame", json!({"path": "hero", "copy_from": 0, "duration_ms": 250}));
    ok(
        &s,
        "draw_line",
        json!({"path": "hero", "layer": 0, "frame": "f1", "x0": 0, "y0": 7, "x1": 7, "y1": 7, "key": "9"}),
    );
    let doc = load(&dir, "hero.pxe.json");
    assert_eq!(doc.rows(0, 0)[1], ".22.....");
    assert_eq!(doc.rows(0, 1)[7], "99999999");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ramps_and_shading() {
    let (s, dir) = server("shade");
    ok(&s, "create_document", json!({"name": "toast", "width": 6, "height": 2}));
    ok(&s, "set_palette_color", json!({"path": "toast", "key": "3", "name": "crust_dark"}));
    let e = err(
        &s,
        "shade",
        json!({"path": "toast", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 1, "h": 1}, "steps": 1}),
    );
    assert!(e.contains("set_ramps first"), "{e}");
    // keys as a string or as names; checked against the palette
    let e = err(&s, "set_ramps", json!({"path": "toast", "ramps": [{"name": "crust", "keys": "38Z"}]}));
    assert!(e.contains("ramps[0]") && e.contains("valid keys"), "{e}");
    let t = ok(
        &s,
        "set_ramps",
        json!({"path": "toast", "ramps": [{"name": "crust", "keys": ["crust_dark", "8", "9", "a"]}, {"name": "leaf", "keys": "56"}]}),
    );
    assert!(
        t.contains("crust: 3 crust_dark > 8 > 9 > a") && t.contains("ramps [] -> [\"crust 389a\", \"leaf 56\"]"),
        "{t}"
    );
    let t = ok(&s, "get_document", json!({"path": "toast"}));
    assert!(t.contains("color ramps (dark > light") && t.contains("leaf: 5 > 6"), "{t}");
    ok(&s, "set_rows", json!({"path": "toast", "layer": 0, "frame": 0, "rows": ["3389a1", "5.6..."]}));
    // a rect: every ramp pixel one step lighter, 1 and '.' stay, a is at the end
    let t = ok(
        &s,
        "shade",
        json!({"path": "toast", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 6, "h": 2}, "steps": 1, "dry_run": true}),
    );
    assert!(t.starts_with("DRY RUN") && t.contains("5 pixels changed"), "{t}");
    let t = ok(
        &s,
        "shade",
        json!({"path": "toast", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 6, "h": 2}, "steps": 1}),
    );
    assert!(t.contains("5 pixels changed") && t.contains("not in any ramp"), "{t}");
    assert_eq!(load(&dir, "toast.pxe.json").rows(0, 0), vec!["889aa1", "6.6..."]);
    // single pixels, darker; a pixel listed twice changes once
    let t = ok(
        &s,
        "shade",
        json!({"path": "toast", "layer": 0, "frame": 0, "steps": -2, "pixels": [{"x": 3, "y": 0}, {"x": 3, "y": 0}, {"x": 5, "y": 0}]}),
    );
    assert!(t.contains("1 pixels changed") && t.contains("Claude: shade f0 Ebene 1"), "{t}");
    assert_eq!(load(&dir, "toast.pxe.json").rows(0, 0)[0], "8898a1");
    assert!(err(&s, "shade", json!({"path": "toast", "layer": 0, "frame": 0, "steps": 1})).contains("give pixels"));
    assert!(err(&s, "shade", json!({"path": "toast", "layer": 0, "frame": 0, "steps": 0, "pixels": []}))
        .contains("not be 0"));
    // undo takes back the last shade; removing a palette color shortens its ramp
    ok(&s, "undo", json!({"path": "toast"}));
    assert_eq!(load(&dir, "toast.pxe.json").rows(0, 0)[0], "889aa1");
    ok(&s, "set_palette_color", json!({"path": "toast", "key": "6", "remove": true}));
    assert_eq!(load(&dir, "toast.pxe.json").ramps.len(), 1, "leaf lost a color and is gone");
    // .aseprite has no place for ramps
    let t = ok(&s, "save_as", json!({"path": "toast", "to": "toast.aseprite"}));
    assert!(t.contains("1 color ramp(s) are not stored"), "{t}");
    let e = err(&s, "set_ramps", json!({"path": "toast.aseprite", "ramps": []}));
    assert!(e.contains(".pxe.json only"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn writes_need_an_explicit_target_and_report_precise_errors() {
    let (s, dir) = server("errors");
    ok(&s, "create_document", json!({"name": "a", "width": 4, "height": 4}));
    let e = err(&s, "set_pixels", json!({"path": "a", "pixels": [{"x": 0, "y": 0, "key": "1"}]}));
    assert!(e.contains("missing required argument \"layer\""), "{e}");
    let e = err(
        &s,
        "set_pixels",
        json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 0, "y": 0, "key": "1"}, {"x": 1, "y": 0, "key": "Z"}]}),
    );
    assert!(e.contains("pixels[1]") && e.contains("valid keys"), "{e}");
    let e = err(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 0, "x": 1, "y": 2, "rows": ["11", "1Z"]}));
    assert!(e.contains("rows[1][1] (x=2, y=3)"), "{e}");
    let e = err(
        &s,
        "draw_rect",
        json!({"path": "a", "layer": 0, "frame": 0, "x": 0, "y": 0, "w": 2, "h": 2, "key": "1", "colour": "1"}),
    );
    assert!(e.contains("unknown argument \"colour\"") && e.contains("filled"), "{e}");
    let e = err(&s, "get_document", json!({"path": "a", "layer": "nope"}));
    assert!(e.contains("0 \"Ebene 1\""), "{e}");
    assert!(err(&s, "get_document", json!({"path": "../outside"})).contains("outside the workdir"));
    assert!(err(&s, "get_document", json!({"path": "missing"})).contains("does not exist"));
    let e = err(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": "walk/0", "pixels": []}));
    assert!(e.contains("no tag \"walk\""), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scoped_writes_diff_and_dry_run() {
    let (s, dir) = server("scope");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 6}));
    ok(&s, "set_region", json!({"path": "a", "name": "left_shoe", "x": 1, "y": 4, "w": 3, "h": 2}));
    // a filled rect over the whole canvas, limited to the region
    let t = ok(
        &s,
        "draw_rect",
        json!({"path": "a", "layer": 0, "frame": 0, "region": "left_shoe", "x": 0, "y": 0, "w": 8, "h": 6, "key": "2", "filled": true}),
    );
    assert!(t.contains("6 pixels changed") && t.contains("x 1..3 y 4..5"), "{t}");
    assert!(t.contains("42 pixels outside the target"), "{t}");
    assert!(t.contains("before:\n  123\n4 ...\n5 ...\nafter:\n  123\n4 222\n5 222\n"), "{t}");
    assert!(t.contains("Claude: draw_rect f0 Ebene 1 [left_shoe] (3x2)"), "{t}");
    let d = load(&dir, "a.pxe.json");
    assert_eq!(d.rows(0, 0), vec!["........", "........", "........", "........", ".222....", ".222...."]);
    // clip rect
    ok(
        &s,
        "fill",
        json!({"path": "a", "layer": 0, "frame": 0, "clip": {"x": 0, "y": 0, "w": 2, "h": 2}, "x": 0, "y": 0, "key": "8"}),
    );
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[0], "88......", "fill did not leak out of the clip");
    // "rect" works like "clip"
    let t = ok(
        &s,
        "clear",
        json!({"path": "a", "layer": 0, "frame": 0, "rect": {"x": 1, "y": 0, "w": 1, "h": 1}, "dry_run": true}),
    );
    assert!(t.contains("1 pixels changed") && t.contains("x 1..1 y 0..0"), "{t}");
    assert!(err(
        &s,
        "clear",
        json!({"path": "a", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 1, "h": 1}, "region": "left_shoe"})
    )
    .contains("not both"));
    let t = ok(&s, "get_rows", json!({"path": "a", "rect": {"x": 0, "y": 0, "w": 2, "h": 1}}));
    assert!(t.contains("0 88"), "{t}");
    // dry run writes nothing
    let t = ok(
        &s,
        "set_rows",
        json!({"path": "a", "layer": 0, "frame": 0, "x": 5, "y": 0, "rows": ["99"], "dry_run": true}),
    );
    assert!(t.starts_with("DRY RUN") && t.contains("2 pixels changed"), "{t}");
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[0], "88......");
    // set_rows defaults to the region's corner, rows given in region coordinates
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 0, "region": "left_shoe", "rows": ["3-3"]}));
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[4], ".323....");
    // palette names work as keys
    ok(&s, "set_palette_color", json!({"path": "a", "key": "9", "name": "harness"}));
    ok(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 7, "y": 0, "key": "harness"}]}));
    assert_eq!(load(&dir, "a.pxe.json").get(0, 0, 7, 0), b'9');
    let t = ok(&s, "get_rows", json!({"path": "a", "region": "left_shoe"}));
    assert!(t.contains("keys used: 2, 3"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn locks_and_linked_cels_refuse_writes() {
    let (s, dir) = server("locks");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 8}));
    let px = json!([{"x": 1, "y": 1, "key": "1"}]);
    ok(&s, "set_layer", json!({"path": "a", "layer": 0, "locked": true}));
    let e = err(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 0, "pixels": px}));
    assert!(e.contains("layer 0 \"Ebene 1\" is locked") && e.contains("nothing was written"), "{e}");
    assert!(err(&s, "set_layer", json!({"path": "a", "layer": 0, "name": "x"})).contains("locked"));
    ok(&s, "set_layer", json!({"path": "a", "layer": 0, "locked": false}));
    ok(&s, "set_frame", json!({"path": "a", "frame": 0, "locked": true}));
    assert!(err(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 0, "pixels": px}))
        .contains("frame 0 \"f0\" is locked"));
    ok(&s, "add_frame", json!({"path": "a"}));
    assert!(err(&s, "delete_frame", json!({"path": "a", "frame": 0})).contains("is locked"));
    ok(&s, "delete_frame", json!({"path": "a", "frame": 1}));
    ok(&s, "set_frame", json!({"path": "a", "frame": 0, "locked": false}));
    ok(&s, "set_region", json!({"path": "a", "name": "face", "x": 0, "y": 0, "w": 4, "h": 4, "locked": true}));
    let e = err(
        &s,
        "draw_line",
        json!({"path": "a", "layer": 0, "frame": 0, "x0": 0, "y0": 2, "x1": 7, "y1": 2, "key": "1"}),
    );
    assert!(e.contains("locked region \"face\""), "{e}");
    assert!(load(&dir, "a.pxe.json").cel_is_empty(0, 0), "nothing written");
    // staying outside the locked region works
    ok(
        &s,
        "draw_line",
        json!({"path": "a", "layer": 0, "frame": 0, "clip": {"x": 4, "y": 0, "w": 4, "h": 8}, "x0": 0, "y0": 2, "x1": 7, "y1": 2, "key": "1"}),
    );
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[2], "....1111");
    assert!(err(&s, "delete_region", json!({"path": "a", "name": "face"})).contains("locked"));
    // linked cels need edit_linked
    ok(&s, "add_frame", json!({"path": "a", "copy_from": 0, "linked": true}));
    let e =
        err(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 1, "pixels": [{"x": 7, "y": 7, "key": "2"}]}));
    assert!(e.contains("linked with frames [0, 1]") && e.contains("edit_linked"), "{e}");
    let t = ok(
        &s,
        "set_pixels",
        json!({"path": "a", "layer": 0, "frame": 1, "edit_linked": true, "pixels": [{"x": 7, "y": 7, "key": "2"}]}),
    );
    assert!(t.contains("linked cel: frames [0, 1]"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn move_and_copy_regions() {
    let (s, dir) = server("move");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 8}));
    ok(&s, "add_frame", json!({"path": "a", "copy_from": 0}));
    ok(&s, "set_tag", json!({"path": "a", "name": "walk_down", "from": 0, "to": 1}));
    ok(
        &s,
        "set_rows",
        json!({"path": "a", "layer": 0, "frame": "walk_down/0", "x": 0, "y": 5, "rows": ["44444444", "4..44..4", "22....33"]}),
    );
    ok(&s, "set_region", json!({"path": "a", "name": "left_shoe", "x": 0, "y": 7, "w": 2, "h": 1}));
    // lift the left shoe 1px on frame 0 only
    let t = ok(
        &s,
        "move_region",
        json!({"path": "a", "layer": 0, "frame": "walk_down/0", "region": "left_shoe", "dy": -1}),
    );
    assert!(t.contains("Claude: move_region walk_down/0 Ebene 1 [left_shoe]"), "{t}");
    let d = load(&dir, "a.pxe.json");
    assert_eq!(d.rows(0, 0)[6], "22.44..4");
    assert_eq!(d.rows(0, 0)[7], "......33");
    assert_eq!(d.regions[0].rect_at(0), Some(Rect::new(0, 6, 2, 1)), "region moved on frame 0");
    assert_eq!(d.regions[0].rect_at(1), Some(Rect::new(0, 7, 2, 1)), "other frames keep the rect");
    // copy the right shoe area into frame 1 at an offset
    ok(
        &s,
        "copy_region",
        json!({"path": "a", "layer": 0, "frame": 0, "clip": {"x": 6, "y": 7, "w": 2, "h": 1}, "to_frame": 1, "dx": -6}),
    );
    let d = load(&dir, "a.pxe.json");
    assert_eq!(d.rows(0, 1)[7], "33......");
    assert_eq!(d.rows(0, 0)[7], "......33", "copy leaves the source");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn history_undo_and_snapshots() {
    let (s, dir) = server("history");
    ok(&s, "create_document", json!({"name": "a", "width": 4, "height": 4}));
    ok(&s, "snapshot", json!({"path": "a", "name": "empty", "note": "leer"}));
    ok(
        &s,
        "set_pixels",
        json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 0, "y": 0, "key": "1"}], "note": "Claude: Auge links"}),
    );
    ok(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 3, "y": 0, "key": "1"}]}));
    let h = ok(&s, "history", json!({"path": "a"}));
    assert!(h.contains("Claude: Auge links") && h.contains("Claude: set_pixels f0 Ebene 1"), "{h}");
    let t = ok(&s, "undo", json!({"path": "a"}));
    assert!(t.contains("Auge links"), "{t}");
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[0], "1...");
    ok(&s, "undo", json!({"path": "a"}));
    assert!(load(&dir, "a.pxe.json").cel_is_empty(0, 0));
    ok(&s, "set_pixels", json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 1, "y": 1, "key": "2"}]}));
    let t = ok(&s, "diff_snapshot", json!({"path": "a", "name": "empty", "layer": 0, "frame": 0}));
    assert!(t.contains("1 changed") && t.contains("after:"), "{t}");
    assert!(ok(&s, "list_snapshots", json!({"path": "a"})).contains("\"empty\""));
    ok(&s, "restore_snapshot", json!({"path": "a", "name": "empty"}));
    assert!(load(&dir, "a.pxe.json").cel_is_empty(0, 0));
    ok(&s, "undo", json!({"path": "a"}));
    assert_eq!(load(&dir, "a.pxe.json").get(0, 0, 1, 1), b'2', "restoring a snapshot can be undone");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn readable_pixel_output() {
    let (s, dir) = server("rows");
    ok(&s, "create_document", json!({"name": "a", "width": 16, "height": 12}));
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 0, "x": 9, "y": 10, "rows": ["12", ".3"]}));
    let t = ok(&s, "get_document", json!({"path": "a"}));
    assert!(t.contains("x 9..10 y 10..11"), "{t}");
    assert!(t.contains("    1\n   90\n10 12\n11 .3\n"), "{t}");
    let t = ok(&s, "get_rows", json!({"path": "a", "x": 8, "y": 9, "w": 4, "h": 3}));
    assert!(t.contains("     11\n   8901\n 9 ....\n10 .12.\n11 ..3.\n"), "{t}");
    ok(&s, "add_frame", json!({"path": "a", "copy_from": 0, "linked": true}));
    ok(&s, "set_tag", json!({"path": "a", "name": "idle", "from": 0, "to": 1}));
    let t = ok(&s, "get_document", json!({"path": "a"}));
    assert!(t.contains("frame idle/1 (\"f1\", index 1): linked to frame 0"), "{t}");
    assert!(t.contains("1 \"f1\" 100 ms = idle/1"), "{t}");
    // arbitrary frame order through linked frames
    let t = ok(&s, "set_tag", json!({"path": "a", "name": "blink", "frames": [0, 1, 0]}));
    assert!(t.contains("appended 3 linked frames"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn preview_exports_and_aseprite() {
    let (s, dir) = server("export");
    ok(&s, "create_document", json!({"name": "hero", "width": 8, "height": 8}));
    ok(
        &s,
        "draw_rect",
        json!({"path": "hero", "layer": 0, "frame": 0, "x": 1, "y": 1, "w": 6, "h": 6, "key": "2", "filled": true}),
    );
    ok(&s, "add_frame", json!({"path": "hero", "copy_from": 0}));
    ok(&s, "set_tag", json!({"path": "hero", "name": "idle", "from": 0, "to": 1}));
    ok(&s, "set_region", json!({"path": "hero", "name": "face", "x": 2, "y": 2, "w": 3, "h": 2, "tag": "idle"}));
    let t = ok(&s, "render_preview", json!({"path": "hero", "inline": false}));
    assert!(dir.join("preview/hero.png").exists(), "{t}");
    for t in ["png_sheet", "godot", "unity", "gif", "aseprite"] {
        ok(&s, "export", json!({"path": "hero", "target": t}));
    }
    assert!(dir.join("export/hero/hero.tres").exists());
    // convert to .aseprite and edit it directly; regions survive as slices
    ok(&s, "save_as", json!({"path": "hero", "to": "hero2.aseprite"}));
    ok(
        &s,
        "set_pixels",
        json!({"path": "hero2.aseprite", "layer": 0, "frame": 0, "region": "face", "pixels": [{"x": 3, "y": 3, "key": "c"}]}),
    );
    let back = load(&dir, "hero2.aseprite");
    assert_eq!(back.get(0, 0, 3, 3), b'c');
    assert_eq!(back.get(0, 0, 1, 1), b'2');
    assert_eq!(back.regions[0].name, "face");
    let list = ok(&s, "list_documents", json!({}));
    assert!(list.contains("hero2.aseprite") && list.contains("regions: [face]"), "{list}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn render_animation_tool() {
    let (s, dir) = server("anim");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 8}));
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 0, "x": 0, "y": 5, "rows": ["88", "88"]}));
    ok(&s, "add_frame", json!({"path": "a", "copy_from": 0}));
    ok(
        &s,
        "move_region",
        json!({"path": "a", "layer": 0, "frame": 1, "clip": {"x": 0, "y": 5, "w": 2, "h": 2}, "dx": 1, "dy": -1}),
    );
    ok(&s, "set_tag", json!({"path": "a", "name": "hop", "from": 0, "to": 1, "direction": "pingpong"}));
    ok(&s, "set_region", json!({"path": "a", "name": "feet", "x": 0, "y": 4, "w": 8, "h": 4}));
    let (ok_, text) =
        call(&s, "render_animation", json!({"path": "a", "tag": "hop", "track": "feet", "region": "0,2,8,6"}));
    assert!(ok_, "{text}");
    assert!(text.contains("hop: 2 steps, 200 ms total, pingpong, loops"), "{text}");
    assert!(text.contains("ground line"), "{text}");
    assert!(text.contains("track feet: centre per step 0:(0.5,5.5)"), "{text}");
    assert!(dir.join("preview/a_hop_anim.png").exists());
    // the image is returned inline
    let r = s
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "render_animation", "arguments": {"path": "a", "tag": "hop"}}}))
        .unwrap();
    assert_eq!(r["result"]["content"][1]["type"], "image");
    assert!(err(&s, "render_animation", json!({"path": "a", "tag": "run"})).contains("tags are [\"hop\"]"));
    assert!(
        err(&s, "render_animation", json!({"path": "a", "tag": "hop", "track": "nose"})).contains("no region \"nose\"")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn marker_regions() {
    let (s, dir) = server("marker");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 8}));
    ok(&s, "add_frame", json!({"path": "a", "copy_from": 0}));
    // a marker is a point: x/y only
    assert!(err(
        &s,
        "set_region",
        json!({"path": "a", "name": "origin", "kind": "marker", "x": 4, "y": 6, "w": 2, "h": 2})
    )
    .contains("only x and y"));
    ok(
        &s,
        "set_region",
        json!({"path": "a", "name": "origin", "kind": "marker", "x": 4, "y": 6, "locked": true, "note": "Claude: origin als gesperrter Marker"}),
    );
    let t = ok(&s, "list_regions", json!({"path": "a"}));
    assert!(
        t.contains("\"origin\" MARKER LOCKED (definition only; pixels stay editable): frames 0..1: at (4, 6)"),
        "{t}"
    );
    // pixel writes at and around the locked marker work
    let t = ok(
        &s,
        "set_pixels",
        json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 4, "y": 6, "key": "8"}], "dry_run": true}),
    );
    assert!(t.contains("DRY RUN") && t.contains("1 pixels changed"), "{t}");
    ok(&s, "draw_line", json!({"path": "a", "layer": 0, "frame": 0, "x0": 0, "y0": 6, "x1": 7, "y1": 6, "key": "8"}));
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[6], "88888888");
    ok(&s, "set_rows", json!({"path": "a", "layer": 0, "frame": 1, "region": "origin", "rows": ["9"]}));
    assert_eq!(load(&dir, "a.pxe.json").get(0, 1, 4, 6), b'9');
    // ... but its definition is protected
    let e = err(&s, "move_region", json!({"path": "a", "layer": 0, "frame": 0, "region": "origin", "dx": 1}));
    assert!(e.contains("marker \"origin\" is locked") && e.contains("nothing was written"), "{e}");
    assert!(err(&s, "set_region", json!({"path": "a", "name": "origin", "x": 5, "y": 6})).contains("locked"));
    assert!(err(&s, "set_region", json!({"path": "a", "name": "origin", "rename": "o"})).contains("locked"));
    assert!(err(&s, "delete_region", json!({"path": "a", "name": "origin"})).contains("locked"));
    assert!(err(&s, "set_region", json!({"path": "a", "name": "origin", "kind": "area"})).contains("locked"));
    // locked areas still protect pixels
    ok(&s, "set_region", json!({"path": "a", "name": "face", "x": 0, "y": 0, "w": 2, "h": 2, "locked": true}));
    assert!(err(
        &s,
        "set_pixels",
        json!({"path": "a", "layer": 0, "frame": 0, "pixels": [{"x": 1, "y": 1, "key": "8"}]})
    )
    .contains("locked region \"face\""));
    // unlocked markers can move; render_animation can track them
    ok(&s, "set_region", json!({"path": "a", "name": "origin", "locked": false}));
    ok(&s, "set_region", json!({"path": "a", "name": "origin", "frame": 1, "x": 5, "y": 5}));
    ok(&s, "set_tag", json!({"path": "a", "name": "t", "from": 0, "to": 1}));
    let t = ok(&s, "render_animation", json!({"path": "a", "tag": "t", "track": "origin", "inline": false}));
    assert!(t.contains("track origin: centre per step 0:(4,6) bottom 6 | 1:(5,5) bottom 5"), "{t}");
    // kind survives .aseprite (slice with pivot)
    ok(&s, "save_as", json!({"path": "a", "to": "a.aseprite"}));
    let back = load(&dir, "a.aseprite");
    let o = back.regions.iter().find(|r| r.name == "origin").unwrap();
    assert!(o.is_marker());
    assert_eq!((o.point_at(0), o.point_at(1)), (Some((4, 6)), Some((5, 5))));
    // the history note was recorded
    assert!(ok(&s, "history", json!({"path": "a", "limit": 50})).contains("Claude: origin als gesperrter Marker"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn live_follow_support() {
    let (s, dir) = server("live");
    ok(&s, "create_document", json!({"name": "a", "width": 8, "height": 8}));
    let j = Journal::for_document(&dir.join("a.pxe.json"));
    // edits carry their location for the editor
    ok(&s, "draw_rect", json!({"path": "a", "layer": 0, "frame": 0, "x": 1, "y": 2, "w": 3, "h": 2, "key": "8"}));
    let e = j.entries().pop().unwrap();
    assert_eq!(e.kind, "edit");
    let f = e.focus.unwrap();
    assert_eq!((f.frame, f.layer, f.bbox), (0, 0, Some([1, 2, 3, 2])));
    // paced: one progress entry per row except the last, then the edit
    let before = j.last_id();
    let t = ok(
        &s,
        "set_rows",
        json!({"path": "a", "layer": 0, "frame": 0, "x": 0, "y": 5, "rows": ["99", "99", "99"], "pace_ms": 1}),
    );
    assert!(t.contains("drawn in 3 strokes"), "{t}");
    let kinds: Vec<String> =
        j.entries_after(before).into_iter().filter(|e| e.author == "Claude").map(|e| e.kind).collect();
    assert_eq!(kinds, vec!["progress", "progress", "edit"]);
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[7], "99......");
    // undo treats the paced write as one step
    ok(&s, "undo", json!({"path": "a"}));
    assert_eq!(load(&dir, "a.pxe.json").rows(0, 0)[5], "........");
    // focus: a view hint, nothing changes
    ok(&s, "set_region", json!({"path": "a", "name": "fist_left", "x": 1, "y": 1, "w": 2, "h": 2}));
    let before = load(&dir, "a.pxe.json");
    let t = ok(&s, "focus", json!({"path": "a", "frame": 0, "region": "fist_left"}));
    assert!(t.contains("Claude: arbeitet an fist_left (f0 · Ebene 1)"), "{t}");
    let e = j.entries().pop().unwrap();
    assert_eq!((e.kind.as_str(), e.focus.unwrap().bbox), ("focus", Some([1, 1, 2, 2])));
    assert_eq!(load(&dir, "a.pxe.json"), before);
    assert!(err(&s, "focus", json!({"path": "a", "frame": 0, "region": "nope"})).contains("no region"));
    // open_editor (dry run: no window in tests)
    let t = ok(&s, "open_editor", json!({"path": "a", "dry_run": true}));
    assert!(t.starts_with("would run: ") && t.contains("a.pxe.json"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- get_user_view -----------------------------------------------------------

/// A 2-frame document with a tag, a named layer, a region and some pixels.
fn view_doc(s: &Server, dir: &Path) -> PathBuf {
    ok(s, "create_document", json!({"name": "toast/walk", "width": 8, "height": 8, "layer_name": "body"}));
    ok(s, "add_frame", json!({"path": "toast/walk"}));
    ok(s, "set_tag", json!({"path": "toast/walk", "name": "walk_side", "from": 0, "to": 1}));
    ok(s, "set_rows", json!({"path": "toast/walk", "layer": 0, "frame": 1, "x": 2, "y": 3, "rows": ["88", "8c"]}));
    ok(s, "set_region", json!({"path": "toast/walk", "name": "face", "x": 2, "y": 3, "w": 2, "h": 2}));
    dir.canonicalize().unwrap().join("toast/walk.pxe.json")
}

fn fake_view(doc: &Path, pid: u32, time_ms: u64) -> crate::view::View {
    let d = io::load(doc).unwrap();
    crate::view::View {
        pid,
        time_ms,
        path: doc.to_path_buf(),
        dirty: false,
        frame: crate::view::frame_ref(&d, 1),
        layer: crate::view::layer_ref(&d, 0),
        selection: Some(crate::view::Area { x: 2, y: 3, w: 2, h: 2 }),
        selection_mask: None,
        hover: Some(crate::view::hover_at(&d, 0, 1, 3, 4)),
        zoom: 12.0,
        following: true,
        follow_paused: false,
        tool: "Auswahl".into(),
        playing: false,
    }
}

#[test]
fn get_user_view_without_an_editor() {
    let (s, dir) = server("view-none");
    view_doc(&s, &dir);
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.starts_with("No pixel-editor GUI is open"), "{t}");
    assert!(t.contains("no \"KI zeigen\" marks"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn get_user_view_reports_view_selection_and_marks() {
    let (s, dir) = server("view-live");
    let doc = view_doc(&s, &dir);
    // an editor in this process (alive) wrote its view
    let v = fake_view(&doc, std::process::id(), crate::view::now_ms());
    crate::view::write_view(&v).unwrap();
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.starts_with("1 editor(s) open"), "{t}");
    assert!(t.contains(&format!("- toast/walk.pxe.json · editor pid {}", std::process::id())), "{t}");
    assert!(t.contains("frame walk_side/1 (index 1, name \"f1\") · layer 0 \"body\""), "{t}");
    assert!(t.contains("zoom 12x · follow assistant edits on · tool Auswahl"), "{t}");
    assert!(t.contains("cursor: x 3, y 4 · color c on this layer"), "{t}");
    assert!(t.contains("region face"), "{t}");
    assert!(t.contains("selection: x 2..3 y 3..4 (2x2)"), "{t}");
    // the selected pixels as get_rows prints them
    let rows =
        ok(&s, "get_rows", json!({"path": "toast/walk", "layer": 0, "frame": 1, "x": 2, "y": 3, "w": 2, "h": 2}));
    for line in rows.lines() {
        assert!(t.contains(&format!("    {line}")), "missing get_rows line {line:?} in {t}");
    }
    assert!(t.contains("no \"KI zeigen\" marks"), "{t}");
    assert!(!t.contains("UNSAVED"));

    // "KI zeigen" without a selection: the whole frame
    let p = crate::view::Pointer {
        pid: std::process::id(),
        time_ms: crate::view::now_ms(),
        path: doc.clone(),
        dirty: true,
        frame: v.frame.clone(),
        layer: v.layer.clone(),
        rect: None,
    };
    // an old single pointer.json is read as mark #1
    std::fs::write(crate::view::pointer_file(&doc), serde_json::to_string(&p).unwrap()).unwrap();
    let mut dirty = v.clone();
    dirty.dirty = true;
    crate::view::write_view(&dirty).unwrap();
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.contains("UNSAVED CHANGES"), "{t}");
    assert!(t.contains("\"KI zeigen\" marks in toast/walk.pxe.json: 1\n"), "{t}");
    assert!(
        t.contains("  frame walk_side/1 (index 1, name \"f1\"):\n    #1 the whole frame · layer 0 \"body\" · 0 s ago"),
        "{t}"
    );
    assert!(t.contains("x 0..7 y 0..7 (8x8)"), "whole frame rows: {t}");

    // more marks on several frames, with notes: grouped by frame
    let mut marks = crate::view::read_marks(&doc);
    let mark = |frame: usize, rect: Option<crate::view::Area>, note: &str| crate::view::Mark {
        n: 0,
        frame: crate::view::FrameRef { index: frame, name: format!("f{frame}"), address: format!("f{frame}") },
        layer: v.layer.clone(),
        rect,
        note: note.into(),
        time_ms: crate::view::now_ms(),
        pid: std::process::id(),
        dirty: false,
    };
    let a = |x, y, w, h| Some(crate::view::Area { x, y, w, h });
    assert_eq!(marks.add(mark(0, a(1, 1, 2, 2), "brauner Block weg")), 2);
    assert_eq!(marks.add(mark(1, a(5, 5, 1, 1), "")), 3);
    assert_eq!(marks.add(mark(0, a(0, 6, 3, 1), "Schatten")), 4);
    crate::view::write_marks(&doc, &marks).unwrap();
    assert!(!crate::view::pointer_file(&doc).exists(), "the old file is replaced");
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.contains("marks in toast/walk.pxe.json: 4"), "{t}");
    let f0 = t.find("  frame f0 (index 0):").expect("frame 0 group");
    let f1 = t.find("  frame walk_side/1 (index 1, name \"f1\"):").expect("frame 1 group");
    assert!(f0 < f1, "frames in order: {t}");
    assert!(t.contains("    #2 x 1..2 y 1..2 (2x2) · layer 0 \"body\" · 0 s ago · note: \"brauner Block weg\""), "{t}");
    assert!(t.contains("    #4 x 0..2 y 6..6 (3x1)") && t.contains("note: \"Schatten\""), "{t}");
    assert!(t.contains("    #3 x 5..5 y 5..5 (1x1)"), "{t}");
    assert!(t.find("#2").unwrap() < t.find("#4").unwrap() && t.find("#4").unwrap() < t.find("#1").unwrap(), "{t}");
    assert!(t.contains("clear_marks"), "{t}");

    // clear_marks: by number, by frame, all
    let r = ok(&s, "clear_marks", json!({"path": "toast/walk", "numbers": [2]}));
    assert_eq!(r, "removed 1 mark(s) (#2) from toast/walk.pxe.json; 3 left");
    assert!(err(&s, "clear_marks", json!({"path": "toast/walk", "numbers": [2, 9]})).contains("no mark #2, #9"));
    let r = ok(&s, "clear_marks", json!({"path": "toast/walk", "frame": "walk_side/1"}));
    assert_eq!(r, "removed 2 mark(s) (#1, #3) from toast/walk.pxe.json; 1 left");
    let r = ok(&s, "clear_marks", json!({"path": "toast/walk"}));
    assert_eq!(r, "removed 1 mark(s) (#4) from toast/walk.pxe.json; 0 left");
    assert!(!crate::view::marks_file(&doc).exists());
    // numbering starts again
    let mut marks = crate::view::read_marks(&doc);
    assert_eq!(marks.add(mark(0, None, "")), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn get_user_view_drops_dead_and_stale_editors() {
    let (s, dir) = server("view-stale");
    let doc = view_doc(&s, &dir);
    // a pid that cannot exist (above Linux's pid_max) and a live one whose
    // file has not been refreshed for 11 minutes
    let dead = fake_view(&doc, 999_999_999, crate::view::now_ms());
    let old = fake_view(&doc, std::process::id(), crate::view::now_ms() - 11 * 60 * 1000);
    crate::view::write_view(&dead).unwrap();
    crate::view::write_view(&old).unwrap();
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.starts_with("No pixel-editor GUI is open"), "{t}");
    assert!(!crate::view::view_file(&doc, 999_999_999).exists(), "dead editor's file removed");
    assert!(!crate::view::view_file(&doc, std::process::id()).exists(), "stale file removed");
    // a pointer from an editor that is closed now is still reported
    let p = crate::view::Pointer {
        pid: 999_999_999,
        time_ms: crate::view::now_ms() - 3 * 60 * 1000,
        path: doc.clone(),
        dirty: false,
        frame: dead.frame.clone(),
        layer: dead.layer.clone(),
        rect: Some(crate::view::Area { x: 2, y: 3, w: 1, h: 1 }),
    };
    std::fs::write(crate::view::pointer_file(&doc), serde_json::to_string(&p).unwrap()).unwrap();
    let t = ok(&s, "get_user_view", json!({}));
    assert!(t.contains("marks in toast/walk.pxe.json: 1 (document not open in an editor)"), "{t}");
    assert!(t.contains("#1 x 2..2 y 3..3 (1x1) · layer 0 \"body\" · 3 min ago"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- author from clientInfo ---------------------------------------------------

#[test]
fn author_names_from_client_info() {
    for (client, want) in [
        (Some("claude-code"), "Claude"),
        (Some("Claude Desktop"), "Claude"),
        (Some("codex-mcp-client"), "Codex"),
        (Some("Codex"), "Codex"),
        (Some("cursor"), "Cursor"),
        (Some("  "), "KI"),
        (None, "KI"),
    ] {
        assert_eq!(author_from_client(client), want, "{client:?}");
    }
}

#[test]
fn two_sessions_record_their_own_authors() {
    let (claude, dir) = server("authors");
    let codex = Server::new(dir.canonicalize().unwrap());
    codex.handle(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "codex-mcp-client", "version": "0.50"}}}));
    let anon = Server::new(dir.canonicalize().unwrap()); // no initialize at all

    ok(&claude, "create_document", json!({"name": "dog", "width": 8, "height": 8}));
    ok(&codex, "set_rows", json!({"path": "dog", "layer": 0, "frame": 0, "x": 1, "y": 1, "rows": ["88"]}));
    ok(&codex, "focus", json!({"path": "dog", "frame": 0, "rect": {"x": 1, "y": 1, "w": 2, "h": 1}}));
    ok(
        &claude,
        "draw_line",
        json!({"path": "dog", "layer": 0, "frame": 0, "x0": 0, "y0": 7, "x1": 3, "y1": 7,
        "key": "9", "note": "Boden"}),
    );
    ok(&anon, "set_pixels", json!({"path": "dog", "layer": 0, "frame": 0, "pixels": [{"x": 7, "y": 7, "key": "2"}]}));
    ok(&codex, "undo", json!({"path": "dog"}));

    let j = Journal::for_document(&dir.join("dog.pxe.json"));
    let got: Vec<(String, String, String)> =
        j.entries().into_iter().filter(|e| e.author != "system").map(|e| (e.author, e.kind, e.note)).collect();
    let authors: Vec<&str> = got.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(authors, vec!["Claude", "Codex", "Codex", "Claude", "KI", "Codex"], "{got:#?}");
    assert_eq!(got[0].2, "Claude: create_document");
    assert!(got[1].2.starts_with("Codex: set_rows f0 Ebene 1"), "{}", got[1].2);
    assert_eq!((got[2].1.as_str(), got[2].2.starts_with("Codex: arbeitet an x 1..2 y 1..1")), ("focus", true));
    assert_eq!(got[3].2, "Boden", "explicit notes are kept; the GUI adds the author when showing it");
    assert!(got[4].2.starts_with("KI: set_pixels"), "{}", got[4].2);
    assert!(got[5].2.starts_with("undo: "), "{}", got[5].2);

    // history shows each author once
    let h = ok(&codex, "history", json!({"path": "dog"}));
    assert!(h.contains("Codex: set_rows") && h.contains("Claude: Boden") && !h.contains("Codex: Codex:"), "{h}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn get_user_view_shows_a_shaped_selection() {
    let (s, dir) = server("view-mask");
    let doc = view_doc(&s, &dir);
    let mut v = fake_view(&doc, std::process::id(), crate::view::now_ms());
    // three of the four pixels of the box: not (3, 3)
    v.selection_mask = Some(vec!["#.".into(), "##".into()]);
    crate::view::write_view(&v).unwrap();
    let t = ok(&s, "get_user_view", json!({}));
    assert!(
        t.contains(
            "selection: 3 pixels in the box x 2..3 y 3..4 (2x2) (a shape, not the whole box; '·' = not selected)"
        ),
        "{t}"
    );
    assert!(t.contains("\n      23\n    3 8·\n    4 8c\n"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn transform_rect_cel_frames_with_diff_history_and_locks() {
    let (s, dir) = server("transform");
    ok(&s, "create_document", json!({"name": "t", "width": 6, "height": 4}));
    ok(&s, "set_rows", json!({"path": "t", "layer": 0, "frame": 0, "rows": ["12....", "3....."]}));
    // rect: only that box, with a diff, dry run first
    let t = ok(
        &s,
        "transform",
        json!({"path": "t", "op": "flip_h", "scope": "rect", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 3, "h": 2}, "dry_run": true}),
    );
    assert!(t.starts_with("DRY RUN"), "{t}");
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0)[0], "12....");
    let t = ok(
        &s,
        "transform",
        json!({"path": "t", "op": "flip_h", "scope": "rect", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 3, "h": 2}}),
    );
    assert!(t.contains("recorded as \"Claude: transform flip_h"), "{t}");
    let d = load(&dir, "t.pxe.json");
    assert_eq!(d.rows(0, 0)[..2], [".21...", "..3..."]);
    // a non-square rect turned by 90°: the 3x2 box becomes a 2x3 box on its centre
    let t = ok(
        &s,
        "transform",
        json!({"path": "t", "op": "rot90", "scope": "rect", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 3, "h": 2}}),
    );
    assert!(t.contains("-> x 0..1 y -1..1 (2x3)") && !t.contains("cut off"), "{t}");
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0)[..2], [".2....", "31...."]);
    let h = ok(&s, "history", json!({"path": "t"}));
    assert!(h.contains("transform rot90"), "{h}");
    ok(&s, "undo", json!({"path": "t"}));
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0)[..2], [".21...", "..3..."]);

    // whole frames: two frames, a linked one, a region and a locked marker
    ok(&s, "add_frame", json!({"path": "t", "copy_from": 0, "linked": true}));
    ok(&s, "add_frame", json!({"path": "t", "copy_from": 0}));
    ok(&s, "set_region", json!({"path": "t", "name": "hand", "x": 1, "y": 0, "w": 2, "h": 1}));
    ok(&s, "set_region", json!({"path": "t", "name": "origin", "kind": "marker", "x": 0, "y": 3, "locked": true}));
    ok(&s, "set_tag", json!({"path": "t", "name": "walk", "from": 0, "to": 1}));
    // frame 0 alone would change its linked frame 1 too
    let e = err(&s, "transform", json!({"path": "t", "op": "flip_h", "scope": "frame", "frame": 0}));
    assert!(e.contains("linked with frames [1]") && e.contains("edit_linked"), "{e}");
    let t = ok(&s, "transform", json!({"path": "t", "op": "flip_h", "scope": "tag", "tag": "walk"}));
    assert!(t.contains("(1 cels)") && t.contains("locked regions kept their rect: origin"), "{t}");
    let d = load(&dir, "t.pxe.json");
    assert_eq!(d.rows(0, 0)[..2], ["...12.", "...3.."]);
    assert_eq!(d.rows(0, 1), d.rows(0, 0));
    assert_eq!(d.rows(0, 2)[0], ".21...", "frame 2 is outside the tag");
    let hand = &d.regions[d.find_region("hand").unwrap()];
    assert_eq!(hand.rect_at(0), Some(Rect::new(3, 0, 2, 1)));
    assert_eq!(hand.rect_at(2), Some(Rect::new(1, 0, 2, 1)));
    assert_eq!(d.regions[d.find_region("origin").unwrap()].point_at(0), Some((0, 3)));
    // scope all + cel, locked layer, unknown scope, wrong args
    ok(&s, "transform", json!({"path": "t", "op": "rot180", "scope": "all"}));
    ok(&s, "transform", json!({"path": "t", "op": "flip_v", "scope": "cel", "layer": 0, "frame": 2}));
    ok(&s, "set_layer", json!({"path": "t", "layer": 0, "locked": true}));
    let e = err(&s, "transform", json!({"path": "t", "op": "flip_h", "scope": "all"}));
    assert!(e.contains("locked"), "{e}");
    let e = err(&s, "transform", json!({"path": "t", "op": "spin", "scope": "all"}));
    assert!(e.contains("unknown op"), "{e}");
    let e = err(&s, "transform", json!({"path": "t", "op": "flip_h", "scope": "cel", "frame": 0}));
    assert!(e.contains("missing \"layer\""), "{e}");
    let e = err(&s, "transform", json!({"path": "t", "op": "flip_h", "scope": "bogus"}));
    assert!(e.contains("unknown scope"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn transform_respects_locked_regions_and_mirror_document_copies() {
    let (s, dir) = server("mirror");
    ok(&s, "create_document", json!({"name": "walk_right", "width": 4, "height": 2}));
    ok(&s, "set_rows", json!({"path": "walk_right", "layer": 0, "frame": 0, "rows": ["12..", "3..."]}));
    ok(&s, "add_frame", json!({"path": "walk_right", "copy_from": 0, "linked": true, "duration_ms": 300}));
    ok(&s, "set_tag", json!({"path": "walk_right", "name": "walk", "from": 0, "to": 1, "direction": "pingpong"}));
    ok(&s, "set_region", json!({"path": "walk_right", "name": "face", "x": 0, "y": 0, "w": 1, "h": 1, "locked": true}));
    let e = err(
        &s,
        "transform",
        json!({"path": "walk_right", "op": "flip_h", "scope": "cel", "layer": 0, "frame": 0, "edit_linked": true}),
    );
    assert!(e.contains("locked region \"face\""), "{e}");
    let t = ok(&s, "mirror_document", json!({"src": "walk_right", "dst": "walk_left"}));
    assert!(t.contains("wrote walk_left.pxe.json") && t.contains("2 frames"), "{t}");
    let src = load(&dir, "walk_right.pxe.json");
    let m = load(&dir, "walk_left.pxe.json");
    assert_eq!(src.rows(0, 0), ["12..", "3..."], "the source stays");
    assert_eq!(m.rows(0, 0), ["..21", "...3"]);
    assert!(m.layers[0].is_linked(1));
    assert_eq!((m.frames[1].duration_ms, &m.tags), (300, &src.tags));
    let face = &m.regions[0];
    assert_eq!((face.rect_at(0), face.locked), (Some(Rect::new(3, 0, 1, 1)), true));
    let h = ok(&s, "history", json!({"path": "walk_left"}));
    assert!(h.contains("mirror_document flip_h from walk_right.pxe.json"), "{h}");
    assert!(err(&s, "mirror_document", json!({"src": "walk_right", "dst": "walk_left"})).contains("already exists"));
    assert!(err(&s, "mirror_document", json!({"src": "walk_right", "dst": "x", "op": "rot90"})).contains("flip_h"));
    ok(&s, "mirror_document", json!({"src": "walk_right", "dst": "walk_left.aseprite"}));
    assert_eq!(io::load(&dir.join("walk_left.aseprite")).unwrap().rows(0, 0), ["..21", "...3"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tiled_preview_and_seam_check() {
    let (s, dir) = server("tiles");
    ok(&s, "create_document", json!({"name": "tile", "width": 8, "height": 4, "palette": "pico8"}));
    ok(
        &s,
        "draw_rect",
        json!({"path": "tile", "layer": 0, "frame": 0, "x": 0, "y": 0, "w": 8, "h": 4, "key": "b", "filled": true}),
    );
    ok(&s, "add_frame", json!({"path": "tile", "copy_from": 0}));
    ok(&s, "set_frame_duration", json!({"path": "tile", "frame": 1, "duration_ms": 100}));
    // frame 1: a white stripe along the right edge, frame 0 stays plain grass
    ok(
        &s,
        "draw_line",
        json!({"path": "tile", "layer": 0, "frame": 1, "x0": 7, "y0": 0, "x1": 7, "y1": 3, "key": "7"}),
    );
    let t = ok(&s, "render_preview", json!({"path": "tile", "frame": 1, "tiled": true, "inline": false}));
    assert!(t.contains("repeated 3x3"), "{t}");
    let png = dir.join("preview/tile_f1_tiled.png");
    let img = pixel_core::render::Image::load_png(&png).unwrap_or_else(|e| panic!("{e}: {t}"));
    let plain = ok(&s, "render_preview", json!({"path": "tile", "frame": 1, "inline": false, "zoom": 8}));
    assert!(plain.contains("tile_f1.png") && !plain.contains("3x3"), "{plain}");
    assert!(img.height > 3 * 4 * 8, "3x3 at zoom 8 or more: {}x{}", img.width, img.height);

    let r = ok(&s, "check_seams", json!({"path": "tile"}));
    assert!(r.contains("frame 0") && r.contains("seamless"), "{r}");
    assert!(r.contains("left/right seam (x=last | x=0): VISIBLE SEAM"), "{r}");
    assert!(r.contains("y 0..3: 7"), "{r}");
    assert!(r.contains("1 visible seam(s)"), "{r}");
    let one = ok(&s, "check_seams", json!({"path": "tile", "frame": 0, "layer": 0}));
    assert!(one.contains("No seam stands out") && !one.contains("frame 1"), "{one}");
    let e = err(&s, "check_seams", json!({"path": "tile", "threshold": 2}));
    assert!(e.contains("0..1"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_color_scopes_locks_and_history() {
    let (s, dir) = server("replace");
    ok(&s, "create_document", json!({"name": "r", "width": 4, "height": 2}));
    ok(&s, "set_rows", json!({"path": "r", "layer": 0, "frame": 0, "rows": ["1121", "1.11"]}));
    ok(&s, "add_frame", json!({"path": "r", "copy_from": 0}));
    ok(&s, "add_frame", json!({"path": "r", "copy_from": 0, "linked": true}));
    ok(&s, "add_layer", json!({"path": "r", "name": "top"}));
    ok(&s, "set_rows", json!({"path": "r", "layer": "top", "frame": 1, "rows": ["...1"]}));
    // one cel by default; dry run first
    let t =
        ok(&s, "replace_color", json!({"path": "r", "layer": 0, "frame": 1, "from": "1", "to": "3", "dry_run": true}));
    assert!(t.starts_with("DRY RUN") && t.contains("6 pixels in 1 cels"), "{t}");
    assert_eq!(load(&dir, "r.pxe.json").rows(0, 1), ["1121", "1.11"]);
    // a rect of one cel
    let t = ok(
        &s,
        "replace_color",
        json!({"path": "r", "layer": 0, "frame": 1, "rect": {"x": 0, "y": 0, "w": 2, "h": 2}, "from": "1", "to": "3"}),
    );
    assert!(t.contains("3 pixels in 1 cels"), "{t}");
    assert_eq!(load(&dir, "r.pxe.json").rows(0, 1), ["3321", "3.11"]);
    // a linked cel needs edit_linked, as every write
    let e = err(&s, "replace_color", json!({"path": "r", "layer": 0, "frame": 0, "from": "1", "to": "3"}));
    assert!(e.contains("edit_linked"), "{e}");
    // all frames, all layers; a locked region keeps its pixel (frames 0/2
    // share a cel, frame 1 has its own) and is counted
    ok(&s, "set_region", json!({"path": "r", "name": "keep", "x": 3, "y": 1, "w": 1, "h": 1, "locked": true}));
    let t = ok(&s, "replace_color", json!({"path": "r", "scope": "all", "from": "1", "to": "5"}));
    assert!(t.contains("5 cels; 2 pixels in locked regions left unchanged"), "{t}");
    let d = load(&dir, "r.pxe.json");
    assert_eq!(
        (d.rows(0, 0), d.rows(0, 2)),
        (vec!["5525".to_string(), "5.51".into()], vec!["5525".to_string(), "5.51".into()])
    );
    assert_eq!((d.rows(0, 1)[1].as_str(), d.rows(1, 1)[0].as_str()), ("3.51", "...5"));
    // only one layer of a frame
    let t = ok(
        &s,
        "replace_color",
        json!({"path": "r", "scope": "frame", "frame": 1, "layer": "top", "from": "5", "to": "."}),
    );
    assert!(t.contains("layer \"top\""), "{t}");
    assert_eq!(load(&dir, "r.pxe.json").rows(1, 1)[0], "....");
    let h = ok(&s, "history", json!({"path": "r"}));
    assert!(h.contains("Claude: replace_color 5 -> . frame f1, layer \"top\""), "{h}");
    assert!(err(&s, "replace_color", json!({"path": "r", "from": "2", "to": "2", "layer": 0, "frame": 1}))
        .contains("same color"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fill_and_rect_with_dither() {
    let (s, dir) = server("dither");
    ok(&s, "create_document", json!({"name": "t", "width": 8, "height": 4}));
    let t = ok(
        &s,
        "fill",
        json!({"path": "t", "layer": 0, "frame": 0, "x": 0, "y": 0, "dither": {"pattern": "checker", "colors": ["2", "3"]}}),
    );
    assert!(t.contains("dither checker of 2 and 3"), "{t}");
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0), ["23232323", "32323232", "23232323", "32323232"]);
    // a filled rect with 25 % over transparent (second color default '.')
    ok(&s, "clear", json!({"path": "t", "layer": 0, "frame": 0}));
    ok(
        &s,
        "draw_rect",
        json!({"path": "t", "layer": 0, "frame": 0, "x": 0, "y": 0, "w": 8, "h": 4, "filled": true, "key": "9", "dither": {"pattern": "25"}}),
    );
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0), ["9...9...", "..9...9.", "9...9...", "..9...9."]);
    // the pattern follows the canvas: a fill inside a clip continues it
    ok(&s, "clear", json!({"path": "t", "layer": 0, "frame": 0}));
    ok(
        &s,
        "fill",
        json!({"path": "t", "layer": 0, "frame": 0, "x": 5, "y": 1, "clip": {"x": 4, "y": 0, "w": 4, "h": 4}, "dither": {"pattern": "lines_h", "colors": ["a", "b"]}}),
    );
    assert_eq!(load(&dir, "t.pxe.json").rows(0, 0), ["....aaaa", "....bbbb", "....aaaa", "....bbbb"]);
    let e = err(
        &s,
        "fill",
        json!({"path": "t", "layer": 0, "frame": 0, "x": 0, "y": 0, "dither": {"pattern": "stripes", "colors": ["a"]}}),
    );
    assert!(e.contains("unknown pattern") && e.contains("bayer1..bayer15"), "{e}");
    let e = err(
        &s,
        "draw_rect",
        json!({"path": "t", "layer": 0, "frame": 0, "x": 0, "y": 0, "w": 2, "h": 2, "key": "1", "dither": {"pattern": "75"}}),
    );
    assert!(e.contains("filled"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn outline_outside_inside_and_rect() {
    let (s, dir) = server("outline");
    ok(&s, "create_document", json!({"name": "o", "width": 5, "height": 5}));
    ok(
        &s,
        "set_rows",
        json!({"path": "o", "layer": 0, "frame": 0, "x": 0, "y": 1, "rows": [".....", "..1..", "....."]}),
    );
    let t = ok(&s, "outline", json!({"path": "o", "layer": 0, "frame": 0, "color": "0", "dry_run": true}));
    assert!(t.starts_with("DRY RUN") && t.contains("outline outside 0 4-connected") && t.contains("4 pixels"), "{t}");
    ok(&s, "outline", json!({"path": "o", "layer": 0, "frame": 0, "color": "0", "connectivity": 8}));
    assert_eq!(load(&dir, "o.pxe.json").rows(0, 0), [".....", ".000.", ".010.", ".000.", "....."]);
    // inside: the ring's own edge; the canvas edge counts as outside
    ok(&s, "outline", json!({"path": "o", "layer": 0, "frame": 0, "color": "c", "mode": "inside"}));
    assert_eq!(load(&dir, "o.pxe.json").rows(0, 0), [".....", ".ccc.", ".c1c.", ".ccc.", "....."]);
    // a rect: the shape is what lies inside it, the outline may go 1 px around it
    ok(&s, "clear", json!({"path": "o", "layer": 0, "frame": 0}));
    ok(&s, "set_rows", json!({"path": "o", "layer": 0, "frame": 0, "x": 0, "y": 2, "rows": [".12.."]}));
    let t = ok(
        &s,
        "outline",
        json!({"path": "o", "layer": 0, "frame": 0, "rect": {"x": 0, "y": 0, "w": 2, "h": 5}, "color": "0", "only_transparent": true}),
    );
    assert!(t.contains("only transparent"), "{t}");
    assert_eq!(load(&dir, "o.pxe.json").rows(0, 0)[1..4], [".0...", "012..", ".0..."]);
    let e = err(&s, "outline", json!({"path": "o", "layer": 0, "frame": 0, "color": "0", "connectivity": 6}));
    assert!(e.contains("4 or 8"), "{e}");
    let e = err(
        &s,
        "outline",
        json!({"path": "o", "layer": 0, "frame": 0, "color": "0", "mode": "inside", "only_transparent": true}),
    );
    assert!(e.contains("outside"), "{e}");
    let h = ok(&s, "history", json!({"path": "o"}));
    assert!(h.contains("Claude: outline outside 0"), "{h}");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn mark_on(dir: &Path, doc: &str, frame: usize, name: &str, note: &str) {
    let path = dir.join(doc);
    let mut marks = crate::view::read_marks(&path);
    marks.add(crate::view::Mark {
        n: 0,
        frame: crate::view::FrameRef { index: frame, name: name.into(), address: name.into() },
        layer: crate::view::LayerRef { index: 0, name: "Ebene 1".into() },
        rect: Some(crate::view::Area { x: 0, y: 0, w: 1, h: 1 }),
        note: note.into(),
        time_ms: crate::view::now_ms(),
        pid: std::process::id(),
        dirty: false,
    });
    crate::view::write_marks(&path, &marks).unwrap();
}

#[test]
fn move_frames_with_tags_links_regions_marks_and_history() {
    let (s, dir) = server("move-frames");
    ok(&s, "create_document", json!({"name": "walk", "width": 4, "height": 1}));
    for i in 1..4 {
        ok(&s, "add_frame", json!({"path": "walk"}));
        ok(&s, "set_pixels", json!({"path": "walk", "layer": 0, "frame": i, "pixels": [{"x": i, "y": 0, "key": "8"}]}));
    }
    ok(&s, "link_cel", json!({"path": "walk", "frame": 3, "to": 1}));
    ok(&s, "set_tag", json!({"path": "walk", "name": "step", "from": 0, "to": 1}));
    ok(&s, "set_region", json!({"path": "walk", "name": "foot", "x": 0, "y": 0, "w": 1, "h": 1}));
    ok(&s, "set_region", json!({"path": "walk", "name": "foot", "x": 3, "y": 0, "w": 1, "h": 1, "frame": 2}));
    ok(&s, "set_layer", json!({"path": "walk", "layer": 0, "locked": true}));
    mark_on(&dir, "walk.pxe.json", 2, "f2", "Fuß kleiner");
    let before = load(&dir, "walk.pxe.json");
    // dry run: nothing written
    let t = ok(&s, "move_frames", json!({"path": "walk", "frames": [2, "f3"], "to": 0, "dry_run": true}));
    assert!(t.starts_with("DRY RUN") && t.contains("new order: f2 f3 f0 f1"), "{t}");
    assert_eq!(load(&dir, "walk.pxe.json"), before);
    // moving is allowed on a locked layer: no frame's pixels change
    let t = ok(&s, "move_frames", json!({"path": "walk", "frames": [2, "f3"], "to": 0}));
    assert!(
        t.contains("moved 2 frame(s): 2 \"f2\" -> 0, 3 \"f3\" -> 1") && t.contains("new order: f2 f3 f0 f1"),
        "{t}"
    );
    assert!(t.contains("frames [f0:100ms f1:100ms f2:100ms f3:100ms] -> [f2:100ms f3:100ms f0:100ms f1:100ms]"), "{t}");
    assert!(t.contains("tags [step 0..1 forward] -> [step 2..3 forward]"), "{t}");
    assert!(!t.contains("pixels:"), "no frame's pixels changed: {t}");
    let d = load(&dir, "walk.pxe.json");
    assert_eq!(d.rows(0, 0), ["..8."]);
    assert_eq!(d.rows(0, 1), d.rows(0, 3), "f3 still shows f1's cel");
    assert_eq!(d.layers[0].linked_frames(1), vec![1, 3], "and stays linked to it");
    assert_eq!((d.tags[0].from, d.tags[0].to), (2, 3));
    let foot = &d.regions[0];
    assert_eq!(foot.rect_at(0), Some(Rect::new(3, 0, 1, 1)), "f2's rect went along");
    assert_eq!(foot.rect_at(2), Some(Rect::new(0, 0, 1, 1)));
    let marks = crate::view::read_marks(&dir.join("walk.pxe.json"));
    assert_eq!((marks.marks[0].frame.index, marks.marks[0].frame.name.as_str()), (0, "f2"), "the mark followed f2");
    let h = ok(&s, "history", json!({"path": "walk"}));
    assert!(h.contains("Claude: move_frames: moved 2 frame(s)"), "{h}");
    // undo: the old order, and the mark goes back too
    ok(&s, "undo", json!({"path": "walk"}));
    assert_eq!(load(&dir, "walk.pxe.json"), before);
    assert_eq!(crate::view::read_marks(&dir.join("walk.pxe.json")).marks[0].frame.index, 2);
    // set_frame move_to uses the same rules
    ok(&s, "set_frame", json!({"path": "walk", "frame": "f2", "move_to": 0}));
    assert_eq!(crate::view::read_marks(&dir.join("walk.pxe.json")).marks[0].frame.index, 0);
    let e = err(&s, "move_frames", json!({"path": "walk", "frames": [0, 1], "to": 3}));
    assert!(e.contains("0..=2"), "{e}");
    let e = err(&s, "move_frames", json!({"path": "walk", "frames": ["nope"], "to": 0}));
    assert!(e.contains("no frame"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn set_layer_opacity_and_reference() {
    let (s, dir) = server("reference");
    ok(&s, "create_document", json!({"name": "held", "width": 4, "height": 4}));
    ok(&s, "set_pixels", json!({"path": "held", "layer": 0, "frame": 0, "pixels": [{"x": 1, "y": 1, "key": "8"}]}));
    let t = ok(&s, "set_layer", json!({"path": "held", "layer": 0, "opacity": 0.5}));
    assert!(t.contains("opacity 1.00 -> 0.50") && t.contains("recorded as"), "{t}");
    assert!(ok(&s, "get_document", json!({"path": "held"})).contains("opacity 0.50"));
    ok(&s, "export", json!({"path": "held", "target": "png_sheet"}));
    let png = pixel_core::render::Image::load_png(&dir.join("export/held/held.png")).unwrap();
    assert_eq!(png.get(1, 1)[3], 128, "the export uses the layer opacity");
    let plain = std::fs::read(dir.join("export/held/held.png")).unwrap();
    // a reference image next to the document
    let mut pic = pixel_core::render::Image::new(8, 6);
    pic.put(0, 0, [10, 200, 30, 255]);
    pic.save_png(&dir.join("skizze.png")).unwrap();
    let t = ok(&s, "set_reference", json!({"path": "held", "image": "skizze.png", "x": -2, "y": 1, "scale": 0.5}));
    assert!(t.contains("reference: none -> image skizze.png at -2,1 ×0.5 50% under"), "{t}");
    assert!(t.contains("8x6 px, shown 4x3 canvas px") && t.contains("never exported"), "{t}");
    let d = load(&dir, "held.pxe.json");
    let r = d.reference.clone().unwrap();
    assert_eq!((r.x, r.y, r.scale, r.opacity, r.above), (-2.0, 1.0, 0.5, 0.5, false));
    assert!(ok(&s, "get_document", json!({"path": "held"}))
        .contains("reference (editor only, never exported): image skizze.png"));
    // only the named fields change; the history has it
    let t = ok(&s, "set_reference", json!({"path": "held", "opacity": 0.3, "above": true, "locked": true}));
    assert!(t.contains("-> image skizze.png at -2,1 ×0.5 30% over, locked"), "{t}");
    assert!(err(&s, "set_reference", json!({"path": "held", "x": 0})).contains("locked"));
    ok(&s, "set_reference", json!({"path": "held", "locked": false, "x": 0}));
    // never exported, never in the pixels
    ok(&s, "export", json!({"path": "held", "target": "png_sheet"}));
    assert_eq!(std::fs::read(dir.join("export/held/held.png")).unwrap(), plain);
    assert_eq!(load(&dir, "held.pxe.json").rows(0, 0), d.rows(0, 0));
    // another document as reference
    ok(&s, "create_document", json!({"name": "vorlage", "width": 2, "height": 2}));
    let t = ok(&s, "set_reference", json!({"path": "held", "document": "vorlage.pxe.json"}));
    assert!(t.contains("document vorlage.pxe.json"), "{t}");
    assert!(err(&s, "set_reference", json!({"path": "held", "image": "vorlage.pxe.json"})).contains("is a document"));
    assert!(err(&s, "set_reference", json!({"path": "held", "image": "gone.png"})).contains("gone.png"));
    assert!(err(&s, "set_reference", json!({"path": "held", "scale": 0})).contains("scale"));
    let h = ok(&s, "history", json!({"path": "held"}));
    assert!(h.contains("Claude: set_reference: reference: image skizze.png"), "{h}");
    ok(&s, "undo", json!({"path": "held"}));
    assert!(matches!(load(&dir, "held.pxe.json").reference.unwrap().source, RefSource::Image(_)));
    let t = ok(&s, "set_reference", json!({"path": "held", "remove": true}));
    assert!(t.contains("removed the reference") && t.contains("-> none"), "{t}");
    assert!(load(&dir, "held.pxe.json").reference.is_none());
    ok(&s, "set_reference", json!({"path": "held", "image": "skizze.png"}));
    let t = ok(&s, "save_as", json!({"path": "held", "to": "held.aseprite"}));
    assert!(t.contains("the reference is not stored in .aseprite"), "{t}");
    assert!(load(&dir, "held.aseprite").reference.is_none());
    assert!(
        err(&s, "set_reference", json!({"path": "held.aseprite", "image": "skizze.png"})).contains(".pxe.json only")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn instructions_of(s: &Server) -> String {
    let init = s.handle(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}})).unwrap();
    init["result"]["instructions"].as_str().unwrap().to_string()
}

#[test]
fn project_instructions_file() {
    let dir = std::env::temp_dir().join(format!("pxe-mcp-instr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // missing: just the built-in text, no error
    let plain = instructions_of(&Server::new(dir.canonicalize().unwrap()));
    assert_eq!(plain, INSTRUCTIONS);
    // present: appended under a heading, read at start
    std::fs::write(dir.join(".pxe-instructions.md"), "Characters 48x48, tiles 32x32.\n").unwrap();
    let s = Server::new(dir.canonicalize().unwrap());
    let text = instructions_of(&s);
    assert!(text.starts_with(INSTRUCTIONS), "{text}");
    assert!(
        text.ends_with("## Project instructions (.pxe-instructions.md)\n\nCharacters 48x48, tiles 32x32."),
        "{text}"
    );
    // too big: cut at the limit (char boundary) with a note
    let big = "ä".repeat(PROJECT_INSTRUCTIONS_MAX); // 2 bytes each
    std::fs::write(dir.join(".pxe-instructions.md"), &big).unwrap();
    let text = instructions_of(&Server::new(dir.canonicalize().unwrap()));
    let appended = text.split("## Project instructions (.pxe-instructions.md)\n\n").nth(1).unwrap();
    assert!(appended.starts_with(&"ä".repeat(PROJECT_INSTRUCTIONS_MAX / 2)), "kept up to the limit");
    assert!(!appended.contains(&"ä".repeat(PROJECT_INSTRUCTIONS_MAX / 2 + 1)), "cut at the limit");
    assert!(
        appended.contains(&format!("is {} bytes; cut off after {PROJECT_INSTRUCTIONS_MAX}", big.len())),
        "{appended}"
    );
    // empty file: nothing appended
    std::fs::write(dir.join(".pxe-instructions.md"), "  \n").unwrap();
    assert_eq!(instructions_of(&Server::new(dir.canonicalize().unwrap())), INSTRUCTIONS);
    std::fs::remove_dir_all(&dir).unwrap();
}
