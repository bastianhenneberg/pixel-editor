//! GUI tests: drive the real app through egui_kittest (offscreen wgpu).

use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use pixel_core::{io, Document, TRANSPARENT};

use super::{EditorApp, Selection, Tool};

/// GPU tests run one at a time: several wgpu devices created in parallel
/// deadlocked on this machine (NVIDIA).
fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn harness(app: EditorApp) -> Harness<'static, EditorApp> {
    harness_ppp(app, 1.0)
}

fn harness_ppp(app: EditorApp, ppp: f32) -> Harness<'static, EditorApp> {
    Harness::builder().with_size([1280.0, 860.0]).with_pixels_per_point(ppp).wgpu().build_eframe(move |_cc| app)
}

/// Screen position (points) of the centre of art pixel (x, y).
fn px_center(h: &Harness<'_, EditorApp>, x: i32, y: i32) -> Pos2 {
    let app = h.state();
    let r = app.canvas.screen;
    let cell = r.width() / app.doc.width as f32;
    Pos2::new(r.min.x + (x as f32 + 0.5) * cell, r.min.y + (y as f32 + 0.5) * cell)
}

fn press(h: &Harness<'_, EditorApp>, pos: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pxe-gui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn pencil_stroke_and_undo() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    assert!(h.state().zoom >= 4.0, "canvas should be fitted");
    h.state_mut().color = b'2';
    let (a, b) = (px_center(&h, 5, 10), px_center(&h, 15, 10));
    h.hover_at(a);
    h.step();
    press(&h, a, true);
    h.step();
    h.hover_at(b);
    h.step();
    press(&h, b, false);
    h.step();
    let doc = &h.state().doc;
    for x in 5..=15 {
        assert_eq!(doc.get(0, 0, x, 10), b'2', "pixel {x},10 should be painted");
    }
    assert_eq!(doc.get(0, 0, 4, 10), TRANSPARENT);
    assert!(h.state().dirty);

    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert_eq!(h.state().doc.get(0, 0, 10, 10), TRANSPARENT, "undo removes the whole stroke");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z);
    h.step();
    assert_eq!(h.state().doc.get(0, 0, 10, 10), b'2', "redo restores it");
}

#[test]
fn rectangle_drag_and_fill_and_shortcuts() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.key_press(egui::Key::U);
    h.step();
    assert_eq!(h.state().tool, Tool::Rect);
    h.state_mut().color = b'9';
    let (a, b) = (px_center(&h, 2, 2), px_center(&h, 8, 6));
    h.hover_at(a);
    h.step();
    press(&h, a, true);
    h.step();
    h.hover_at(b);
    h.step();
    // not committed while dragging
    assert_eq!(h.state().doc.get(0, 0, 2, 2), TRANSPARENT);
    press(&h, b, false);
    h.step();
    let doc = &h.state().doc;
    assert_eq!(doc.get(0, 0, 2, 2), b'9');
    assert_eq!(doc.get(0, 0, 8, 6), b'9');
    assert_eq!(doc.get(0, 0, 5, 4), TRANSPARENT, "outline only");

    // flood fill the inside
    h.key_press(egui::Key::G);
    h.step();
    assert_eq!(h.state().tool, Tool::Fill);
    h.state_mut().color = b'4';
    let c = px_center(&h, 5, 4);
    h.hover_at(c);
    h.step();
    press(&h, c, true);
    h.step();
    press(&h, c, false);
    h.step();
    assert_eq!(h.state().doc.get(0, 0, 5, 4), b'4');
    assert_eq!(h.state().doc.get(0, 0, 0, 0), TRANSPARENT, "fill stays inside the outline");
}

#[test]
fn playback_follows_durations_and_tags() {
    let mut app = EditorApp::new(None);
    app.doc.add_frame(1, None, false).unwrap();
    app.doc.add_frame(2, None, false).unwrap();
    app.doc.frames[0].duration_ms = 100;
    app.doc.frames[1].duration_ms = 300;
    app.doc.frames[2].duration_ms = 100;
    let ctx = egui::Context::default();
    app.toggle_play(0.0);
    assert!(app.playing);
    app.advance_playback(&ctx, 0.05);
    assert_eq!(app.frame, 0);
    app.advance_playback(&ctx, 0.15);
    assert_eq!(app.frame, 1);
    app.advance_playback(&ctx, 0.39);
    assert_eq!(app.frame, 1, "frame 1 lasts 300 ms");
    app.advance_playback(&ctx, 0.41);
    assert_eq!(app.frame, 2);
    app.advance_playback(&ctx, 0.51);
    assert_eq!(app.frame, 0, "loops");

    let mut once = pixel_core::Tag::new("once", 1, 2);
    once.looping = false;
    app.doc.set_tag(once).unwrap();
    app.playing = false;
    app.play_tag = Some(0);
    app.frame = 1;
    app.toggle_play(1.0);
    app.advance_playback(&ctx, 1.5);
    assert_eq!(app.frame, 2);
    assert!(!app.playing, "non-looping tag stops at its last frame");
}

#[test]
fn auto_reload_on_external_change() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("reload");
    let path = dir.join("a.pxe.json");
    let doc = Document::new(8, 8, Document::builtin_palette("pico8").unwrap()).unwrap();
    io::save(&doc, &path).unwrap();

    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(2);
    assert_eq!(h.state().doc, doc);

    // another process (the MCP server) edits the file
    std::thread::sleep(Duration::from_millis(20));
    let mut changed = doc.clone();
    changed.set(0, 0, 3, 3, b'8');
    io::save(&changed, &path).unwrap();
    h.state_mut().last_poll = Instant::now() - Duration::from_secs(1);
    h.run_steps(2);
    assert_eq!(h.state().doc.get(0, 0, 3, 3), b'8', "reloaded");
    assert!(!h.state().dirty);
    assert!(h.state().status.contains("Neu geladen"), "status: {}", h.state().status);

    // with unsaved local edits, an external change raises a conflict instead
    h.state_mut().edit("test", |d| d.set(0, 0, 0, 0, b'1'));
    std::thread::sleep(Duration::from_millis(20));
    changed.set(0, 0, 4, 4, b'9');
    io::save(&changed, &path).unwrap();
    h.state_mut().last_poll = Instant::now() - Duration::from_secs(1);
    h.run_steps(2);
    assert!(h.state().external_conflict);
    assert_eq!(h.state().doc.get(0, 0, 0, 0), b'1', "local edit kept");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Renders the editor offscreen to `target/gui-screenshot.png` for a visual check.
#[test]
fn screenshot() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("shot");
    let path = dir.join("hero.pxe.json");
    let mut doc = Document::new(32, 32, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    doc.set_points(0, 0, &pixel_core::draw::ellipse(15, 18, 10, 8, true), b'6');
    doc.set_points(0, 0, &pixel_core::draw::ellipse(15, 18, 10, 8, false), b'0');
    doc.set_rows(0, 0, 10, 15, &["c0---c0".into()]).unwrap();
    doc.add_frame(1, Some(0), false).unwrap();
    doc.add_frame(2, Some(0), true).unwrap();
    doc.set_rows(0, 1, 10, 15, &["00---00".into()]).unwrap();
    let top = doc.add_layer("Glanz", 1);
    doc.set_points(top, 0, &pixel_core::draw::line(9, 13, 12, 12), b'c');
    doc.set_tag(pixel_core::Tag::new("idle", 0, 1)).unwrap();
    let mut blink = pixel_core::Tag::new("blink", 1, 2);
    blink.direction = pixel_core::document::Direction::PingPong;
    doc.set_tag(blink).unwrap();
    io::save(&doc, &path).unwrap();

    let mut app = EditorApp::new(Some(path));
    let mut face = pixel_core::Region::new("face", pixel_core::Rect::new(9, 13, 13, 6));
    face.locked = true;
    app.doc.regions.push(face);
    app.doc.regions.push(pixel_core::Region::new("left_foot", pixel_core::Rect::new(6, 23, 6, 3)));
    app.doc.palette[6].name = "slime".into();
    app.selection = Some(Selection::rect(pixel_core::Rect::new(18, 22, 6, 4)));
    app.history.push(app.doc.clone(), "Claude: set_rows idle/0 Ebene 1 [face] (4x2)");
    app.onion = true;
    app.frame = 1;
    app.play_tag = Some(0);
    let mut h = harness(app);
    h.run_steps(4);
    h.hover_at(px_center(&h, 12, 20));
    h.run_steps(2);
    let img = h.render().expect("offscreen render");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gui-screenshot.png");
    img.save(&out).unwrap();
    eprintln!("saved {}", out.display());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Every art pixel must cover exactly zoom x zoom screen pixels of its exact
/// palette color: no smoothing, no seams, also with fractional UI scaling.
#[test]
fn canvas_is_pixel_exact() {
    let _gpu = gpu_lock();
    for ppp in [1.0, 1.5] {
        let mut doc = Document::new(8, 6, Document::builtin_palette("pico8").unwrap()).unwrap();
        for y in 0..6 {
            for x in 0..8 {
                doc.set(0, 0, x, y, if (x + y) % 2 == 0 { b'8' } else { b'c' });
            }
        }
        let mut app = EditorApp::new(None);
        app.replace_doc(doc.clone());
        app.grid = false;
        app.preview.settings.open = false; // the floating preview would cover the zoomed canvas
        let mut h = harness_ppp(app, ppp);
        h.run_steps(4);
        let zoom = h.state().zoom as u32;
        let min = h.state().canvas.screen.min;
        let (ox, oy) = ((min.x * ppp).round() as u32, (min.y * ppp).round() as u32);
        assert!(((min.x * ppp) - ox as f32).abs() < 0.01, "canvas origin on the physical pixel grid");
        let img = h.render().unwrap();
        for y in 0..6u32 {
            for x in 0..8u32 {
                let want = doc.color_of(doc.get(0, 0, x as i32, y as i32)).unwrap().0;
                for dy in 0..zoom {
                    for dx in 0..zoom {
                        let got = img.get_pixel(ox + x * zoom + dx, oy + y * zoom + dy).0;
                        assert_eq!(got, want, "ppp {ppp}: art pixel ({x},{y}) screen offset ({dx},{dy})");
                    }
                }
            }
        }
    }
}

#[test]
fn timeline_and_aseprite_shortcuts() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.state_mut().doc.set(0, 0, 1, 1, b'8');
    // Alt+N: new frame (copy), Alt+B: new empty frame, Shift+N: new layer
    h.key_press_modifiers(Modifiers::ALT, egui::Key::N);
    h.step();
    assert_eq!(h.state().doc.frames.len(), 2);
    assert_eq!(h.state().frame, 1);
    assert_eq!(h.state().doc.get(0, 1, 1, 1), b'8');
    h.key_press_modifiers(Modifiers::ALT, egui::Key::B);
    h.step();
    assert_eq!(h.state().frame, 2);
    assert!(h.state().doc.cel_is_empty(0, 2));
    h.key_press_modifiers(Modifiers::SHIFT, egui::Key::N);
    h.step();
    assert_eq!(h.state().doc.layers.len(), 2);
    assert_eq!(h.state().layer, 1);
    // Alt+, goes back one frame
    h.key_press_modifiers(Modifiers::ALT, egui::Key::Comma);
    h.step();
    assert_eq!(h.state().frame, 1);
    h.key_press_modifiers(Modifiers::SHIFT, egui::Key::U);
    h.step();
    assert_eq!(h.state().tool, Tool::Ellipse);
    // "+ Verknüpft" creates a linked frame
    h.get_by_label("+ Verknüpft").click();
    h.run_steps(2);
    assert_eq!(h.state().frame, 2);
    assert!(h.state().doc.layers[0].is_linked(2));
    // drawing on the linked frame changes its source too
    h.state_mut().tool = Tool::Pencil;
    h.state_mut().color = b'9';
    h.state_mut().layer = 0;
    let p = px_center(&h, 5, 5);
    h.hover_at(p);
    h.step();
    press(&h, p, true);
    h.step();
    press(&h, p, false);
    h.step();
    assert_eq!(h.state().doc.get(0, 1, 5, 5), b'9');
}

#[test]
fn opens_and_saves_aseprite() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("ase");
    let path = dir.join("held.aseprite");
    let mut doc = Document::new(8, 8, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    doc.set(0, 0, 2, 2, b'5');
    io::save(&doc, &path).unwrap();
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(2);
    assert_eq!(h.state().doc.get(0, 0, 2, 2), b'5');
    h.state_mut().edit("test", |d| d.set(0, 0, 3, 3, b'9'));
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::S);
    h.step();
    let back = io::load(&path).unwrap();
    assert_eq!(back.get(0, 0, 3, 3), b'9');
    assert!(!h.state().dirty);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn click_px(h: &mut Harness<'_, EditorApp>, x: i32, y: i32) {
    let p = px_center(h, x, y);
    h.hover_at(p);
    h.step();
    press(h, p, true);
    h.step();
    press(h, p, false);
    h.step();
}

fn drag_px(h: &mut Harness<'_, EditorApp>, from: (i32, i32), to: (i32, i32)) {
    let (a, b) = (px_center(h, from.0, from.1), px_center(h, to.0, to.1));
    h.hover_at(a);
    h.step();
    press(h, a, true);
    h.step();
    h.hover_at(b);
    h.step();
    press(h, b, false);
    h.step();
}

#[test]
fn names_in_status_and_history() {
    let _gpu = gpu_lock();
    let mut app = EditorApp::new(None);
    let mut doc = Document::new(16, 16, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    doc.palette[9].name = "harness".into();
    doc.add_frame(1, None, false).unwrap();
    doc.add_frame(2, None, false).unwrap();
    doc.set_tag(pixel_core::Tag::new("walk_down", 1, 2)).unwrap();
    doc.rename_layer(0, "body");
    doc.set(0, 2, 4, 4, b'9');
    doc.regions.push(pixel_core::Region::new("left_shoe", pixel_core::Rect::new(3, 3, 3, 3)));
    app.replace_doc(doc);
    app.frame = 2;
    let mut h = harness(app);
    h.run_steps(3);
    h.hover_at(px_center(&h, 4, 4));
    h.step();
    let line = h.state().status_line();
    assert_eq!(line, "Frame walk_down/1 (f2) · Ebene body · x 4, y 4 · Farbe 9 harness · Region left_shoe");
    // a stroke becomes one described history step
    h.state_mut().color = b'9';
    drag_px(&mut h, (0, 0), (2, 0));
    let steps: Vec<String> = h.state().history.undo_steps().iter().map(|s| s.to_string()).collect();
    assert_eq!(steps, vec!["Stift 9 harness: 3 Pixel · walk_down/1 · body"]);
}

#[test]
fn locks_block_gui_edits() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.state_mut().color = b'2';
    h.state_mut().doc.layers[0].locked = true;
    click_px(&mut h, 1, 1);
    assert!(h.state().doc.cel_is_empty(0, 0), "locked layer");
    assert!(h.state().status.starts_with("Gesperrt"), "{}", h.state().status);
    h.state_mut().doc.layers[0].locked = false;
    let mut r = pixel_core::Region::new("face", pixel_core::Rect::new(0, 0, 4, 4));
    r.locked = true;
    h.state_mut().doc.regions.push(r);
    // a line across the locked region only paints outside it
    h.key_press(egui::Key::L);
    h.step();
    drag_px(&mut h, (0, 2), (7, 2));
    let d = &h.state().doc;
    assert_eq!(d.get(0, 0, 3, 2), TRANSPARENT);
    assert_eq!(d.get(0, 0, 4, 2), b'2');
    assert_eq!(d.get(0, 0, 7, 2), b'2');
}

