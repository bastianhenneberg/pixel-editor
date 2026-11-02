//! Start the GUI for a document on the user's screen, detached from the
//! caller (CLI `pixel-editor open`, MCP `open_editor`). If an editor window
//! for the workdir is already running, the document opens there as a tab
//! (see `crate::ipc`). The GUI also records its pid next to every open
//! document's journal (`gui.pid`), which older editors without tabs use.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn pid_file(doc: &Path) -> PathBuf {
    pixel_core::journal::Journal::for_document(doc).dir().join("gui.pid")
}

/// Called by the GUI when it shows `doc`.
pub fn register_gui(doc: &Path) {
    let f = pid_file(doc);
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(f, std::process::id().to_string());
}

/// Called by the GUI when it closes or switches documents.
pub fn unregister_gui(doc: &Path) {
    let f = pid_file(doc);
    if std::fs::read_to_string(&f).ok().and_then(|s| s.trim().parse::<u32>().ok()) == Some(std::process::id()) {
        let _ = std::fs::remove_file(f);
    }
}

/// Pid of a running editor that shows `doc`, if any.
pub fn running_gui(doc: &Path) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file(doc)).ok()?.trim().parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmdline = String::from_utf8_lossy(&cmdline);
    (cmdline.contains("pixel-editor") && !cmdline.contains("\0mcp\0")).then_some(pid)
}

/// How long a freshly started editor must survive to count as opened.
const STARTUP_CHECK: Duration = Duration::from_millis(1500);

/// Display variables the GUI needs but the caller lacks. The MCP server is
/// started without WAYLAND_DISPLAY/DISPLAY, so they are derived from the
/// session: the newest `wayland-*` socket in XDG_RUNTIME_DIR (default
/// /run/user/<uid>), DISPLAY=:0 only as a fallback when an X0 socket exists,
/// and the Hyprland instance if there is exactly one.
pub fn display_env(get: &dyn Fn(&str) -> Option<String>, uid: u32, x11_dir: &Path) -> Vec<(String, String)> {
    let set = |k: &str| get(k).is_some_and(|v| !v.is_empty());
    let mut out = Vec::new();
    let runtime = match get("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(r) => PathBuf::from(r),
        None => {
            let r = PathBuf::from(format!("/run/user/{uid}"));
            if r.is_dir() {
                out.push(("XDG_RUNTIME_DIR".into(), r.display().to_string()));
            }
            r
        }
    };
    if !set("WAYLAND_DISPLAY") && !set("WAYLAND_SOCKET") && !set("DISPLAY") {
        match newest_wayland_socket(&runtime) {
            Some(name) => out.push(("WAYLAND_DISPLAY".into(), name)),
            None if x11_dir.join("X0").exists() => out.push(("DISPLAY".into(), ":0".into())),
            None => {}
        }
    }
    if !set("HYPRLAND_INSTANCE_SIGNATURE") {
        let instances: Vec<String> = std::fs::read_dir(runtime.join("hypr"))
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if let [one] = instances.as_slice() {
            out.push(("HYPRLAND_INSTANCE_SIGNATURE".into(), one.clone()));
        }
    }
    out
}

fn newest_wayland_socket(runtime: &Path) -> Option<String> {
    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;
    std::fs::read_dir(runtime)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("wayland-") || name.ends_with(".lock") {
                return None;
            }
            let meta = e.metadata().ok()?;
            #[cfg(unix)]
            if !meta.file_type().is_socket() {
                return None;
            }
            Some((meta.modified().ok()?, name))
        })
        .max()
        .map(|(_, name)| name)
}

pub(crate) fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(m) = std::fs::metadata("/proc/self") {
            return m.uid();
        }
    }
    0
}

/// Where the started editor's stderr goes.
pub fn log_file(doc: &Path) -> PathBuf {
    pixel_core::journal::Journal::for_document(doc).dir().join("gui.log")
}

/// Open `doc` in the editor: as a tab of the running window whose workdir
/// contains it (`workdir` preferred, e.g. the MCP server's), else in a new
/// window for `workdir` (default: see `ipc::default_workdir`).
pub fn open_editor(doc: &Path, dry_run: bool, workdir: Option<&Path>) -> Result<String, String> {
    open_editor_with(doc, dry_run, workdir, &crate::ipc::socket_dir())
}

