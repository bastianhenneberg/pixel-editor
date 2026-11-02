//! Unity: a `.png.meta` that slices the sheet (Sprite Mode Multiple, Point
//! filter, no compression) plus a JSON with animation timings.
//!
//! Unity's sprite rects have their origin at the bottom-left, so y is flipped.
//! GUIDs and IDs are derived from names, so re-exporting keeps references
//! stable inside a Unity project.

use std::fmt::Write;

use serde_json::{json, Map, Value};

use super::{fnv64, Sheet};
use crate::document::Document;

/// Sprite name per frame: `<tag>_<n>` for the first tag containing the
/// frame (n = position within that tag), otherwise the frame name.
pub fn sprite_names(doc: &Document, sheet: &Sheet) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (i, c) in sheet.cells.iter().enumerate() {
        // frames sharing a sheet region (identical images) share the sprite
        if let Some(j) = sheet.cells[..i].iter().position(|o| (o.x, o.y) == (c.x, c.y)) {
            let name = names[j].clone();
            names.push(name);
            continue;
        }
        let base = doc
            .tags
            .iter()
            .find_map(|t| t.contains(c.frame).then(|| format!("{}_{}", t.name, c.frame - t.from)))
            .unwrap_or_else(|| c.name.clone());
        let mut name = base.clone();
        let mut i = 2;
        while names.contains(&name) {
            name = format!("{base}_{i}");
            i += 1;
        }
        names.push(name);
    }
    names
}

fn guid(seed: &str) -> String {
    format!("{:016x}{:016x}", fnv64(seed), fnv64(&format!("{seed}#2")))
}

fn internal_id(seed: &str) -> i64 {
    // positive, non-zero, like Unity's generated IDs
    ((fnv64(seed) >> 2) as i64).max(1)
}

