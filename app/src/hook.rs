//! On-save hook: a command from `<workdir>/.pxe-project.json` that runs
//! after a document was saved (GUI and MCP), e.g. a project's own export.
//!
//! ```json
//! {"on_save": ["python3", "../tools/export.py", "--only", "{file}"], "label": "Export"}
//! ```
//!
//! Placeholders in every argument: `{file}` (the saved document, relative
//! to the workdir), `{abs}` (absolute) and `{workdir}`. The command runs in
//! the workdir, in the background, debounced (`DEBOUNCE`): a series of
//! saves gives one run, and saves during a run give exactly one more run
//! afterwards. With `{file}`/`{abs}` it runs once per saved document.
//!
//! A repository must not run commands just by being opened: a command runs
//! only once the user trusted it for this workdir. The consent lives outside
//! the workdir (`trusted.json` in the config directory) as the SHA-256 of
//! the command, so a changed command asks again.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const PROJECT_FILE: &str = ".pxe-project.json";
/// Quiet time after the last save before the command runs.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// A run that takes longer is killed.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Kept output per run (the end, where errors are).
const OUTPUT_MAX: usize = 16 * 1024;

// ---- configuration --------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Program and arguments, with placeholders.
    pub on_save: Vec<String>,
    /// Name in the status bar ("Export"); default "on_save".
    pub label: String,
}

impl Config {
    /// The workdir's hook: None without file or without `on_save`.
    pub fn load(workdir: &Path) -> Result<Option<Config>, String> {
        let path = workdir.join(PROJECT_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{PROJECT_FILE}: {e}"))?;
        let Some(cmd) = v.get("on_save").filter(|c| !c.is_null()) else {
            return Ok(None);
        };
        let on_save: Vec<String> = cmd
            .as_array()
            .and_then(|a| a.iter().map(|s| s.as_str().map(str::to_string)).collect())
            .ok_or(format!("{PROJECT_FILE}: on_save must be a list of strings (program, arguments...)"))?;
        if on_save.first().is_none_or(|p| p.trim().is_empty()) {
            return Err(format!("{PROJECT_FILE}: on_save needs at least the program"));
        }
        let label = v.get("label").and_then(|l| l.as_str()).filter(|l| !l.trim().is_empty()).unwrap_or("on_save");
        Ok(Some(Config { on_save, label: label.trim().to_string() }))
    }

    /// What the trust store remembers: SHA-256 of the command as JSON.
    pub fn hash(&self) -> String {
        sha256_hex(serde_json::to_string(&self.on_save).unwrap().as_bytes())
    }

    /// The command for display: arguments with spaces quoted.
    pub fn display(&self) -> String {
        self.on_save
            .iter()
            .map(|a| if a.is_empty() || a.contains(char::is_whitespace) { format!("{a:?}") } else { a.clone() })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn per_file(&self) -> bool {
        self.on_save.iter().any(|a| a.contains("{file}") || a.contains("{abs}"))
    }

    fn args_for(&self, workdir: &Path, file: &str) -> Vec<String> {
        let abs = workdir.join(file);
        self.on_save
            .iter()
            .map(|a| {
                a.replace("{file}", file)
                    .replace("{abs}", &abs.display().to_string())
                    .replace("{workdir}", &workdir.display().to_string())
            })
            .collect()
    }
}

// ---- trust ------------------------------------------------------------------

/// Which workdir trusts which command, kept outside every workdir.
#[derive(Clone, Debug)]
pub struct TrustStore {
    /// None: nothing is trusted and nothing is stored (tests).
    path: Option<PathBuf>,
}

/// What a workdir's hook may do.
#[derive(Clone, Debug, PartialEq)]
pub enum Trust {
    /// No `.pxe-project.json` or no `on_save`.
    None,
    Trusted(Config),
    /// Configured, but not (or not in this form) confirmed by the user.
    Untrusted(Config),
    /// The project file cannot be used.
    Invalid(String),
}

impl TrustStore {
    /// `$PIXEL_EDITOR_TRUST_FILE`, else `$XDG_CONFIG_HOME/pixel-editor/trusted.json`
    /// (`~/.config/...`); in unit tests only the variable.
    pub fn from_env() -> TrustStore {
        if let Some(p) = std::env::var_os("PIXEL_EDITOR_TRUST_FILE").filter(|v| !v.is_empty()) {
            return TrustStore::at(PathBuf::from(p));
        }
        if cfg!(test) {
            return TrustStore { path: None };
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        TrustStore { path: base.map(|b| b.join("pixel-editor").join("trusted.json")) }
    }

    pub fn at(path: PathBuf) -> TrustStore {
        TrustStore { path: Some(path) }
    }

    fn read(&self) -> BTreeMap<String, String> {
        self.path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn key(workdir: &Path) -> String {
        workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf()).display().to_string()
    }

    pub fn is_trusted(&self, workdir: &Path, cfg: &Config) -> bool {
        self.read().get(&Self::key(workdir)) == Some(&cfg.hash())
    }

    /// Remember that `workdir` may run `cfg` (replaces an older command).
    pub fn trust(&self, workdir: &Path, cfg: &Config) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("no place to store the consent (no config directory)".into());
        };
        let mut all = self.read();
        all.insert(Self::key(workdir), cfg.hash());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&all).unwrap())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The workdir's hook and whether it may run (reads the file each time,
    /// so an edited command is noticed at the next save).
    pub fn check(&self, workdir: &Path) -> Trust {
        match Config::load(workdir) {
            Ok(None) => Trust::None,
            Ok(Some(cfg)) if self.is_trusted(workdir, &cfg) => Trust::Trusted(cfg),
            Ok(Some(cfg)) => Trust::Untrusted(cfg),
            Err(e) => Trust::Invalid(e),
        }
    }
}

