//! Pixel-art knowledge in three layers, merged when read:
//! - built-in: `knowledge/*.md` of the repo, compiled into the binary;
//! - personal: owned by this installation (`$PXE_KNOWLEDGE_DIR`, else
//!   `$XDG_DATA_HOME/pixel-editor/knowledge/`, `~/.local/share/...`);
//! - project: `<workdir>/.pxe-knowledge/`.
//!
//! On conflict the project beats personal, personal beats built-in; the
//! layers are shown in that reading order (built-in first, project last),
//! so the later text has the last word.

use std::path::{Path, PathBuf};

/// The topics shipped with the editor (name, text).
pub const BUILT_IN: &[(&str, &str)] = &[
    ("workflow", include_str!("../../knowledge/workflow.md")),
    ("fundamentals", include_str!("../../knowledge/fundamentals.md")),
    ("animation-principles", include_str!("../../knowledge/animation-principles.md")),
    ("pixel-animation", include_str!("../../knowledge/pixel-animation.md")),
    ("checklist", include_str!("../../knowledge/checklist.md")),
];

/// Folder of the project layer inside the workdir.
pub const PROJECT_DIR: &str = ".pxe-knowledge";

/// Where knowledge comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    BuiltIn,
    Personal,
    Project,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::BuiltIn => "built-in",
            Layer::Personal => "personal",
            Layer::Project => "project",
        }
    }
}

/// One topic of one layer.
pub struct Entry {
    pub layer: Layer,
    pub text: String,
    /// The file (personal/project); `None` for built-in.
    pub path: Option<PathBuf>,
}

/// The personal layer's folder: `$PXE_KNOWLEDGE_DIR`, else
/// `$XDG_DATA_HOME/pixel-editor/knowledge` (`~/.local/share/...`); in unit
/// tests only the variable (tests inject their own folder).
pub fn personal_dir_from_env() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PXE_KNOWLEDGE_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if cfg!(test) {
        return None;
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")));
    base.map(|b| b.join("pixel-editor").join("knowledge"))
}

/// Topic names: lowercase letters, digits and '-', not starting with '-'.
pub fn valid_topic(topic: &str) -> Result<(), String> {
    let ok = !topic.is_empty()
        && topic.len() <= 64
        && !topic.starts_with('-')
        && topic.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid topic {topic:?}: use lowercase letters, digits and '-' (e.g. \"walk-cycles\")"))
    }
}

/// Knowledge as seen from one workdir.
pub struct Knowledge {
    pub personal: Option<PathBuf>,
    pub project: PathBuf,
}

impl Knowledge {
    pub fn new(personal: Option<PathBuf>, workdir: &Path) -> Knowledge {
        Knowledge { personal, project: workdir.join(PROJECT_DIR) }
    }

    /// The folder of a writable layer.
    pub fn dir(&self, layer: Layer) -> Result<&Path, String> {
        match layer {
            Layer::BuiltIn => Err("the built-in layer is part of the editor and read-only".into()),
            Layer::Personal => self.personal.as_deref().ok_or_else(|| {
                "no personal knowledge folder (set PXE_KNOWLEDGE_DIR, XDG_DATA_HOME or HOME)".to_string()
            }),
            Layer::Project => Ok(&self.project),
        }
    }

    /// All topic names of all layers, sorted (built-in first in their order,
    /// then the others alphabetically).
    pub fn topics(&self) -> Vec<String> {
        let mut out: Vec<String> = BUILT_IN.iter().map(|(n, _)| n.to_string()).collect();
        let mut extra = Vec::new();
        for dir in [self.personal.as_deref(), Some(self.project.as_path())].into_iter().flatten() {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(t) = name.strip_suffix(".md") {
                    if valid_topic(t).is_ok() && !out.iter().any(|o| o == t) && !extra.iter().any(|o| o == t) {
                        extra.push(t.to_string());
                    }
                }
            }
        }
        extra.sort();
        out.extend(extra);
        out
    }

    /// One topic in reading order: built-in, personal, project (only the
    /// layers that have it; empty files are skipped).
    pub fn entries(&self, topic: &str) -> Vec<Entry> {
        let mut out = Vec::new();
        if let Some((_, text)) = BUILT_IN.iter().find(|(n, _)| *n == topic) {
            out.push(Entry { layer: Layer::BuiltIn, text: text.to_string(), path: None });
        }
        for (layer, dir) in
            [(Layer::Personal, self.personal.as_deref()), (Layer::Project, Some(self.project.as_path()))]
        {
            let Some(dir) = dir else { continue };
            let path = dir.join(format!("{topic}.md"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.trim().is_empty() {
                    out.push(Entry { layer, text, path: Some(path) });
                }
            }
        }
        out
    }

    /// The path and line `append` would write.
    pub fn entry_for(
        &self,
        layer: Layer,
        topic: &str,
        text: &str,
        source: Option<&str>,
        date: &str,
    ) -> Result<(PathBuf, String), String> {
        valid_topic(topic)?;
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Err("text is empty".into());
        }
        let path = self.dir(layer)?.join(format!("{topic}.md"));
        let line = match source.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => format!("- {date}: {text} ({s})"),
            None => format!("- {date}: {text}"),
        };
        Ok((path, line))
    }

    /// Append a dated line to a layer's `<topic>.md`, creating the folder and
    /// the file (with a `# <topic>` heading) if needed.
    pub fn append(&self, path: &Path, topic: &str, line: &str) -> Result<(), String> {
        let dir = path.parent().ok_or("no folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let mut content = std::fs::read_to_string(path).unwrap_or_default();
        if content.trim().is_empty() {
            content = format!("# {topic}\n\n");
        } else if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(line);
        content.push('\n');
        std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

/// The first heading (without '#') or else the first non-empty line.
pub fn summary(text: &str) -> &str {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    first.trim_start_matches('#').trim()
}

/// Today's date (UTC) as YYYY-MM-DD.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    date_of_days((secs / 86_400) as i64)
}

/// Civil date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn date_of_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_of_days(0), "1970-01-01");
        assert_eq!(date_of_days(59), "1970-03-01");
        assert_eq!(date_of_days(11_016), "2000-02-29");
        assert_eq!(date_of_days(20_760), "2026-11-03");
    }

    #[test]
    fn topic_names() {
        for t in ["walk", "walk-cycles", "a1", "checklist"] {
            assert!(valid_topic(t).is_ok(), "{t}");
        }
        for t in ["", "Walk", "../x", "a/b", "a.md", "-x", "a b", "ä"] {
            assert!(valid_topic(t).is_err(), "{t}");
        }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary("\n# Walk cycles\n\ntext"), "Walk cycles");
        assert_eq!(summary("- 2026-01-01: a lesson"), "- 2026-01-01: a lesson");
    }
}
