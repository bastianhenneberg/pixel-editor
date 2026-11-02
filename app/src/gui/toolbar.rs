//! The tool palette as an icon toolbar with groups (like Photoshop): one
//! button per group shows its last used tool; a small triangle marks
//! groups with more tools, which a right click, a long press or a click on
//! the triangle opens as a flyout. Also the selection menu (canvas context
//! menu and menu bar).

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};

use super::{EditorApp, Tool};

/// Tool groups: name and tools, the first one is the default.
pub const GROUPS: [(&str, &[Tool]); 8] = [
    ("Stift", &[Tool::Pencil, Tool::Shade]),
    ("Radierer", &[Tool::Eraser]),
    ("Füllen", &[Tool::Fill]),
    ("Linie", &[Tool::Line]),
    ("Formen", &[Tool::Rect, Tool::Ellipse]),
    ("Pipette", &[Tool::Eyedropper]),
    ("Auswahl-Werkzeuge", &[Tool::Select, Tool::SelectBrush, Tool::Wand, Tool::Lasso]),
    ("KI zeigen", &[Tool::Point]),
];

const BUTTON: f32 = 34.0;
/// Seconds a press on a group button takes to open its flyout.
const LONG_PRESS: f64 = 0.45;

pub fn group_of(t: Tool) -> usize {
    GROUPS.iter().position(|(_, tools)| tools.contains(&t)).unwrap_or(0)
}

/// Name and shortcut, e.g. "Zauberstab (W)".
pub fn tool_title(t: Tool) -> String {
    format!("{} ({})", t.label(), t.shortcut_label())
}

/// The flyout of a tool group.
#[derive(Clone, Copy)]
pub struct Flyout {
    pub group: usize,
    /// Screen position of its top-left corner.
    at: Pos2,
    /// Frame it was opened in (the opening click must not close it).
    opened: u64,
    /// Opened by a long press: the release of that press is no click.
    long: bool,
}