// ---- runner -----------------------------------------------------------------

/// One finished run.
#[derive(Clone, Debug)]
pub struct RunResult {
    pub label: String,
    /// The saved documents it ran for (relative to the workdir).
    pub files: Vec<String>,
    /// The command as run (placeholders filled).
    pub command: String,
    pub ok: bool,
    /// Exit code; None if it could not start, was killed or died by a signal.
    pub code: Option<i32>,
    pub duration: Duration,
    /// stdout, then stderr (the end, if long), or why it did not run.
    pub output: String,
    pub finished: SystemTime,
}

/// What the runner is doing, for status bars and `hook_status`.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub label: String,
    /// Running since, for these files.
    pub running: Option<(Instant, Vec<String>)>,
    /// Saved, waiting for the quiet time or the running command.
    pub pending: Vec<String>,
    pub last: Option<RunResult>,
    /// The last result per file.
    pub by_file: BTreeMap<String, RunResult>,
    /// Number of command runs so far.
    pub runs: u64,
}

#[derive(Default)]
struct State {
    status: Status,
    config: Option<Config>,
    due: Option<Instant>,
    worker: bool,
}

struct Shared {
    workdir: PathBuf,
    debounce: Duration,
    state: Mutex<State>,
    changed: Condvar,
}

/// Runs a workdir's hook in the background. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Runner {
    shared: Arc<Shared>,
}

impl Runner {
    pub fn new(workdir: &Path) -> Runner {
        Runner::with_debounce(workdir, DEBOUNCE)
    }

    pub fn with_debounce(workdir: &Path, debounce: Duration) -> Runner {
        let workdir = workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf());
        Runner { shared: Arc::new(Shared { workdir, debounce, state: Mutex::default(), changed: Condvar::new() }) }
    }

    pub fn workdir(&self) -> &Path {
        &self.shared.workdir
    }

    /// The document `path` was saved: run `cfg` for it after the quiet time.
    /// False if `path` lies outside the workdir (nothing queued).
    pub fn notify(&self, cfg: &Config, path: &Path) -> bool {
        let Some(rel) = self.relative(path) else {
            return false;
        };
        let mut st = self.shared.state.lock().unwrap();
        if !st.status.pending.contains(&rel) {
            st.status.pending.push(rel);
        }
        st.status.label = cfg.label.clone();
        st.config = Some(cfg.clone());
        st.due = Some(Instant::now() + self.shared.debounce);
        if !st.worker {
            st.worker = true;
            let shared = self.shared.clone();
            std::thread::spawn(move || worker(&shared));
        }
        self.shared.changed.notify_all();
        true
    }

    fn relative(&self, path: &Path) -> Option<String> {
        let abs = match path.parent().and_then(|d| d.canonicalize().ok()) {
            Some(d) => d.join(path.file_name()?),
            None => path.to_path_buf(),
        };
        let rel = abs.strip_prefix(&self.shared.workdir).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
    }

    pub fn status(&self) -> Status {
        self.shared.state.lock().unwrap().status.clone()
    }

    /// Wait until nothing is pending or running; false on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut st = self.shared.state.lock().unwrap();
        loop {
            if st.status.pending.is_empty() && st.status.running.is_none() {
                return true;
            }
            let now = Instant::now();
            if now >= end {
                return false;
            }
            st = self.shared.changed.wait_timeout(st, end - now).unwrap().0;
        }
    }
}

