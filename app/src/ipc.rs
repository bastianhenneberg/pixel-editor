//! One editor window per workdir: the GUI listens on a unix socket, and
//! `pixel-editor open` / MCP `open_editor` hand it the file to open as a tab
//! instead of starting another window.
//!
//! The socket lives in `$XDG_RUNTIME_DIR/pixel-editor/` (or
//! `/run/user/<uid>/pixel-editor/`), named after a hash of the window's
//! workdir, so a client finds the window for a file by trying the file's
//! directories from the innermost outwards. Protocol: one JSON line each way.
//! Request `{"cmd":"open","path":"/abs/file"}` or `{"cmd":"ping"}`, reply
//! `{"ok":true,"pid":123,"workdir":"/abs","result":"opened"|"switched"|"shown"|"queued"|"pong"}`
//! or `{"ok":false,"error":"…"}`.
//!
//! A socket file whose window is gone (crash, kill) refuses connections; the
//! client skips it and the next window for that workdir replaces it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// How long the listener waits for the GUI to handle a request. A window
/// that is not painted (hidden workspace) answers "queued" and opens the
/// file on its next frame.
const REPLY_WAIT: Duration = Duration::from_secs(3);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the sockets live: `PIXEL_EDITOR_IPC_DIR` (tests), else the
/// session's runtime dir.
pub fn socket_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PIXEL_EDITOR_IPC_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let r = PathBuf::from(format!("/run/user/{}", crate::launch::current_uid()));
            r.is_dir().then_some(r)
        })
        .unwrap_or_else(|| std::env::temp_dir().join(format!("pixel-editor-{}", crate::launch::current_uid())));
    runtime.join("pixel-editor")
}

/// FNV-1a, stable across runs and builds (unlike `DefaultHasher`).
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

pub fn socket_path(dir: &Path, workdir: &Path) -> PathBuf {
    dir.join(format!("{:016x}.sock", fnv64(workdir.as_os_str().as_encoded_bytes())))
}

/// The workdir a window for `doc` gets when nothing else says: the current
/// directory if the file is inside it, else the file's folder.
pub fn default_workdir(doc: &Path) -> PathBuf {
    let parent = doc.parent().map(Path::to_path_buf).unwrap_or_default();
    match std::env::current_dir().and_then(|d| d.canonicalize()) {
        Ok(cwd) if doc.starts_with(&cwd) => cwd,
        _ => parent,
    }
}

/// Wakes the GUI when a request arrives (set once the window exists).
type Waker = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// A request from another process, answered by the GUI thread.
pub struct Request {
    pub msg: Value,
    reply: Sender<Value>,
}

impl Request {
    pub fn reply(self, v: Value) {
        let _ = self.reply.send(v);
    }
}

/// The GUI's end: the socket and the queue of requests.
pub struct Server {
    pub path: PathBuf,
    pub workdir: PathBuf,
    rx: Receiver<Request>,
    waker: Waker,
}

impl Server {
    /// Listen for `workdir`. Fails if a live window already listens there; a
    /// stale socket file is replaced.
    pub fn listen(dir: &Path, workdir: &Path) -> Result<Server, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = socket_path(dir, workdir);
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(format!("another editor window already serves {} ({})", workdir.display(), path.display()));
            }
            std::fs::remove_file(&path).map_err(|e| format!("stale socket {}: {e}", path.display()))?;
        }
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (tx, rx) = mpsc::channel();
        let waker: Waker = Default::default();
        let (w, wd) = (waker.clone(), workdir.to_path_buf());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &tx, &w, &wd);
            }
        });
        Ok(Server { path, workdir: workdir.to_path_buf(), rx, waker })
    }

    /// Called when a request arrives (the GUI repaints to handle it).
    pub fn set_waker(&self, f: impl Fn() + Send + 'static) {
        *self.waker.lock().unwrap() = Some(Box::new(f));
    }

    pub fn try_recv(&self) -> Option<Request> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn base_reply(workdir: &Path) -> Value {
    json!({"ok": true, "pid": std::process::id(), "workdir": workdir})
}

fn serve(stream: UnixStream, tx: &Sender<Request>, waker: &Mutex<Option<Box<dyn Fn() + Send>>>, workdir: &Path) {
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    let mut line = String::new();
    let Ok(mut writer) = stream.try_clone() else { return };
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Err(e) => json!({"ok": false, "error": format!("bad request: {e}")}),
        Ok(msg) if msg["cmd"] == "ping" => {
            let mut r = base_reply(workdir);
            r["result"] = "pong".into();
            r
        }
        Ok(msg) => {
            let (rtx, rrx) = mpsc::channel();
            if tx.send(Request { msg, reply: rtx }).is_err() {
                return; // the window is closing
            }
            if let Some(w) = waker.lock().unwrap().as_ref() {
                w();
            }
            rrx.recv_timeout(REPLY_WAIT).unwrap_or_else(|_| {
                let mut r = base_reply(workdir);
                r["result"] = "queued".into();
                r
            })
        }
    };
    let _ = writeln!(writer, "{reply}");
}

