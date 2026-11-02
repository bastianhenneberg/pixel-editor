//! Godot 4 tile export: the tileset as an atlas PNG plus a `TileSet`
//! resource, a map as a scene with a `TileMapLayer` (and a `Sprite2D` for
//! every object standing on a cell).
//!
//! Atlas: tile i of the tileset (frame i) sits at column i % 8, row i / 8.
//! Every tile carries its frame name in the custom data layer `name`, so
//! game code can find tiles by name (`get_custom_data("name")`).
//!
//! `TileMapLayer.tile_map_data` is Godot's packed cell list: a u16 format
//! version (0), then per cell i16 x, i16 y, u16 source, u16 atlas x,
//! u16 atlas y, u16 alternative, all little endian.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::render::Image;
use crate::tilemap::{Assets, TileMap, Tileset, EMPTY};

pub const ATLAS_COLUMNS: u32 = 8;

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn res_path(name: &str, res_dir: Option<&str>) -> String {
    match res_dir {
        Some(dir) => format!("{}/{name}", dir.trim_end_matches('/')),
        None => name.to_string(),
    }
}

/// Atlas coordinates of tile `i`.
pub fn atlas_coords(i: usize) -> (u32, u32) {
    (i as u32 % ATLAS_COLUMNS, i as u32 / ATLAS_COLUMNS)
}

pub fn atlas_image(ts: &Tileset) -> Image {
    let n = ts.images.len().max(1) as u32;
    let cols = n.min(ATLAS_COLUMNS);
    let rows = n.div_ceil(ATLAS_COLUMNS);
    let mut img = Image::new(cols * ts.tile_w, rows * ts.tile_h);
    for (i, t) in ts.images.iter().enumerate() {
        let (c, r) = atlas_coords(i);
        img.paste(t, c * ts.tile_w, r * ts.tile_h);
    }
    img
}

/// The `TileSet` resource text for an atlas named `png_name`.
pub fn tileset_tres(ts: &Tileset, png_name: &str, res_dir: Option<&str>) -> String {
    let mut out = String::new();
    writeln!(out, "[gd_resource type=\"TileSet\" load_steps=3 format=3]\n").unwrap();
    writeln!(out, "[ext_resource type=\"Texture2D\" path={} id=\"1_atlas\"]\n", quote(&res_path(png_name, res_dir)))
        .unwrap();
    writeln!(out, "[sub_resource type=\"TileSetAtlasSource\" id=\"TileSetAtlasSource_0\"]").unwrap();
    writeln!(out, "texture = ExtResource(\"1_atlas\")").unwrap();
    writeln!(out, "texture_region_size = Vector2i({}, {})", ts.tile_w, ts.tile_h).unwrap();
    for (i, name) in ts.names.iter().enumerate() {
        let (c, r) = atlas_coords(i);
        writeln!(out, "{c}:{r}/0 = 0").unwrap();
        writeln!(out, "{c}:{r}/0/custom_data_0 = {}", quote(name)).unwrap();
    }
    writeln!(out, "\n[resource]").unwrap();
    writeln!(out, "tile_size = Vector2i({}, {})", ts.tile_w, ts.tile_h).unwrap();
    writeln!(out, "custom_data_layer_0/name = \"name\"").unwrap();
    writeln!(out, "custom_data_layer_0/type = 4").unwrap();
    writeln!(out, "sources/0 = SubResource(\"TileSetAtlasSource_0\")").unwrap();
    out
}

/// Write `<name>_atlas.png` and `<name>_tileset.tres` into `out_dir`.
pub fn export_tileset(ts: &Tileset, out_dir: &Path, name: &str, res_dir: Option<&str>) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let png = format!("{name}_atlas.png");
    let png_path = out_dir.join(&png);
    atlas_image(ts).save_png(&png_path)?;
    let tres_path = out_dir.join(format!("{name}_tileset.tres"));
    crate::io::write_atomic(&tres_path, tileset_tres(ts, &png, res_dir).as_bytes())?;
    Ok(vec![png_path, tres_path])
}

