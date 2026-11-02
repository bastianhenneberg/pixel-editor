//! The drawing canvas: textures, zoom/pan, tool input and overlays.

use eframe::egui::{self, Color32, PointerButton, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use pixel_core::{draw, render, TRANSPARENT};

use super::select::{lasso_points, SelOp, Selection};
use super::tiled::TileMode;
use super::{EditorApp, Tool};

const WHEEL_STEP: f32 = 40.0;
/// Kachel-Modus: the copies around the image are drawn slightly darker.
const COPY_TINT: Color32 = Color32::from_gray(165);
/// Color of the "KI zeigen" mark (and its drag preview).
pub const PIN_COLOR: Color32 = Color32::from_rgb(90, 200, 255);
const ZOOM_LEVELS: [f32; 14] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0];

/// An in-progress mouse gesture on the canvas.
struct Gesture {
    start: (i32, i32),
    last: (i32, i32),
    key: u8,
    painted: usize,
    /// Freehand tools: the stroke so far in canvas pixels (before wrapping
    /// and symmetry), redrawn as a whole on every move, so Pixel-Perfect
    /// can take back corners and shading changes each pixel once.
    path: Vec<(i32, i32)>,
    /// The cel before the stroke (what a taken-back pixel returns to).
    base: Vec<u8>,
    /// Image pixels the stroke has written so far.
    written: Vec<(i32, i32)>,
    /// Schattieren: +1 lighter (left button), -1 darker (right).
    steps: i32,
}

impl Gesture {
    fn new(start: (i32, i32), key: u8) -> Gesture {
        Gesture {
            start,
            last: start,
            key,
            painted: 0,
            path: vec![start],
            base: Vec::new(),
            written: Vec::new(),
            steps: 0,
        }
    }
}

/// A running gesture of a selection tool. `base` is the selection before
/// it started, `op` how the new shape combines with it.
enum SelectDrag {
    /// Rectangle (tool M).
    New { start: (i32, i32), last: (i32, i32), op: SelOp, base: Option<Selection> },
    /// Freehand outline.
    Lasso { path: Vec<(i32, i32)>, op: SelOp, base: Option<Selection> },
    /// Selection brush: paints pixels into (or out of) the selection.
    Brush { last: (i32, i32), op: SelOp },
    /// Selection brush, right button: a drag removes pixels, a click
    /// leaves the selection alone (it opens the menu).
    RightBrush { start: (i32, i32), last: (i32, i32) },
    /// Zauberstab pressed inside the selection: a drag moves it, a click selects.
    Wand { start: (i32, i32) },
    /// Moving the floating pixels.
    Move { grab: (i32, i32) },
}

pub struct CanvasState {
    /// The image pixel under the pointer (in the Kachel-Modus also over a copy).
    pub hover: Option<(i32, i32)>,
    /// The canvas pixel under the pointer before wrapping (where the hover box goes).
    hover_raw: Option<(i32, i32)>,
    /// Selection tools in the Kachel-Modus: the copy the gesture started in
    /// (pointer position minus wrapped position), so moving stays smooth.
    sel_shift: (i32, i32),
    /// Screen rect of the document this frame.
    pub screen: Rect,
    /// Screen rect of the whole canvas area (the preview sits in its corner).
    pub area: Rect,
    /// Bring this area into view on the next frame (follow mode).
    pub focus_request: Option<pixel_core::Rect>,
    gesture: Option<Gesture>,
    select_drag: Option<SelectDrag>,
    /// "KI zeigen" tool: the running drag (new mark, move, resize).
    pub(super) mark_drag: Option<super::marks::MarkDrag>,
    /// The note field of the selected mark had the keyboard last frame.
    pub(super) note_focused: bool,
    /// Give the note field the keyboard on its next paint.
    pub(super) focus_note: bool,
    panning: bool,
    /// The symmetry axis being dragged by its grip.
    pub(super) axis_drag: Option<super::transform::AxisDrag>,
    wheel: f32,
    tex: Option<TextureHandle>,
    prev_tex: Option<TextureHandle>,
    next_tex: Option<TextureHandle>,
    checker: Option<TextureHandle>,
    /// (rev, frame, onion) the textures were built for.
    built: Option<(u64, usize, bool)>,
}

impl Default for CanvasState {
    fn default() -> Self {
        CanvasState {
            hover: None,
            hover_raw: None,
            sel_shift: (0, 0),
            screen: Rect::NOTHING,
            area: Rect::NOTHING,
            focus_request: None,
            gesture: None,
            select_drag: None,
            mark_drag: None,
            note_focused: false,
            focus_note: false,
            panning: false,
            axis_drag: None,
            wheel: 0.0,
            tex: None,
            prev_tex: None,
            next_tex: None,
            checker: None,
            built: None,
        }
    }
}

impl CanvasState {
    /// Force a texture rebuild on the next frame.
    pub fn invalidate(&mut self) {
        self.built = None;
    }
}

fn image_to_color(img: &render::Image) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels)
}

impl EditorApp {
    pub(super) fn zoom_step(&mut self, dir: i32, anchor: Option<(Pos2, Pos2)>) {
        let i = ZOOM_LEVELS.iter().position(|z| *z >= self.zoom).unwrap_or(ZOOM_LEVELS.len() - 1) as i32;
        let new = ZOOM_LEVELS[(i + dir).clamp(0, ZOOM_LEVELS.len() as i32 - 1) as usize];
        // keep the canvas point under the mouse fixed
        if let Some((mouse, center)) = anchor {
            let rel = mouse - center - self.pan;
            self.pan += rel - rel * (new / self.zoom);
        }
        self.zoom = new;
    }

