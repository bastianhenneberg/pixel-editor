//! Mini preview ("Vorschau"): a small floating window in the top-right
//! corner of the canvas with the current frame's composite at an integer
//! scale (1x/2x/3x physical pixels), always without grid or overlays. It can
//! loop the current tag in real timing, and it follows every document
//! change (also Claude's edits in follow mode) because it is rebuilt from
//! the document revision.

use std::time::Duration;

use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use pixel_core::render;

use super::settings::{PreviewBg, PreviewSettings};
use super::tiled::TileMode;
use super::EditorApp;

#[derive(Default)]
pub struct PreviewState {
    pub settings: PreviewSettings,
    pub playing: bool,
    /// Position in the play sequence and time spent on it.
    play_pos: usize,
    play_elapsed_ms: f64,
    play_last: f64,
    /// Frame shown in the last paint.
    pub shown: usize,
    /// Screen rect (points) of the image in the last paint.
    pub image_rect: Option<Rect>,
    tex: Option<TextureHandle>,
    checker: Option<TextureHandle>,
    /// (rev, frame) the texture was built for.
    built: Option<(u64, usize)>,
}

impl PreviewState {
    /// Another tab is shown: rebuild the texture, stop the preview's own loop.
    pub fn tab_switched(&mut self) {
        self.built = None;
        self.playing = false;
    }
}

impl EditorApp {
    /// The tag the preview loops: the first one containing the current frame.
    fn preview_tag(&self) -> Option<usize> {
        self.doc.tags.iter().position(|t| t.contains(self.frame))
    }

    pub(super) fn toggle_preview_play(&mut self, now: f64) {
        let p = &mut self.preview;
        p.playing = !p.playing;
        if p.playing {
            let seq = self.doc.sequence(self.preview_tag());
            let p = &mut self.preview;
            p.play_pos = seq.iter().position(|f| *f == self.frame).unwrap_or(0);
            p.play_elapsed_ms = 0.0;
            p.play_last = now;
        }
    }

    /// The frame to show now; advances the preview's own playback.
    pub(super) fn preview_frame(&mut self, ctx: &egui::Context, now: f64) -> usize {
        if !self.preview.playing {
            return self.frame;
        }
        let seq = self.doc.sequence(self.preview_tag());
        let p = &mut self.preview;
        p.play_elapsed_ms += (now - p.play_last).max(0.0) * 1000.0;
        p.play_last = now;
        p.play_pos %= seq.len();
        loop {
            let ms = self.doc.frames[seq[p.play_pos]].duration_ms.max(1) as f64;
            if p.play_elapsed_ms < ms {
                ctx.request_repaint_after(Duration::from_secs_f64((ms - p.play_elapsed_ms) / 1000.0));
                break;
            }
            p.play_elapsed_ms -= ms;
            p.play_pos = (p.play_pos + 1) % seq.len(); // always loops
        }
        seq[p.play_pos]
    }

