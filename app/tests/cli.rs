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