#[test]
fn select_move_copy_paste() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.state_mut().doc.set_rows(0, 0, 2, 2, &["88".into(), "8c".into()]).unwrap();
    h.key_press(egui::Key::M);
    h.step();
    assert_eq!(h.state().tool, Tool::Select);
    drag_px(&mut h, (2, 2), (3, 3));
    assert_eq!(h.state().selection, Some(Selection::rect(pixel_core::Rect::new(2, 2, 2, 2))));
    // drag the selected pixels 3 to the right
    drag_px(&mut h, (2, 2), (5, 2));
    h.key_press(egui::Key::Enter);
    h.step();
    let d = &h.state().doc;
    assert_eq!(d.get(0, 0, 2, 2), TRANSPARENT, "moved away");
    assert_eq!(d.get(0, 0, 5, 2), b'8');
    assert_eq!(d.get(0, 0, 6, 3), b'c');
    let steps = h.state().history.undo_steps().join(" | ");
    assert!(steps.contains("Auswahl verschoben um (3, 0)"), "{steps}");
    // nudge with the arrow keys
    h.key_press(egui::Key::ArrowDown);
    h.step();
    h.key_press(egui::Key::Enter);
    h.step();
    assert_eq!(h.state().doc.get(0, 0, 5, 3), b'8');
    // copy, paste into another frame
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::C);
    h.step();
    h.key_press_modifiers(Modifiers::ALT, egui::Key::B);
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::V);
    h.step();
    h.key_press(egui::Key::Enter);
    h.step();
    let d = &h.state().doc;
    assert_eq!(h.state().frame, 1);
    assert_eq!(d.get(0, 1, 5, 3), b'8');
    assert_eq!(d.get(0, 1, 6, 4), b'c');
    // undo walks back through the described steps
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert!(h.state().doc.cel_is_empty(0, 1));
}

#[test]
fn region_from_selection_and_sequence_tag() {
    let _gpu = gpu_lock();
    let mut app = EditorApp::new(None);
    app.selection = Some(Selection::rect(pixel_core::Rect::new(1, 2, 3, 4)));
    app.region_draft.name = "face".into();
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Region aus Auswahl").click();
    h.run_steps(2);
    assert_eq!(h.state().doc.regions[0].rect_at(0), Some(pixel_core::Rect::new(1, 2, 3, 4)));
    assert_eq!(h.state().history.undo_steps().last().copied(), Some("Region \"face\" x 1..3 y 2..5 (3x4)"));
    // arbitrary frame order through linked frames
    h.state_mut().doc.add_frame(1, None, false).unwrap();
    h.state_mut().tag_draft.tag.name = "blink".into();
    h.state_mut().tag_draft.sequence = "0 1 0".into();
    h.run_steps(1);
    h.get_by_label("Als Folge anlegen").click();
    h.run_steps(2);
    let d = &h.state().doc;
    let t = d.tags.iter().find(|t| t.name == "blink").expect("tag created");
    assert_eq!((t.from, t.to), (2, 4));
    assert_eq!(d.layers[0].link_source(4), 0);
}

#[test]
fn reload_uses_journal_note() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("journal");
    let path = dir.join("a.pxe.json");
    let doc = Document::new(8, 8, Document::builtin_palette("pico8").unwrap()).unwrap();
    io::save(&doc, &path).unwrap();
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(2);
    // what the MCP server does: save + journal entry
    std::thread::sleep(Duration::from_millis(20));
    let mut changed = doc.clone();
    changed.set(0, 0, 1, 1, b'8');
    io::save(&changed, &path).unwrap();
    pixel_core::journal::Journal::for_document(&path).record(&changed, "claude", "Claude: lift left_shoe 1px").unwrap();
    h.state_mut().last_poll = Instant::now() - Duration::from_secs(1);
    h.run_steps(2);
    assert_eq!(h.state().history.undo_steps(), vec!["Claude: lift left_shoe 1px"]);
    assert!(h.state().status.contains("Claude: lift left_shoe 1px"));
    // a GUI save records its steps in the journal
    h.state_mut().color = b'2';
    click_px(&mut h, 3, 3);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::S);
    h.step();
    let entries = pixel_core::journal::Journal::for_document(&path).entries();
    let last = entries.last().unwrap();
    assert_eq!(last.author, "user");
    assert!(last.note.contains("Stift 2"), "{}", last.note);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn markers_in_the_gui() {
    let _gpu = gpu_lock();
    let mut app = EditorApp::new(None);
    let mut m = pixel_core::Region::marker("origin", 10, 10);
    m.locked = true;
    m.color = Some(pixel_core::Rgba([0, 255, 0, 255]));
    app.doc.regions.push(m);
    app.grid = false;
    let mut h = harness(app);
    h.run_steps(3);
    // painting at a locked marker works
    h.state_mut().color = b'2';
    click_px(&mut h, 10, 10);
    assert_eq!(h.state().doc.get(0, 0, 10, 10), b'2');
    // the marker is drawn as a crosshair around the pixel, not as a box
    h.remove_cursor();
    h.run_steps(1);
    let img = h.render().unwrap();
    let app = h.state();
    let cell = app.canvas.screen.width() / app.doc.width as f32;
    let at = |x: f32, y: f32| img.get_pixel(x as u32, y as u32).0;
    let (px0, py0) = (app.canvas.screen.min.x + 10.0 * cell, app.canvas.screen.min.y + 10.0 * cell);
    let red = app.doc.color_of(b'2').unwrap().0;
    assert_eq!(at(px0 + cell / 2.0, py0 + cell / 2.0), red, "marked pixel stays visible");
    assert_eq!(at(px0 - 3.0, py0 + cell / 2.0 - 1.0), [0, 255, 0, 255], "left arm");
    assert_ne!(at(px0 - 1.0, py0 - 1.0), [0, 255, 0, 255], "no box corner");
    // new marker from the selection
    let mut h2 = {
        drop(h);
        let mut app = EditorApp::new(None);
        app.selection = Some(Selection::rect(pixel_core::Rect::new(3, 4, 5, 5)));
        app.region_draft.name = "hand".into();
        app.region_draft.marker = true;
        harness(app)
    };
    h2.run_steps(2);
    h2.get_by_label("Region aus Auswahl").click();
    h2.run_steps(2);
    let r = &h2.state().doc.regions[0];
    assert!(r.is_marker());
    assert_eq!(r.point_at(0), Some((3, 4)));
}

// ---- follow Claude ----------------------------------------------------------

/// A document on disk (3 frames, 2 layers) and an editor showing it.
fn follow_setup(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, Document) {
    let dir = scratch_dir(tag);
    let path = dir.join("hero.pxe.json");
    let mut doc = Document::new(32, 32, Document::builtin_palette("pico8").unwrap()).unwrap();
    doc.add_frame(1, None, false).unwrap();
    doc.add_frame(2, None, false).unwrap();
    doc.add_layer("body", 1);
    io::save(&doc, &path).unwrap();
    (dir, path, doc)
}

/// What the MCP server does for an edit: save, then journal with location.
fn external_edit(path: &std::path::Path, doc: &Document, note: &str, frame: usize, layer: usize, bbox: [i32; 4]) {
    io::save(doc, path).unwrap();
    let f = pixel_core::journal::Focus { frame, layer, bbox: Some(bbox), region: None };
    pixel_core::journal::Journal::for_document(path).record_edit(doc, "claude", note, Some(f)).unwrap();
}

fn poll(h: &mut Harness<'_, EditorApp>) {
    h.state_mut().last_poll = Instant::now() - Duration::from_secs(1);
    h.run_steps(2);
}

#[test]
fn follow_jumps_flashes_and_banners() {
    let _gpu = gpu_lock();
    let (dir, path, mut doc) = follow_setup("follow");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    assert_eq!((h.state().frame, h.state().layer), (0, 1));
    assert_eq!(h.state().follow.badge(), "Folgen: an");
    // Claude draws on frame 2, layer 0 (not the visible one)
    doc.set_rows(0, 2, 20, 26, &["8888".into(), "8888".into()]).unwrap();
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&path, &doc, "Claude: fist_left gefüllt", 2, 0, [20, 26, 4, 2]);
    poll(&mut h);
    let app = h.state();
    assert_eq!((app.frame, app.layer), (2, 0), "jumped to the edit");
    assert_eq!(app.doc.get(0, 2, 21, 27), b'8');
    assert_eq!(app.history.undo_steps(), vec!["Claude: fist_left gefüllt"]);
    assert_eq!(app.follow.banner_text(), Some("Claude: fist_left gefüllt"));
    let fl = app.follow.flashes.last().expect("flash");
    assert_eq!((fl.frame, fl.points.len()), (2, 8));
    // the flash is drawn: changed pixels are tinted, not their plain color
    let img = h.render().unwrap();
    let app = h.state();
    let cell = app.canvas.screen.width() / 32.0;
    let p = app.canvas.screen.min + egui::vec2(21.5 * cell, 27.5 * cell);
    let plain = app.doc.color_of(b'8').unwrap().0;
    assert_ne!(img.get_pixel(p.x as u32, p.y as u32).0, plain, "flash overlay visible");
    img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/follow-screenshot.png")).unwrap();
    // the banner is on the canvas too
    // status-bar banner and history step
    assert!(h.query_all_by_label_contains("Claude: fist_left gefüllt").count() >= 2);
    // after ~0.6 s the flash is gone and the pixel shows its color again
    h.run_steps(60);
    let img = h.render().unwrap();
    assert_eq!(img.get_pixel(p.x as u32, p.y as u32).0, plain, "flash faded");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn follow_brings_offscreen_area_into_view() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("follow-pan");
    let path = dir.join("big.pxe.json");
    let mut doc = Document::new(256, 256, Document::builtin_palette("pico8").unwrap()).unwrap();
    io::save(&doc, &path).unwrap();
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    // zoom in so the corner is off screen
    h.state_mut().zoom = 16.0;
    h.state_mut().pan = egui::Vec2::ZERO;
    h.run_steps(2);
    doc.set(0, 0, 250, 250, b'8');
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&path, &doc, "Claude: Ecke", 0, 0, [250, 250, 1, 1]);
    poll(&mut h);
    h.run_steps(1);
    let app = h.state();
    let cell = app.canvas.screen.width() / 256.0;
    let p = app.canvas.screen.min + egui::vec2(250.5 * cell, 250.5 * cell);
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 860.0));
    assert!(area.contains(p), "edited pixel is on screen after following: {p:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn manual_input_pauses_following_and_toggle_disables_it() {
    let _gpu = gpu_lock();
    let (dir, path, mut doc) = follow_setup("follow-pause");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    // the user clicks somewhere on the canvas (eyedropper, so nothing changes)
    h.state_mut().tool = Tool::Eyedropper;
    click_px(&mut h, 1, 1);
    assert!(h.state().follow.badge().starts_with("Folgen: pausiert"), "{}", h.state().follow.badge());
    doc.set(0, 2, 5, 5, b'8');
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&path, &doc, "Claude: Punkt", 2, 0, [5, 5, 1, 1]);
    poll(&mut h);
    assert_eq!(h.state().frame, 0, "paused: the view stays");
    assert_eq!(h.state().doc.get(0, 2, 5, 5), b'8', "content is still updated");
    assert_eq!(h.state().follow.banner_text(), Some("Claude: Punkt"));
    // following switched off entirely
    h.state_mut().follow.paused_until = 0.0;
    h.state_mut().follow.enabled = false;
    h.run_steps(1);
    assert_eq!(h.state().follow.badge(), "Folgen: aus · Claude", "names who changed it last");
    doc.set(0, 1, 6, 6, b'8');
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&path, &doc, "Claude: noch ein Punkt", 1, 0, [6, 6, 1, 1]);
    poll(&mut h);
    assert_eq!(h.state().frame, 0);
    assert_eq!(h.state().doc.get(0, 1, 6, 6), b'8');
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rapid_writes_are_all_replayed() {
    let _gpu = gpu_lock();
    let (dir, path, mut doc) = follow_setup("follow-rapid");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    // five writes before the editor polls once
    for i in 0..5 {
        doc.set(0, 0, i, 0, b'8');
        external_edit(&path, &doc, &format!("Claude: Pixel {i}"), 0, 0, [i, 0, 1, 1]);
    }
    poll(&mut h);
    let steps: Vec<String> = h.state().history.undo_steps().iter().map(|s| s.to_string()).collect();
    assert_eq!(steps, (0..5).map(|i| format!("Claude: Pixel {i}")).collect::<Vec<_>>(), "every write is a step");
    assert_eq!(h.state().doc, doc);
    // and writes spread over polls
    for i in 5..8 {
        doc.set(0, 0, i, 0, b'8');
        external_edit(&path, &doc, &format!("Claude: Pixel {i}"), 0, 0, [i, 0, 1, 1]);
        poll(&mut h);
    }
    assert_eq!(h.state().history.undo_steps().len(), 8);
    assert_eq!(h.state().doc, doc);
    assert!(!h.state().dirty && !h.state().external_conflict);
    // undo walks back one write at a time
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert_eq!(h.state().doc.get(0, 0, 7, 0), TRANSPARENT);
    assert_eq!(h.state().doc.get(0, 0, 6, 0), b'8');
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn paced_progress_focus_and_race() {
    let _gpu = gpu_lock();
    let (dir, path, doc) = follow_setup("follow-progress");
    let j = pixel_core::journal::Journal::for_document(&path);
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    // focus hint: view moves, nothing changes
    let f =
        pixel_core::journal::Focus { frame: 1, layer: 0, bbox: Some([2, 2, 4, 4]), region: Some("fist_left".into()) };
    j.record_focus("Claude", "Claude: arbeitet an fist_left", f.clone()).unwrap();
    poll(&mut h);
    assert_eq!(h.state().frame, 1);
    assert_eq!(h.state().follow.banner_text(), Some("Claude: arbeitet an fist_left"));
    assert!(h.state().history.undo_steps().is_empty(), "focus is not an edit");
    // a paced edit: two progress states, then the final edit -> one undo step
    let mut d1 = doc.clone();
    d1.set_rows(0, 1, 2, 2, &["8888".into()]).unwrap();
    io::save(&d1, &path).unwrap();
    j.record_progress(&d1, "Claude", "Claude: fist_left (1/2)", Some(f.clone())).unwrap();
    poll(&mut h);
    assert_eq!(h.state().doc.get(0, 1, 2, 2), b'8', "progress is shown live");
    assert!(h.state().history.undo_steps().is_empty());
    let mut d2 = d1.clone();
    d2.set_rows(0, 1, 2, 3, &["8888".into()]).unwrap();
    external_edit(&path, &d2, "Claude: fist_left", 1, 0, [2, 2, 4, 2]);
    poll(&mut h);
    assert_eq!(h.state().history.undo_steps(), vec!["Claude: fist_left"]);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert_eq!(h.state().doc, doc, "one undo removes the whole paced edit");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z);
    h.step();
    // race: the editor sees the file before the journal entry exists
    let mut d3 = d2.clone();
    d3.set(0, 1, 9, 9, b'8');
    io::save(&d3, &path).unwrap();
    poll(&mut h);
    assert_eq!(h.state().history.undo_steps().last().copied(), Some("Extern geändert"));
    let f3 = pixel_core::journal::Focus { frame: 1, layer: 0, bbox: Some([9, 9, 1, 1]), region: None };
    j.record_edit(&d3, "claude", "Claude: Punkt 9,9", Some(f3)).unwrap();
    poll(&mut h);
    assert_eq!(h.state().history.undo_steps().last().copied(), Some("Claude: Punkt 9,9"), "step gets its note");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn local_edits_still_get_the_conflict_bar() {
    let _gpu = gpu_lock();
    let (dir, path, mut doc) = follow_setup("follow-conflict");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    h.state_mut().edit("lokal", |d| d.set(0, 0, 0, 0, b'1'));
    doc.set(0, 2, 5, 5, b'8');
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&path, &doc, "Claude: Punkt", 2, 0, [5, 5, 1, 1]);
    poll(&mut h);
    assert!(h.state().external_conflict);
    assert_eq!(h.state().frame, 0);
    assert_eq!(h.state().doc.get(0, 0, 0, 0), b'1', "local edit kept");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// End to end: the real MCP tool draws a paced edit in another thread while
/// the editor polls; the user sees it grow row by row, gets the final state
/// and one undo step.
#[test]
fn live_paced_mcp_write_is_followed() {
    let _gpu = gpu_lock();
    let (dir, path, _) = follow_setup("follow-e2e");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    let workdir = dir.clone();
    let writer = std::thread::spawn(move || {
        crate::mcp::test_call(
            &workdir,
            "set_rows",
            serde_json::json!({"path": "hero", "layer": 0, "frame": 2, "x": 4, "y": 4,
                "rows": ["8888", "8888", "8888", "8888", "8888", "8888"], "pace_ms": 120,
                "note": "Claude: Block gemalt"}),
        )
    });
    let mut seen_rows = std::collections::BTreeSet::new();
    let started = Instant::now();
    while !writer.is_finished() && started.elapsed() < Duration::from_secs(10) {
        poll(&mut h);
        let rows = (4..10).filter(|y| h.state().doc.get(0, 2, 4, *y) == b'8').count();
        seen_rows.insert(rows);
        std::thread::sleep(Duration::from_millis(40));
    }
    let (ok, text) = writer.join().unwrap();
    assert!(ok, "{text}");
    poll(&mut h);
    let app = h.state();
    assert!(seen_rows.iter().any(|n| *n > 0 && *n < 6), "intermediate strokes were visible: {seen_rows:?}");
    assert_eq!((4..10).filter(|y| app.doc.get(0, 2, 4, *y) == b'8').count(), 6);
    assert_eq!(app.frame, 2, "followed to the edited frame");
    assert_eq!(app.history.undo_steps(), vec!["Claude: Block gemalt"], "one undo step for the paced write");
    assert!(!app.dirty && !app.external_conflict);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- shared view for Claude ---------------------------------------------------

/// Write the view on the next frame regardless of the 200 ms throttle.
fn flush_view(h: &mut Harness<'_, EditorApp>) {
    h.state_mut().share.last_write = None;
    h.step();
}

fn read_view(path: &std::path::Path) -> crate::view::View {
    let f = crate::view::view_file(path, std::process::id());
    serde_json::from_str(&std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()))).unwrap()
}