    pub(super) fn preview_window(&mut self, ctx: &egui::Context, now: f64) {
        if !self.preview.settings.open {
            return;
        }
        let area = self.canvas.area;
        let frame = self.preview_frame(ctx, now);
        // texture: the composite only, never any overlay
        if self.preview.built != Some((self.rev, frame)) || self.preview.tex.is_none() {
            let img = render::composite(&self.doc, frame);
            let img = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
            match &mut self.preview.tex {
                Some(t) => t.set(img, TextureOptions::NEAREST_REPEAT),
                None => self.preview.tex = Some(ctx.load_texture("preview", img, TextureOptions::NEAREST_REPEAT)),
            }
            self.preview.built = Some((self.rev, frame));
        }
        if self.preview.checker.is_none() {
            let [a, b] = render::CHECKER;
            let img = egui::ColorImage::from_rgba_unmultiplied([2, 2], &[a, b, b, a].concat());
            self.preview.checker = Some(ctx.load_texture("preview-checker", img, TextureOptions::NEAREST_REPEAT));
        }

        let s = self.preview.settings.clone();
        let mut open = true;
        let mut window =
            egui::Window::new("Vorschau").open(&mut open).collapsible(true).resizable(false).default_open(!s.collapsed);
        window = match s.pos {
            Some([x, y]) => window.default_pos(Pos2::new(x, y)),
            None if area.is_positive() => {
                window.pivot(egui::Align2::RIGHT_TOP).default_pos(area.right_top() + Vec2::new(-12.0, 12.0))
            }
            None => window,
        };
        if area.is_positive() {
            window = window.constrain_to(area);
        }
        let ppp = ctx.pixels_per_point();
        // gekachelt: the copies of the Kachel-Modus, 3x3 while it is off
        let (nx, ny) = match self.tile_mode {
            _ if !s.tiled => (1, 1),
            TileMode::Off => (3, 3),
            m => m.repeats(),
        };
        let (nx, ny) = (nx as f32, ny as f32);
        let (w, h) = (self.doc.width as f32 * nx, self.doc.height as f32 * ny);
        let label = {
            let d = &self.doc;
            let mut l = d.frame_label(frame);
            if l != d.frames[frame].name {
                l += &format!(" ({})", d.frames[frame].name);
            }
            l
        };
        let mut toggle_play = false;
        let mut image_rect = Rect::NOTHING;
        let (tex, checker) = (self.preview.tex.clone().unwrap(), self.preview.checker.clone().unwrap());
        let shown = window.show(ctx, |ui| {
            let st = &mut self.preview.settings;
            ui.horizontal(|ui| {
                for z in 1..=3 {
                    ui.selectable_value(&mut st.scale, z, format!("{z}×"));
                }
                ui.toggle_value(&mut st.tiled, "Kacheln").on_hover_text("das Bild wiederholt zeigen, wie gekachelt");
                let play = if self.preview.playing { "Anhalten" } else { "Tag abspielen" };
                if ui.button(play).on_hover_text("aktuellen Tag in Echtzeit abspielen (Schleife)").clicked() {
                    toggle_play = true;
                }
            });
            egui::ComboBox::from_label("Hintergrund").selected_text(st.background.label()).width(96.0).show_ui(
                ui,
                |ui| {
                    for bg in PreviewBg::ALL {
                        ui.selectable_value(&mut st.background, bg, bg.label());
                    }
                },
            );
            // exact integer scale in physical pixels, on the physical pixel grid
            let size = Vec2::new(w, h) * st.scale as f32 / ppp;
            // plain space, not a widget: a press on the image drags the window
            let (_, slot) = ui.allocate_space(size + Vec2::splat(1.0 / ppp));
            let min = Pos2::new((slot.min.x * ppp).ceil() / ppp, (slot.min.y * ppp).ceil() / ppp);
            let r = Rect::from_min_size(min, size);
            let painter = ui.painter();
            match st.background {
                PreviewBg::Checker => {
                    // one checker square per 4 art pixels
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(w / 4.0, h / 4.0));
                    painter.image(checker.id(), r, uv, Color32::WHITE);
                }
                bg => {
                    let [cr, cg, cb, _] = bg.color();
                    painter.rect_filled(r, 0.0, Color32::from_rgb(cr, cg, cb));
                }
            }
            painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(nx, ny)), Color32::WHITE);
            image_rect = r;
            ui.label(egui::RichText::new(label).small().weak());
        });
        if toggle_play {
            self.toggle_preview_play(now);
        }
        self.preview.shown = frame;
        self.preview.image_rect = image_rect.is_positive().then_some(image_rect);
        let st = &mut self.preview.settings;
        if let Some(resp) = shown {
            st.collapsed = resp.inner.is_none();
            // remember a position only once the user moved it
            if s.pos.is_some() || resp.response.dragged() {
                st.pos = Some([resp.response.rect.min.x, resp.response.rect.min.y]);
            }
        }
        if !open {
            st.open = false;
        }
    }
}