fn wake(shared: &Shared) {
    shared.changed.notify_all();
}

/// Runs while there is something to do, then ends (`notify` starts a new one).
fn worker(shared: &Shared) {
    loop {
        let (cfg, files) = {
            let mut st = shared.state.lock().unwrap();
            loop {
                match st.due {
                    None => {
                        st.worker = false;
                        return;
                    }
                    Some(due) => {
                        let now = Instant::now();
                        if now >= due {
                            break;
                        }
                        st = shared.changed.wait_timeout(st, due - now).unwrap().0;
                    }
                }
            }
            st.due = None;
            let files = std::mem::take(&mut st.status.pending);
            st.status.running = Some((Instant::now(), files.clone()));
            (st.config.clone().expect("notify sets the config"), files)
        };
        wake(shared);
        // with {file}/{abs} once per document, otherwise once for all
        let batches: Vec<Vec<String>> =
            if cfg.per_file() { files.iter().map(|f| vec![f.clone()]).collect() } else { vec![files] };
        for batch in batches {
            let args = cfg.args_for(&shared.workdir, batch.first().map_or("", String::as_str));
            let mut result = run(&shared.workdir, &args);
            result.label = cfg.label.clone();
            result.files = batch;
            let mut st = shared.state.lock().unwrap();
            st.status.runs += 1;
            for f in &result.files {
                st.status.by_file.insert(f.clone(), result.clone());
            }
            st.status.last = Some(result);
        }
        shared.state.lock().unwrap().status.running = None;
        wake(shared);
    }
}

/// Run `args` in `workdir` and collect the result (killed after `TIMEOUT`).
fn run(workdir: &Path, args: &[String]) -> RunResult {
    let start = Instant::now();
    let command = Config { on_save: args.to_vec(), label: String::new() }.display();
    let mut result = RunResult {
        label: String::new(),
        files: Vec::new(),
        command,
        ok: false,
        code: None,
        duration: Duration::ZERO,
        output: String::new(),
        finished: SystemTime::now(),
    };
    let child = Command::new(&args[0])
        .args(&args[1..])
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            result.output = format!("could not start {:?}: {e}", args[0]);
            result.duration = start.elapsed();
            return result;
        }
    };
    let collect = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = collect(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = collect(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                killed = true;
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    let mut output = out;
    if !err.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output += &err;
    }
    if killed {
        output += &format!("\n[killed after {} s]", TIMEOUT.as_secs());
    }
    if output.len() > OUTPUT_MAX {
        let mut start = output.len() - OUTPUT_MAX;
        while !output.is_char_boundary(start) {
            start += 1;
        }
        output = format!("[…]\n{}", &output[start..]);
    }
    result.code = status.and_then(|s| s.code());
    result.ok = !killed && status.is_some_and(|s| s.success());
    result.output = output;
    result.duration = start.elapsed();
    result.finished = SystemTime::now();
    result
}

/// "0,4 s" (German decimal comma, as in the status bar).
pub fn seconds_de(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64()).replace('.', ",")
}

