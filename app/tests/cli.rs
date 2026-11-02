//! The command line entry points of the binary.

use std::process::Command;

#[test]
fn render_animation_cli() {
    let dir = std::env::temp_dir().join(format!("pxe-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = r##"{"version":1,"width":4,"height":4,"palette":[{"key":"8","color":"#ff004d"}],
        "layers":[{"name":"l"}],"frames":[{"name":"a","duration_ms":120},{"name":"b","duration_ms":80}],
        "tags":[{"name":"blink","from":0,"to":1}],
        "cels":[{"layer":0,"frame":0,"rows":["....","....",".88.",".88."]},
                {"layer":0,"frame":1,"rows":["....",".88.",".88.","...."]}]}"##;
    let file = dir.join("t.pxe.json");
    std::fs::write(&file, doc).unwrap();
    let out = dir.join("t.png");
    let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["render-animation", file.to_str().unwrap(), "blink", out.to_str().unwrap(), "--zoom", "8"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "{stdout} {}", String::from_utf8_lossy(&r.stderr));
    assert!(stdout.contains("blink: 2 steps, 200 ms total, forward, loops"), "{stdout}");
    assert!(stdout.contains("ground line"), "{stdout}");
    assert!(out.exists());
    let bad = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["render-animation", file.to_str().unwrap(), "nope", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("no tag \"nope\""));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn open_cli_dry_run() {
    let dir = std::env::temp_dir().join(format!("pxe-cli-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("o.pxe.json");
    std::fs::write(&file, r#"{"version":1,"width":1,"height":1,"layers":[{"name":"l"}],"frames":[{"name":"a"}]}"#)
        .unwrap();
    let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["open", file.to_str().unwrap(), "--dry-run"])
        .env("PIXEL_EDITOR_IPC_DIR", dir.join("ipc"))
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success() && out.starts_with("would run: ") && out.contains("o.pxe.json"), "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `open` checks that the started editor survives its first 1.5 s; the test
/// hooks keep the child from opening a window.
#[test]
fn open_cli_reports_a_dying_editor() {
    let dir = std::env::temp_dir().join(format!("pxe-cli-open-fail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("o.pxe.json");
    std::fs::write(&file, r#"{"version":1,"width":1,"height":1,"layers":[{"name":"l"}],"frames":[{"name":"a"}]}"#)
        .unwrap();
    let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["open", file.to_str().unwrap()])
        .env("PIXEL_EDITOR_TEST_FAIL", "neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set")
        .env("PIXEL_EDITOR_IPC_DIR", dir.join("ipc"))
        .output()
        .unwrap();
    let (out, err) = (String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    assert!(!r.status.success(), "must fail: {out}");
    assert!(!out.contains("opened"), "{out}");
    assert!(err.contains("the editor exited right after starting"), "{err}");
    assert!(err.contains("neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set"), "stderr quoted: {err}");
    assert!(err.contains("gui.log"), "{err}");
    let log = dir.join(".pxe-history/o.pxe.json/gui.log");
    assert!(std::fs::read_to_string(&log).unwrap().contains("WAYLAND_DISPLAY"));

    // a child that stays alive counts as opened, with the log path
    let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["open", file.to_str().unwrap()])
        .env("PIXEL_EDITOR_TEST_HOLD_MS", "2500")
        .env("PIXEL_EDITOR_IPC_DIR", dir.join("ipc"))
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "{out} {}", String::from_utf8_lossy(&r.stderr));
    assert!(out.starts_with("opened ") && out.contains("log: ") && out.contains("gui.log"), "{out}");
    std::thread::sleep(std::time::Duration::from_millis(1500)); // let the child finish before cleanup
    std::fs::remove_dir_all(&dir).unwrap();
}

/// With an editor window running for the workdir, `open` hands it the file
/// over its socket (one JSON line each way) instead of starting a window.
#[test]
fn open_cli_adds_a_tab_to_the_running_window() {
    use std::io::{BufRead, BufReader, Write};
    let dir = std::env::temp_dir().join(format!("pxe-cli-open-tab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art/hero")).unwrap();
    let art = dir.join("art").canonicalize().unwrap();
    let file = art.join("hero/walk.pxe.json");
    std::fs::write(&file, r#"{"version":1,"width":1,"height":1,"layers":[{"name":"l"}],"frames":[{"name":"a"}]}"#)
        .unwrap();
    // the socket name: FNV-1a of the window's workdir
    let hash = art
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
    let ipc = dir.join("ipc");
    std::fs::create_dir_all(&ipc).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(ipc.join(format!("{hash:016x}.sock"))).unwrap();
    let window = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let mut w = &stream;
        writeln!(w, r#"{{"ok":true,"pid":4242,"result":"opened"}}"#).unwrap();
        line
    });
    let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["open", file.to_str().unwrap()])
        .env("PIXEL_EDITOR_IPC_DIR", &ipc)
        .env("PIXEL_EDITOR_TEST_FAIL", "must not start a window")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "{out} {}", String::from_utf8_lossy(&r.stderr));
    assert_eq!(
        out.trim(),
        format!("opened {} as tab in the running editor (pid 4242); it follows edits live", file.display())
    );
    let request: serde_json::Value = serde_json::from_str(&window.join().unwrap()).unwrap();
    assert_eq!(request, serde_json::json!({"cmd": "open", "path": file}));
    assert!(!art.join("hero/.pxe-history/walk.pxe.json/gui.log").exists(), "no window started");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn world_txt_and_tilemap_cli() {
    let dir = std::env::temp_dir().join(format!("pxe-cli-world-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ts = r##"{"version":1,"width":2,"height":2,"palette":[{"key":"1","color":"#00ff00"}],
        "layers":[{"name":"l"}],"frames":[{"name":"grass"},{"name":"path"}],
        "cels":[{"layer":0,"frame":0,"rows":["11","11"]}]}"##;
    std::fs::write(dir.join("tiles.pxe.json"), ts).unwrap();
    let world = "; .  Gras   =  Weg\n.=.\n...\n";
    std::fs::write(dir.join("world.txt"), world).unwrap();
    let run = |args: &[&str]| {
        let r = Command::new(env!("CARGO_BIN_EXE_pixel-editor")).current_dir(&dir).args(args).output().unwrap();
        assert!(r.status.success(), "{args:?}: {}", String::from_utf8_lossy(&r.stderr));
        String::from_utf8_lossy(&r.stdout).into_owned()
    };
    let out = run(&["import-world", "world.txt", "maps/w.tilemap.json", "--tileset", "tiles.pxe.json"]);
    assert!(out.contains("3x2 Kacheln"), "{out}");
    let map = std::fs::read_to_string(dir.join("maps/w.tilemap.json")).unwrap();
    assert!(map.contains("\"tileset\": \"../tiles.pxe.json\""), "{map}");
    run(&["export-world", "maps/w.tilemap.json", "back.txt"]);
    assert_eq!(std::fs::read_to_string(dir.join("back.txt")).unwrap(), world);
    let out = run(&["export", "maps/w.tilemap.json", "godot", "godot"]);
    assert!(out.contains("tiles_tileset.tres") && out.contains("w.tscn"), "{out}");
    run(&["preview", "maps/w.tilemap.json", "w.png", "--zoom", "2"]);
    assert!(dir.join("w.png").exists());
    let out = run(&["export", "tiles.pxe.json", "godot_tileset", "ts"]);
    assert!(out.contains("tiles_atlas.png"), "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The real binary as MCP server over stdio: ramps and shading end to end.
#[test]
fn mcp_server_shades_along_a_ramp() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("pxe-cli-shade-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = r##"{"version":1,"width":4,"height":1,"palette":[{"key":"b","color":"#64280e"},
        {"key":"8","color":"#b76010"},{"key":"9","color":"#ebb056"},{"key":"0","color":"#000000"}],
        "layers":[{"name":"l"}],"frames":[{"name":"a"}],"cels":[{"layer":0,"frame":0,"rows":["b890"]}]}"##;
    std::fs::write(dir.join("t.pxe.json"), doc).unwrap();
    let calls = [
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":{"name":"claude-code"}}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_ramps","arguments":{"path":"t","ramps":[{"name":"Kruste","keys":"b89"}]}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shade","arguments":{"path":"t","layer":0,"frame":0,"rect":{"x":0,"y":0,"w":4,"h":1},"steps":1}}}"#,
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["mcp", dir.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all((calls.join("\n") + "\n").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Kruste: b > 8 > 9") && text.contains("2 pixels changed"), "{text}");
    let saved = std::fs::read_to_string(dir.join("t.pxe.json")).unwrap();
    assert!(
        saved.contains(
            r#""ramps": [
    {"name": "Kruste", "keys": "b89"}
  ],"#
        ) && saved.contains(r#""8990""#),
        "{saved}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The real binary as MCP server: replace_color, outline and a dither fill
/// end to end, each recorded in the history.
#[test]
fn mcp_server_replaces_outlines_and_dithers() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("pxe-cli-recolor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = r##"{"version":1,"width":4,"height":3,"palette":[{"key":"1","color":"#3fc46e"},
        {"key":"2","color":"#ff004d"},{"key":"o","color":"#16242a"}],
        "layers":[{"name":"l"}],"frames":[{"name":"a"},{"name":"b"}],
        "cels":[{"layer":0,"frame":0,"rows":["....",".1..","...."]},{"layer":0,"frame":1,"rows":["....","....","...."]}]}"##;
    std::fs::write(dir.join("t.pxe.json"), doc).unwrap();
    let calls = [
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":{"name":"claude-code"}}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"replace_color","arguments":{"path":"t","scope":"all","from":"1","to":"2"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"outline","arguments":{"path":"t","layer":0,"frame":"a","color":"o"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fill","arguments":{"path":"t","layer":0,"frame":"b","x":0,"y":0,"dither":{"pattern":"checker","colors":["1","2"]}}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"history","arguments":{"path":"t"}}}"#,
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_pixel-editor"))
        .args(["mcp", dir.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all((calls.join("\n") + "\n").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("replace_color 1 -> 2: all frames, all layers: 1 pixels in 2 cels"), "{text}");
    assert!(text.contains("outline outside o 4-connected") && text.contains("dither checker of 1 and 2"), "{text}");
    assert!(text.contains("Claude: outline outside o") && text.contains("Claude: replace_color 1 -> 2"), "{text}");
    let saved = std::fs::read_to_string(dir.join("t.pxe.json")).unwrap();
    assert!(saved.contains(r#"".o..""#) && saved.contains(r#""o2o.""#), "{saved}");
    assert!(saved.contains("1212") && saved.contains("2121"), "{saved}");
    std::fs::remove_dir_all(&dir).unwrap();
}