/// Draw the icon of `t` into `r`.
pub fn paint_tool_icon(p: &egui::Painter, r: Rect, t: Tool, c: Color32) {
    let s = r.width() / 16.0; // icons are designed on a 16 x 16 grid
    let at = |x: f32, y: f32| r.min + Vec2::new(x, y) * s;
    let stroke = Stroke::new(1.6 * s, c);
    let thin = Stroke::new(1.0 * s, c);
    let dashed_rect = |rr: Rect| {
        let mut segs = Vec::new();
        for (a, b) in [
            (rr.left_top(), rr.right_top()),
            (rr.right_top(), rr.right_bottom()),
            (rr.right_bottom(), rr.left_bottom()),
            (rr.left_bottom(), rr.left_top()),
        ] {
            segs.extend(Shape::dashed_line(&[a, b], thin, 2.0 * s, 1.5 * s));
        }
        p.extend(segs);
    };
    match t {
        Tool::Pencil => {
            p.line_segment([at(3.0, 13.0), at(12.0, 4.0)], Stroke::new(3.0 * s, c));
            p.add(Shape::convex_polygon(vec![at(1.5, 14.5), at(4.0, 13.8), at(2.2, 12.0)], c, Stroke::NONE));
        }
        Tool::Shade => {
            // a pencil over three steps of a ramp, dark to light
            for (i, a) in [90u8, 160, 235].into_iter().enumerate() {
                let col = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
                p.rect_filled(Rect::from_min_size(at(1.0 + 4.0 * i as f32, 11.0), Vec2::new(4.0, 4.0) * s), 0.0, col);
            }
            p.line_segment([at(6.0, 9.0), at(13.0, 2.0)], Stroke::new(2.6 * s, c));
            p.add(Shape::convex_polygon(vec![at(4.5, 10.5), at(6.8, 9.9), at(5.1, 8.2)], c, Stroke::NONE));
        }
        Tool::Eraser => {
            let body = vec![at(2.0, 10.0), at(8.0, 4.0), at(14.0, 8.0), at(8.0, 14.0)];
            p.add(Shape::closed_line(body, stroke));
            p.add(Shape::convex_polygon(
                vec![at(2.0, 10.0), at(5.0, 7.0), at(11.0, 11.0), at(8.0, 14.0)],
                c,
                Stroke::NONE,
            ));
        }
        Tool::Fill => {
            let bucket = vec![at(2.0, 7.0), at(7.0, 2.0), at(12.0, 7.0), at(7.0, 12.0)];
            p.add(Shape::closed_line(bucket, stroke));
            p.line_segment([at(4.5, 9.5), at(9.5, 4.5)], thin);
            p.circle_filled(at(13.0, 12.0), 1.8 * s, c);
        }
        Tool::Line => {
            p.line_segment([at(3.0, 13.0), at(13.0, 3.0)], stroke);
            p.rect_filled(Rect::from_center_size(at(3.0, 13.0), Vec2::splat(3.0 * s)), 0.0, c);
            p.rect_filled(Rect::from_center_size(at(13.0, 3.0), Vec2::splat(3.0 * s)), 0.0, c);
        }
        Tool::Rect => {
            p.rect_stroke(Rect::from_min_max(at(2.5, 3.5), at(13.5, 12.5)), 0.0, stroke, StrokeKind::Inside);
        }
        Tool::Ellipse => {
            p.add(egui::epaint::EllipseShape::stroke(at(8.0, 8.0), Vec2::new(6.0, 4.8) * s, stroke));
        }
        Tool::Eyedropper => {
            p.line_segment([at(3.0, 13.0), at(10.0, 6.0)], Stroke::new(2.0 * s, c));
            p.circle_filled(at(11.5, 4.5), 2.6 * s, c);
            p.circle_filled(at(2.5, 13.5), 1.0 * s, c);
        }
        Tool::Select => dashed_rect(Rect::from_min_max(at(2.5, 3.5), at(13.5, 12.5))),
        Tool::SelectBrush => {
            // single pixels picked into a dashed outline
            for (x, y) in [(3.0, 9.0), (6.0, 9.0), (6.0, 6.0), (9.0, 6.0), (9.0, 3.0)] {
                p.rect_filled(Rect::from_min_size(at(x, y), Vec2::splat(2.6 * s)), 0.0, c);
            }
            p.line_segment([at(10.0, 13.5), at(14.0, 9.5)], Stroke::new(2.4 * s, c));
            dashed_rect(Rect::from_min_max(at(2.0, 2.0), at(13.0, 13.0)));
        }
        Tool::Wand => {
            p.line_segment([at(2.5, 13.5), at(10.0, 6.0)], Stroke::new(2.2 * s, c));
            let (cx, cy) = (11.5, 4.5);
            for (dx, dy) in [(0.0, -3.5), (0.0, 3.5), (-3.5, 0.0), (3.5, 0.0)] {
                p.line_segment([at(cx, cy), at(cx + dx, cy + dy)], thin);
            }
            p.circle_filled(at(cx, cy), 1.4 * s, c);
        }
        Tool::Lasso => {
            p.add(egui::epaint::EllipseShape::stroke(at(8.5, 6.5), Vec2::new(5.5, 4.0) * s, thin));
            p.add(Shape::line(vec![at(5.0, 9.8), at(4.0, 12.0), at(6.0, 14.5)], thin));
        }
        Tool::Point => {
            let rr = Rect::from_center_size(r.center(), Vec2::splat(11.0 * s));
            p.rect_stroke(rr, 0.0, Stroke::new(1.8 * s, super::canvas::PIN_COLOR), StrokeKind::Inside);
            p.rect_filled(Rect::from_center_size(r.center(), Vec2::splat(3.5 * s)), 0.0, super::canvas::PIN_COLOR);
        }
    }
}

impl EditorApp {
    /// The tool a group button shows: the active one, else the last used.
    fn shown_tool(&self, g: usize) -> Tool {
        if group_of(self.tool) == g {
            self.tool
        } else {
            self.group_tool[g]
        }
    }

