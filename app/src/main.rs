mod gui;
mod hook;
mod ipc;
mod knowledge;
mod launch;
mod mcp;
mod view;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pixel_core::export::godot_tiles;
use pixel_core::export::{self, ExportOptions, Target};
use pixel_core::tilemap::{self, Assets, Tileset};
use pixel_core::{io, render};

const USAGE: &str = "\
pixel-editor – Pixelart- und Animationseditor

Aufruf:
  pixel-editor [--workdir ORDNER] [DATEI.pxe.json]
                                          Editor öffnen (Datei wird angelegt, wenn sie fehlt)
  pixel-editor open <DATEI> [--workdir ORDNER] [--dry-run]
                                          Datei als Tab im laufenden Editorfenster des Arbeitsordners
                                          öffnen, sonst ein neues Fenster starten (losgelöst)
  pixel-editor mcp <ARBEITSORDNER>        MCP-Server über stdio starten; Pixelart-Leitfäden (get_guide):
                                          eingebaut, persönlich ($PXE_KNOWLEDGE_DIR, sonst
                                          $XDG_DATA_HOME/pixel-editor/knowledge), Projekt (.pxe-knowledge/)
  pixel-editor export <DATEI> <ZIEL> [AUSGABEORDNER] [--scale N] [--godot-res-dir res://…]
                                          ZIEL: png_sheet | godot | unity | gif | aseprite | godot_tileset
                                          (godot_tileset: Atlas-PNG + TileSet .tres eines Tilesets);
                                          bei einer Karte (.tilemap.json): godot (Atlas, TileSet, .tscn)
                                          oder world_txt
  pixel-editor preview <DATEI> <AUS.png> [--zoom N] [--frame F] [--no-grid]
                                          (bei einer Karte: die ganze Karte mit Kachelraster)
  pixel-editor import-world <world.txt> <KARTE.tilemap.json> --tileset <TILESET.pxe.json> [--force]
                                          world.txt der Sandbox als Karte übernehmen
  pixel-editor export-world <KARTE.tilemap.json> <world.txt>
                                          Karte als world.txt schreiben (Kommentare bleiben erhalten)
  pixel-editor render-animation <DATEI> <TAG> <AUS.png> [--zoom N] [--region NAME|x,y,w,h]
                                          [--track NAME|x,y,w,h] [--jump N]
                                          Animation als Filmstreifen + Onion + Differenz, Analyse auf stdout
";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            // switches without a value
            skip = !matches!(a.as_str(), "--force" | "--no-grid" | "--dry-run");
        } else {
            out.push(a.clone());
        }
    }
    out
}

/// Export a map or a tileset for Godot (and a map as world.txt).
fn cli_export_tiles(file: &Path, target: &str, out: Option<&String>, res_dir: Option<String>) -> Result<(), String> {
    let stem = if tilemap::is_tilemap_path(file) { tilemap::stem(file) } else { io::stem(file) };
    let out = match out {
        Some(d) => PathBuf::from(d),
        None => file.parent().unwrap_or(Path::new(".")).join("export").join(&stem),
    };
    let files = match (tilemap::is_tilemap_path(file), target) {
        (true, "godot") => {
            let map = tilemap::load(file)?;
            let assets = Assets::load(&map, file);
            let ts_name = io::stem(&map.tileset_path(file));
            let (files, warnings) = godot_tiles::export_map(&map, &assets, &ts_name, &out, &stem, res_dir.as_deref())?;
            for w in warnings {
                eprintln!("Hinweis: {w}");
            }
            files
        }
        (true, "world_txt" | "world") => {
            let map = tilemap::load(file)?;
            let (text, warnings) = tilemap::export_world(&map);
            for w in warnings {
                eprintln!("Hinweis: {w}");
            }
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let path = out.join("world.txt");
            io::write_atomic(&path, text.as_bytes())?;
            vec![path]
        }
        (true, t) => return Err(format!("Karten exportieren nach godot oder world_txt, nicht {t:?}")),
        (false, _) => godot_tiles::export_tileset(&Tileset::load(file)?, &out, &stem, res_dir.as_deref())?,
    };
    for f in files {
        println!("{}", f.display());
    }
    Ok(())
}

fn cli_import_world(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let [world, out, ..] = pos.as_slice() else { return Err(USAGE.into()) };
    let ts = flag(args, "--tileset").ok_or("--tileset <TILESET.pxe.json> fehlt")?;
    let out = PathBuf::from(out);
    if !tilemap::is_tilemap_path(&out) {
        return Err(format!("{} muss auf {} enden", out.display(), tilemap::EXTENSION));
    }
    if out.exists() && !args.iter().any(|a| a == "--force") {
        return Err(format!("{} gibt es schon (--force überschreibt)", out.display()));
    }
    let tileset = Tileset::load(Path::new(&ts))?;
    let text = std::fs::read_to_string(world).map_err(|e| format!("{world}: {e}"))?;
    let rel = tilemap::relative_to_map(&out, Path::new(&ts));
    let (map, notes) = tilemap::import_world(&text, &rel, (tileset.tile_w, tileset.tile_h))?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    tilemap::save(&map, &out)?;
    for n in notes.iter().chain(&Assets::load(&map, &out).warnings) {
        eprintln!("Hinweis: {n}");
    }
    println!("{} ({}x{} Kacheln, {} Legenden-Einträge)", out.display(), map.width, map.height, map.legend.len());
    Ok(())
}