/// YAML scalar: quote names that could be misread.
fn yaml_str(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !s.is_empty() {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

pub fn texture_meta(sheet: &Sheet, names: &[String], stem: &str, ppu: u32) -> String {
    let mut sprites = String::new();
    let mut table = String::new();
    for (i, (c, name)) in sheet.cells.iter().zip(names).enumerate() {
        if names[..i].contains(name) {
            continue; // same region as an earlier frame
        }
        let id = internal_id(&format!("{stem}/{name}"));
        let y = sheet.image.height - c.y - c.h;
        write!(
            sprites,
            "    - serializedVersion: 2
      name: {n}
      rect:
        serializedVersion: 2
        x: {x}
        y: {y}
        width: {w}
        height: {h}
      alignment: 0
      pivot: {{x: 0.5, y: 0.5}}
      border: {{x: 0, y: 0, z: 0, w: 0}}
      customData:
      outline: []
      physicsShape: []
      tessellationDetail: -1
      bones: []
      spriteID: {sid}
      internalID: {id}
      vertices: []
      indices:
      edges: []
      weights: []
",
            n = yaml_str(name),
            x = c.x,
            w = c.w,
            h = c.h,
            sid = guid(&format!("{stem}/{name}/sprite")),
        )
        .unwrap();
        writeln!(table, "      {}: {id}", yaml_str(name)).unwrap();
    }
    format!(
        "fileFormatVersion: 2
guid: {guid}
TextureImporter:
  internalIDToNameTable: []
  externalObjects: {{}}
  serializedVersion: 13
  mipmaps:
    mipMapMode: 0
    enableMipMap: 0
    sRGBTexture: 1
    linearTexture: 0
    fadeOut: 0
    borderMipMap: 0
    mipMapsPreserveCoverage: 0
    alphaTestReferenceValue: 0.5
    mipMapFadeDistanceStart: 1
    mipMapFadeDistanceEnd: 3
  bumpmap:
    convertToNormalMap: 0
    externalNormalMap: 0
    heightScale: 0.25
    normalMapFilter: 0
    flipGreenChannel: 0
  isReadable: 0
  streamingMipmaps: 0
  streamingMipmapsPriority: 0
  vTOnly: 0
  ignoreMipmapLimit: 0
  grayScaleToAlpha: 0
  generateCubemap: 6
  cubemapConvolution: 0
  seamlessCubemap: 0
  textureFormat: 1
  maxTextureSize: 2048
  textureSettings:
    serializedVersion: 2
    filterMode: 0
    aniso: 1
    mipBias: 0
    wrapU: 1
    wrapV: 1
    wrapW: 1
  nPOTScale: 0
  lightmap: 0
  compressionQuality: 50
  spriteMode: 2
  spriteExtrude: 1
  spriteMeshType: 0
  alignment: 0
  spritePivot: {{x: 0.5, y: 0.5}}
  spritePixelsToUnits: {ppu}
  spriteBorder: {{x: 0, y: 0, z: 0, w: 0}}
  spriteGenerateFallbackPhysicsShape: 1
  alphaUsage: 1
  alphaIsTransparency: 1
  spriteTessellationDetail: -1
  textureType: 8
  textureShape: 1
  singleChannelComponent: 0
  flipbookRows: 1
  flipbookColumns: 1
  maxTextureSizeSet: 0
  compressionQualitySet: 0
  textureFormatSet: 0
  ignorePngGamma: 0
  applyGammaDecoding: 0
  swizzle: 50462976
  cookieLightType: 0
  platformSettings:
  - serializedVersion: 4
    buildTarget: DefaultTexturePlatform
    maxTextureSize: 2048
    resizeAlgorithm: 0
    textureFormat: -1
    textureCompression: 0
    compressionQuality: 50
    crunchedCompression: 0
    allowsAlphaSplitting: 0
    overridden: 0
    ignorePlatformSupport: 0
    androidETC2FallbackOverride: 0
    forceMaximumCompressionQuality_BC6H_BC7: 0
  spriteSheet:
    serializedVersion: 2
    sprites:
{sprites}    outline: []
    customData:
    physicsShape: []
    bones: []
    spriteID:
    internalID: 0
    vertices: []
    indices:
    edges: []
    weights: []
    secondaryTextures: []
    spriteCustomMetadata:
      entries: []
    nameFileIdTable:
{table}  mipmapLimitGroupName:
  pSDRemoveMatte: 0
  userData:
  assetBundleName:
  assetBundleVariant:
",
        guid = guid(&format!("{stem}.png")),
    )
}

/// Animation timings keyed by sprite name, for building AnimationClips.
pub fn anim_json(sheet: &Sheet, names: &[String], png_name: &str) -> Value {
    let mut anims = Map::new();
    for a in &sheet.anims {
        let steps: Vec<Value> = a.steps.iter().map(|(f, ms)| json!({"sprite": names[*f], "ms": ms})).collect();
        anims.insert(a.name.clone(), json!({"steps": steps, "loop": a.looping}));
    }
    json!({
        "image": png_name,
        "frameWidth": sheet.frame_w,
        "frameHeight": sheet.frame_h,
        "sprites": names,
        "anims": anims,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{build_sheet, tests::sample};

    #[test]
    fn names_and_meta() {
        let d = sample();
        let s = build_sheet(&d, 1, Some(2));
        let names = sprite_names(&d, &s);
        assert_eq!(names, ["idle_0", "idle_1", "hit_2"]);
        let meta = texture_meta(&s, &names, "hero", 32);
        assert!(meta.contains("spriteMode: 2"));
        assert!(meta.contains("filterMode: 0"));
        assert!(meta.contains("textureCompression: 0"));
        // frame 2 is on the second row; Unity's y is bottom-up -> 0
        let hit = meta.split("name: hit_2").nth(1).unwrap();
        assert!(hit.contains("x: 0\n        y: 0\n"));
        let idle = meta.split("name: idle_0").nth(1).unwrap();
        assert!(idle.contains("y: 4\n"));
        assert_eq!(meta, texture_meta(&s, &names, "hero", 32), "deterministic");
        let j = anim_json(&s, &names, "hero.png");
        assert_eq!(j["anims"]["hit"]["steps"][2]["sprite"], "hit_2");
    }
}
