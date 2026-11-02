//! Aseprite-style timeline: a grid of cels (layers x frames), tags as
//! coloured ranges above the frame numbers, linked cels joined by a line.
//!
//! Scrubbing: press the left button on a frame number (or a cel) and drag
//! left/right; the current frame follows the column under the pointer. The
//! mouse wheel over the frame numbers steps one frame per notch (over the
//! cels it scrolls as before).

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Vec2,
};
use pixel_core::document::Direction;
use pixel_core::{render, Rgba};

use super::panels::direction_label;
use super::EditorApp;

const NAME_W: f32 = 170.0;
const ROW_H: f32 = 22.0;
const LANE_H: f32 = 16.0;
const HEADER_H: f32 = 18.0;
const NAME_ROW_H: f32 = 14.0;
/// x where the lock column starts and the layer name starts
const LOCK_X: f32 = 22.0;
const LAYER_NAME_X: f32 = 40.0;
/// Points of wheel movement per frame step (one notch).
const WHEEL_STEP: f32 = 40.0;

/// Default tag colors when a tag has none (cycled by index).
pub fn auto_tag_color(i: usize) -> Rgba {
    const C: [[u8; 4]; 6] = [
        [70, 150, 230, 255],
        [230, 120, 60, 255],
        [90, 190, 110, 255],
        [200, 90, 200, 255],
        [220, 190, 60, 255],
        [70, 190, 190, 255],
    ];
    Rgba(C[i % C.len()])
}

fn direction_glyph(d: Direction) -> &'static str {
    match d {
        Direction::Forward => ">",
        Direction::Reverse => "<",
        Direction::PingPong => "<>",
        Direction::PingPongReverse => "><",
    }
}

/// Which part of the grid the pointer is on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Tag(usize),
    Frame(usize),
    Eye(usize),
    Lock(usize),
    LayerName(usize),
    Cel(usize, usize),
}

pub struct TimelineState {
    thumbs: Vec<TextureHandle>,
    thumbs_rev: Option<u64>,
    pub show_thumbs: bool,
    context: Option<Context>,
    /// A scrub drag is running (started on a frame number or a cel).
    pub scrubbing: bool,
    /// Wheel movement over the frame numbers not yet turned into steps.
    wheel: f32,
    /// Screen rects of the frame number cells in the last paint.
    pub frame_cells: Vec<Rect>,
}

#[derive(Clone, Copy)]
enum Context {
    Cel(usize, usize),
    Frame(usize),
}

impl Default for TimelineState {
    fn default() -> Self {
        TimelineState {
            thumbs: Vec::new(),
            thumbs_rev: None,
            show_thumbs: true,
            context: None,
            scrubbing: false,
            wheel: 0.0,
            frame_cells: Vec::new(),
        }
    }
}

impl EditorApp {
    fn rebuild_thumbs(&mut self, ctx: &egui::Context) {
        let t = &mut self.timeline;
        if t.thumbs_rev == Some(self.rev) && t.thumbs.len() == self.doc.frames.len() {
            return;
        }
        t.thumbs.truncate(self.doc.frames.len());
        for f in 0..self.doc.frames.len() {
            let img = render::composite(&self.doc, f);
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
            match t.thumbs.get_mut(f) {
                Some(h) => h.set(ci, egui::TextureOptions::NEAREST),
                None => t.thumbs.push(ctx.load_texture(format!("thumb{f}"), ci, egui::TextureOptions::NEAREST)),
            }
        }
        t.thumbs_rev = Some(self.rev);
    }

    /// Thumbnail edge in physical pixels: the sprite at an integer scale
    /// (or an integer fraction) that fits in ~48 px.
    fn thumb_size(&self, ppp: f32) -> Vec2 {
        let (w, h) = (self.doc.width as f32, self.doc.height as f32);
        let target = 48.0 * ppp;
        let m = w.max(h);
        let s = if m <= target { (target / m).floor() } else { 1.0 / (m / target).ceil() };
        Vec2::new(w * s, h * s) / ppp
    }