    /// Pixels of the shape being dragged (line/rect/ellipse), not yet
    /// committed, with their keys (a filled shape may dither).
    fn shape_preview(&self) -> Option<Vec<(i32, i32, u8)>> {
        let g = self.canvas.gesture.as_ref()?;
        let (s, e) = (g.start, g.last);
        let pts = match self.tool {
            Tool::Line => draw::line(s.0, s.1, e.0, e.1),
            Tool::Rect => draw::rect(s.0, s.1, e.0, e.1, self.filled),
            Tool::Ellipse => draw::ellipse_in_box(s.0, s.1, e.0, e.1, self.filled),
            _ => return None,
        };
        Some(self.tool_plan(&self.paint_points(pts), g.key).pixels)
    }

    /// Rebuild textures when the document, frame or onion setting changed.
    /// A shape being dragged is drawn into the canvas texture itself, so the
    /// preview is pixel-exact like the final result.
    fn rebuild_textures(&mut self, ctx: &egui::Context) {
        let preview = self.shape_preview();
        let key = (self.rev, self.frame, self.onion);
        let floating = self.floating.as_ref().filter(|f| f.frame == self.frame);
        if preview.is_none() && floating.is_none() && self.canvas.built == Some(key) && self.canvas.tex.is_some() {
            return;
        }
        let set = |slot: &mut Option<TextureHandle>, name: &str, img: egui::ColorImage| match slot {
            Some(t) => t.set(img, TextureOptions::NEAREST),
            None => *slot = Some(ctx.load_texture(name, img, TextureOptions::NEAREST)),
        };
        // floating pixels: composite as if dropped (in front or behind), so
        // the preview matches the result exactly
        let mut img = match floating {
            Some(fl) => {
                let mut d = self.doc.clone();
                super::select::apply_floating(&mut d, fl, self.drop_behind);
                render::composite(&d, self.frame)
            }
            None => render::composite(&self.doc, self.frame),
        };
        if let Some(pts) = &preview {
            for &(x, y, k) in pts {
                if self.doc.in_bounds(x, y) {
                    img.put(x as u32, y as u32, self.doc.color_of(k).map_or([0, 0, 0, 0], |c| c.0));
                }
            }
        }
        let rebuild_next = preview.is_some() || floating.is_some();
        set(&mut self.canvas.tex, "canvas", image_to_color(&img));
        if self.onion {
            let n = self.doc.frames.len();
            if self.frame > 0 {
                set(
                    &mut self.canvas.prev_tex,
                    "onion-prev",
                    image_to_color(&render::composite(&self.doc, self.frame - 1)),
                );
            }
            if self.frame + 1 < n {
                set(
                    &mut self.canvas.next_tex,
                    "onion-next",
                    image_to_color(&render::composite(&self.doc, self.frame + 1)),
                );
            }
        }
        if self.canvas.checker.is_none() {
            let [a, b] = render::CHECKER;
            let px: Vec<u8> = [a, b, b, a].concat();
            let img = egui::ColorImage::from_rgba_unmultiplied([2, 2], &px);
            self.canvas.checker = Some(ctx.load_texture("checker", img, TextureOptions::NEAREST_REPEAT));
        }
        self.canvas.built = if rebuild_next { None } else { Some(key) };
    }

    pub(super) fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let (area, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.canvas.area = area;
        // clean view hides every overlay without touching the individual
        // toggles, so switching it off restores them exactly
        let clean = self.clean_view;
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_gray(24));

