//! Editor settings that survive a restart (`~/.config/pixel-editor/settings.json`,
//! or `$XDG_CONFIG_HOME/pixel-editor/`): the mini preview and the open tabs
//! per workdir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PreviewBg {
    #[default]
    Checker,
    Dark,
    Light,
}

impl PreviewBg {
    pub const ALL: [PreviewBg; 3] = [PreviewBg::Checker, PreviewBg::Dark, PreviewBg::Light];

    pub fn label(self) -> &'static str {
        match self {
            PreviewBg::Checker => "Schachbrett",
            PreviewBg::Dark => "Dunkel",
            PreviewBg::Light => "Hell",
        }
    }

    /// Plain background color (the checker uses `render::CHECKER`).
    pub fn color(self) -> [u8; 4] {
        match self {
            PreviewBg::Checker => pixel_core::render::CHECKER[0],
            PreviewBg::Dark => [24, 24, 28, 255],
            PreviewBg::Light => [235, 235, 235, 255],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PreviewSettings {
    pub open: bool,
    /// Physical screen pixels per art pixel: 1, 2 or 3.
    pub scale: u32,
    pub background: PreviewBg,
    pub collapsed: bool,
    /// Top-left corner in points; None = top-right corner of the canvas.
    pub pos: Option<[f32; 2]>,
    /// Show the frame tiled (as the Kachel-Modus does, 3x3 when it is off).
    pub tiled: bool,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        PreviewSettings {
            open: true,
            scale: 2,
            background: PreviewBg::Checker,
            collapsed: false,
            pos: None,
            tiled: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Settings {
    pub preview: PreviewSettings,
    /// Open documents per window workdir, reopened on the next start.
    pub open_tabs: std::collections::BTreeMap<String, Vec<PathBuf>>,
    /// Kachel-Modus per document (absolute path).
    pub tile_modes: std::collections::BTreeMap<String, super::tiled::TileMode>,
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pixel-editor").join("settings.json"))
}

impl Settings {
    /// Missing or unreadable file: defaults.
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        pixel_core::io::write_atomic(path, serde_json::to_string_pretty(self).unwrap().as_bytes())
    }
}