/// Reply for an `open` request handled by the GUI.
pub fn open_reply(workdir: &Path, result: &str) -> Value {
    let mut r = base_reply(workdir);
    r["result"] = result.into();
    r
}

/// Send `msg` to the window at `sock`. `None`: no live window there (a stale
/// socket file counts as none).
fn send_to(sock: &Path, msg: &Value) -> Option<Result<Value, String>> {
    let mut stream = UnixStream::connect(sock).ok()?;
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT + REPLY_WAIT));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    let result = (|| {
        writeln!(stream, "{msg}").map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply {line:?}: {e}"))?;
        if v["ok"] == true {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("the editor refused").to_string())
        }
    })();
    Some(result.map_err(|e| format!("editor window at {}: {e}", sock.display())))
}

/// Workdirs whose window could take `doc`: `prefer` first (if the file is
/// inside it), then the file's folders from the innermost outwards.
fn candidates(doc: &Path, prefer: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = prefer.filter(|p| doc.starts_with(p)).map(Path::to_path_buf).into_iter().collect();
    for a in doc.ancestors().skip(1) {
        if !out.iter().any(|p| p == a) {
            out.push(a.to_path_buf());
        }
    }
    out
}

/// Ask the running window that covers `doc` (see `candidates`) to handle
/// `msg`. `None`: no window is running for it.
pub fn send(dir: &Path, doc: &Path, prefer: Option<&Path>, msg: &Value) -> Option<Result<Value, String>> {
    candidates(doc, prefer).iter().find_map(|wd| {
        let sock = socket_path(dir, wd);
        if sock.exists() {
            send_to(&sock, msg)
        } else {
            None
        }
    })
}

pub fn open_msg(doc: &Path) -> Value {
    json!({"cmd": "open", "path": doc})
}

pub fn ping_msg() -> Value {
    json!({"cmd": "ping"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pxe-ipc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stand-in GUI: answer every open with "opened".
    fn answer_in_background(server: Server) -> std::thread::JoinHandle<Vec<Value>> {
        std::thread::spawn(move || {
            let mut got = Vec::new();
            let start = std::time::Instant::now();
            while got.is_empty() && start.elapsed() < Duration::from_secs(5) {
                if let Some(r) = server.try_recv() {
                    got.push(r.msg.clone());
                    r.reply(open_reply(&server.workdir, "opened"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            got
        })
    }

    #[test]
    fn finds_the_window_of_an_enclosing_workdir_and_skips_stale_sockets() {
        let d = dir("find");
        let (socks, work) = (d.join("socks"), d.join("art"));
        std::fs::create_dir_all(work.join("hero")).unwrap();
        let doc = work.join("hero/walk.pxe.json");
        assert!(send(&socks, &doc, None, &ping_msg()).is_none(), "nothing running");

        // a stale socket (its window died) for the inner folder is skipped
        std::fs::create_dir_all(&socks).unwrap();
        drop(UnixListener::bind(socket_path(&socks, &work.join("hero"))).unwrap());
        assert!(socket_path(&socks, &work.join("hero")).exists());
        assert!(send(&socks, &doc, None, &ping_msg()).is_none());

        let server = Server::listen(&socks, &work).unwrap();
        assert!(Server::listen(&socks, &work).is_err(), "one window per workdir");
        let pong = send(&socks, &doc, None, &ping_msg()).unwrap().unwrap();
        assert_eq!((pong["result"].as_str(), pong["pid"].as_u64()), (Some("pong"), Some(std::process::id() as u64)));
        let gui = answer_in_background(server);
        let r = send(&socks, &doc, Some(&work), &open_msg(&doc)).unwrap().unwrap();
        assert_eq!(r["result"], "opened");
        assert_eq!(gui.join().unwrap(), vec![open_msg(&doc)]);
        // the server is gone with its window: its socket file too
        assert!(!socket_path(&socks, &work).exists());
        // a new window replaces a stale socket file for its own workdir
        drop(UnixListener::bind(socket_path(&socks, &work)).unwrap());
        let again = Server::listen(&socks, &work).unwrap();
        assert!(send(&socks, &doc, None, &ping_msg()).unwrap().is_ok());
        drop(again);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_window_that_does_not_paint_answers_queued() {
        let d = dir("queued");
        let work = d.join("w");
        std::fs::create_dir_all(&work).unwrap();
        let server = Server::listen(&d.join("s"), &work).unwrap();
        let r = send(&d.join("s"), &work.join("a.pxe.json"), None, &open_msg(&work.join("a.pxe.json")));
        assert_eq!(r.unwrap().unwrap()["result"], "queued");
        // the request is still there for the next frame
        assert!(server.try_recv().is_some());
        drop(server);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