/// Godot's packed `tile_map_data` for the cells of `map` that show a tile
/// of the tileset; returns the bytes and the keys that were left out.
pub fn tile_map_data(map: &TileMap, ts: &Tileset) -> (Vec<u8>, Vec<u8>) {
    let mut bytes = vec![0u8, 0u8];
    let mut skipped = Vec::new();
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            let k = map.get(x, y);
            if k == EMPTY {
                continue;
            }
            let Some(i) = map.entry(k).and_then(|e| e.tile.as_deref()).and_then(|t| ts.index(t)) else {
                // an object alone has no tile; anything else is a placeholder
                if map.entry(k).is_none_or(|e| e.object.is_none() || e.tile.is_some()) && !skipped.contains(&k) {
                    skipped.push(k);
                }
                continue;
            };
            let (c, r) = atlas_coords(i);
            for v in [x as i16 as u16, y as i16 as u16, 0, c as u16, r as u16, 0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    (bytes, skipped)
}

/// Export a map: the tileset (atlas + `TileSet`), object PNGs and
/// `<stem>.tscn`. Returns the written files and warnings.
pub fn export_map(
    map: &TileMap,
    assets: &Assets,
    tileset_name: &str,
    out_dir: &Path,
    stem: &str,
    res_dir: Option<&str>,
) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    if assets.tileset.images.is_empty() {
        return Err(format!("the tileset {} could not be loaded: {}", map.tileset, assets.warnings.join("; ")));
    }
    let mut files = export_tileset(&assets.tileset, out_dir, tileset_name, res_dir)?;
    let mut warnings = assets.warnings.clone();
    let (data, skipped) = tile_map_data(map, &assets.tileset);
    if !skipped.is_empty() {
        let keys: Vec<String> = skipped.iter().map(|k| format!("{:?}", *k as char)).collect();
        warnings.push(format!("cells without a tileset tile left empty: {}", keys.join(", ")));
    }
    // object textures, one per legend entry
    let mut objects: Vec<(u8, String, String)> = Vec::new(); // key, png name, resource id
    for e in &map.legend {
        let (Some(o), Some(obj)) = (&e.object, assets.objects.get(&e.key)) else { continue };
        let png = format!("{}.png", crate::io::stem(Path::new(o)));
        if !objects.iter().any(|(_, p, _)| *p == png) {
            let path = out_dir.join(&png);
            obj.image.save_png(&path)?;
            files.push(path);
        }
        let id = format!("{}_obj", objects.len() + 2);
        objects.push((e.key, png, id));
    }
    let mut out = String::new();
    writeln!(out, "[gd_scene load_steps={} format=3]\n", 2 + objects.len()).unwrap();
    let tres = format!("{tileset_name}_tileset.tres");
    writeln!(out, "[ext_resource type=\"TileSet\" path={} id=\"1_tileset\"]", quote(&res_path(&tres, res_dir)))
        .unwrap();
    for (_, png, id) in &objects {
        writeln!(out, "[ext_resource type=\"Texture2D\" path={} id=\"{id}\"]", quote(&res_path(png, res_dir))).unwrap();
    }
    writeln!(out, "\n[node name={} type=\"Node2D\"]", quote(stem)).unwrap();
    writeln!(out, "texture_filter = 1").unwrap();
    writeln!(out, "\n[node name=\"Tiles\" type=\"TileMapLayer\" parent=\".\"]").unwrap();
    let nums: Vec<String> = data.iter().map(u8::to_string).collect();
    writeln!(out, "tile_map_data = PackedByteArray({})", nums.join(", ")).unwrap();
    writeln!(out, "tile_set = ExtResource(\"1_tileset\")").unwrap();
    if !objects.is_empty() {
        writeln!(out, "\n[node name=\"Objects\" type=\"Node2D\" parent=\".\"]").unwrap();
        writeln!(out, "y_sort_enabled = true").unwrap();
        for y in 0..map.height as i32 {
            for x in 0..map.width as i32 {
                let k = map.get(x, y);
                let Some((_, _, id)) = objects.iter().find(|(key, _, _)| *key == k) else { continue };
                let obj = &assets.objects[&k];
                let foot = (x * map.tile_w as i32 + map.tile_w as i32 / 2, (y + 1) * map.tile_h as i32);
                writeln!(out, "\n[node name=\"{}_{x}_{y}\" type=\"Sprite2D\" parent=\"Objects\"]", k as char).unwrap();
                writeln!(out, "position = Vector2({}, {})", foot.0, foot.1).unwrap();
                writeln!(out, "texture = ExtResource(\"{id}\")").unwrap();
                writeln!(out, "centered = false").unwrap();
                writeln!(out, "offset = Vector2({}, {})", -obj.anchor.0, -obj.anchor.1).unwrap();
            }
        }
    }
    let scene = out_dir.join(format!("{stem}.tscn"));
    crate::io::write_atomic(&scene, out.as_bytes())?;
    files.push(scene);
    Ok((files, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Rect};
    use crate::tilemap::{LegendEntry, Object};

    fn tileset() -> Tileset {
        let mut d = Document::new(2, 2, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        for i in 1..10 {
            d.add_frame(i, None, false).unwrap();
        }
        for (i, f) in d.frames.iter_mut().enumerate() {
            f.name = format!("t{i}");
        }
        Tileset::from_document(&d)
    }

    #[test]
    fn atlas_and_tres() {
        let ts = tileset();
        let img = atlas_image(&ts);
        assert_eq!((img.width, img.height), (16, 4)); // 10 tiles: 8 + 2
        let t = tileset_tres(&ts, "a_atlas.png", Some("res://tiles"));
        assert!(t.contains("path=\"res://tiles/a_atlas.png\""));
        assert!(t.contains("1:1/0/custom_data_0 = \"t9\""));
        assert!(t.contains("texture_region_size = Vector2i(2, 2)"));
    }

    #[test]
    fn packed_cells() {
        let ts = tileset();
        let mut m = TileMap::new("t", 2, 2, 3, 2).unwrap();
        let k = m.key_for_tile("t9").unwrap();
        m.fill_rect(Rect::new(0, 0, 1, 1), k);
        m.legend.push(LegendEntry { key: b'?', tile: None, object: None, name: String::new() });
        m.set(2, 1, b'?');
        let (bytes, skipped) = tile_map_data(&m, &ts);
        assert_eq!(bytes, vec![0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0]);
        assert_eq!(skipped, vec![b'?']);
    }

    #[test]
    fn scene_with_objects() {
        let dir = std::env::temp_dir().join(format!("pxe-godot-tiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = TileMap::new("t", 2, 2, 2, 2).unwrap();
        m.legend.push(LegendEntry {
            key: b'T',
            tile: Some("t0".into()),
            object: Some("tree.pxe.json".into()),
            name: String::new(),
        });
        m.set(1, 1, b'T');
        let mut assets = Assets { tileset: tileset(), ..Default::default() };
        let obj = Document::new(4, 4, Document::builtin_palette("sweetie16").unwrap()).unwrap();
        assets.objects.insert(b'T', Object::from_document(&obj));
        let (files, _) = export_map(&m, &assets, "tiles", &dir, "world", None).unwrap();
        let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["tiles_atlas.png", "tiles_tileset.tres", "tree.png", "world.tscn"]);
        let scene = std::fs::read_to_string(dir.join("world.tscn")).unwrap();
        assert!(scene.contains("[node name=\"T_1_1\" type=\"Sprite2D\" parent=\"Objects\"]"));
        assert!(scene.contains("position = Vector2(3, 4)"));
        assert!(scene.contains("offset = Vector2(-2, -4)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