#[test]
fn shared_view_and_claude_zeigen() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("share");
    let path = dir.join("walk.pxe.json");
    let mut doc = Document::new(16, 16, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    doc.palette[9].name = "harness".into();
    doc.add_frame(1, None, false).unwrap();
    doc.set_tag(pixel_core::Tag::new("walk_side", 0, 1)).unwrap();
    doc.rename_layer(0, "body");
    doc.set_rows(0, 1, 4, 4, &["99".into(), "9c".into()]).unwrap();
    doc.regions.push(pixel_core::Region::new("face", pixel_core::Rect::new(4, 4, 3, 3)));
    io::save(&doc, &path).unwrap();

    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    let v = read_view(&path);
    assert_eq!(v.pid, std::process::id());
    assert_eq!(v.path, std::path::absolute(&path).unwrap());
    assert_eq!((v.frame.index, v.layer.name.as_str(), v.selection), (0, "body", None));

    // frame 1, select x 4..5 y 4..5 with the selection tool, hover (5, 5)
    h.key_press(egui::Key::ArrowRight);
    h.step();
    h.key_press(egui::Key::M);
    h.step();
    drag_px(&mut h, (4, 4), (5, 5));
    assert_eq!(h.state().selection, Some(Selection::rect(pixel_core::Rect::new(4, 4, 2, 2))));
    h.hover_at(px_center(&h, 5, 5));
    h.step();
    flush_view(&mut h);
    let v = read_view(&path);
    assert_eq!((v.frame.index, v.frame.name.as_str(), v.frame.address.as_str()), (1, "f1", "walk_side/1"));
    assert_eq!((v.layer.index, v.layer.name.as_str()), (0, "body"));
    assert_eq!(v.selection, Some(crate::view::Area { x: 4, y: 4, w: 2, h: 2 }));
    let hover = v.hover.expect("hovered pixel");
    assert_eq!((hover.x, hover.y, hover.key.as_str(), hover.color.as_str()), (5, 5, "c", "c"));
    assert_eq!(hover.regions, vec!["face"]);
    assert_eq!(v.zoom, h.state().zoom);
    assert!(v.following && !v.dirty);
    assert_eq!(v.tool, "Auswahl");
    assert!(crate::view::now_ms() - v.time_ms < 10_000);

    // writes are throttled: a change right after a write waits
    h.state_mut().share.last_write = Some(Instant::now());
    h.state_mut().layer = 0;
    h.state_mut().zoom += 1.0;
    h.step();
    assert_eq!(read_view(&path).zoom, v.zoom, "throttled");
    h.state_mut().share.last_write = Some(Instant::now() - Duration::from_millis(250));
    h.step();
    assert_eq!(read_view(&path).zoom, h.state().zoom, "written after the throttle interval");

    // Ctrl+Shift+K marks the selection for Claude
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::K);
    h.step();
    let status = h.state().status.clone();
    assert_eq!(status, "KI-Markierung #1: Auswahl x 4..5 y 4..5 (2x2) · walk_side/1 · body");
    let marks = crate::view::read_marks(&path);
    let p = &marks.marks[0];
    assert_eq!(p.rect, Some(crate::view::Area { x: 4, y: 4, w: 2, h: 2 }));
    assert_eq!((p.frame.address.as_str(), p.layer.name.as_str(), p.pid), ("walk_side/1", "body", std::process::id()));
    assert_eq!(h.state().marks, marks);

    // what Claude gets from the MCP tool
    let (ok, text) = crate::mcp::test_call(&dir, "get_user_view", serde_json::json!({}));
    assert!(ok, "{text}");
    assert!(text.contains("- walk.pxe.json · editor pid"), "{text}");
    assert!(text.contains("selection: x 4..5 y 4..5 (2x2)"), "{text}");
    assert!(text.contains("\"KI zeigen\" marks in walk.pxe.json: 1"), "{text}");
    assert!(text.contains("4 99\n") && text.contains("5 9c"), "selected rows: {text}");

    // the status bar button without a selection points at the whole frame
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::D);
    h.step();
    h.get_by_label("KI zeigen").click();
    h.run_steps(2);
    assert!(h.state().status.starts_with("KI-Markierung #2: ganzer Frame · walk_side/1"), "{}", h.state().status);
    let marks = crate::view::read_marks(&path);
    assert_eq!(marks.marks.len(), 2, "added, not replaced");
    assert_eq!(marks.marks[1].rect, None);
    assert_eq!(read_view(&path).selection, None, "view published right after pointing");

    // closing removes the view file; the marks stay
    eframe::App::on_exit(h.state_mut());
    assert!(!crate::view::view_file(&path, std::process::id()).exists());
    assert!(crate::view::marks_file(&path).exists());
    drop(h);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- clean view and mini preview ----------------------------------------------

/// An opaque 8x6 two-colour document with a 3-frame tag `walk` (100/300/100 ms):
/// frame 1 is all '8', frame 2 all '9'.
fn opaque_doc() -> Document {
    let mut doc = Document::new(8, 6, Document::builtin_palette("pico8").unwrap()).unwrap();
    for y in 0..6 {
        for x in 0..8 {
            doc.set(0, 0, x, y, if (x + y) % 2 == 0 { b'8' } else { b'c' });
        }
    }
    doc.add_frame(1, None, false).unwrap();
    doc.add_frame(2, None, false).unwrap();
    for (f, k) in [(1, b'8'), (2, b'9')] {
        for y in 0..6 {
            for x in 0..8 {
                doc.set(0, f, x, y, k);
            }
        }
    }
    doc.frames[0].duration_ms = 100;
    doc.frames[1].duration_ms = 300;
    doc.frames[2].duration_ms = 100;
    doc.set_tag(pixel_core::Tag::new("walk", 0, 2)).unwrap();
    doc
}

/// Pixels of the canvas area (outside the preview) that are neither the
/// canvas background nor the exact art colour beneath them. kittest draws
/// its simulated mouse cursor into the image; that sprite is skipped.
fn overlay_pixels(h: &mut Harness<'_, EditorApp>) -> usize {
    let mouse = h.ctx.input(|i| i.pointer.latest_pos());
    let img = h.render().unwrap();
    let app = h.state();
    let (area, canvas) = (app.canvas.area.shrink(2.0), app.canvas.screen);
    let cell = canvas.width() / app.doc.width as f32;
    let mut bad = 0;
    for y in area.min.y as u32..area.max.y as u32 {
        for x in area.min.x as u32..area.max.x as u32 {
            let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            if mouse.is_some_and(|m| (m.x - 2.0..m.x + 22.0).contains(&p.x) && (m.y - 2.0..m.y + 22.0).contains(&p.y)) {
                continue;
            }
            let want = if canvas.contains(p) {
                let (ax, ay) = (((p.x - canvas.min.x) / cell) as i32, ((p.y - canvas.min.y) / cell) as i32);
                app.doc.color_of(app.doc.top_key(app.frame, ax, ay)).unwrap().0
            } else {
                [24, 24, 24, 255]
            };
            if img.get_pixel(x, y).0 != want {
                bad += 1;
            }
        }
    }
    bad
}

#[test]
fn clean_view_hides_overlays_and_restores_settings() {
    let _gpu = gpu_lock();
    let mut app = EditorApp::new(None);
    app.replace_doc(opaque_doc());
    app.preview.settings.open = false;
    let mut face = pixel_core::Region::new("face", pixel_core::Rect::new(1, 1, 3, 2));
    face.locked = true;
    app.doc.regions.push(face);
    let mut origin = pixel_core::Region::new("origin", pixel_core::Rect::new(4, 5, 1, 1));
    origin.kind = pixel_core::document::RegionKind::Marker;
    app.doc.regions.push(origin);
    app.selection = Some(Selection::rect(pixel_core::Rect::new(2, 2, 3, 3)));
    app.onion = true;
    let mut h = harness(app);
    h.run_steps(3);
    // a follow flash and banner, and the cursor over the canvas
    let now = h.state().follow.now;
    h.state_mut().follow.flashes.push(super::follow::Flash {
        frame: 0,
        points: vec![(0, 0)],
        bbox: Some(pixel_core::Rect::new(0, 0, 2, 2)),
        start: now,
    });
    h.state_mut().follow.banner = Some(("Claude: arbeitet an face".into(), now));
    h.hover_at(px_center(&h, 6, 3));
    h.step();
    assert!(overlay_pixels(&mut h) > 1000, "grid, regions, marker, selection, flash, banner, hover, border visible");

    // H: everything off at once; the individual toggles keep their values
    h.key_press(egui::Key::H);
    h.step();
    assert!(h.state().clean_view);
    // flash and banner live right now
    let now = h.state().follow.now;
    h.state_mut().follow.flashes[0].start = now;
    h.state_mut().follow.banner = Some(("Claude: arbeitet an face".into(), now));
    h.step();
    assert!(h.state().follow.banner_text().is_some() && !h.state().follow.flashes.is_empty());
    assert_eq!(overlay_pixels(&mut h), 0, "clean view shows only the art");
    let a = h.state();
    assert!(a.grid && a.show_regions && a.onion && a.follow.enabled && a.selection.is_some());
    assert!(h.query_by_label("Saubere Ansicht: an").is_some(), "status bar badge");

    // a toggle changed by the user before stays as set
    h.key_press(egui::Key::H);
    h.step();
    h.state_mut().grid = false;
    h.key_press(egui::Key::H);
    h.step();
    h.get_by_label("Saubere Ansicht: an").click(); // the status bar button turns it off
    h.run_steps(2);
    let a = h.state();
    assert!(!a.clean_view);
    assert!(!a.grid, "grid stays off as the user set it");
    assert!(a.show_regions && a.onion && a.selection == Some(Selection::rect(pixel_core::Rect::new(2, 2, 3, 3))));
    assert!(h.query_by_label("Saubere Ansicht").is_some());
    assert!(overlay_pixels(&mut h) > 0, "regions and selection are back");
}

/// Check the preview shows `frame` exactly: every art pixel is a
/// `scale` x `scale` block of its colour (transparent = `bg`).
fn assert_preview_exact(h: &mut Harness<'_, EditorApp>, ppp: f32, frame: usize, bg: [u8; 4]) {
    let img = h.render().unwrap();
    let app = h.state();
    let r = app.preview.image_rect.expect("preview painted");
    let scale = app.preview.settings.scale;
    let (ox, oy) = ((r.min.x * ppp).round() as u32, (r.min.y * ppp).round() as u32);
    assert!((r.min.x * ppp - ox as f32).abs() < 0.01, "on the physical pixel grid");
    assert_eq!(((r.width() * ppp).round() as u32), app.doc.width * scale, "integer scale");
    for y in 0..app.doc.height {
        for x in 0..app.doc.width {
            let k = app.doc.top_key(frame, x as i32, y as i32);
            let want = app.doc.color_of(k).map_or(bg, |c| c.0);
            for d in 0..scale * scale {
                let got = img.get_pixel(ox + x * scale + d % scale, oy + y * scale + d / scale).0;
                assert_eq!(got, want, "ppp {ppp} scale {scale}: frame {frame} pixel ({x},{y})");
            }
        }
    }
}