    pub(super) fn timeline_panel(&mut self, ui: &mut egui::Ui, now: f64) {
        self.timeline_toolbar(ui, now);
        ui.add_space(2.0);
        let ppp = ui.ctx().pixels_per_point();
        if self.timeline.show_thumbs {
            self.rebuild_thumbs(&ui.ctx().clone());
        }
        let thumb = self.thumb_size(ppp);
        // wide enough for frame names (shown under the numbers)
        let name_w = self.doc.frames.iter().map(|f| f.name.chars().count()).max().unwrap_or(2) as f32 * 6.5 + 8.0;
        let base_w = if self.timeline.show_thumbs { (thumb.x + 6.0).max(28.0) } else { 28.0 };
        let cell_w = base_w.max(name_w.min(90.0));
        let thumb_h = if self.timeline.show_thumbs { thumb.y + 6.0 } else { 0.0 };

        // tag lanes: greedy, so overlapping tags stack
        let mut lanes: Vec<usize> = Vec::new(); // lane -> last `to`
        let mut tag_lane = vec![0usize; self.doc.tags.len()];
        let mut order: Vec<usize> = (0..self.doc.tags.len()).collect();
        order.sort_by_key(|i| self.doc.tags[*i].from);
        for i in order {
            let t = &self.doc.tags[i];
            let lane = lanes.iter().position(|end| *end < t.from).unwrap_or(lanes.len());
            if lane == lanes.len() {
                lanes.push(t.to);
            } else {
                lanes[lane] = t.to;
            }
            tag_lane[i] = lane;
        }
        let tags_h = lanes.len().max(1) as f32 * LANE_H;
        let n_frames = self.doc.frames.len();
        let n_layers = self.doc.layers.len();
        let head_h = tags_h + HEADER_H + NAME_ROW_H + thumb_h;
        let size = Vec2::new(NAME_W + n_frames as f32 * cell_w + 4.0, head_h + n_layers as f32 * ROW_H + 4.0);

        egui::ScrollArea::both().id_salt("timeline_grid").auto_shrink([false, false]).show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let o = rect.min;
            let col_x = |f: usize| o.x + NAME_W + f as f32 * cell_w;
            // layers are listed top layer first
            let row_y = |l: usize| o.y + head_h + (n_layers - 1 - l) as f32 * ROW_H;
            let hit_at = |p: Pos2| -> Option<Hit> {
                let (x, y) = (p.x - o.x, p.y - o.y);
                if x < 0.0 || y < 0.0 {
                    return None;
                }
                let frame = ((x - NAME_W) / cell_w).floor();
                let frame = (x >= NAME_W && (frame as usize) < n_frames).then_some(frame as usize);
                if y < tags_h {
                    let lane = (y / LANE_H) as usize;
                    let f = frame?;
                    return (0..self.doc.tags.len())
                        .find(|i| tag_lane[*i] == lane && self.doc.tags[*i].contains(f))
                        .map(Hit::Tag);
                }
                if y < head_h {
                    return frame.map(Hit::Frame);
                }
                let row = ((y - head_h) / ROW_H) as usize;
                if row >= n_layers {
                    return None;
                }
                let l = n_layers - 1 - row;
                match frame {
                    Some(f) => Some(Hit::Cel(l, f)),
                    None if x < LOCK_X => Some(Hit::Eye(l)),
                    None if x < LAYER_NAME_X => Some(Hit::Lock(l)),
                    None => Some(Hit::LayerName(l)),
                }
            };

            let p = ui.painter_at(rect);
            let font = FontId::proportional(12.0);
            let small = FontId::proportional(10.5);
            let text = ui.visuals().text_color();
            let weak = ui.visuals().weak_text_color();
            p.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

            // current frame column and layer row
            let cur_col = Rect::from_min_size(Pos2::new(col_x(self.frame), o.y), Vec2::new(cell_w, size.y));
            p.rect_filled(cur_col, 0.0, Color32::from_rgba_unmultiplied(80, 140, 220, 30));
            let cur_row = Rect::from_min_size(Pos2::new(o.x, row_y(self.layer)), Vec2::new(size.x, ROW_H));
            p.rect_filled(cur_row, 0.0, Color32::from_rgba_unmultiplied(80, 140, 220, 30));

            // tags
            for (i, t) in self.doc.tags.iter().enumerate() {
                let c = t.color.unwrap_or_else(|| auto_tag_color(i));
                let r = Rect::from_min_max(
                    Pos2::new(col_x(t.from) + 1.0, o.y + tag_lane[i] as f32 * LANE_H + 1.0),
                    Pos2::new(col_x(t.to + 1) - 1.0, o.y + (tag_lane[i] + 1) as f32 * LANE_H - 1.0),
                );
                let selected = self.play_tag == Some(i);
                p.rect_filled(r, 3.0, super::to_color32(c).gamma_multiply(if selected { 1.0 } else { 0.7 }));
                if selected {
                    p.rect_stroke(r, 3.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
                }
                let label = format!("{} {}", direction_glyph(t.direction), t.name);
                p.with_clip_rect(r).text(
                    r.left_center() + Vec2::new(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    label,
                    small.clone(),
                    Color32::BLACK,
                );
            }

            // frame numbers (+ thumbnails)
            for f in 0..n_frames {
                let x = col_x(f);
                let top = o.y + tags_h;
                let locked = self.doc.frames[f].locked;
                let lock_col = Color32::from_rgb(230, 90, 80);
                p.text(
                    Pos2::new(x + cell_w / 2.0, top + HEADER_H / 2.0),
                    Align2::CENTER_CENTER,
                    f.to_string(),
                    font.clone(),
                    if locked {
                        lock_col
                    } else if f == self.frame {
                        text
                    } else {
                        weak
                    },
                );
                // the frame's unique name, as used by the user and Claude
                let cell = Rect::from_min_size(Pos2::new(x + 1.0, top + HEADER_H), Vec2::new(cell_w - 2.0, NAME_ROW_H));
                p.with_clip_rect(cell).text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    &self.doc.frames[f].name,
                    small.clone(),
                    if locked { lock_col } else { weak },
                );
                if locked {
                    p.line_segment([cell.left_bottom(), cell.right_bottom()], Stroke::new(2.0, lock_col));
                }
                if self.timeline.show_thumbs {
                    if let Some(t) = self.timeline.thumbs.get(f) {
                        let min = Pos2::new(x + (cell_w - thumb.x) / 2.0, top + HEADER_H + NAME_ROW_H + 3.0);
                        // align to physical pixels so the thumbnail is not resampled
                        let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
                        let r = Rect::from_min_size(min, thumb);
                        p.rect_filled(r, 0.0, Color32::from_gray(64));
                        p.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                }
            }

            // layer rows
            for l in 0..n_layers {
                let y = row_y(l);
                let layer = &self.doc.layers[l];
                p.line_segment(
                    [Pos2::new(o.x, y), Pos2::new(o.x + size.x, y)],
                    Stroke::new(1.0, Color32::from_gray(45)),
                );
                // eye
                let eye = Pos2::new(o.x + 11.0, y + ROW_H / 2.0);
                if layer.visible {
                    p.circle_filled(eye, 4.5, text);
                } else {
                    p.circle_stroke(eye, 4.5, Stroke::new(1.0, weak));
                }
                // lock
                let lock = Rect::from_center_size(
                    Pos2::new(o.x + (LOCK_X + LAYER_NAME_X) / 2.0, y + ROW_H / 2.0),
                    Vec2::new(9.0, 9.0),
                );
                if layer.locked {
                    p.rect_filled(lock, 1.0, Color32::from_rgb(230, 90, 80));
                } else {
                    p.rect_stroke(lock, 1.0, Stroke::new(1.0, weak), StrokeKind::Inside);
                }
                p.with_clip_rect(Rect::from_min_size(
                    Pos2::new(o.x + LAYER_NAME_X, y),
                    Vec2::new(NAME_W - LAYER_NAME_X - 4.0, ROW_H),
                ))
                .text(
                    Pos2::new(o.x + LAYER_NAME_X + 2.0, y + ROW_H / 2.0),
                    Align2::LEFT_CENTER,
                    &layer.name,
                    font.clone(),
                    if layer.visible { text } else { weak },
                );
                for f in 0..n_frames {
                    let c = Pos2::new(col_x(f) + cell_w / 2.0, y + ROW_H / 2.0);
                    if f + 1 < n_frames && layer.link_source(f + 1) == layer.link_source(f) {
                        p.line_segment([c, c + Vec2::new(cell_w, 0.0)], Stroke::new(2.0, text));
                    }
                    if self.doc.cel_is_empty(l, f) {
                        p.circle_stroke(c, 4.0, Stroke::new(1.0, weak));
                    } else {
                        p.circle_filled(c, 4.5, text);
                    }
                    if layer.is_linked(f) {
                        // linked cel: extra ring (Aseprite joins neighbours with a line)
                        p.circle_stroke(c, 7.0, Stroke::new(1.5, text));
                    }
                    if l == self.layer && f == self.frame {
                        let r = Rect::from_center_size(c, Vec2::new(cell_w - 2.0, ROW_H - 2.0));
                        p.rect_stroke(r, 2.0, Stroke::new(1.5, Color32::from_rgb(120, 190, 255)), StrokeKind::Inside);
                    }
                }
            }
            p.line_segment(
                [Pos2::new(o.x + NAME_W, o.y), Pos2::new(o.x + NAME_W, o.y + size.y)],
                Stroke::new(1.0, Color32::from_gray(60)),
            );

            // interaction (hits first: the code below changes the document)
            let hover = resp.hover_pos().and_then(hit_at);
            let origin = ui.input(|i| i.pointer.press_origin()).and_then(hit_at);
            // scrubbing: a drag that starts on a frame number or a cel moves
            // the current frame with the pointer (anywhere left/right)
            self.timeline.frame_cells = (0..n_frames)
                .map(|f| Rect::from_min_size(Pos2::new(col_x(f), o.y + tags_h), Vec2::new(cell_w, HEADER_H)))
                .collect();
            // frames with "KI zeigen" marks: a light blue badge with the count
            for (f, cell) in self.timeline.frame_cells.iter().enumerate() {
                let n = self.marks.on_frame(f).count();
                if n > 0 {
                    let c = Pos2::new(cell.max.x - 7.0, cell.min.y + 7.0);
                    p.circle_filled(c, 6.5, super::canvas::PIN_COLOR);
                    p.text(c, Align2::CENTER_CENTER, n.to_string(), FontId::proportional(9.5), Color32::BLACK);
                }
            }
            let frame_at_x = |x: f32| (((x - o.x - NAME_W) / cell_w).floor().max(0.0) as usize).min(n_frames - 1);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(Hit::Frame(_) | Hit::Cel(..)) = origin {
                    if let Some(Hit::Cel(l, _)) = origin {
                        self.layer = l;
                    }
                    self.timeline.scrubbing = true;
                    self.playing = false; // paused, and stays paused
                }
            }
            if self.timeline.scrubbing {
                if let Some(pos) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos())) {
                    let f = frame_at_x(pos.x);
                    if f != self.frame {
                        self.goto_frame(f);
                    }
                }
                if !resp.dragged() || resp.drag_stopped() {
                    self.timeline.scrubbing = false;
                }
            }
            // wheel over the frame numbers: one frame per notch
            let over_header = resp.hover_pos().is_some_and(|p| p.y < o.y + head_h && p.x >= o.x + NAME_W);
            if over_header {
                let wheel = ui.input_mut(|i| {
                    let mut d = 0.0;
                    for e in &i.raw.events {
                        if let egui::Event::MouseWheel { unit, delta, .. } = e {
                            d += match unit {
                                egui::MouseWheelUnit::Point => delta.y + delta.x,
                                egui::MouseWheelUnit::Line => (delta.y + delta.x) * WHEEL_STEP,
                                egui::MouseWheelUnit::Page => (delta.y + delta.x) * WHEEL_STEP * 4.0,
                            };
                        }
                    }
                    // the frame row does not scroll the grid
                    i.smooth_scroll_delta = Vec2::ZERO;
                    d
                });
                self.timeline.wheel += wheel;
                while self.timeline.wheel.abs() >= WHEEL_STEP {
                    let up = self.timeline.wheel > 0.0;
                    self.timeline.wheel -= self.timeline.wheel.signum() * WHEEL_STEP;
                    // wheel down / right = next frame
                    let f = if up { (self.frame + n_frames - 1) % n_frames } else { (self.frame + 1) % n_frames };
                    self.goto_frame(f);
                }
            } else {
                self.timeline.wheel = 0.0;
            }