        // `zoom` counts *physical* pixels per art pixel and the canvas origin
        // is snapped to the physical pixel grid, so every art pixel covers
        // exactly zoom x zoom screen pixels, also with fractional UI scaling.
        let ppp = ui.ctx().pixels_per_point();
        let size = Vec2::new(self.doc.width as f32, self.doc.height as f32);
        let (rep_x, rep_y) = self.tile_mode.repeats();
        if self.fit_pending && area.width() > 50.0 {
            // the Kachel-Modus fits all copies
            let shown = size * Vec2::new(rep_x as f32, rep_y as f32);
            let fit = ((area.width() * ppp - 40.0) / shown.x).min((area.height() * ppp - 40.0) / shown.y).max(1.0);
            self.zoom = ZOOM_LEVELS.iter().rev().find(|z| **z <= fit).copied().unwrap_or(1.0);
            self.pan = Vec2::ZERO;
            self.fit_pending = false;
        }
        let center = area.center();
        if let Some(r) = self.canvas.focus_request.take() {
            // zoom out only if the area does not fit, pan only if it is not visible
            let fits =
                |z: f32| r.w as f32 * z / ppp <= area.width() * 0.9 && r.h as f32 * z / ppp <= area.height() * 0.9;
            if !fits(self.zoom) {
                self.zoom = ZOOM_LEVELS.iter().rev().find(|z| fits(**z)).copied().unwrap_or(1.0);
            }
            let cell = self.zoom / ppp;
            let min = center + self.pan - size * cell / 2.0;
            let sr = Rect::from_min_size(
                min + Vec2::new(r.x as f32, r.y as f32) * cell,
                Vec2::new(r.w as f32, r.h as f32) * cell,
            );
            if !area.shrink(16.0).contains_rect(sr) {
                let rc = Vec2::new(r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
                self.pan = size * cell / 2.0 - rc * cell;
            }
        }
        let snap = |v: f32| (v * ppp).round() / ppp;
        let cell = self.zoom / ppp; // points per art pixel
        let min_phys = (center + self.pan) * ppp - size * self.zoom / 2.0;
        let min = Pos2::new(min_phys.x.round() / ppp, min_phys.y.round() / ppp);
        let canvas = Rect::from_min_size(min, size * cell);
        self.canvas.screen = canvas;

        // ---- input: zoom & pan
        let (wheel, pointer, space, mods) = ui.input(|i| {
            let wheel: f32 = i
                .raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Line => delta.y * WHEEL_STEP,
                        egui::MouseWheelUnit::Page => delta.y * WHEEL_STEP * 4.0,
                    }),
                    _ => None,
                })
                .sum();
            (wheel, i.pointer.clone(), i.key_down(egui::Key::Space), i.modifiers)
        });
        if resp.hovered() {
            if let Some(mouse) = pointer.hover_pos() {
                // one zoom level per wheel notch; trackpads accumulate
                self.canvas.wheel += wheel;
                while self.canvas.wheel.abs() >= WHEEL_STEP {
                    let dir = self.canvas.wheel.signum();
                    self.canvas.wheel -= dir * WHEEL_STEP;
                    self.zoom_step(dir as i32, Some((mouse, center)));
                }
            }
        } else {
            self.canvas.wheel = 0.0;
        }
        if resp.hovered() && (pointer.button_pressed(PointerButton::Middle) || (space && pointer.primary_pressed())) {
            self.canvas.panning = true;
        }
        if self.canvas.panning {
            if pointer.any_down() {
                self.pan += pointer.delta();
            } else {
                self.canvas.panning = false;
            }
        }

        let to_px = move |p: Pos2| -> (i32, i32) {
            let v = (p - canvas.min) / cell;
            (v.x.floor() as i32, v.y.floor() as i32)
        };
        self.canvas.hover_raw =
            pointer.hover_pos().filter(|p| area.contains(*p)).map(to_px).filter(|p| {
                self.doc.in_bounds(p.0, p.1) || (self.tile_mode != TileMode::Off && self.in_tiled_view(*p))
            });
        self.canvas.hover = self.canvas.hover_raw.map(|p| self.wrap(p));

        // ---- input: symmetry axis grips, then tools
        let grips = self.axis_grips(canvas, cell);
        if !self.canvas.panning && !self.axis_input(&resp, &pointer, canvas, cell, &grips) {
            self.tool_input(&resp, &pointer, mods, to_px);
        }
        // selection tools: a right click (not a drag) opens the selection menu
        if self.tool.selects() {
            resp.context_menu(|ui| self.selection_menu_entries(ui));
        }
        self.rebuild_textures(ui.ctx());

        // ---- paint
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        // Kachel-Modus: the copies around the image, slightly darker
        let copies: Vec<Rect> = (-(rep_x / 2)..=rep_x / 2)
            .flat_map(|ix| (-(rep_y / 2)..=rep_y / 2).map(move |iy| (ix, iy)))
            .filter(|&c| c != (0, 0))
            .map(|(ix, iy)| canvas.translate(Vec2::new(ix as f32 * canvas.width(), iy as f32 * canvas.height())))
            .collect();
        let shown = copies.iter().fold(canvas, |a, r| a.union(*r));
        if let (Some(ch), Some(t)) = (&self.canvas.checker, &self.canvas.tex) {
            let uv = Rect::from_min_max(Pos2::ZERO, (canvas.size() / 16.0).to_pos2());
            for r in &copies {
                painter.image(ch.id(), *r, uv, COPY_TINT);
                painter.image(t.id(), *r, full_uv, COPY_TINT);
            }
        }
        if let Some(ch) = &self.canvas.checker {
            let uv = Rect::from_min_max(Pos2::ZERO, (canvas.size() / 16.0).to_pos2());
            painter.image(ch.id(), canvas, uv, Color32::WHITE);
        }
        if self.onion && !clean && !self.playing {
            let n = self.doc.frames.len();
            if self.frame > 0 {
                if let Some(t) = &self.canvas.prev_tex {
                    painter.image(t.id(), canvas, full_uv, Color32::from_rgba_unmultiplied(255, 110, 110, 90));
                }
            }
            if self.frame + 1 < n {
                if let Some(t) = &self.canvas.next_tex {
                    painter.image(t.id(), canvas, full_uv, Color32::from_rgba_unmultiplied(110, 170, 255, 90));
                }
            }
        }
        if let Some(t) = &self.canvas.tex {
            painter.image(t.id(), canvas, full_uv, Color32::WHITE);
        }
        // Grid, hover box and border as untextured meshes: egui does not
        // anti-alias meshes, so these lines stay exactly 1 physical px wide.
        let px = 1.0 / ppp;
        let mut mesh = egui::Mesh::default();
        let mut line = |r: Rect, c: Color32| mesh.add_colored_rect(r, c);
        if self.grid && !clean && self.zoom >= 4.0 {
            // over the copies too: they are drawn in as well
            let thin = Color32::from_white_alpha(22);
            let thick = Color32::from_white_alpha(60);
            let (w, h) = (self.doc.width as i32, self.doc.height as i32);
            for x in -(rep_x / 2) * w..=(rep_x / 2 + 1) * w {
                let sx = snap(canvas.min.x + x as f32 * cell).min(shown.max.x - px);
                let c = if x.rem_euclid(w) % 8 == 0 { thick } else { thin };
                line(Rect::from_min_max(Pos2::new(sx, shown.min.y), Pos2::new(sx + px, shown.max.y)), c);
            }
            for y in -(rep_y / 2) * h..=(rep_y / 2 + 1) * h {
                let sy = snap(canvas.min.y + y as f32 * cell).min(shown.max.y - px);
                let c = if y.rem_euclid(h) % 8 == 0 { thick } else { thin };
                line(Rect::from_min_max(Pos2::new(shown.min.x, sy), Pos2::new(shown.max.x, sy + px)), c);
            }
        }
        let mut frame_rect = |r: Rect, w: f32, c: Color32| {
            line(Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + w)), c);
            line(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - w), r.max), c);
            line(Rect::from_min_max(r.min, Pos2::new(r.min.x + w, r.max.y)), c);
            line(Rect::from_min_max(Pos2::new(r.max.x - w, r.min.y), r.max), c);
        };
        if let Some((x, y)) = self.canvas.hover_raw.filter(|_| !clean) {
            let r = Rect::from_min_size(canvas.min + Vec2::new(x as f32, y as f32) * cell, Vec2::splat(cell));
            frame_rect(r, px, Color32::from_white_alpha(200));
        }
        if !clean && copies.is_empty() {
            frame_rect(canvas.expand(px), px, Color32::from_gray(90));
        } else if !clean {
            // Kachel-Modus: a clear frame around the real image
            frame_rect(canvas.expand(px), px, Color32::from_black_alpha(200));
            frame_rect(canvas.expand(2.0 * px), px, Color32::from_rgb(255, 210, 90));
        }
        let to_screen = |r: pixel_core::Rect| {
            Rect::from_min_size(
                canvas.min + Vec2::new(r.x as f32, r.y as f32) * cell,
                Vec2::new(r.w as f32, r.h as f32) * cell,
            )
        };
        let mut labels = Vec::new();
        let mut marker_arms: Vec<(Rect, Color32)> = Vec::new();
        if self.show_regions && !clean {
            for (i, r) in self.doc.regions.iter().enumerate() {
                let Some(rc) = r.rect_at(self.frame) else { continue };
                let c = r.color.map_or(render::auto_color(i), |c| c.0);
                let col = Color32::from_rgb(c[0], c[1], c[2]);
                let sr = to_screen(rc);
                if r.is_marker() {
                    // a point: crosshair arms around the pixel, the pixel stays visible
                    let (cx, cy) = (sr.center().x, sr.center().y);
                    let arm = sr.width().max(8.0 * px);
                    let t = 2.0 * px;
                    for arm_rect in [
                        Rect::from_min_max(Pos2::new(sr.min.x - arm, cy - px), Pos2::new(sr.min.x, cy - px + t)),
                        Rect::from_min_max(Pos2::new(sr.max.x, cy - px), Pos2::new(sr.max.x + arm, cy - px + t)),
                        Rect::from_min_max(Pos2::new(cx - px, sr.min.y - arm), Pos2::new(cx - px + t, sr.min.y)),
                        Rect::from_min_max(Pos2::new(cx - px, sr.max.y), Pos2::new(cx - px + t, sr.max.y + arm)),
                    ] {
                        marker_arms.push((arm_rect, col));
                    }
                    let mut label = format!("+ {}", r.name);
                    if r.locked {
                        label += " (gesperrt)";
                    }
                    labels.push((Pos2::new(sr.max.x + 2.0 * px, sr.min.y - 2.0 * px), label, col));
                    continue;
                }
                frame_rect(sr, 2.0 * px, col);
                if !r.applies_to_layer(&self.doc, self.layer) {
                    frame_rect(sr.shrink(2.0 * px), px, Color32::from_black_alpha(160));
                }
                let mut label = r.name.clone();
                if r.locked {
                    label += " (gesperrt)";
                }
                labels.push((sr.left_top(), label, col));
            }
        }
        // the selection outline follows the mask: white inside, black outside
        if let Some(sel) = self.selection.as_ref().filter(|_| !clean) {
            let edges = sel.outline();
            let corner = |x: i32, y: i32| canvas.min + Vec2::new(x as f32, y as f32) * cell;
            for (c, outer) in [(Color32::BLACK, true), (Color32::WHITE, false)] {
                for &(x, y, horizontal) in &edges {
                    let a = corner(x, y);
                    // which side is outside: the unselected neighbour
                    let r = if horizontal {
                        let below_out = !sel.contains(x, y);
                        let y0 = if below_out == outer { a.y } else { a.y - px };
                        Rect::from_min_size(Pos2::new(a.x - px, y0), Vec2::new(cell + 2.0 * px, px))
                    } else {
                        let right_out = !sel.contains(x, y);
                        let x0 = if right_out == outer { a.x } else { a.x - px };
                        Rect::from_min_size(Pos2::new(x0, a.y - px), Vec2::new(px, cell + 2.0 * px))
                    };
                    let r = if outer {
                        r
                    } else {
                        r.shrink2(Vec2::new(if horizontal { px } else { 0.0 }, if horizontal { 0.0 } else { px }))
                    };
                    line(r, c);
                }
            }
        }
        // "KI zeigen": the marks of this frame, the selected one with handles
        if !clean {
            let (rects, texts) = self.mark_overlays(to_screen, canvas, px);
            marker_arms.extend(rects);
            labels.extend(texts);
        }
        // follow mode: changed pixels flash, the outline fades out
        for fl in self.follow.flashes.iter().filter(|_| !clean) {
            let t = ((self.follow.now - fl.start) / super::follow::FLASH_SECS) as f32;
            if fl.frame != self.frame || !(0.0..1.0).contains(&t) {
                continue;
            }
            let a = 1.0 - t;
            let fill = Color32::from_rgba_unmultiplied(255, 220, 60, (170.0 * a) as u8);
            for &(x, y) in &fl.points {
                let r = Rect::from_min_size(canvas.min + Vec2::new(x as f32, y as f32) * cell, Vec2::splat(cell));
                marker_arms.push((r, fill));
            }
            if let Some(b) = fl.bbox {
                let r = to_screen(b).expand(2.0 * px);
                let c = Color32::from_rgba_unmultiplied(255, 220, 60, (255.0 * a) as u8);
                let w = 2.0 * px;
                marker_arms.push((Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + w)), c));
                marker_arms.push((Rect::from_min_max(Pos2::new(r.min.x, r.max.y - w), r.max), c));
                marker_arms.push((Rect::from_min_max(r.min, Pos2::new(r.min.x + w, r.max.y)), c));
                marker_arms.push((Rect::from_min_max(Pos2::new(r.max.x - w, r.min.y), r.max), c));
            }
        }
        for (r, c) in marker_arms {
            mesh.add_colored_rect(r, c);
        }
        painter.add(egui::Shape::mesh(mesh));
        if !clean {
            self.paint_axes(&painter, canvas, cell, &grips);
        }
        // follow mode: the journal note as a banner at the top of the canvas
        if let Some(note) = self.follow.banner_text().filter(|_| !clean) {
            let galley = painter.layout_no_wrap(note.to_string(), egui::FontId::proportional(15.0), Color32::BLACK);
            let r = Rect::from_center_size(
                Pos2::new(area.center().x, area.min.y + 22.0),
                galley.size() + Vec2::new(16.0, 8.0),
            );
            painter.rect_filled(r, 6.0, Color32::from_rgb(255, 210, 90));
            painter.galley(r.min + Vec2::new(8.0, 4.0), galley, Color32::BLACK);
        }
        // labels are UI text (anti-aliased), drawn outside the art pixels
        for (pos, text, col) in labels {
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.0), Color32::BLACK);
            let r =
                Rect::from_min_size(pos - Vec2::new(0.0, galley.size().y + 2.0), galley.size() + Vec2::new(4.0, 2.0));
            painter.rect_filled(r, 2.0, col);
            painter.galley(r.min + Vec2::new(2.0, 1.0), galley, Color32::BLACK);
        }
    }

    fn tool_input(
        &mut self,
        resp: &egui::Response,
        pointer: &egui::PointerState,
        mods: egui::Modifiers,
        to_px: impl Fn(Pos2) -> (i32, i32),
    ) {
        let alt = mods.alt;
        let pos = pointer.latest_pos().map(&to_px);
        let pressed = resp.hovered() && (pointer.primary_pressed() || pointer.secondary_pressed());
        let (layer, frame) = (self.layer, self.frame);

        if self.tool == Tool::Point {
            self.mark_input(resp, pointer, pos);
            return;
        }
        if pressed && self.tool.selects() {
            let Some(p) = pos else { return };
            let w = self.wrap(p);
            self.canvas.sel_shift = (p.0 - w.0, p.1 - w.1);
            self.playing = false;
            if pointer.secondary_pressed() {
                // the right button opens the selection menu (on release, see
                // canvas_ui); with the selection brush, dragging it removes
                if self.tool == Tool::SelectBrush {
                    self.canvas.select_drag = Some(SelectDrag::RightBrush { start: p, last: p });
                }
                return;
            }
            let op = SelOp::from_modifiers(mods);
            self.select_press(p, op);
            return;
        }
        if pressed {
            let Some(p) = pos else { return };
            // right button paints transparent; Alt+click picks a color
            let key = if pointer.secondary_pressed() { TRANSPARENT } else { self.color };
            if alt || self.tool == Tool::Eyedropper {
                let p = self.wrap(p);
                if self.doc.in_bounds(p.0, p.1) {
                    let k = match self.doc.get(layer, frame, p.0, p.1) {
                        TRANSPARENT => self.doc.top_key(frame, p.0, p.1),
                        k => k,
                    };
                    if k != TRANSPARENT {
                        self.color = k;
                    }
                }
                return;
            }
            self.playing = false;
            self.commit_floating();
            if self.blocked() {
                return;
            }
            match self.tool {
                Tool::Fill => {
                    // with symmetry, each mirrored seed fills its own area too
                    // in the Kachel-Modus the fill runs across the edges
                    let seeds: Vec<(i32, i32)> =
                        self.paint_points(vec![p]).into_iter().filter(|&(x, y)| self.doc.in_bounds(x, y)).collect();
                    if !seeds.is_empty() {
                        let mut seen = std::collections::HashSet::new();
                        let pts: Vec<(i32, i32)> = seeds
                            .iter()
                            .flat_map(|s| self.flood(s.0, s.1))
                            .filter(|&(x, y)| self.may_paint(x, y) && seen.insert((x, y)))
                            .collect();
                        let with = match self.active_dither(key) {
                            Some((pat, a, b)) => format!(
                                "Dither {} aus {} und {}",
                                pat.label(),
                                self.doc.key_label(a),
                                self.doc.key_label(b)
                            ),
                            None => self.doc.key_label(key),
                        };
                        let plan = self.tool_plan(&pts, key);
                        let desc = format!("Füllen {} Pixel mit {with} · {}", pts.len(), self.here());
                        self.edit(desc, |d| plan.apply(d, layer, frame));
                    }
                }
                Tool::Pencil | Tool::Eraser | Tool::Shade => {
                    let key = if self.tool == Tool::Eraser { TRANSPARENT } else { key };
                    self.history.push(self.doc.clone(), "Stift");
                    let mut g = Gesture::new(p, key);
                    g.base = self.doc.cel(layer, frame).to_vec();
                    g.steps = if pointer.secondary_pressed() { -1 } else { 1 };
                    self.canvas.gesture = Some(g);
                    self.redraw_stroke();
                }
                Tool::Line | Tool::Rect | Tool::Ellipse => {
                    self.canvas.gesture = Some(Gesture::new(p, key));
                }
                _ => {}
            }
            return;
        }

        if self.canvas.select_drag.is_some() {
            self.select_drag(pos, pointer.primary_down() || pointer.secondary_down());
            return;
        }
        let Some(g) = &mut self.canvas.gesture else { return };
        let down = pointer.primary_down() || pointer.secondary_down();
        if let Some(p) = pos {
            if down && p != g.last {
                if matches!(self.tool, Tool::Pencil | Tool::Eraser | Tool::Shade) {
                    g.path.extend(draw::line(g.last.0, g.last.1, p.0, p.1).into_iter().skip(1));
                    g.last = p;
                    self.redraw_stroke();
                } else {
                    g.last = p;
                }
            }
        }
        if !down {
            let g = self.canvas.gesture.take().unwrap();
            let (s, e) = (g.start, g.last);
            let filled = self.filled;
            let pts = match self.tool {
                Tool::Line => draw::line(s.0, s.1, e.0, e.1),
                Tool::Rect => draw::rect(s.0, s.1, e.0, e.1, filled),
                Tool::Ellipse => draw::ellipse_in_box(s.0, s.1, e.0, e.1, filled),
                _ => {
                    // stroke finished: one undo step with a description
                    let what = match self.tool {
                        Tool::Eraser => "Radierer".to_string(),
                        Tool::Shade if g.steps > 0 => "Schattieren heller".to_string(),
                        Tool::Shade => "Schattieren dunkler".to_string(),
                        _ => format!("Stift {}", self.doc.key_label(g.key)),
                    };
                    let desc = format!("{what}: {} Pixel · {}", g.painted, self.here());
                    self.history.describe_last(&desc);
                    self.history.discard_if_unchanged(&self.doc);
                    if self.history.undo_steps().last() == Some(&desc.as_str()) {
                        self.unsaved_steps.push(desc);
                    }
                    return;
                }
            };
            let pts = self.paint_points(pts);
            let with = match self.active_dither(g.key) {
                Some((pat, a, b)) => {
                    format!("Dither {} aus {} und {}", pat.label(), self.doc.key_label(a), self.doc.key_label(b))
                }
                None => self.doc.key_label(g.key),
            };
            let plan = self.tool_plan(&pts, g.key);
            let desc = format!("{} {with} · {}", self.tool.label(), self.here());
            self.edit(desc, |d| plan.apply(d, layer, frame));
        }
    }

    /// Freehand tools: write the whole stroke again from the cel it started
    /// on. With Pixel-Perfect (Stift, Radierer) the corner pixels of the
    /// path's "L"s are left out, and pixels a corner had already painted
    /// return to their color before the stroke. Wrapping (Kachel-Modus) and
    /// symmetry apply to the cleaned path, so every copy is clean too.
    /// Schattieren steps each pixel once from its color before the stroke.
    fn redraw_stroke(&mut self) {
        let (layer, frame, tool) = (self.layer, self.frame, self.tool);
        let Some(g) = &self.canvas.gesture else { return };
        let path =
            if self.pixel_perfect && tool != Tool::Shade { draw::pixel_perfect(&g.path) } else { g.path.clone() };
        let pts = self.paint_points(path);
        let keep: std::collections::HashSet<(i32, i32)> = pts.iter().copied().collect();
        let Some(g) = &mut self.canvas.gesture else { return };
        let w = self.doc.width as i32;
        let base = |(x, y): (i32, i32)| g.base[(y * w + x) as usize];
        let mut changed = false;
        for &p in &g.written {
            if !keep.contains(&p) {
                changed |= self.doc.set(layer, frame, p.0, p.1, base(p));
            }
        }
        let mut painted = 0;
        for &p in &pts {
            let k = match tool {
                Tool::Shade => self.doc.shade_key(base(p), g.steps).unwrap_or(base(p)),
                _ => g.key,
            };
            changed |= self.doc.set(layer, frame, p.0, p.1, k);
            painted += usize::from(k != base(p));
        }
        g.painted = painted;
        g.written = pts;
        if changed {
            self.touch();
        }
    }

    /// Selection tools, press: grab the selected pixels to move them (a
    /// plain press inside the selection) or start a selection gesture.
    /// `p` is the canvas pixel; in the Kachel-Modus it may lie on a copy.
    fn select_press(&mut self, p: (i32, i32), op: SelOp) {
        let q = self.wrap(p);
        let inside_float = self.floating.as_ref().is_some_and(|f| f.rect.contains(q.0, q.1))
            && self.selection.as_ref().is_some_and(|s| s.contains(q.0, q.1));
        let inside_sel = self.selection.as_ref().is_some_and(|s| s.contains(q.0, q.1));
        let brush = self.tool == Tool::SelectBrush;
        if op == SelOp::Replace && !brush && (inside_float || inside_sel) {
            if self.tool == Tool::Wand {
                self.canvas.select_drag = Some(SelectDrag::Wand { start: q });
                return;
            }
            if inside_float || self.lift_selection() {
                self.start_move(q);
                return;
            }
        }
        self.commit_floating();
        let base = self.selection.clone();
        match self.tool {
            Tool::SelectBrush => {
                // painting adds; Alt or Ctrl (or dragging the right button) take pixels away
                let op = if op == SelOp::Subtract { SelOp::Subtract } else { SelOp::Add };
                self.brush_select(&[p], op);
                self.canvas.select_drag = Some(SelectDrag::Brush { last: p, op });
            }
            Tool::Wand => self.wand_select(q, op),
            Tool::Lasso => {
                self.canvas.select_drag = Some(SelectDrag::Lasso { path: vec![p], op, base });
            }
            _ => {
                if op == SelOp::Replace {
                    self.selection = None;
                }
                self.canvas.select_drag = Some(SelectDrag::New { start: p, last: p, op, base });
                self.select_drag(Some(p), true);
            }
        }
    }

    fn start_move(&mut self, p: (i32, i32)) {
        let r = self.floating.as_ref().unwrap().rect;
        self.canvas.select_drag = Some(SelectDrag::Move { grab: (p.0 - r.x, p.1 - r.y) });
    }

    fn brush_select(&mut self, pts: &[(i32, i32)], op: SelOp) {
        if let Some(new) = Selection::from_points(self.wrap_points(pts.iter().copied())) {
            let base = self.selection.clone();
            self.combine_selection(base.as_ref(), &new, op);
        }
    }

    fn wand_select(&mut self, p: (i32, i32), op: SelOp) {
        let Some(new) = Selection::from_points(self.wand_points(p.0, p.1)) else { return };
        let base = self.selection.clone();
        self.combine_selection(base.as_ref(), &new, op);
        if let Some(s) = &self.selection {
            self.status = format!("Zauberstab: {} Pixel ausgewählt", s.count());
        }
    }

    /// The rectangle selection between two canvas pixels; in the Kachel-Modus
    /// the part over a copy wraps into the image.
    fn rect_selection(&self, a: (i32, i32), b: (i32, i32)) -> Option<Selection> {
        if self.tile_mode == TileMode::Off {
            return Some(Selection::rect(drag_rect(a, b)));
        }
        Selection::from_points(self.wrap_points(draw::rect(a.0, a.1, b.0, b.1, true)))
    }

    fn select_drag(&mut self, pos: Option<(i32, i32)>, down: bool) {
        let Some(mut drag) = self.canvas.select_drag.take() else { return };
        let (w, h) = (self.doc.width, self.doc.height);
        let mut moving = None;
        // moving: relative to the copy the gesture started in
        let shift = self.canvas.sel_shift;
        if let Some(p) = pos {
            let local = (p.0 - shift.0, p.1 - shift.1);
            match &mut drag {
                SelectDrag::Move { grab } => self.move_floating_to(local.0 - grab.0, local.1 - grab.1),
                SelectDrag::New { start, last, op, base } => {
                    *last = p;
                    self.selection = match self.rect_selection(*start, p) {
                        Some(shape) => Selection::combine(base.as_ref(), &shape, *op, w, h),
                        None => base.clone(),
                    };
                }
                SelectDrag::Lasso { path, op, base } => {
                    if path.last() != Some(&p) {
                        path.push(p);
                    }
                    let shape = Selection::from_points(self.wrap_points(lasso_points(path)));
                    self.selection = match shape {
                        Some(shape) => Selection::combine(base.as_ref(), &shape, *op, w, h),
                        None => base.clone(),
                    };
                }
                SelectDrag::Brush { last, op } => {
                    if *last != p {
                        let pts = draw::line(last.0, last.1, p.0, p.1);
                        *last = p;
                        let op = *op;
                        self.brush_select(&pts, op);
                    }
                }
                SelectDrag::RightBrush { start, last } => {
                    if *last != p {
                        let mut pts = draw::line(last.0, last.1, p.0, p.1);
                        if *last == *start {
                            pts.push(*start); // the first pixel waited for the drag
                        }
                        *last = p;
                        self.brush_select(&pts, SelOp::Subtract);
                    }
                }
                SelectDrag::Wand { start } => {
                    // dragged away from the press: move the selection instead
                    if local != *start && self.lift_selection() {
                        let r = self.floating.as_ref().unwrap().rect;
                        let grab = (start.0 - r.x, start.1 - r.y);
                        self.move_floating_to(local.0 - grab.0, local.1 - grab.1);
                        moving = Some(SelectDrag::Move { grab });
                    }
                }
            }
        }
        if let Some(m) = moving {
            drag = m;
        }
        if down {
            self.canvas.select_drag = Some(drag);
            return;
        }
        match drag {
            SelectDrag::New { start, last, op: SelOp::Replace, .. } if start == last && pos == Some(start) => {
                // a plain click deselects
                self.selection = None;
            }
            SelectDrag::Lasso { path, op: SelOp::Replace, .. } if path.len() == 1 => self.selection = None,
            SelectDrag::Wand { start } => self.wand_select(start, SelOp::Replace),
            _ => {}
        }
    }
}