pub fn open_editor_with(doc: &Path, dry_run: bool, workdir: Option<&Path>, ipc_dir: &Path) -> Result<String, String> {
    use crate::ipc;
    let doc = doc.canonicalize().map_err(|e| format!("{}: {e}", doc.display()))?;
    let workdir = workdir.and_then(|w| w.canonicalize().ok()).filter(|w| doc.starts_with(w));
    let msg = if dry_run { ipc::ping_msg() } else { ipc::open_msg(&doc) };
    if let Some(reply) = ipc::send(ipc_dir, &doc, workdir.as_deref(), &msg) {
        let r = reply?;
        let pid = r["pid"].as_u64().unwrap_or(0);
        let text = match r["result"].as_str().unwrap_or("") {
            "pong" => format!("would open {} as tab in the running editor (pid {pid})", doc.display()),
            "switched" => format!("switched to the tab of {} in the running editor (pid {pid})", doc.display()),
            "shown" => format!("the running editor (pid {pid}) already shows {}", doc.display()),
            "queued" => format!(
                "sent {} to the running editor (pid {pid}); it opens as tab when the window repaints",
                doc.display()
            ),
            _ => format!("opened {} as tab in the running editor (pid {pid})", doc.display()),
        };
        return Ok(format!("{text}; it follows edits live"));
    }
    if let Some(pid) = running_gui(&doc) {
        // an editor without tabs (older build) shows it
        return Ok(format!("the editor already shows {} (pid {pid}); it follows edits live", doc.display()));
    }
    let workdir = workdir.unwrap_or_else(|| ipc::default_workdir(&doc));
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let env = display_env(&|k| std::env::var(k).ok(), current_uid(), Path::new("/tmp/.X11-unix"));
    let env_text: String = env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
    if dry_run {
        return Ok(format!("would run: {env_text}{} --workdir {} {}", exe.display(), workdir.display(), doc.display()));
    }
    let log = log_file(&doc);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let stderr = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut cmd = Command::new(&exe);
    cmd.arg("--workdir")
        .arg(&workdir)
        .arg(&doc)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // detach from the caller's process group
    }
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    let pid = child.id();
    let started = Instant::now();
    while started.elapsed() < STARTUP_CHECK {
        if let Ok(Some(status)) = child.try_wait() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let text = text.trim();
            let tail: Vec<&str> = text.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
            return Err(format!(
                "the editor exited right after starting ({status}){}{}\nlog: {}",
                if env_text.is_empty() { String::new() } else { format!(", started with {}", env_text.trim()) },
                if tail.is_empty() { ", no output".to_string() } else { format!(":\n{}", tail.join("\n")) },
                log.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // reap it when it closes, so a long-running MCP server leaves no zombie
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(format!(
        "opened {} in a new editor window (pid {pid}, workdir {}); it follows edits live; log: {}",
        doc.display(),
        workdir.display(),
        log.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn derives_display_env_from_the_runtime_dir() {
        let dir = std::env::temp_dir().join(format!("pxe-launch-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (run, x11) = (dir.join("run"), dir.join("x11"));
        std::fs::create_dir_all(run.join("hypr/abc_123")).unwrap();
        std::fs::create_dir_all(&x11).unwrap();
        let rt = run.display().to_string();
        let get = |k: &str, v: &[(&str, &str)]| display_env(&env_of(v), 1000, &x11).into_iter().find(|(n, _)| n == k);

        // nothing found: no WAYLAND_DISPLAY, and no DISPLAY without an X0 socket
        assert_eq!(get("WAYLAND_DISPLAY", &[("XDG_RUNTIME_DIR", &rt)]), None);
        assert_eq!(get("DISPLAY", &[("XDG_RUNTIME_DIR", &rt)]), None);
        std::fs::write(x11.join("X0"), "").unwrap();
        assert_eq!(get("DISPLAY", &[("XDG_RUNTIME_DIR", &rt)]), Some(("DISPLAY".into(), ":0".into())));

        // sockets: the newest wins, .lock files and plain files are ignored
        let _a = std::os::unix::net::UnixListener::bind(run.join("wayland-0")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let _b = std::os::unix::net::UnixListener::bind(run.join("wayland-1")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(run.join("wayland-1.lock"), "").unwrap();
        std::fs::write(run.join("wayland-9"), "not a socket").unwrap();
        let all = display_env(&env_of(&[("XDG_RUNTIME_DIR", &rt)]), 1000, &x11);
        assert_eq!(
            all,
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
                ("HYPRLAND_INSTANCE_SIGNATURE".to_string(), "abc_123".to_string()),
            ],
            "wayland found, so no DISPLAY fallback"
        );

        // variables that are set are kept
        let set = display_env(
            &env_of(&[("XDG_RUNTIME_DIR", &rt), ("DISPLAY", ":1"), ("HYPRLAND_INSTANCE_SIGNATURE", "x")]),
            1000,
            &x11,
        );
        assert!(set.is_empty(), "{set:?}");
        // two Hyprland instances: ambiguous, not passed
        std::fs::create_dir_all(run.join("hypr/other")).unwrap();
        assert_eq!(get("HYPRLAND_INSTANCE_SIGNATURE", &[("XDG_RUNTIME_DIR", &rt)]), None);
        // XDG_RUNTIME_DIR unset: /run/user/<uid>, only if it exists
        let none = display_env(&env_of(&[]), 4_000_000_000, &x11);
        assert!(none.iter().all(|(k, _)| k != "XDG_RUNTIME_DIR"), "{none:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