#[test]
fn preview_is_pixel_exact_live_and_plays_the_tag() {
    let _gpu = gpu_lock();
    for ppp in [1.0, 1.5] {
        let mut app = EditorApp::new(None);
        let mut doc = opaque_doc();
        doc.set(0, 0, 0, 0, TRANSPARENT);
        app.replace_doc(doc);
        // overlays on in the main canvas must not reach the preview
        app.selection = Some(Selection::rect(pixel_core::Rect::new(0, 0, 8, 6)));
        app.doc.regions.push(pixel_core::Region::new("all", pixel_core::Rect::new(0, 0, 8, 6)));
        app.preview.settings.background = super::settings::PreviewBg::Dark;
        let mut h = harness_ppp(app, ppp);
        h.run_steps(3);
        let r = h.state().preview.image_rect.unwrap();
        let area = h.state().canvas.area;
        assert!(r.max.x > area.center().x && r.min.y < area.center().y, "top-right corner of the canvas: {r:?}");
        let dark = super::settings::PreviewBg::Dark.color();
        assert_preview_exact(&mut h, ppp, 0, dark);
        h.get_by_label("3×").click();
        h.run_steps(2);
        assert_eq!(h.state().preview.settings.scale, 3);
        assert_preview_exact(&mut h, ppp, 0, dark);

        // live: a change of the document (e.g. Claude's edit) shows at once
        h.state_mut().edit("test", |d| d.set(0, 0, 0, 0, b'2'));
        h.step();
        assert_preview_exact(&mut h, ppp, 0, dark);
    }

    // playing loops the tag in real time (kittest's default 0.25 s step
    // would alias against the 500 ms loop)
    let mut h = Harness::builder()
        .with_size([1280.0, 860.0])
        .with_step_dt(0.04)
        .wgpu()
        .build_eframe(|_cc| EditorApp::new(None));
    h.state_mut().replace_doc(opaque_doc());
    h.run_steps(2);
    h.get_by_label("Tag abspielen").click();
    h.run_steps(1);
    assert!(h.state().preview.playing);
    let mut seen = std::collections::BTreeSet::new();
    let started = Instant::now();
    while seen.len() < 3 && started.elapsed() < Duration::from_secs(10) {
        h.step();
        seen.insert(h.state().preview.shown);
    }
    assert_eq!(seen.len(), 3, "all frames of the tag were shown: {seen:?}");
    assert_eq!(h.state().frame, 0, "the editor's own frame does not move");
    let shown = h.state().preview.shown;
    assert_preview_exact(&mut h, 1.0, shown, super::settings::PreviewBg::Checker.color());
}