/// The rectangle spanned by two pixels (both included).
pub(super) fn drag_rect(a: (i32, i32), b: (i32, i32)) -> pixel_core::Rect {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    pixel_core::Rect::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32)
}

/// Screen size of a symmetry axis grip (points).
const GRIP: f32 = 12.0;

impl EditorApp {
    /// Grips of the active symmetry axes: just outside the canvas, above
    /// (vertical axis) and left of it (horizontal axis).
    fn axis_grips(&self, canvas: Rect, cell: f32) -> Vec<(super::transform::AxisDrag, Rect)> {
        use super::transform::AxisDrag;
        let m = self.symmetry.mode;
        let (ax, ay) = self.axis2();
        let mut out = Vec::new();
        if m.vertical() {
            let x = canvas.min.x + ax as f32 / 2.0 * cell;
            out.push((AxisDrag::X, Rect::from_center_size(Pos2::new(x, canvas.min.y - GRIP), Vec2::splat(GRIP))));
        }
        if m.horizontal() {
            let y = canvas.min.y + ay as f32 / 2.0 * cell;
            out.push((AxisDrag::Y, Rect::from_center_size(Pos2::new(canvas.min.x - GRIP, y), Vec2::splat(GRIP))));
        }
        out
    }

    /// Drag an axis by its grip; true while the pointer belongs to it.
    fn axis_input(
        &mut self,
        resp: &egui::Response,
        pointer: &egui::PointerState,
        canvas: Rect,
        cell: f32,
        grips: &[(super::transform::AxisDrag, Rect)],
    ) -> bool {
        use super::transform::AxisDrag;
        if resp.hovered() && pointer.primary_pressed() {
            if let Some(p) = pointer.interact_pos() {
                if let Some((a, _)) = grips.iter().find(|(_, r)| r.expand(3.0).contains(p)) {
                    self.canvas.axis_drag = Some(*a);
                }
            }
        }
        let Some(axis) = self.canvas.axis_drag else { return false };
        if let Some(p) = pointer.latest_pos() {
            // snap to pixel edges and pixel centres
            let (w, h) = (self.doc.width as i32, self.doc.height as i32);
            match axis {
                AxisDrag::X => {
                    self.symmetry.x2 = Some((((p.x - canvas.min.x) / cell * 2.0).round() as i32).clamp(1, 2 * w - 1))
                }
                AxisDrag::Y => {
                    self.symmetry.y2 = Some((((p.y - canvas.min.y) / cell * 2.0).round() as i32).clamp(1, 2 * h - 1))
                }
            }
        }
        if !pointer.primary_down() {
            self.canvas.axis_drag = None;
            let (ax, ay) = self.axis2();
            self.status = format!("Symmetrieachse: x {} · y {}", ax as f32 / 2.0, ay as f32 / 2.0);
        }
        true
    }