fn cli_export_world(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let [map, out, ..] = pos.as_slice() else { return Err(USAGE.into()) };
    let map = tilemap::load(Path::new(map))?;
    let (text, warnings) = tilemap::export_world(&map);
    for w in warnings {
        eprintln!("Hinweis: {w}");
    }
    io::write_atomic(Path::new(out), text.as_bytes())?;
    println!("{out}");
    Ok(())
}

fn cli_export(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    if let [f, t, ..] = pos.as_slice() {
        if tilemap::is_tilemap_path(Path::new(f)) || t == "godot_tileset" {
            return cli_export_tiles(Path::new(f), t, pos.get(2), flag(args, "--godot-res-dir"));
        }
    }
    let (file, target) = match pos.as_slice() {
        [f, t, ..] => (PathBuf::from(f), Target::parse(t)?),
        _ => return Err(USAGE.into()),
    };
    let doc = io::load(&file)?;
    let stem = io::stem(&file);
    let out = match pos.get(2) {
        Some(d) => PathBuf::from(d),
        None => file.parent().unwrap_or(Path::new(".")).join("export").join(&stem),
    };
    let opts = ExportOptions {
        scale: flag(args, "--scale").and_then(|s| s.parse().ok()),
        godot_res_dir: flag(args, "--godot-res-dir"),
        ..Default::default()
    };
    for f in export::export(&doc, target, &out, &stem, &opts)? {
        println!("{}", f.display());
    }
    Ok(())
}

fn cli_preview(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let [file, out, ..] = pos.as_slice() else { return Err(USAGE.into()) };
    if tilemap::is_tilemap_path(Path::new(file)) {
        let map = tilemap::load(Path::new(file))?;
        let assets = Assets::load(&map, Path::new(file));
        for w in &assets.warnings {
            eprintln!("Hinweis: {w}");
        }
        let zoom = flag(args, "--zoom").and_then(|z| z.parse().ok()).unwrap_or(1);
        let grid = !args.iter().any(|a| a == "--no-grid");
        tilemap::preview(&map, &assets, zoom, grid).save_png(Path::new(out))?;
        println!("{out}");
        return Ok(());
    }
    let doc = io::load(Path::new(file))?;
    let frames = match flag(args, "--frame") {
        Some(f) => vec![doc.find_frame(&f)?],
        None => (0..doc.frames.len()).collect(),
    };
    let zoom = flag(args, "--zoom").and_then(|z| z.parse().ok()).unwrap_or(render::PreviewOptions::auto_zoom(&doc));
    let opts = render::PreviewOptions { zoom, grid: true, layer: None, regions: true, tiled: false };
    render::preview(&doc, &frames, &opts).save_png(Path::new(out))?;
    println!("{out}");
    Ok(())
}

fn cli_render_animation(args: &[String]) -> Result<(), String> {
    use pixel_core::animation::{self, AnimOptions, Area};
    let pos = positional(args);
    let [file, tag, out, ..] = pos.as_slice() else { return Err(USAGE.into()) };
    let doc = io::load(Path::new(file))?;
    let t = doc.tags.iter().position(|x| &x.name == tag).ok_or_else(|| {
        format!("no tag {tag:?}; tags: {}", doc.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "))
    })?;
    let area = |f: &str| flag(args, f).map(|s| Area::parse(&s)).transpose();
    let opts = AnimOptions {
        zoom: flag(args, "--zoom").and_then(|z| z.parse().ok()),
        area: area("--region")?,
        track: area("--track")?,
        jump_threshold: flag(args, "--jump").and_then(|j| j.parse().ok()).unwrap_or(2),
    };
    let a = animation::analyze(&doc, t, &opts)?;
    animation::render(&doc, t, &a, &opts)?.save_png(Path::new(out))?;
    print!("{}", animation::summary(&doc, &a));
    println!("{out}");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("mcp") => match args.get(1) {
            Some(dir) => mcp::run(Path::new(dir)).map_err(|e| e.to_string()),
            None => Err("Aufruf: pixel-editor mcp <ARBEITSORDNER>".into()),
        },
        Some("open") => {
            let workdir = flag(&args, "--workdir").map(PathBuf::from);
            // args[i] is the argument before args[1..][i]
            let file = args[1..].iter().enumerate().find(|(i, a)| !a.starts_with("--") && args[*i] != "--workdir");
            match file {
                Some((_, f)) => {
                    launch::open_editor(Path::new(f), args.iter().any(|a| a == "--dry-run"), workdir.as_deref())
                        .map(|m| println!("{m}"))
                }
                None => Err("Aufruf: pixel-editor open <DATEI> [--workdir ORDNER]".into()),
            }
        }
        Some("export") => cli_export(&args[1..]),
        Some("preview") => cli_preview(&args[1..]),
        Some("render-animation") => cli_render_animation(&args[1..]),
        Some("import-world") => cli_import_world(&args[1..]),
        Some("export-world") => cli_export_world(&args[1..]),
        Some("--workdir") => match args.get(1) {
            Some(dir) => gui::run(args.get(2).map(PathBuf::from), Some(PathBuf::from(dir))),
            None => Err(USAGE.into()),
        },
        Some(path) => gui::run(Some(PathBuf::from(path)), None),
        None => gui::run(None, None),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