            if let Some(h) = hover {
                let tip = match h {
                    Hit::Tag(i) => {
                        let t = &self.doc.tags[i];
                        format!(
                            "Tag {:?}: Frames {}..{}, {}, {}",
                            t.name,
                            t.from,
                            t.to,
                            direction_label(t.direction),
                            if t.looping { "Loop" } else { "einmal" }
                        )
                    }
                    Hit::Frame(f) => format!(
                        "Frame {} · {} ms{}{} (Rechtsklick: sperren)",
                        self.doc.frame_label_long(f),
                        self.doc.frames[f].duration_ms,
                        if self.doc.frames[f].locked { " · gesperrt" } else { "" },
                        match self.marks.on_frame(f).count() {
                            0 => String::new(),
                            n => format!(" · {n} KI-Markierung{}", if n == 1 { "" } else { "en" }),
                        }
                    ),
                    Hit::Eye(_) => "Sichtbarkeit umschalten".into(),
                    Hit::Lock(l) => format!(
                        "Ebene {:?} {}",
                        self.doc.layers[l].name,
                        if self.doc.layers[l].locked { "gesperrt (Klick entsperrt)" } else { "sperren" }
                    ),
                    Hit::LayerName(l) => format!("Ebene {l}"),
                    Hit::Cel(l, f) => {
                        let linked = self.doc.layers[l].linked_frames(f);
                        if linked.len() > 1 {
                            format!("Ebene {l}, Frame {f} – verknüpft mit {linked:?}")
                        } else {
                            format!("Ebene {l}, Frame {f}")
                        }
                    }
                };
                resp.clone().on_hover_text(tip);
            }
            if resp.clicked() {
                match hover {
                    Some(Hit::Tag(i)) => self.select_tag(Some(i)),
                    Some(Hit::Frame(f)) => self.goto_frame(f),
                    Some(Hit::Eye(l)) => {
                        let ly = &self.doc.layers[l];
                        let desc =
                            format!("Ebene {} {}", ly.name, if ly.visible { "ausgeblendet" } else { "eingeblendet" });
                        self.edit(desc, |d| d.layers[l].visible = !d.layers[l].visible);
                    }
                    Some(Hit::Lock(l)) => {
                        let ly = &self.doc.layers[l];
                        let desc = format!("Ebene {} {}", ly.name, if ly.locked { "entsperrt" } else { "gesperrt" });
                        self.edit(desc, |d| d.layers[l].locked = !d.layers[l].locked);
                    }
                    Some(Hit::LayerName(l)) => self.layer = l,
                    Some(Hit::Cel(l, f)) => {
                        self.layer = l;
                        self.goto_frame(f);
                    }
                    None => {}
                }
            }
            if resp.secondary_clicked() {
                match hover {
                    Some(Hit::Cel(l, f)) => {
                        self.layer = l;
                        self.goto_frame(f);
                        self.timeline.context = Some(Context::Cel(l, f));
                    }
                    Some(Hit::Frame(f)) => self.timeline.context = Some(Context::Frame(f)),
                    _ => self.timeline.context = None,
                }
            }
            resp.context_menu(|ui| match self.timeline.context {
                Some(Context::Frame(f)) => {
                    ui.label(format!("Frame {}", self.doc.frame_label_long(f)));
                    let locked = self.doc.frames[f].locked;
                    if ui.button(if locked { "Frame entsperren" } else { "Frame sperren" }).clicked() {
                        let desc = format!(
                            "Frame {} {}",
                            self.doc.frame_label(f),
                            if locked { "entsperrt" } else { "gesperrt" }
                        );
                        self.edit(desc, |d| d.frames[f].locked = !locked);
                        ui.close();
                    }
                }
                Some(Context::Cel(l, f)) => {
                    ui.label(format!("Ebene {}, Frame {}", self.doc.layers[l].name, self.doc.frame_label(f)));
                    let blocked = self.doc.edit_blocked(l, f);
                    if let Some(why) = &blocked {
                        ui.label(RichText::new(why).weak());
                    }
                    let here = format!("{} · {}", self.doc.frame_label(f), self.doc.layers[l].name);
                    if ui.add_enabled(self.doc.layers[l].is_linked(f), egui::Button::new("Verknüpfung lösen")).clicked()
                    {
                        self.edit(format!("Verknüpfung gelöst: {here}"), |d| d.layers[l].unlink(f));
                        ui.close();
                    }
                    let can_link = f > 0 && blocked.is_none();
                    if ui.add_enabled(can_link, egui::Button::new("Mit vorherigem Frame verknüpfen")).clicked() {
                        self.edit(format!("Verknüpft mit Frame {}: {here}", f - 1), |d| d.layers[l].link(f, f - 1));
                        ui.close();
                    }
                    if ui.add_enabled(blocked.is_none(), egui::Button::new("Cel leeren")).clicked() {
                        self.edit(format!("Cel geleert: {here}"), |d| {
                            *d.cel_mut(l, f) = vec![pixel_core::TRANSPARENT; d.pixel_count()]
                        });
                        ui.close();
                    }
                }
                None => ui.close(),
            });
        });
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(if self.playing { "Pause" } else { "Abspielen" }).on_hover_text("Enter").clicked() {
                self.toggle_play(now);
            }
            let lbl = ui.label("Animation");
            let mut sel = self.play_tag;
            let current = sel.and_then(|t| self.doc.tags.get(t)).map_or("Alle Frames".to_string(), |t| t.name.clone());
            egui::ComboBox::from_id_salt("play_tag")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sel, None, "Alle Frames");
                    for (i, t) in self.doc.tags.iter().enumerate() {
                        ui.selectable_value(&mut sel, Some(i), &t.name);
                    }
                })
                .response
                .labelled_by(lbl.id);
            if sel != self.play_tag {
                self.select_tag(sel);
            }
            ui.checkbox(&mut self.onion, "Onion Skin").on_hover_text("F3");
            ui.checkbox(&mut self.timeline.show_thumbs, "Vorschaubilder");
            ui.separator();
            ui.label("Frame:");
            let f = self.frame;
            if ui.button("+ Neu").on_hover_text("Kopie des aktuellen Frames (Alt+N)").clicked() {
                self.new_frame(true, false);
            }
            if ui.button("+ Leer").on_hover_text("Leerer Frame (Alt+B)").clicked() {
                self.new_frame(false, false);
            }
            if ui
                .button("+ Verknüpft")
                .on_hover_text("Neuer Frame mit verknüpften Cels: Änderungen gelten für beide")
                .clicked()
            {
                self.new_frame(true, true);
            }
            let n = self.doc.frames.len();
            if ui.add_enabled(n > 1, egui::Button::new("Löschen")).on_hover_text("Alt+C").clicked() {
                let desc = format!("Frame {} gelöscht", self.doc.frame_label(f));
                if let Err(e) = self.edit(desc, |d| d.delete_frame(f)) {
                    self.status = e;
                }
            }
            if ui.add_enabled(f > 0, egui::Button::new("<")).on_hover_text("Frame nach links verschieben").clicked() {
                self.edit(format!("Frame {f} nach links"), |d| d.move_frame(f, f - 1)).ok();
                self.frame -= 1;
            }
            if ui
                .add_enabled(f + 1 < n, egui::Button::new(">"))
                .on_hover_text("Frame nach rechts verschieben")
                .clicked()
            {
                self.edit(format!("Frame {f} nach rechts"), |d| d.move_frame(f, f + 1)).ok();
                self.frame += 1;
            }
            ui.separator();
            ui.label("Ebene:");
            let l = self.layer;
            if ui.button("+ Neu").on_hover_text("Umschalt+N").clicked() {
                self.new_layer();
            }
            let nl = self.doc.layers.len();
            if ui.add_enabled(nl > 1, egui::Button::new("Löschen")).clicked() {
                let desc = format!("Ebene {} gelöscht", self.doc.layers[l].name);
                if let Err(e) = self.edit(desc, |d| d.delete_layer(l)) {
                    self.status = e;
                }
            }
            if ui.add_enabled(l + 1 < nl, egui::Button::new("Hoch")).on_hover_text("Ebene nach oben").clicked() {
                self.edit(format!("Ebene {} nach oben", self.doc.layers[l].name), |d| d.move_layer(l, l + 1)).ok();
                self.layer += 1;
            }
            if ui.add_enabled(l > 0, egui::Button::new("Runter")).on_hover_text("Ebene nach unten").clicked() {
                self.edit(format!("Ebene {} nach unten", self.doc.layers[l].name), |d| d.move_layer(l, l - 1)).ok();
                self.layer -= 1;
            }
        });
    }

    pub(super) fn new_frame(&mut self, copy: bool, linked: bool) {
        let f = self.frame;
        let src = copy.then_some(f);
        let how = match (copy, linked) {
            (true, true) => "verknüpft",
            (true, false) => "Kopie",
            _ => "leer",
        };
        let desc = format!("Neuer Frame nach {} ({how})", self.doc.frame_label(f));
        if let Ok(i) = self.edit(desc, |d| d.add_frame(f + 1, src, linked)) {
            self.goto_frame(i);
        }
    }

    pub(super) fn new_layer(&mut self) {
        let at = self.layer + 1;
        self.layer = self.edit("Neue Ebene", |d| d.add_layer("Ebene", at));
    }

    pub(super) fn goto_frame(&mut self, f: usize) {
        self.playing = false;
        self.frame = f.min(self.doc.frames.len() - 1);
    }

    pub(super) fn select_tag(&mut self, tag: Option<usize>) {
        self.play_tag = tag;
        self.play_pos = 0;
        if let Some(f) = self.doc.sequence(tag).first() {
            self.frame = *f;
            self.panel_frame = *f; // a manual choice: the panel follows even while playing
        }
    }
}