#[test]
fn preview_timing_and_shortcut_and_settings() {
    // timing: durations 100/300/100 ms, looping
    let mut app = EditorApp::new(None);
    app.replace_doc(opaque_doc());
    let ctx = egui::Context::default();
    app.toggle_preview_play(0.0);
    let at = |app: &mut EditorApp, t: f64| app.preview_frame(&ctx, t);
    assert_eq!(at(&mut app, 0.05), 0);
    assert_eq!(at(&mut app, 0.15), 1);
    assert_eq!(at(&mut app, 0.39), 1, "frame 1 lasts 300 ms");
    assert_eq!(at(&mut app, 0.41), 2);
    assert_eq!(at(&mut app, 0.51), 0, "loops");
    assert_eq!(app.frame, 0);

    // P toggles the preview; settings are saved and loaded again
    let _gpu = gpu_lock();
    let dir = scratch_dir("settings");
    let file = dir.join("settings.json");
    let mut app = EditorApp::new(None);
    app.use_settings_file(file.clone());
    let mut h = harness(app);
    h.run_steps(2);
    assert!(h.state().preview.settings.open, "open by default");
    h.get_by_label("1×").click();
    h.run_steps(2);
    // drag the window by its title: the new position is remembered
    let title = h.get_by_label("Vorschau").rect().min + egui::vec2(70.0, 10.0); // title bar
    let before = h.state().preview.image_rect.unwrap().min;
    h.hover_at(title);
    h.step();
    press(&h, title, true);
    h.step();
    for i in 1..=10 {
        h.hover_at(title + egui::vec2(-15.0, 6.0) * i as f32);
        h.step();
    }
    press(&h, title + egui::vec2(-150.0, 60.0), false);
    h.run_steps(2);
    let moved = h.state().preview.image_rect.unwrap().min;
    assert!((moved - before - egui::vec2(-150.0, 60.0)).length() < 2.0, "{before:?} -> {moved:?}");
    let pos = h.state().preview.settings.pos.expect("position remembered");
    assert_eq!(super::settings::Settings::load(&file).preview.pos, Some(pos));
    h.key_press(egui::Key::P);
    h.run_steps(2);
    assert!(!h.state().preview.settings.open);
    let saved = super::settings::Settings::load(&file);
    assert_eq!((saved.preview.open, saved.preview.scale), (false, 1));
    let mut again = EditorApp::new(None);
    again.use_settings_file(file.clone());
    assert_eq!(again.preview.settings, saved.preview);
    h.key_press(egui::Key::P);
    h.run_steps(2);
    assert!(super::settings::Settings::load(&file).preview.open);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Edits from another AI client (Codex) are named as such in the banner,
/// the history panel and the follow badge.
#[test]
fn follow_names_the_author() {
    let _gpu = gpu_lock();
    let (dir, path, mut doc) = follow_setup("author");
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    doc.set(0, 1, 2, 2, b'8');
    io::save(&doc, &path).unwrap();
    let f = pixel_core::journal::Focus { frame: 1, layer: 0, bbox: Some([2, 2, 1, 1]), region: None };
    let j = pixel_core::journal::Journal::for_document(&path);
    j.record_edit(&doc, "Codex", "Augen nachgezogen", Some(f.clone())).unwrap();
    poll(&mut h);
    let app = h.state();
    assert_eq!(app.follow.banner_text(), Some("Codex: Augen nachgezogen"));
    assert_eq!(app.history.undo_steps(), vec!["Codex: Augen nachgezogen"]);
    assert_eq!(app.follow.badge(), "Folgen: an · Codex");
    assert!(h.query_by_label("Folgen: an · Codex").is_some(), "status bar badge");
    // a note that already names its author is not prefixed twice; older
    // journals wrote "claude"
    doc.set(0, 1, 3, 3, b'8');
    io::save(&doc, &path).unwrap();
    j.record_edit(&doc, "claude", "Claude: Nase", Some(f)).unwrap();
    poll(&mut h);
    assert_eq!(h.state().follow.banner_text(), Some("Claude: Nase"));
    assert_eq!(h.state().follow.badge(), "Folgen: an · Claude");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The right panel stands still while an animation plays, but follows a
/// manual frame change.
#[test]
fn right_panel_stands_still_during_playback() {
    let _gpu = gpu_lock();
    // 4 frames in a tag; frame 1 is linked to frame 0, frame 3 to frame 2
    // (different panel rows before the fix), durations and a region that
    // differ per frame
    let mut doc = Document::new(16, 16, Document::builtin_palette("pico8").unwrap()).unwrap();
    doc.set(0, 0, 1, 1, b'8');
    doc.add_frame(0, Some(0), true).unwrap();
    doc.add_frame(2, None, false).unwrap();
    doc.add_frame(2, Some(2), true).unwrap();
    for (f, ms) in [(0, 100), (1, 150), (2, 200), (3, 250)] {
        doc.frames[f].duration_ms = ms;
    }
    doc.set_tag(pixel_core::Tag::new("walk", 0, 3)).unwrap();
    let mut face = pixel_core::Region::new("face", pixel_core::Rect::new(1, 1, 2, 2));
    face.set_rect_for(2, 3, Some(pixel_core::Rect::new(5, 5, 3, 3)), 4);
    doc.regions.push(face);
    let mut app = EditorApp::new(None);
    app.replace_doc(doc);
    app.preview.settings.open = false;
    let mut h = Harness::builder().with_size([1280.0, 860.0]).with_step_dt(0.03).wgpu().build_eframe(move |_cc| app);
    h.run_steps(3);

    // the panel as the user sees it: headings and their positions, the
    // frame-dependent texts, and the canvas width (= panel width)
    let snapshot = |h: &Harness<'_, EditorApp>| {
        let at = |l: &str| h.query_by_label(l).map(|n| n.rect().min);
        let texts: Vec<bool> = ["Frame #1: walk/1", "Cel verknüpft mit Frames [0, 1]", "face · x 1..2 y 1..2 (2x2)"]
            .iter()
            .map(|t| h.query_by_label(t).is_some())
            .collect();
        (at("Tag (Animation)"), at("Regionen"), at("Verlauf"), texts, h.state().canvas.area.max.x)
    };
    h.key_press(egui::Key::ArrowRight); // manual: frame 1
    h.run_steps(2);
    assert_eq!(h.state().frame, 1);
    let before = snapshot(&h);
    assert_eq!(before.3, vec![true, true, true], "panel shows frame 1");

    h.get_by_label("Abspielen").click();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..60 {
        h.step();
        assert!(h.state().playing);
        seen.insert(h.state().frame);
        assert_eq!(snapshot(&h), before, "panel changed while playing frame {}", h.state().frame);
    }
    assert_eq!(seen.len(), 4, "all frames were played: {seen:?}");

    // a manual frame change switches the panel; its layout stays where it was
    h.state_mut().goto_frame(2);
    h.run_steps(2);
    assert!(!h.state().playing);
    assert!(h.query_by_label("Frame #2: walk/2").is_some());
    assert!(h.query_by_label("face · x 5..7 y 5..7 (3x3)").is_some());
    let now = snapshot(&h);
    assert_eq!((now.0, now.1, now.2, now.4), (before.0, before.1, before.2, before.4), "no shift between frames");

    // after playback stops, the panel shows the frame that is shown
    h.get_by_label("Abspielen").click();
    h.run_steps(7);
    h.key_press(egui::Key::Enter); // stop
    h.run_steps(2);
    assert!(!h.state().playing);
    let f = h.state().frame;
    assert!(h.query_by_label(&format!("Frame #{f}: walk/{f}")).is_some(), "panel follows the stopped frame {f}");
}

// ---- tabs -------------------------------------------------------------------

/// A workdir `art/` with two documents in `hero/` and an editor on the first.
fn tabs_setup(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, Document) {
    let dir = scratch_dir(tag);
    std::fs::create_dir_all(dir.join("art/hero")).unwrap();
    let art = dir.join("art").canonicalize().unwrap();
    let mut doc = Document::new(16, 16, Document::builtin_palette("pico8").unwrap()).unwrap();
    doc.add_frame(0, None, false).unwrap();
    doc.add_frame(0, None, false).unwrap();
    doc.add_layer("body", 1);
    let (walk, idle) = (art.join("hero/walk.pxe.json"), art.join("hero/idle.pxe.json"));
    io::save(&doc, &walk).unwrap();
    io::save(&doc, &idle).unwrap();
    (dir, walk, idle, doc)
}

fn tabs_app(walk: &std::path::Path) -> EditorApp {
    let mut app = EditorApp::new(Some(walk.to_path_buf()));
    app.workdir = walk.parent().and_then(|p| p.parent()).map(std::path::Path::to_path_buf);
    app.preview.settings.open = false;
    app
}

#[test]
fn tabs_open_switch_and_close() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, _) = tabs_setup("tabs");
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    assert!(h.query_by_label("hero/walk").is_some(), "one tab, named relative to the workdir");
    assert_eq!(h.state_mut().open_tab(&idle), Ok("opened"));
    h.run_steps(2);
    let app = h.state();
    assert_eq!((app.tabs.len(), app.active), (2, 1));
    assert_eq!(app.path.as_deref(), Some(idle.as_path()));
    assert!(h.query_by_label("hero/walk").is_some() && h.query_by_label("hero/idle").is_some());
    assert_eq!(h.query_all_by_label("×").count(), 2, "a close button per tab");
    // an open file is not opened twice
    assert_eq!(h.state_mut().open_tab(&walk), Ok("switched"));
    assert_eq!(h.state_mut().open_tab(&walk), Ok("shown"));
    assert_eq!(h.state().tabs.len(), 2);

    // Ctrl+Tab / Ctrl+Shift+Tab cycle, a click selects
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(h.state().active, 1);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(h.state().active, 0, "wraps around");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(h.state().active, 1);
    h.get_by_label("hero/walk").click();
    h.run_steps(1);
    assert_eq!((h.state().active, h.state().path.as_deref()), (0, Some(walk.as_path())));

    // unsaved changes: the tab says so, and Ctrl+W asks
    h.state_mut().edit("Punkt", |d| d.set(0, 0, 1, 1, b'8'));
    h.run_steps(1);
    assert!(h.query_by_label("• hero/walk").is_some(), "unsaved marker");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
    h.run_steps(2);
    assert!(h.query_by_label("hero/walk hat ungespeicherte Änderungen.").is_some());
    h.get_by_label("Abbrechen").click();
    h.run_steps(2);
    assert_eq!(h.state().tabs.len(), 2, "cancelled");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
    h.run_steps(2);
    h.get_by_label("Verwerfen").click();
    h.run_steps(2);
    let app = h.state();
    assert_eq!((app.tabs.len(), app.active), (1, 0));
    assert_eq!(app.path.as_deref(), Some(idle.as_path()), "the next tab is shown");
    assert_eq!(io::load(&walk).unwrap().get(0, 0, 1, 1), TRANSPARENT, "discarded, not saved");

    // closing the last tab leaves an empty document; opening reuses it
    h.state_mut().request_close_tab(0);
    h.run_steps(1);
    assert_eq!((h.state().tabs.len(), h.state().path.as_deref()), (1, None));
    assert_eq!(h.state_mut().open_tab(&walk), Ok("opened"));
    assert_eq!(h.state().tabs.len(), 1, "the untouched empty tab was reused");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tabs_keep_their_own_state() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, _) = tabs_setup("tabs-state");
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    // tab 0: frame 2, layer 0, a selection, zoom, an undo step, playing
    {
        let app = h.state_mut();
        app.goto_frame(2);
        app.layer = 0;
        app.selection = Some(Selection::rect(pixel_core::Rect::new(1, 1, 3, 3)));
        app.edit("Punkt", |d| d.set(0, 2, 2, 2, b'8'));
        app.color = b'9';
    }
    h.run_steps(2);
    h.state_mut().zoom = 3.0;
    h.state_mut().follow.banner = Some(("Claude: arbeitet an walk".into(), h.state().follow.now));
    h.get_by_label("Abspielen").click();
    h.run_steps(2);
    assert!(h.state().playing);
    let panel_frame = h.state().panel_frame;

    h.state_mut().open_tab(&idle).unwrap();
    h.run_steps(2);
    let app = h.state();
    assert_eq!((app.frame, app.layer, app.selection.clone(), app.playing), (0, 1, None, false), "fresh state");
    assert!(app.history.undo_steps().is_empty() && !app.dirty);
    assert_eq!(app.follow.banner_text(), None, "the banner belongs to the other document");
    assert_ne!(app.zoom, 3.0);
    assert_eq!(app.panel_frame, 0, "the right panel shows this document's frame");
    // window-wide: follow mode and the tool
    h.state_mut().follow.enabled = false;
    h.state_mut().tool = Tool::Fill;

    h.state_mut().switch_tab(0);
    h.run_steps(1);
    let app = h.state();
    assert_eq!((app.layer, app.selection.clone()), (0, Some(Selection::rect(pixel_core::Rect::new(1, 1, 3, 3)))));
    assert_eq!((app.zoom, app.color, app.panel_frame), (3.0, b'9', panel_frame));
    assert!(app.playing, "still playing");
    assert_eq!(app.history.undo_steps(), vec!["Punkt"]);
    assert!(app.dirty && app.tab_dirty(0) && !app.tab_dirty(1));
    assert_eq!(app.follow.banner_text(), Some("Claude: arbeitet an walk"));
    assert!(!app.follow.enabled, "follow mode is window-wide");
    assert_eq!(app.tool, Tool::Fill);
    assert_eq!(app.doc.get(0, 2, 2, 2), b'8');
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn background_change_marks_the_tab_without_switching() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, mut doc) = tabs_setup("tabs-bg");
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    h.state_mut().open_tab(&idle).unwrap();
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    // Claude edits idle (in the background), frame 2
    doc.set_rows(0, 2, 4, 4, &["88".into()]).unwrap();
    std::thread::sleep(Duration::from_millis(15));
    external_edit(&idle, &doc, "Claude: Arm", 2, 0, [4, 4, 2, 1]);
    poll(&mut h);
    let app = h.state();
    assert_eq!(app.active, 0, "a background edit does not take the window");
    assert_eq!(app.path.as_deref(), Some(walk.as_path()));
    assert_eq!(app.doc.get(0, 2, 4, 4), TRANSPARENT, "the shown document is untouched");
    assert!(app.tabs[1].marked && !app.tabs[0].marked);
    assert!(app.tabs[1].note.as_deref().is_some_and(|n| n.contains("Claude: Arm")), "{:?}", app.tabs[1].note);
    assert_eq!(app.follow.banner_text(), None, "no banner for another tab");
    // the dot is painted next to the tab name
    let tab = h.get_by_label("hero/idle").rect();
    let img = h.render().unwrap();
    let dot = (tab.right() as u32..tab.right() as u32 + 16)
        .any(|x| img.get_pixel(x, tab.center().y as u32).0[..3] == super::tabs::MARK.to_array()[..3]);
    assert!(dot, "marker dot after the name");
    img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tabs-screenshot.png")).unwrap();

    // viewing it clears the mark; it was reloaded and followed meanwhile
    h.get_by_label("hero/idle").click();
    h.run_steps(2);
    let app = h.state();
    assert!(!app.tabs[1].marked);
    assert_eq!(app.doc.get(0, 2, 4, 4), b'8');
    assert_eq!((app.frame, app.layer), (2, 0), "at the edit");
    assert_eq!(app.history.undo_steps(), vec!["Claude: Arm"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn focus_switches_to_its_tab() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, _) = tabs_setup("tabs-focus");
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    h.state_mut().open_tab(&idle).unwrap();
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    let focus = |frame| pixel_core::journal::Focus { frame, layer: 0, bbox: Some([2, 2, 3, 3]), region: None };
    let journal = pixel_core::journal::Journal::for_document(&idle);
    journal.record_focus("claude", "Claude: arbeitet an x 2..4 y 2..4", focus(1)).unwrap();
    poll(&mut h);
    let app = h.state();
    assert_eq!(app.active, 1, "focus switches to the document's tab");
    assert_eq!((app.frame, app.layer), (1, 0));
    assert!(!app.tabs[1].marked, "it is being viewed");
    assert_eq!(app.follow.banner_text(), Some("Claude: arbeitet an x 2..4 y 2..4"));
    assert!(h.query_by_label("Claude: arbeitet an x 2..4 y 2..4").is_some(), "banner shown");

    // while the user works (paused) a focus only marks the tab
    h.state_mut().switch_tab(0);
    h.state_mut().follow.paused_until = h.state().follow.now + 5.0;
    pixel_core::journal::Journal::for_document(&idle)
        .record_focus("claude", "Claude: arbeitet an Frame 2", focus(2))
        .unwrap();
    poll(&mut h);
    assert_eq!(h.state().active, 0);
    assert!(h.state().tabs[1].marked);
    // and with following off as well
    h.state_mut().follow.paused_until = 0.0;
    h.state_mut().follow.enabled = false;
    journal.record_focus("claude", "Claude: noch mal", focus(0)).unwrap();
    poll(&mut h);
    assert_eq!(h.state().active, 0);

    // a document that is not open in a tab is not opened by focus
    h.state_mut().follow.enabled = true;
    let other = walk.parent().unwrap().join("run.pxe.json");
    io::save(&io::load(&walk).unwrap(), &other).unwrap();
    pixel_core::journal::Journal::for_document(&other).record_focus("claude", "Claude: run", focus(0)).unwrap();
    poll(&mut h);
    assert_eq!((h.state().tabs.len(), h.state().active), (2, 0));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pixel-editor open` / MCP `open_editor` while a window for the workdir
/// runs: the file becomes a tab there (through the real socket).
#[test]
fn open_from_another_process_adds_a_tab() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, _) = tabs_setup("tabs-ipc");
    let socks = dir.join("ipc");
    let art = walk.parent().unwrap().parent().unwrap().to_path_buf();
    let mut app = tabs_app(&walk);
    app.ipc = Some(crate::ipc::Server::listen(&socks, &art).unwrap());
    let mut h = harness(app);
    h.run_steps(2);
    let open = |h: &mut Harness<'_, EditorApp>, file: &std::path::Path, dry_run: bool| {
        let (file, socks) = (file.to_path_buf(), socks.clone());
        let t = std::thread::spawn(move || crate::launch::open_editor_with(&file, dry_run, None, &socks));
        let start = Instant::now();
        while !t.is_finished() && start.elapsed() < Duration::from_secs(10) {
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        t.join().unwrap()
    };
    let pid = std::process::id();
    let t = open(&mut h, &idle, true).unwrap();
    assert_eq!(
        t,
        format!("would open {} as tab in the running editor (pid {pid}); it follows edits live", idle.display())
    );
    assert_eq!(h.state().tabs.len(), 1, "dry run");
    let t = open(&mut h, &idle, false).unwrap();
    assert!(t.starts_with(&format!("opened {} as tab in the running editor (pid {pid})", idle.display())), "{t}");
    h.run_steps(1);
    assert_eq!((h.state().tabs.len(), h.state().active), (2, 1));
    assert!(h.query_by_label("hero/idle").is_some());
    let t = open(&mut h, &walk, false).unwrap();
    assert!(t.starts_with("switched to the tab of"), "{t}");
    assert_eq!((h.state().tabs.len(), h.state().active), (2, 0));
    let missing = open(&mut h, &walk.with_file_name("nope.pxe.json"), false);
    assert!(missing.is_err(), "{missing:?}");
    drop(h);
    assert!(!crate::ipc::socket_path(&socks, &art).exists(), "socket removed with the window");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn restores_tabs_in_saved_order() {
    let (dir, walk, idle, doc) = tabs_setup("tabs-restore");
    let run = walk.with_file_name("run.pxe.json");
    io::save(&doc, &run).unwrap();
    let gone = walk.with_file_name("deleted.pxe.json");
    // last time: idle, walk, run, deleted; now started for walk
    let mut app = tabs_app(&walk);
    app.restore_tabs(&[idle.clone(), walk.clone(), run.clone(), gone]);
    assert_eq!(app.tab_paths(), vec![idle.clone(), walk.clone(), run.clone()]);
    assert_eq!((app.active, app.path.as_deref()), (1, Some(walk.as_path())), "the requested file stays active");
    let labels: Vec<String> = (0..3).map(|i| app.tab_label(i)).collect();
    assert_eq!(labels, ["hero/idle", "hero/walk", "hero/run"]);
    // and they are saved per workdir
    let s = app.settings();
    assert_eq!(s.open_tabs.get(&app.workdir.as_ref().unwrap().display().to_string()), Some(&app.tab_paths()));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- pixel-shaped selections ------------------------------------------------

/// Run `f` with modifier keys held (pointer events read them from the input).
fn with_mods(h: &mut Harness<'_, EditorApp>, m: Modifiers, f: impl FnOnce(&mut Harness<'_, EditorApp>)) {
    h.event(egui::Event::ModifiersChanged(m));
    h.step();
    f(h);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn right_click_px(h: &mut Harness<'_, EditorApp>, x: i32, y: i32) {
    let p = px_center(h, x, y);
    h.hover_at(p);
    h.step();
    press_btn(h, p, PointerButton::Secondary, true);
    h.step();
    press_btn(h, p, PointerButton::Secondary, false);
    h.run_steps(3); // the menu fades in
}

/// Drag through several pixels (a lasso outline).
fn path_px(h: &mut Harness<'_, EditorApp>, pts: &[(i32, i32)]) {
    let first = px_center(h, pts[0].0, pts[0].1);
    h.hover_at(first);
    h.step();
    press(h, first, true);
    h.step();
    for p in &pts[1..] {
        let sp = px_center(h, p.0, p.1);
        h.hover_at(sp);
        h.step();
    }
    let last = px_center(h, pts[pts.len() - 1].0, pts[pts.len() - 1].1);
    press(h, last, false);
    h.step();
}

fn sel_points(h: &Harness<'_, EditorApp>) -> Vec<(i32, i32)> {
    h.state().selection.as_ref().map_or(Vec::new(), |s| s.points().collect())
}

fn select_app() -> Harness<'static, EditorApp> {
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    // an L of '8' with a 'c' in its box, a separate 'c', and a far '8'
    let d = &mut h.state_mut().doc;
    d.set_rows(0, 0, 2, 2, &["8c".into(), "88".into(), "...".into(), "..c".into()]).unwrap();
    d.set(0, 0, 10, 10, b'8');
    h.step();
    h
}

#[test]
fn mask_selection_with_every_tool_adds_and_subtracts() {
    let _gpu = gpu_lock();
    let mut h = select_app();
    // Zauberstab: the connected '8' pixels, not the 'c' in their box
    h.key_press(egui::Key::W);
    h.step();
    assert_eq!(h.state().tool, Tool::Wand);
    click_px(&mut h, 2, 2);
    assert_eq!(sel_points(&h), vec![(2, 2), (2, 3), (3, 3)]);
    assert_eq!(h.state().status, "Zauberstab: 3 Pixel ausgewählt");
    let rows = h.state().selection_clipped().unwrap().mask_rows();
    assert_eq!(rows, Some(vec!["#.".to_string(), "##".to_string()]), "the view reports the shape");
    // Shift adds, Alt and Ctrl subtract, a plain click replaces
    with_mods(&mut h, Modifiers::SHIFT, |h| click_px(h, 4, 5));
    assert_eq!(sel_points(&h).len(), 4);
    with_mods(&mut h, Modifiers::ALT, |h| click_px(h, 3, 3));
    assert_eq!(sel_points(&h), vec![(4, 5)]);
    click_px(&mut h, 3, 2);
    assert_eq!(sel_points(&h), vec![(3, 2)], "replaced");
    // "alle gleichfarbigen": the far '8' too
    h.state_mut().wand_all = true;
    click_px(&mut h, 2, 2);
    assert_eq!(sel_points(&h), vec![(2, 2), (2, 3), (3, 3), (10, 10)]);
    h.state_mut().wand_all = false;

    // Auswahl-Pinsel: painting adds, right button / Alt / Ctrl remove
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::D);
    h.step();
    h.key_press_modifiers(Modifiers::SHIFT, egui::Key::M);
    h.step();
    assert_eq!(h.state().tool, Tool::SelectBrush);
    drag_px(&mut h, (0, 0), (3, 0));
    assert_eq!(sel_points(&h), vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
    drag_px(&mut h, (0, 1), (1, 1));
    assert_eq!(sel_points(&h).len(), 6, "a second stroke adds");
    // a right click opens the selection menu and keeps the selection
    right_click_px(&mut h, 1, 0);
    assert_eq!(sel_points(&h).len(), 6);
    h.get_by_label("Kopieren  (Strg+C)").click();
    h.run_steps(2);
    assert!(h.query_by_label("Kopieren  (Strg+C)").is_none(), "menu closed");
    // dragging with the right button removes
    let (a, b) = (px_center(&h, 1, 0), px_center(&h, 2, 0));
    h.hover_at(a);
    h.step();
    press_btn(&h, a, PointerButton::Secondary, true);
    h.step();
    h.hover_at(b);
    h.step();
    press_btn(&h, b, PointerButton::Secondary, false);
    h.step();
    assert_eq!(sel_points(&h), vec![(0, 0), (3, 0), (0, 1), (1, 1)]);
    assert!(h.query_by_label("Kopieren  (Strg+C)").is_none(), "no menu after a drag");
    with_mods(&mut h, Modifiers::ALT, |h| click_px(h, 0, 0));
    with_mods(&mut h, Modifiers::COMMAND, |h| click_px(h, 0, 1));
    assert_eq!(sel_points(&h), vec![(3, 0), (1, 1)]);
    assert!(h.state().doc.get(0, 0, 0, 0) == TRANSPARENT, "selecting paints nothing");

    // rectangle: replace, Shift adds, Alt/Ctrl subtract
    h.key_press(egui::Key::M);
    h.step();
    drag_px(&mut h, (0, 0), (3, 3));
    assert_eq!(h.state().selection, Some(Selection::rect(pixel_core::Rect::new(0, 0, 4, 4))));
    with_mods(&mut h, Modifiers::SHIFT, |h| drag_px(h, (5, 5), (6, 6)));
    assert_eq!(sel_points(&h).len(), 20);
    with_mods(&mut h, Modifiers::ALT, |h| drag_px(h, (1, 1), (2, 2)));
    assert_eq!(sel_points(&h).len(), 16);
    with_mods(&mut h, Modifiers::COMMAND, |h| drag_px(h, (0, 0), (0, 3)));
    assert_eq!(sel_points(&h).len(), 12);
    assert!(h.state().status_line().contains("Auswahl x 1..6 y 0..6 (6x7), 12 Pixel"), "{}", h.state().status_line());

    // lasso: a triangle, then Shift adds a second one
    h.key_press(egui::Key::Q);
    h.step();
    assert_eq!(h.state().tool, Tool::Lasso);
    path_px(&mut h, &[(0, 0), (6, 0), (0, 6)]);
    let s = h.state().selection.clone().unwrap();
    assert!(s.contains(0, 0) && s.contains(1, 1) && s.contains(2, 3) && s.contains(0, 6) && s.contains(6, 0));
    assert!(!s.contains(5, 5) && !s.contains(4, 4));
    let n = s.count();
    with_mods(&mut h, Modifiers::SHIFT, |h| path_px(h, &[(10, 10), (12, 10), (12, 12), (10, 12)]));
    assert_eq!(sel_points(&h).len(), n + 9);
    // the outline follows the mask: the selected corner (0,0) has a white edge, the empty (5,5) none
    h.remove_cursor();
    h.run_steps(1);
    let img = h.render().unwrap();
    let cell = h.state().canvas.screen.width() / h.state().doc.width as f32;
    let at = |x: i32, y: i32, dx: f32, dy: f32| {
        let p = px_center(&h, x, y) + egui::vec2(dx, dy) * cell;
        let c = img.get_pixel(p.x as u32, p.y as u32).0;
        [c[0], c[1], c[2]]
    };
    let inner = 0.5 - 0.5 / cell; // half a screen pixel inside the pixel's edge
    assert_eq!(at(0, 6, 0.0, inner), [255, 255, 255], "white edge inside the bottom of (0,6)");
    assert_eq!(at(0, 6, 0.0, 0.5 + 0.5 / cell), [0, 0, 0], "black just outside");
    assert_ne!(at(6, 5, inner, 0.0), [255, 255, 255], "no bounding-box edge next to unselected pixels");
}

#[test]
fn moving_a_masked_shape_in_front_and_behind() {
    let _gpu = gpu_lock();
    for behind in [false, true] {
        let mut h = select_app();
        // a row of '2' where the L lands
        h.state_mut().doc.set_rows(0, 0, 6, 2, &["2222".into()]).unwrap();
        h.step();
        if behind {
            h.key_press_modifiers(Modifiers::SHIFT, egui::Key::H);
            h.step();
            assert!(h.state().drop_behind);
            assert!(h.state().status.starts_with("Ablegen: hinten"));
        }
        h.key_press(egui::Key::W);
        h.step();
        click_px(&mut h, 2, 2);
        // drag the L from (2,2) to (6,2); the 'c' in its box stays
        drag_px(&mut h, (2, 2), (6, 2));
        assert!(h.state().floating.is_some());
        let d = &h.state().doc;
        assert_eq!((d.get(0, 0, 2, 2), d.get(0, 0, 2, 3), d.get(0, 0, 3, 3)), (TRANSPARENT, TRANSPARENT, TRANSPARENT));
        assert_eq!(d.get(0, 0, 3, 2), b'c', "outside the mask, untouched");
        // the live preview shows the chosen mode
        h.remove_cursor();
        h.run_steps(1);
        let img = h.render().unwrap();
        let p = px_center(&h, 6, 2);
        let shown = img.get_pixel(p.x as u32, p.y as u32).0;
        let want = if behind { b'2' } else { b'8' };
        assert_eq!(shown, h.state().doc.color_of(want).unwrap().0, "preview behind={behind}");
        h.key_press(egui::Key::Enter);
        h.step();
        let d = &h.state().doc;
        assert_eq!(d.get(0, 0, 6, 2), want, "behind={behind}");
        assert_eq!(d.get(0, 0, 7, 2), b'2', "the unselected spot of the box writes nothing");
        assert_eq!((d.get(0, 0, 6, 3), d.get(0, 0, 7, 3)), (b'8', b'8'));
        assert_eq!(d.get(0, 0, 3, 2), b'c');
        let steps = h.state().history.undo_steps().join(" | ");
        assert!(steps.contains("Auswahl verschoben um (4, 0) x 6..7 y 2..3 (2x2), 3 Pixel"), "{steps}");
        assert_eq!(steps.contains("· hinten"), behind, "{steps}");
        // arrows move the shape too; Esc puts it back
        h.key_press(egui::Key::ArrowDown);
        h.step();
        assert_eq!(h.state().doc.get(0, 0, 6, 3), TRANSPARENT);
        h.key_press(egui::Key::Escape);
        h.step();
        assert_eq!(h.state().doc.get(0, 0, 6, 3), b'8');
    }
}

#[test]
fn copy_cut_paste_and_delete_keep_the_mask() {
    let _gpu = gpu_lock();
    let mut h = select_app();
    h.key_press(egui::Key::W);
    h.step();
    click_px(&mut h, 2, 2);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::C);
    h.step();
    assert!(h.state().status.starts_with("Kopiert: 3 Pixel (2x2)"), "{}", h.state().status);
    // into a new frame full of '2': the 'c' of the box is not pasted
    h.key_press_modifiers(Modifiers::ALT, egui::Key::B);
    h.step();
    let (w, hh) = (h.state().doc.width, h.state().doc.height);
    h.state_mut().edit("fill", |d| *d.cel_mut(0, 1) = vec![b'2'; (w * hh) as usize]);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::V);
    h.step();
    assert_eq!(sel_points(&h), vec![(2, 2), (2, 3), (3, 3)], "the pasted selection keeps its shape");
    h.key_press(egui::Key::Enter);
    h.step();
    let d = &h.state().doc;
    assert_eq!((d.get(0, 1, 2, 2), d.get(0, 1, 2, 3), d.get(0, 1, 3, 3)), (b'8', b'8', b'8'));
    assert_eq!(d.get(0, 1, 3, 2), b'2', "not part of the copied shape");
    // Del removes only the selected pixels
    h.key_press(egui::Key::Delete);
    h.step();
    let d = &h.state().doc;
    assert_eq!((d.get(0, 1, 2, 2), d.get(0, 1, 3, 3)), (TRANSPARENT, TRANSPARENT));
    assert_eq!((d.get(0, 1, 3, 2), d.get(0, 1, 4, 4)), (b'2', b'2'));
    // cut on frame 0 keeps the 'c'
    h.state_mut().goto_frame(0);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::X);
    h.step();
    let d = &h.state().doc;
    assert_eq!((d.get(0, 0, 2, 2), d.get(0, 0, 3, 2)), (TRANSPARENT, b'c'));
}

#[test]
fn toolbar_groups_and_flyouts() {
    let _gpu = gpu_lock();
    let mut h = select_app();
    // one button per group, showing its first tool
    assert!(h.query_by_label("Rechteck (U)").is_some());
    assert!(h.query_by_label("Ellipse (Umschalt+U)").is_none(), "hidden in the Formen group");
    // right click on the group button opens the flyout
    let r = h.get_by_label("Rechteck (U)").rect();
    h.hover_at(r.center());
    h.step();
    press_btn(&h, r.center(), PointerButton::Secondary, true);
    h.step();
    press_btn(&h, r.center(), PointerButton::Secondary, false);
    h.run_steps(2);
    h.get_by_label_contains("Ellipse  (Umschalt+U)").click();
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Ellipse);
    assert!(h.state().flyout.is_none(), "picking closes the flyout");
    assert!(h.query_by_label("Ellipse (Umschalt+U)").is_some() && h.query_by_label("Rechteck (U)").is_none());
    // the group remembers it: pencil, then a click on the group button gives the ellipse again
    h.key_press(egui::Key::B);
    h.step();
    h.get_by_label("Ellipse (Umschalt+U)").click();
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Ellipse);
    // a click on the small arrow of the selection group
    let r = h.get_by_label("Auswahl (M)").rect();
    let arrow = r.max - egui::vec2(4.0, 4.0);
    h.hover_at(arrow);
    h.step();
    press(&h, arrow, true);
    h.step();
    press(&h, arrow, false);
    h.run_steps(2);
    for t in ["Auswahl-Pinsel  (Umschalt+M)", "Zauberstab  (W)", "Lasso  (Q)"] {
        assert!(h.query_by_label_contains(t).is_some(), "{t} in the flyout");
    }
    let shot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/toolbar-screenshot.png");
    h.render().unwrap().save(shot).unwrap();
    assert_eq!(h.state().tool, Tool::Ellipse, "the arrow only opens the flyout");
    // a click elsewhere closes it
    let away = h.get_by_label("Palette").rect().center();
    h.hover_at(away);
    h.step();
    press(&h, away, true);
    h.step();
    press(&h, away, false);
    h.run_steps(2);
    assert!(h.state().flyout.is_none());
    // a long press opens it too
    let r = h.get_by_label("Auswahl (M)").rect();
    h.hover_at(r.center());
    h.step();
    press(&h, r.center(), true);
    let start = Instant::now();
    while h.state().flyout.is_none() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
        h.step();
    }
    assert!(h.state().flyout.is_some(), "long press");
    press(&h, r.center(), false);
    h.run_steps(2);
    h.get_by_label_contains("Zauberstab  (W)").click();
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Wand);
    assert!(h.query_by_label("Zauberstab (W)").is_some(), "the group shows the wand now");
    // the Werkzeuge menu has the same groups as submenus
    menu(&mut h, "Werkzeuge");
    h.get_by_label("Formen").hover();
    h.run_steps(3);
    assert!(h.query_by_label("Ellipse  (Umschalt+U)").is_some());
    h.get_by_label("Rechteck  (U)").click();
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Rect);
}

#[test]
fn select_none_all_invert_and_the_selection_menus() {
    let _gpu = gpu_lock();
    let mut h = select_app();
    let (w, hh) = (h.state().doc.width as usize, h.state().doc.height as usize);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.step();
    assert_eq!(sel_points(&h).len(), w * hh);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::D);
    h.step();
    assert!(h.state().selection.is_none());
    // invert the wand's L, and back
    h.key_press(egui::Key::W);
    h.step();
    click_px(&mut h, 2, 2);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::I);
    h.step();
    assert_eq!(sel_points(&h).len(), w * hh - 3);
    assert!(!h.state().selection.as_ref().unwrap().contains(2, 2));
    assert!(h.state().selection.as_ref().unwrap().contains(3, 2));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::I);
    h.step();
    assert_eq!(sel_points(&h), vec![(2, 2), (2, 3), (3, 3)]);

    // canvas context menu (rectangle tool): Auswahl aufheben
    h.key_press(egui::Key::M);
    h.step();
    right_click_px(&mut h, 8, 8);
    assert_eq!(sel_points(&h).len(), 3, "a right click selects nothing by itself");
    h.get_by_label("Auswahl aufheben  (Strg+D)").click();
    h.run_steps(1);
    assert!(h.state().selection.is_none());
    // the Auswahl menu in the menu bar
    menu(&mut h, "Auswahl");
    h.get_by_label("Alles auswählen  (Strg+A)").click();
    h.run_steps(1);
    assert_eq!(sel_points(&h).len(), w * hh);
    menu(&mut h, "Auswahl");
    h.get_by_label("Auswahl umkehren  (Strg+Umschalt+I)").click();
    h.run_steps(1);
    assert!(h.state().selection.is_none(), "everything inverted is nothing");

    // floating pixels: drop them behind from the context menu
    h.state_mut().doc.set(0, 0, 6, 2, b'2');
    h.key_press(egui::Key::W);
    h.step();
    click_px(&mut h, 2, 2);
    drag_px(&mut h, (2, 2), (6, 2));
    right_click_px(&mut h, 6, 3);
    assert!(h.state().floating.is_some(), "the menu keeps the pixels floating");
    h.get_by_label("Hier hinten ablegen").click();
    h.run_steps(1);
    let d = &h.state().doc;
    assert!(h.state().floating.is_none());
    assert_eq!((d.get(0, 0, 6, 2), d.get(0, 0, 6, 3)), (b'2', b'8'), "behind the existing pixel");
    assert!(!h.state().drop_behind, "a one-off: the mode stays Vorne");
}

// ---- "KI zeigen" marks ---------------------------------------------------------

fn press_btn(h: &Harness<'_, EditorApp>, pos: Pos2, button: PointerButton, pressed: bool) {
    h.event(egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
}

/// Drag between two screen points with the left button.
fn drag_screen(h: &mut Harness<'_, EditorApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.step();
    press(h, a, true);
    h.step();
    h.hover_at(b);
    h.step();
    press(h, b, false);
    h.step();
}

fn type_note(h: &mut Harness<'_, EditorApp>, text: &str) {
    h.run_steps(1);
    assert!(h.state().canvas.note_focused, "the note field has the keyboard");
    h.event(egui::Event::Text(text.into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
}

fn menu(h: &mut Harness<'_, EditorApp>, label: &str) {
    let top = h.get_all_by_label(label).min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).unwrap();
    top.click();
    h.run_steps(3); // the popup fades in
}

macro_rules! rgb_at {
    ($img:expr, $p:expr) => {{
        let p: Pos2 = $p;
        let c = $img.get_pixel(p.x as u32, p.y as u32).0;
        [c[0], c[1], c[2]]
    }};
}

const PIN: [u8; 3] = [90, 200, 255];

/// Point on the left outline of mark `n` (drawn 2 px outside its pixels),
/// between the corner and the middle handle.
fn outline_point(h: &Harness<'_, EditorApp>, n: u32) -> Pos2 {
    let app = h.state();
    let m = app.marks.marks.iter().find(|m| m.n == n).unwrap();
    let sr = app.mark_screen(m);
    Pos2::new(sr.min.x - 1.5, sr.max.y - sr.height() * 0.25)
}

#[test]
fn marks_add_note_select_move_resize_and_remove() {
    let _gpu = gpu_lock();
    let (dir, walk, _, _) = tabs_setup("marks");
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    h.key_press(egui::Key::K);
    h.step();
    assert_eq!(h.state().tool, Tool::Point);
    let before = h.state().doc.clone();

    // drag: mark #1, then type its note (Enter saves)
    drag_px(&mut h, (5, 6), (2, 3));
    assert!(h.state().status.starts_with("KI-Markierung #1: Bereich x 2..5 y 3..6 (4x4) · "), "{}", h.state().status);
    type_note(&mut h, "brauner Block weg");
    let m = crate::view::read_marks(&walk);
    assert_eq!(m.marks[0].note, "brauner Block weg");
    assert_eq!(m.marks[0].rect, Some(crate::view::Area { x: 2, y: 3, w: 4, h: 4 }));
    assert!(!h.state().canvas.note_focused);

    // a click adds #2 (one pixel); clicking away from its note field only closes it
    click_px(&mut h, 10, 10);
    h.run_steps(1);
    assert!(h.state().canvas.note_focused);
    click_px(&mut h, 13, 1);
    assert_eq!(crate::view::read_marks(&walk).marks.len(), 2, "the click closed the note field, no new mark");
    assert_eq!(crate::view::read_marks(&walk).marks[1].rect, Some(crate::view::Area { x: 10, y: 10, w: 1, h: 1 }));

    // frame 1: mark #3; marks of frame 0 are not drawn there
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(1);
    drag_px(&mut h, (0, 0), (3, 1));
    h.key_press(egui::Key::Enter); // no note
    h.run_steps(2);
    let m = crate::view::read_marks(&walk);
    assert_eq!(m.marks.iter().map(|m| (m.n, m.frame.index)).collect::<Vec<_>>(), vec![(1, 0), (2, 0), (3, 1)]);
    let o1 = outline_point(&h, 1);
    let img = h.render().unwrap();
    assert_ne!(rgb_at!(img, o1), PIN, "#1 is on another frame");
    assert_eq!(rgb_at!(img, outline_point(&h, 3)), PIN, "#3 is drawn");
    // timeline: a badge on frames 0 and 1, none on 2
    let cells = h.state().timeline.frame_cells.clone();
    let badge = |f: usize| Pos2::new(cells[f].max.x - 7.0 - 4.5, cells[f].min.y + 7.0);
    assert_eq!(rgb_at!(img, badge(0)), PIN);
    assert_eq!(rgb_at!(img, badge(1)), PIN);
    assert_ne!(rgb_at!(img, badge(2)), PIN);
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(1);
    let img = h.render().unwrap();
    assert_eq!(rgb_at!(img, outline_point(&h, 1)), PIN, "#1 drawn on its frame");
    assert!(h.query_by_label("#1 brauner Block weg").is_none(), "labels are painted");
    img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/marks-screenshot.png")).unwrap();

    // click inside #1: selected, its note can be edited
    click_px(&mut h, 3, 4);
    assert_eq!(h.state().mark_sel, Some(1));
    assert_eq!(h.state().note_draft, "brauner Block weg");
    type_note(&mut h, " bitte");
    assert_eq!(crate::view::read_marks(&walk).marks[0].note, "brauner Block weg bitte");

    // drag the inside: move by 3 to the right (a moved mark opens no note field)
    drag_px(&mut h, (3, 4), (6, 4));
    h.run_steps(1);
    assert!(!h.state().canvas.note_focused);
    assert_eq!(crate::view::read_marks(&walk).marks[0].rect, Some(crate::view::Area { x: 5, y: 3, w: 4, h: 4 }));
    // the right edge handle: resize to x 12
    let sr = {
        let app = h.state();
        app.mark_screen(&app.marks.marks[0])
    };
    let to = px_center(&h, 12, 4);
    drag_screen(&mut h, Pos2::new(sr.max.x, sr.center().y), to);
    h.run_steps(1);
    assert_eq!(crate::view::read_marks(&walk).marks[0].rect, Some(crate::view::Area { x: 5, y: 3, w: 8, h: 4 }));
    // the top-left corner: to (7, 1)
    let sr = {
        let app = h.state();
        app.mark_screen(&app.marks.marks[0])
    };
    let to = px_center(&h, 7, 1);
    drag_screen(&mut h, sr.min, to);
    h.run_steps(1);
    assert_eq!(crate::view::read_marks(&walk).marks[0].rect, Some(crate::view::Area { x: 7, y: 1, w: 6, h: 6 }));
    assert_eq!(h.state().doc, before, "marking changes no pixel");
    assert!(h.state().history.undo_steps().is_empty());

    // right click removes only the mark under the pointer
    let p = px_center(&h, 10, 10);
    h.hover_at(p);
    h.step();
    press_btn(&h, p, PointerButton::Secondary, true);
    h.step();
    press_btn(&h, p, PointerButton::Secondary, false);
    h.run_steps(2);
    let ns: Vec<u32> = crate::view::read_marks(&walk).marks.iter().map(|m| m.n).collect();
    assert_eq!(ns, vec![1, 3]);
    assert_eq!(h.state().status, "KI-Markierung #2 entfernt");

    // menu: the marks of this frame; status bar: all
    menu(&mut h, "Werkzeuge");
    h.get_by_label("KI-Markierungen dieses Frames entfernen").click();
    h.run_steps(1);
    let ns: Vec<u32> = crate::view::read_marks(&walk).marks.iter().map(|m| m.n).collect();
    assert_eq!(ns, vec![3], "frame 1 keeps its mark");
    h.get_by_label("KI-Markierungen (1) ×").click();
    h.run_steps(1);
    assert!(h.state().marks.marks.is_empty() && !crate::view::marks_file(&walk).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Marks belong to the document: each tab has its own, they survive a
/// restart, the old pointer.json is taken over, and marks the AI removes
/// (MCP clear_marks) disappear live.
#[test]
fn marks_per_document_persist_and_follow_clear_marks() {
    let _gpu = gpu_lock();
    let (dir, walk, idle, doc) = tabs_setup("marks-docs");
    // an old single pointer for idle
    let old = crate::view::Pointer {
        pid: 1,
        time_ms: crate::view::now_ms(),
        path: idle.clone(),
        dirty: false,
        frame: crate::view::frame_ref(&doc, 2),
        layer: crate::view::layer_ref(&doc, 0),
        rect: Some(crate::view::Area { x: 1, y: 1, w: 2, h: 2 }),
    };
    let pf = crate::view::pointer_file(&idle);
    std::fs::create_dir_all(pf.parent().unwrap()).unwrap();
    std::fs::write(&pf, serde_json::to_string(&old).unwrap()).unwrap();
    let mut h = harness(tabs_app(&walk));
    h.run_steps(2);
    h.state_mut().tool = Tool::Point;
    drag_px(&mut h, (1, 1), (4, 4));
    h.state_mut().open_tab(&idle).unwrap();
    h.run_steps(1);
    assert_eq!(h.state().marks.marks.iter().map(|m| (m.n, m.frame.index)).collect::<Vec<_>>(), vec![(1, 2)]);
    // Ctrl+Shift+K adds one too (here: the whole frame), also while typing in a field
    h.get_all_by_role(egui::accesskit::Role::TextInput).next().unwrap().click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::K);
    h.run_steps(1);
    assert!(h.state().status.starts_with("KI-Markierung #2: ganzer Frame"), "{}", h.state().status);
    assert!(!crate::view::pointer_file(&idle).exists(), "migrated to marks.json");
    assert_eq!(crate::view::read_marks(&idle).marks.len(), 2);
    assert_eq!(crate::view::read_marks(&walk).marks.len(), 1, "the other tab keeps its own mark");
    h.state_mut().switch_tab(0);
    assert_eq!(h.state().marks.marks.len(), 1);

    // the AI handled walk's mark: it disappears from the open tab
    let art = walk.parent().unwrap().parent().unwrap().to_path_buf();
    let (ok, t) = crate::mcp::test_call(&art, "get_user_view", serde_json::json!({}));
    assert!(ok && t.contains("marks in hero/walk.pxe.json: 1") && t.contains("marks in hero/idle.pxe.json: 2"), "{t}");
    std::thread::sleep(Duration::from_millis(20));
    let (ok, t) = crate::mcp::test_call(&art, "clear_marks", serde_json::json!({"path": "hero/walk"}));
    assert!(ok, "{t}");
    poll(&mut h);
    assert!(h.state().marks.marks.is_empty(), "reloaded");
    // a new editor finds idle's marks again
    let again = EditorApp::new(Some(idle.clone()));
    assert_eq!(again.marks.marks.len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- timeline scrubbing --------------------------------------------------------

/// Drag on the frame numbers: the frame follows the pointer, playback
/// pauses and stays paused; a click still selects; the wheel over the
/// numbers steps frames.
#[test]
fn timeline_scrubbing_and_wheel() {
    let _gpu = gpu_lock();
    let mut doc = Document::new(16, 16, Document::builtin_palette("pico8").unwrap()).unwrap();
    for f in 1..6 {
        doc.add_frame(f - 1, None, false).unwrap();
        doc.set(0, f, f as i32, 0, b'8');
    }
    doc.add_layer("body", 1);
    let mut app = EditorApp::new(None);
    app.replace_doc(doc);
    app.preview.settings.open = false;
    app.timeline.show_thumbs = false; // the cel rows right under the numbers
    let mut h = harness(app);
    h.run_steps(3);
    let cells = h.state().timeline.frame_cells.clone();
    let cell = |f: usize| cells[f].center();
    let mv = |h: &mut Harness<'_, EditorApp>, p: Pos2| {
        h.event(egui::Event::PointerMoved(p));
        h.step();
    };

    // playing, then press on frame 1's number and drag right over 2, 3, 4
    h.get_by_label("Abspielen").click();
    h.run_steps(2);
    assert!(h.state().playing);
    let start = cell(1);
    mv(&mut h, start);
    press(&h, start, true);
    h.step();
    let mut seen = Vec::new();
    for f in 2..=4 {
        mv(&mut h, cell(f) + egui::vec2(3.0, 0.0));
        assert!(h.state().timeline.scrubbing, "scrubbing at {f}");
        assert!(!h.state().playing, "playback paused while scrubbing");
        seen.push(h.state().frame);
        // canvas texture and right panel follow live
        assert_eq!(h.state().panel_frame, h.state().frame);
    }
    assert_eq!(seen, vec![2, 3, 4]);
    // back left, below the row and beyond the last frame: still the column
    mv(&mut h, Pos2::new(cell(2).x, cell(2).y + 60.0));
    assert_eq!(h.state().frame, 2, "left/right counts, not the row");
    mv(&mut h, cell(5) + egui::vec2(400.0, 0.0));
    assert_eq!(h.state().frame, 5, "clamped to the last frame");
    press(&h, cell(5), false);
    h.run_steps(2);
    assert!(!h.state().timeline.scrubbing && !h.state().playing, "released: stays paused");
    assert!(h.query_by_label_contains("Frame #5").is_some(), "the right panel shows the scrubbed frame");
    // the canvas shows frame 5: its marked pixel
    let img = h.render().unwrap();
    let app = h.state();
    let c = app.canvas.screen.width() / 16.0;
    let p = app.canvas.screen.min + egui::vec2(5.5 * c, 0.5 * c);
    assert_eq!(&img.get_pixel(p.x as u32, p.y as u32).0[..3], &app.doc.color_of(b'8').unwrap().0[..3]);

    // scrubbing over the cels selects the pressed layer
    h.state_mut().layer = 0;
    let cel_y = cells[0].max.y + 14.0 + 11.0; // name row, then the top layer (body)
    let a = Pos2::new(cell(0).x, cel_y);
    mv(&mut h, a);
    press(&h, a, true);
    h.step();
    mv(&mut h, Pos2::new(cell(3).x, a.y));
    press(&h, Pos2::new(cell(3).x, a.y), false);
    h.run_steps(1);
    assert_eq!((h.state().frame, h.state().layer), (3, 1));

    // a plain click still selects
    let c1 = cell(1);
    mv(&mut h, c1);
    press(&h, c1, true);
    h.step();
    press(&h, c1, false);
    h.run_steps(1);
    assert_eq!(h.state().frame, 1);
    assert!(!h.state().timeline.scrubbing);

    // wheel over the numbers: down = next, up = previous (wraps)
    let wheel = |h: &mut Harness<'_, EditorApp>, dy: f32| {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, dy),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.step();
    };
    wheel(&mut h, -1.0);
    assert_eq!(h.state().frame, 2);
    wheel(&mut h, -2.0);
    assert_eq!(h.state().frame, 4);
    wheel(&mut h, 1.0);
    assert_eq!(h.state().frame, 3);
    // over the cels the wheel does not change the frame
    mv(&mut h, Pos2::new(c1.x, cel_y));
    wheel(&mut h, -1.0);
    assert_eq!(h.state().frame, 3);
}

// ---- mirroring, rotating, symmetric drawing --------------------------------------

#[test]
fn transform_selection_cel_frames_and_menus() {
    let _gpu = gpu_lock();
    let mut h = select_app();
    let alt = |h: &mut Harness<'_, EditorApp>, k: egui::Key| {
        h.key_press_modifiers(Modifiers::ALT, k);
        h.step();
    };
    // rectangle selection over "8c" / "88", mirrored with Alt+H
    h.key_press(egui::Key::M);
    h.step();
    drag_px(&mut h, (2, 2), (3, 3));
    alt(&mut h, egui::Key::H);
    let row = |h: &Harness<'_, EditorApp>, y: i32| (h.state().doc.get(0, 0, 2, y), h.state().doc.get(0, 0, 3, y));
    assert_eq!((row(&h, 2), row(&h, 3)), ((b'c', b'8'), (b'8', b'8')));
    assert!(h.state().status.starts_with("Horizontal spiegeln: Auswahl"), "{}", h.state().status);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert_eq!(row(&h, 2), (b'8', b'c'), "one undo step");
    // the selection tools' context menu turns it 90° clockwise
    right_click_px(&mut h, 2, 2);
    h.get_by_label("90° rechts drehen  (Alt+R)").click();
    h.run_steps(1);
    assert_eq!((row(&h, 2), row(&h, 3)), ((b'8', b'8'), (b'8', b'c')));
    // a shaped selection (wand) keeps its shape: 180° moves only the three 8s
    h.key_press(egui::Key::W);
    h.step();
    click_px(&mut h, 2, 2);
    assert_eq!(sel_points(&h).len(), 3);
    h.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, egui::Key::R);
    h.step();
    assert_eq!((row(&h, 2), row(&h, 3)), ((TRANSPARENT, b'8'), (b'8', b'8')));
    assert_eq!(sel_points(&h), vec![(3, 2), (2, 3), (3, 3)], "the mask turned with the pixels");

    // floating pixels turn without touching the cel until they are dropped
    h.key_press(egui::Key::M);
    h.step();
    drag_px(&mut h, (10, 10), (11, 10));
    drag_px(&mut h, (10, 10), (10, 12));
    assert!(h.state().floating.is_some());
    alt(&mut h, egui::Key::H);
    assert_eq!(h.state().doc.get(0, 0, 10, 10), TRANSPARENT, "lifted");
    h.key_press(egui::Key::Enter);
    h.step();
    let d = &h.state().doc;
    assert_eq!((d.get(0, 0, 10, 12), d.get(0, 0, 11, 12)), (TRANSPARENT, b'8'));
    assert!(h.state().history.undo_steps().last().unwrap().contains("verschoben"));

    // no selection: the Bild menu works on the cel, around the canvas centre
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::D);
    h.step();
    menu(&mut h, "Bild");
    h.get_by_label("Horizontal spiegeln  (Alt+H)").click();
    h.run_steps(1);
    assert_eq!(h.state().doc.get(0, 0, 63 - 11, 12), b'8');
    assert!(h.state().status.starts_with("Horizontal spiegeln: Cel"), "{}", h.state().status);

    // frame scope turns every layer; a locked layer is skipped and named
    let top = h.state_mut().doc.add_layer("oben", 1);
    h.state_mut().doc.set(top, 0, 0, 0, b'2');
    h.state_mut().rev += 1;
    menu(&mut h, "Bild");
    h.get_by_label("Frame (alle Ebenen)").hover();
    h.run_steps(3);
    h.get_by_label("Vertikal spiegeln").click();
    h.run_steps(1);
    let d = &h.state().doc;
    assert_eq!((d.get(top, 0, 0, 63), d.get(0, 0, 52, 63 - 12)), (b'2', b'8'));
    h.state_mut().doc.layers[top].locked = true;
    h.state_mut().doc.add_frame(1, Some(0), false).unwrap();
    h.state_mut().doc.set_tag(pixel_core::Tag::new("walk", 0, 1)).unwrap();
    h.state_mut().rev += 1;
    menu(&mut h, "Bild");
    h.get_by_label("Frames des Tags walk").hover();
    h.run_steps(3);
    h.get_by_label("180° drehen").click();
    h.run_steps(1);
    let d = &h.state().doc;
    for f in 0..2 {
        assert_eq!(d.get(0, f, 63 - 52, 12), b'8', "frame {f} turned");
        assert_eq!(d.get(top, f, 0, 63), b'2', "locked layer stays on frame {f}");
    }
    assert!(h.state().status.contains("übersprungen: layer 1 \"oben\" is locked"), "{}", h.state().status);
    // all frames, once more through the menu
    menu(&mut h, "Bild");
    h.get_by_label("Alle Frames").hover();
    h.run_steps(3);
    h.get_by_label("90° links drehen").click();
    h.run_steps(1);
    assert_eq!(h.state().doc.get(0, 1, 12, 63 - 11), b'8');
}

#[test]
fn symmetric_drawing_with_movable_axes() {
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.state_mut().color = b'2';
    h.key_press_modifiers(Modifiers::ALT, egui::Key::S);
    h.step();
    assert_eq!(h.state().symmetry.mode, super::transform::SymMode::Vertical);
    let get = |h: &Harness<'_, EditorApp>, x: i32, y: i32| h.state().doc.get(0, 0, x, y);
    // pencil: the mirrored pixel too
    click_px(&mut h, 1, 1);
    assert_eq!((get(&h, 1, 1), get(&h, 62, 1)), (b'2', b'2'));
    // rectangle: preview and result mirrored
    h.key_press(egui::Key::U);
    h.step();
    drag_px(&mut h, (2, 5), (5, 8));
    assert_eq!((get(&h, 2, 5), get(&h, 61, 5), get(&h, 58, 8)), (b'2', b'2', b'2'));
    // line, then fill inside the left rectangle fills the right one too
    h.key_press(egui::Key::L);
    h.step();
    drag_px(&mut h, (10, 3), (12, 3));
    assert_eq!(get(&h, 53, 3), b'2');
    h.key_press(egui::Key::G);
    h.step();
    h.state_mut().color = b'4';
    click_px(&mut h, 3, 6);
    assert_eq!((get(&h, 3, 6), get(&h, 60, 6), get(&h, 30, 30)), (b'4', b'4', TRANSPARENT));
    // both axes: four pixels per click, the eraser too
    h.state_mut().symmetry.mode = super::transform::SymMode::Both;
    h.key_press(egui::Key::B);
    h.step();
    h.state_mut().color = b'9';
    click_px(&mut h, 3, 20);
    for (x, y) in [(3, 20), (60, 20), (3, 43), (60, 43)] {
        assert_eq!(get(&h, x, y), b'9', "({x}, {y})");
    }
    h.key_press(egui::Key::E);
    h.step();
    click_px(&mut h, 60, 43);
    assert_eq!((get(&h, 3, 20), get(&h, 60, 20)), (TRANSPARENT, TRANSPARENT));
    // drag the vertical axis by its grip above the canvas to x = 10
    let app = h.state();
    let (r, cell) = (app.canvas.screen, app.canvas.screen.width() / 64.0);
    let grip = Pos2::new(r.min.x + 32.0 * cell, r.min.y - 12.0);
    let to = Pos2::new(r.min.x + 10.0 * cell, r.min.y - 12.0);
    drag_screen(&mut h, grip, to);
    assert_eq!(h.state().symmetry.x2, Some(20));
    assert!(h.state().canvas.axis_drag.is_none());
    h.key_press(egui::Key::B);
    h.step();
    click_px(&mut h, 2, 30);
    assert_eq!((get(&h, 2, 30), get(&h, 17, 30), get(&h, 2, 33)), (b'9', b'9', b'9'));
    // the axis through a pixel centre (typed 12.5): that column maps to itself
    h.state_mut().symmetry =
        super::transform::Symmetry { mode: super::transform::SymMode::Vertical, x2: Some(25), y2: None };
    click_px(&mut h, 10, 50);
    assert_eq!((get(&h, 10, 50), get(&h, 14, 50)), (b'9', b'9'));

    // screenshot: both axes, a symmetric figure and the Bild menu
    let d = &mut h.state_mut().doc;
    d.set_points(0, 0, &pixel_core::draw::ellipse(31, 32, 14, 10, true), b'6');
    d.set_points(0, 0, &pixel_core::draw::ellipse(31, 32, 14, 10, false), b'0');
    h.state_mut().symmetry =
        super::transform::Symmetry { mode: super::transform::SymMode::Both, x2: Some(64), y2: Some(64) };
    h.state_mut().rev += 1;
    h.run_steps(2);
    menu(&mut h, "Bild");
    h.get_by_label("Alle Frames").hover();
    h.run_steps(3);
    let shot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/transform-screenshot.png");
    h.render().unwrap().save(&shot).unwrap();
    eprintln!("saved {}", shot.display());
}

#[test]
fn mirrored_duplicate_opens_in_a_new_tab() {
    let _gpu = gpu_lock();
    let dir = scratch_dir("mirror");
    let path = dir.join("walk_right.pxe.json");
    let mut doc = Document::new(8, 4, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    doc.set_rows(0, 0, 0, 0, &["12".into(), "3".into()]).unwrap();
    doc.add_frame(1, Some(0), true).unwrap();
    doc.frames[1].duration_ms = 240;
    doc.set_tag(pixel_core::Tag::new("walk", 0, 1)).unwrap();
    doc.regions.push(pixel_core::Region::new("hand", pixel_core::Rect::new(0, 0, 2, 1)));
    io::save(&doc, &path).unwrap();
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    menu(&mut h, "Bild");
    h.get_by_label("Animation gespiegelt duplizieren…").click();
    h.run_steps(2);
    let want = dir.join("walk_left.pxe.json");
    assert_eq!(h.state().mirror_dialog.as_deref(), Some(want.to_str().unwrap()));
    h.get_by_label("Duplizieren").click();
    h.run_steps(3);
    let m = io::load(&want).unwrap();
    assert_eq!(m.rows(0, 0)[..2], ["......21", ".......3"]);
    assert!(m.layers[0].is_linked(1));
    assert_eq!((m.frames[1].duration_ms, &m.tags), (240, &doc.tags));
    assert_eq!(m.regions[0].rect_at(0), Some(pixel_core::Rect::new(6, 0, 2, 1)));
    assert_eq!(io::load(&path).unwrap(), doc, "the source stays as it was");
    assert_eq!(h.state().path.as_deref(), Some(want.as_path()), "the copy opens");
    assert_eq!(h.state().tabs.len(), 2);
    assert!(h.state().status.starts_with("Gespiegelt dupliziert"), "{}", h.state().status);
    // a second copy under the same name is refused
    menu(&mut h, "Bild");
    h.get_by_label("Animation gespiegelt duplizieren…").click();
    h.run_steps(2);
    h.state_mut().mirror_dialog = Some(want.display().to_string());
    h.get_by_label("Duplizieren").click();
    h.run_steps(2);
    assert!(h.state().status.contains("gibt es schon"), "{}", h.state().status);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tiled_mode_draws_in_all_copies() {
    use super::tiled::TileMode;
    let _gpu = gpu_lock();
    let dir = scratch_dir("tiled").join("tiles");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ground.pxe.json");
    let doc = Document::new(32, 32, Document::builtin_palette("sweetie16").unwrap()).unwrap();
    io::save(&doc, &path).unwrap();
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    assert_eq!(h.state().tile_mode, TileMode::Both, "a document in a tiles folder starts tiled");
    let (screen, area) = (h.state().canvas.screen, h.state().canvas.area);
    assert!(screen.width() * 3.0 <= area.width() && screen.height() * 3.0 <= area.height(), "all copies fit");
    let get = |h: &Harness<'_, EditorApp>, x: i32, y: i32| h.state().doc.get(0, 0, x, y);
    h.state_mut().color = b'2';
    // a pencil stroke over the right edge comes back in on the left
    drag_px(&mut h, (29, 5), (35, 5));
    for x in (29..32).chain(0..=3) {
        assert_eq!(get(&h, x, 5), b'2', "x {x}");
    }
    assert_eq!(get(&h, 4, 5), TRANSPARENT);
    // a click on the top-left copy paints the real pixel
    click_px(&mut h, -2, -2);
    assert_eq!(get(&h, 30, 30), b'2');
    // hovering a copy shows the real pixel
    h.hover_at(px_center(&h, -1, 33));
    h.step();
    assert_eq!(h.state().canvas.hover, Some((31, 1)));
    // rectangle across the bottom edge
    h.key_press(egui::Key::U);
    h.step();
    drag_px(&mut h, (10, 30), (12, 33));
    assert_eq!((get(&h, 10, 30), get(&h, 12, 31), get(&h, 10, 1), get(&h, 11, 1)), (b'2', b'2', b'2', b'2'));
    assert_eq!(get(&h, 11, 0), TRANSPARENT, "outline only");
    // a wall at x = 16; the fill from the left half runs around the edge into the right half
    h.state_mut().doc.set_points(0, 0, &pixel_core::draw::line(16, 0, 16, 31), b'9');
    h.key_press(egui::Key::G);
    h.step();
    h.state_mut().color = b'5';
    click_px(&mut h, 2, 20);
    assert_eq!((get(&h, 2, 20), get(&h, 20, 20), get(&h, 16, 20)), (b'5', b'5', b'9'));
    // symmetry works together with the wrap
    h.key_press(egui::Key::B);
    h.step();
    h.state_mut().symmetry =
        super::transform::Symmetry { mode: super::transform::SymMode::Vertical, x2: None, y2: None };
    h.state_mut().color = b'c';
    click_px(&mut h, -1, 12);
    assert_eq!((get(&h, 31, 12), get(&h, 0, 12)), (b'c', b'c'));
    h.state_mut().symmetry.mode = super::transform::SymMode::Off;
    // a rectangle selection over the right edge selects both sides
    h.key_press(egui::Key::M);
    h.step();
    drag_px(&mut h, (30, 2), (33, 3));
    let sel = h.state().selection.clone().expect("selection");
    assert!(sel.contains(31, 2) && sel.contains(0, 3) && sel.contains(1, 2) && !sel.contains(2, 2));
    assert_eq!(sel.count(), 8);
    // Alt+T cycles the mode, remembered for this document
    h.key_press_modifiers(Modifiers::ALT, egui::Key::T);
    h.step();
    assert_eq!(h.state().tile_mode, TileMode::Horizontal);
    let key = super::tiled::memory_key(&path);
    assert_eq!(h.state().tile_memory.get(&key), Some(&TileMode::Horizontal));
    assert_eq!(h.state().settings().tile_modes.get(&key), Some(&TileMode::Horizontal));
    // only side by side now: the copy above is not drawn in
    h.state_mut().selection = None;
    h.key_press(egui::Key::B);
    h.step();
    click_px(&mut h, 8, -3);
    assert_eq!(get(&h, 8, 29), b'5', "unchanged (filled before)");
    // the preview shows the copies too
    h.state_mut().preview.settings.tiled = true;
    h.state_mut().preview.settings.scale = 2;
    h.run_steps(3);
    let r = h.state().preview.image_rect.expect("preview shown");
    assert_eq!((r.width(), r.height()), (3.0 * 64.0, 64.0));
    // reopening restores the remembered mode
    h.state_mut().tile_mode = TileMode::Off;
    let p = path.clone();
    h.state_mut().open(&p);
    assert_eq!(h.state().tile_mode, TileMode::Horizontal);
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn new_tile_document_starts_tiled() {
    use super::tiled::TileMode;
    let _gpu = gpu_lock();
    let mut h = harness(EditorApp::new(None));
    h.run_steps(3);
    h.state_mut().new_document();
    h.run_steps(2);
    assert!(!h.state().new_dialog.as_ref().unwrap().tiled, "not in a tile folder");
    h.get_by_label("Tile 32×32").click();
    h.run_steps(2);
    h.get_by_label("Erstellen").click();
    h.run_steps(3);
    assert_eq!((h.state().doc.width, h.state().tile_mode), (32, TileMode::Both));
    menu(&mut h, "Ansicht");
    h.get_by_label("Kachel-Modus: Beide").hover();
    h.run_steps(3);
    h.get_by_label("Aus – nur das Bild").click();
    h.run_steps(2);
    assert_eq!(h.state().tile_mode, TileMode::Off);
}

#[test]
fn tiled_screenshot_of_the_tileset() {
    let _gpu = gpu_lock();
    let src = std::path::Path::new(env!("HOME")).join("Development/pixelart-studio/art/tiles32.pxe.json");
    let dir = scratch_dir("tileset");
    let path = dir.join("tiles32.pxe.json");
    let doc = match io::load(&src) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("tiles32 not found ({e}), screenshot with a plain tile");
            let mut d = Document::new(32, 32, Document::builtin_palette("sweetie16").unwrap()).unwrap();
            d.set_points(0, 0, &pixel_core::draw::rect(0, 0, 31, 31, true), b'5');
            d.frames[0].name = "path".into();
            d
        }
    };
    io::save(&doc, &path).unwrap(); // a copy: the original stays untouched
    let mut h = harness(EditorApp::new(Some(path.clone())));
    h.run_steps(3);
    let f = h.state().doc.find_frame("path").unwrap();
    h.state_mut().goto_frame(f);
    h.state_mut().layer = 0;
    let red = h.state().doc.palette.iter().find(|p| p.name == "berry").map_or(b'2', |p| p.key);
    h.state_mut().color = red;
    // the tiled preview small in the corner, out of the canvas copies' way
    let st = &mut h.state_mut().preview.settings;
    (st.tiled, st.scale, st.pos) = (true, 1, Some([850.0, 330.0]));
    h.run_steps(2);
    // one stroke from the middle out over the right edge, back in on the left
    drag_px(&mut h, (20, 10), (42, 20));
    for (x, y) in pixel_core::draw::line(20, 10, 42, 20) {
        assert_eq!(h.state().doc.get(0, f, x % 32, y), red, "({x}, {y})");
    }
    h.hover_at(px_center(&h, 40, 19));
    h.run_steps(3);
    let shot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tiled-screenshot.png");
    h.render().unwrap().save(&shot).unwrap();
    eprintln!("saved {}", shot.display());
    std::fs::remove_dir_all(&dir).unwrap();
}
