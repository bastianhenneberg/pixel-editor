//! The workdir's on-save hook in the window: run it after saves, ask once
//! whether the folder may run it, show its state in the status bar and its
//! last output on click (see `crate::hook`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Color32};

use super::EditorApp;
use crate::hook::{self, Config, Runner, Trust};

/// The open trust question: the command and the saves waiting for it.
pub struct TrustAsk {
    cfg: Config,
    saved: Vec<PathBuf>,
}

/// Window-wide hook state (not per tab).
pub struct HookState {
    pub trust: hook::TrustStore,
    runner: Option<Runner>,
    ask: Option<TrustAsk>,
    /// Hash of the command the user declined in this session.
    declined: Option<String>,
    pub show_output: bool,
}

impl Default for HookState {
    fn default() -> Self {
        HookState { trust: hook::TrustStore::from_env(), runner: None, ask: None, declined: None, show_output: false }
    }
}

impl HookState {
    pub fn runner(&self) -> Option<&Runner> {
        self.runner.as_ref()
    }

    fn runner_for(&mut self, workdir: &Path) -> &Runner {
        if self.runner.as_ref().is_none_or(|r| r.workdir() != workdir.canonicalize().unwrap_or(workdir.into())) {
            self.runner = Some(Runner::new(workdir));
        }
        self.runner.as_ref().unwrap()
    }

    /// Something saved or opened: run the hook for `saved`, or ask first.
    /// Returns a problem with the project file for the status line.
    fn handle(&mut self, workdir: &Path, saved: Option<&Path>) -> Option<String> {
        match self.trust.check(workdir) {
            Trust::None => None,
            Trust::Invalid(e) => saved.map(|_| format!("on_save nicht ausgeführt: {e}")),
            Trust::Trusted(cfg) => {
                if let Some(p) = saved {
                    self.runner_for(workdir).notify(&cfg, p);
                }
                None
            }
            Trust::Untrusted(cfg) => {
                if self.declined.as_deref() == Some(cfg.hash().as_str()) {
                    return None;
                }
                let ask = self.ask.get_or_insert_with(|| TrustAsk { cfg: cfg.clone(), saved: Vec::new() });
                ask.cfg = cfg;
                if let Some(p) = saved {
                    if !ask.saved.iter().any(|s| s == p) {
                        ask.saved.push(p.to_path_buf());
                    }
                }
                None
            }
        }
    }
}

impl EditorApp {
    /// The command the window is asking about, if any.
    #[cfg(test)]
    pub(super) fn hook_asking(&self) -> Option<&Config> {
        self.hook.ask.as_ref().map(|a| &a.cfg)
    }

    /// The workdir was opened: ask now if it has a hook nobody trusted yet.
    pub(super) fn hook_check_workdir(&mut self) {
        if let Some(w) = self.workdir.clone() {
            self.hook.handle(&w, None);
        }
    }

    /// A document (or map) was saved to `path`.
    pub(super) fn hook_after_save(&mut self, path: &Path) {
        let Some(w) = self.workdir.clone() else { return };
        if let Some(problem) = self.hook.handle(&w, Some(path)) {
            self.status += &format!(" · {problem}");
        }
    }

    /// "Diesem Ordner vertrauen?" and the output window.
    pub(super) fn hook_windows(&mut self, ctx: &egui::Context) {
        if self.hook.runner().is_some_and(|r| {
            let s = r.status();
            s.running.is_some() || !s.pending.is_empty()
        }) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let (Some(ask), Some(workdir)) = (&self.hook.ask, self.workdir.clone()) {
            let mut answer = None;
            egui::Modal::new(egui::Id::unique("hook-trust")).show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.heading("Diesem Ordner vertrauen?");
                ui.label(format!(
                    "{} möchte nach jedem Speichern diesen Befehl ausführen:",
                    workdir.join(hook::PROJECT_FILE).display()
                ));
                ui.add(egui::Label::new(egui::RichText::new(ask.cfg.display()).monospace()).wrap());
                ui.label(
                    "Nur zustimmen, wenn du den Ordner und den Befehl kennst. Die Zustimmung gilt für \
                     diesen Ordner und genau diesen Befehl; ändert er sich, fragt der Editor erneut.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Vertrauen").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Nicht ausführen").clicked() {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => {
                    let ask = self.hook.ask.take().unwrap();
                    match self.hook.trust.trust(&workdir, &ask.cfg) {
                        Ok(()) => {
                            for p in &ask.saved {
                                self.hook.runner_for(&workdir).notify(&ask.cfg, p);
                            }
                            self.status = format!("{} vertraut: läuft nach jedem Speichern", ask.cfg.label);
                        }
                        Err(e) => self.status = format!("Zustimmung nicht gespeichert: {e}"),
                    }
                }
                Some(false) => {
                    let ask = self.hook.ask.take().unwrap();
                    self.hook.declined = Some(ask.cfg.hash());
                    self.status = format!("{} wird in dieser Sitzung nicht ausgeführt", ask.cfg.label);
                }
                None => {}
            }
        }
        if self.hook.show_output {
            let last = self.hook.runner().and_then(|r| r.status().last);
            let mut open = true;
            egui::Window::new("Ausgabe von on_save").open(&mut open).default_size([560.0, 320.0]).show(ctx, |ui| {
                let Some(r) = last else {
                    ui.label("Noch kein Lauf in dieser Sitzung.");
                    return;
                };
                ui.label(format!(
                    "{} für {}: {} in {}",
                    r.label,
                    r.files.join(", "),
                    match (r.ok, r.code) {
                        (true, _) => "ok".to_string(),
                        (false, Some(c)) => format!("Fehler (Exit-Code {c})"),
                        (false, None) => "Fehler".to_string(),
                    },
                    hook::seconds_de(r.duration)
                ));
                ui.add(egui::Label::new(egui::RichText::new(&r.command).monospace().weak()).wrap());
                ui.separator();
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    let text = if r.output.trim().is_empty() { "(keine Ausgabe)" } else { r.output.as_str() };
                    ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
                });
            });
            self.hook.show_output = open;
        }
    }
}

/// The status bar entry: "Export läuft …", "Export ok (0,4 s)", "Export
/// Fehler"; true when clicked (show the output).
pub fn badge(ui: &mut egui::Ui, runner: Option<&Runner>) -> bool {
    let Some(st) = runner.map(Runner::status) else { return false };
    let (text, color) = if st.running.is_some() || !st.pending.is_empty() {
        (format!("{} läuft …", st.label), Color32::from_rgb(255, 210, 90))
    } else if let Some(r) = &st.last {
        if r.ok {
            (format!("{} ok ({})", r.label, hook::seconds_de(r.duration)), Color32::from_rgb(120, 210, 120))
        } else {
            (format!("{} Fehler", r.label), Color32::from_rgb(255, 110, 100))
        }
    } else {
        return false;
    };
    ui.separator();
    ui.add(egui::Button::new(egui::RichText::new(text).color(color)).frame(false))
        .on_hover_text("Letzte Ausgabe des on_save-Befehls zeigen")
        .clicked()
}