// ---- SHA-256 (FIPS 180-4), for the trust store --------------------------------

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pxe-hook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn sh(script: &str) -> Config {
        Config { on_save: vec!["sh".into(), "-c".into(), script.into()], label: "Export".into() }
    }

    #[test]
    fn sha256_known_values() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(sha256_hex(long), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn config_file() {
        let d = dir("config");
        assert_eq!(Config::load(&d), Ok(None), "no file");
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": ["python3", "x.py", "--only", "{file}"]}"#).unwrap();
        let cfg = Config::load(&d).unwrap().unwrap();
        assert_eq!(cfg.label, "on_save");
        assert_eq!(cfg.args_for(&d, "a/b.pxe.json")[3], "a/b.pxe.json");
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": "python3 x.py"}"#).unwrap();
        assert!(Config::load(&d).unwrap_err().contains("list of strings"));
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": []}"#).unwrap();
        assert!(Config::load(&d).unwrap_err().contains("at least the program"));
        std::fs::write(d.join(PROJECT_FILE), r#"{"label": "Export"}"#).unwrap();
        assert_eq!(Config::load(&d), Ok(None), "no on_save");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn trust_is_per_workdir_and_command() {
        let d = dir("trust");
        let store = TrustStore::at(d.join("cfg/trusted.json"));
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": ["true"], "label": "Export"}"#).unwrap();
        let Trust::Untrusted(cfg) = store.check(&d) else { panic!("new workdir is not trusted") };
        store.trust(&d, &cfg).unwrap();
        assert!(matches!(store.check(&d), Trust::Trusted(_)));
        assert!(!std::fs::read_to_string(d.join("cfg/trusted.json")).unwrap().contains("true\""), "only the hash");
        // a changed command asks again; the label alone does not
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": ["true"], "label": "Godot"}"#).unwrap();
        assert!(matches!(store.check(&d), Trust::Trusted(_)));
        std::fs::write(d.join(PROJECT_FILE), r#"{"on_save": ["false"]}"#).unwrap();
        assert!(matches!(store.check(&d), Trust::Untrusted(_)));
        // another workdir with the same command is not trusted
        let other = d.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(PROJECT_FILE), r#"{"on_save": ["true"]}"#).unwrap();
        assert!(matches!(store.check(&other), Trust::Untrusted(_)));
        // no store: never trusted, nothing written
        assert!(matches!(TrustStore { path: None }.check(&d), Trust::Untrusted(_)));
        std::fs::write(d.join(PROJECT_FILE), "{").unwrap();
        assert!(matches!(store.check(&d), Trust::Invalid(_)));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn debounced_run_per_file_with_output() {
        let d = dir("run");
        let r = Runner::with_debounce(&d, Duration::from_millis(100));
        let cfg = sh("echo \"$PWD {file} {workdir}\" >> log.txt; echo out; echo err >&2");
        for _ in 0..10 {
            assert!(r.notify(&cfg, &d.join("a.pxe.json")));
        }
        assert!(r.notify(&cfg, &d.join("b.pxe.json")));
        assert!(!r.notify(&cfg, Path::new("/elsewhere/c.pxe.json")), "outside the workdir");
        assert!(r.wait_idle(Duration::from_secs(10)));
        let s = r.status();
        assert_eq!(s.runs, 2, "one run per file, not per save");
        let log = std::fs::read_to_string(d.join("log.txt")).unwrap();
        let wd = d.display();
        assert_eq!(log, format!("{wd} a.pxe.json {wd}\n{wd} b.pxe.json {wd}\n"));
        let last = s.last.unwrap();
        assert!(last.ok && last.code == Some(0));
        assert_eq!(last.output, "out\nerr\n");
        assert_eq!(s.by_file["a.pxe.json"].files, vec!["a.pxe.json"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn saves_during_a_run_give_one_more_run() {
        let d = dir("again");
        let r = Runner::with_debounce(&d, Duration::from_millis(20));
        let cfg = sh("sleep 0.3; echo x >> log.txt");
        r.notify(&cfg, &d.join("a.pxe.json"));
        let t = Instant::now();
        while r.status().running.is_none() {
            assert!(t.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..5 {
            r.notify(&cfg, &d.join("a.pxe.json"));
        }
        assert!(r.wait_idle(Duration::from_secs(10)));
        assert_eq!(r.status().runs, 2);
        assert_eq!(std::fs::read_to_string(d.join("log.txt")).unwrap(), "x\nx\n");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn failures_are_reported() {
        let d = dir("fail");
        let r = Runner::with_debounce(&d, Duration::ZERO);
        r.notify(&sh("echo broken sheet >&2; exit 3"), &d.join("a.pxe.json"));
        assert!(r.wait_idle(Duration::from_secs(10)));
        let last = r.status().last.unwrap();
        assert!(!last.ok);
        assert_eq!(last.code, Some(3));
        assert_eq!(last.output, "broken sheet\n");
        let missing = Config { on_save: vec!["/nonexistent/tool".into()], label: "x".into() };
        r.notify(&missing, &d.join("a.pxe.json"));
        assert!(r.wait_idle(Duration::from_secs(10)));
        let last = r.status().last.unwrap();
        assert!(!last.ok && last.code.is_none());
        assert!(last.output.contains("could not start"), "{}", last.output);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