    /// The icon toolbar (left panel).
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let frame = ui.ctx().cumulative_frame_nr();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            for (g, (name, tools)) in GROUPS.iter().enumerate() {
                let t = self.shown_tool(g);
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
                let title = tool_title(t);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, self.tool == t, &title));
                let active = self.tool == t;
                let visuals = ui.style().interact_selectable(&resp, active);
                let p = ui.painter();
                p.rect(rect, 4.0, visuals.weak_bg_fill, visuals.bg_stroke, StrokeKind::Inside);
                paint_tool_icon(p, rect.shrink(7.0), t, visuals.fg_stroke.color);
                let more = tools.len() > 1;
                let corner = Rect::from_min_max(rect.max - Vec2::splat(10.0), rect.max);
                if more {
                    let c = visuals.fg_stroke.color;
                    let (a, b, d) = (
                        rect.max - Vec2::new(3.0, 8.0),
                        rect.max - Vec2::new(3.0, 3.0),
                        rect.max - Vec2::new(8.0, 3.0),
                    );
                    p.add(Shape::convex_polygon(vec![a, b, d], c, Stroke::NONE));
                }
                let mut tip = title.clone();
                if more {
                    tip += &format!("\n{name}: Rechtsklick, langes Drücken oder Klick auf den Pfeil zeigt alle");
                }
                if t == Tool::Point {
                    tip += &format!("\n{}", super::panels::POINT_TOOL_HELP);
                }
                let resp = resp.on_hover_text(tip);
                // a long press opens the flyout
                let held = resp.is_pointer_button_down_on()
                    && ui.input(|i| i.pointer.press_start_time()).is_some_and(|s| now - s >= LONG_PRESS);
                if resp.is_pointer_button_down_on() && more {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
                let on_arrow = resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| corner.contains(p));
                let open = more && (resp.secondary_clicked() || on_arrow || held);
                if open {
                    if self.flyout.is_none_or(|f| f.group != g) {
                        self.flyout = Some(Flyout {
                            group: g,
                            at: rect.right_top() + Vec2::new(2.0, 0.0),
                            opened: frame,
                            long: held,
                        });
                    }
                } else if resp.clicked() {
                    match &mut self.flyout {
                        Some(f) if f.group == g && f.long => f.long = false,
                        _ => {
                            self.set_tool(t);
                            self.flyout = None;
                        }
                    }
                }
            }
        });
        self.flyout_ui(ui.ctx(), frame);
    }

    fn flyout_ui(&mut self, ctx: &egui::Context, frame: u64) {
        let Some(fl) = self.flyout else { return };
        let tools = GROUPS[fl.group].1;
        let mut pick = None;
        let area = egui::Area::new(egui::Id::unique("tool_flyout"))
            .fixed_pos(fl.at)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for &t in tools {
                        let label = format!("      {}  ({})", t.label(), t.shortcut_label());
                        let r =
                            ui.add(egui::Button::selectable(self.tool == t, label).min_size(Vec2::new(190.0, 24.0)));
                        let icon = Rect::from_min_size(r.rect.left_center() - Vec2::new(-6.0, 8.0), Vec2::splat(16.0));
                        paint_tool_icon(ui.painter(), icon, t, ui.visuals().text_color());
                        if r.clicked() {
                            pick = Some(t);
                        }
                    }
                });
            });
        if let Some(t) = pick {
            self.set_tool(t);
            self.flyout = None;
            return;
        }
        // a click elsewhere (or Esc) closes it
        let outside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p))
        });
        if frame != fl.opened && (outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.flyout = None;
        }
    }

    /// The Werkzeuge menu: single tools as entries, groups as submenus.
    pub(super) fn tool_menu_entries(&mut self, ui: &mut egui::Ui) {
        for (name, tools) in GROUPS {
            let entry = |app: &mut EditorApp, ui: &mut egui::Ui, t: Tool| {
                let mut r =
                    ui.add(egui::Button::selectable(app.tool == t, format!("{}  ({})", t.label(), t.shortcut_label())));
                if t == Tool::Point {
                    r = r.on_hover_text(super::panels::POINT_TOOL_HELP);
                }
                if r.clicked() {
                    app.set_tool(t);
                    ui.close();
                }
            };
            if tools.len() == 1 {
                entry(self, ui, tools[0]);
            } else {
                ui.menu_button(name, |ui| {
                    for &t in tools {
                        entry(self, ui, t);
                    }
                });
            }
        }
    }

    /// Selection commands: canvas context menu and the Auswahl menu.
    pub(super) fn selection_menu_entries(&mut self, ui: &mut egui::Ui) {
        let has = self.selection.is_some();
        if ui.add_enabled(has, egui::Button::new("Auswahl aufheben  (Strg+D)")).clicked() {
            self.select_none();
            ui.close();
        }
        if ui.button("Alles auswählen  (Strg+A)").clicked() {
            self.select_all();
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Auswahl umkehren  (Strg+Umschalt+I)")).clicked() {
            self.invert_selection();
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(has, egui::Button::new("Ausschneiden  (Strg+X)")).clicked() {
            self.cut_selection();
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Kopieren  (Strg+C)")).clicked() {
            self.copy_selection();
            ui.close();
        }
        if ui.add_enabled(self.clipboard.is_some(), egui::Button::new("Einfügen  (Strg+V)")).clicked() {
            self.paste();
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Löschen  (Entf)")).clicked() {
            self.delete_selection();
            ui.close();
        }
        ui.separator();
        self.selection_transform_entries(ui);
        if self.floating.is_some() {
            ui.separator();
            if ui.button("Hier vorne ablegen").clicked() {
                self.commit_floating_as(false);
                ui.close();
            }
            if ui.button("Hier hinten ablegen").clicked() {
                self.commit_floating_as(true);
                ui.close();
            }
        }
    }
}