    /// The symmetry axes as dashed lines with their grips.
    fn paint_axes(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        cell: f32,
        grips: &[(super::transform::AxisDrag, Rect)],
    ) {
        use super::transform::AxisDrag;
        let (ax, ay) = self.axis2();
        let col = Color32::from_rgb(255, 120, 200);
        for (dark, w) in [(true, 3.0), (false, 1.5)] {
            let stroke = egui::Stroke::new(w, if dark { Color32::from_black_alpha(140) } else { col });
            for (axis, grip) in grips {
                let (a, b) = match axis {
                    AxisDrag::X => {
                        let x = canvas.min.x + ax as f32 / 2.0 * cell;
                        (Pos2::new(x, canvas.min.y - 4.0), Pos2::new(x, canvas.max.y + 4.0))
                    }
                    AxisDrag::Y => {
                        let y = canvas.min.y + ay as f32 / 2.0 * cell;
                        (Pos2::new(canvas.min.x - 4.0, y), Pos2::new(canvas.max.x + 4.0, y))
                    }
                };
                painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 6.0, 4.0));
                if !dark {
                    // a triangle pointing at the canvas
                    let c = grip.center();
                    let pts = match axis {
                        AxisDrag::X => vec![grip.left_top(), grip.right_top(), Pos2::new(c.x, grip.max.y)],
                        AxisDrag::Y => vec![grip.left_top(), grip.left_bottom(), Pos2::new(grip.max.x, c.y)],
                    };
                    painter.add(egui::Shape::convex_polygon(pts, col, egui::Stroke::new(1.0, Color32::BLACK)));
                }
            }
        }
    }
}
