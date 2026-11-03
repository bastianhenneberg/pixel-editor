//! MCP tool `lint`: pixel-art craft checks (see `pixel_core::lint`) on the
//! composite or one layer, for one frame, several, a tag or all. Read-only.

use pixel_core::document::Rect;
use pixel_core::lint::{self, Check, Findings};
use pixel_core::TRANSPARENT;
use serde_json::{json, Value};

use super::{index_or_name, opt_int, rect_arg, Out, Server};

impl Server {
    pub(super) fn lint(&self, a: &Value) -> Result<Out, String> {
        let (path, doc, _) = self.load(a)?;
        let scope = if a.get("tag").is_some() {
            "tag"
        } else if a.get("frames").is_some() {
            "frames"
        } else if index_or_name(a.get("frame")).is_some_and(|f| f != "all") {
            "frame"
        } else {
            "all"
        };
        let (frames, label) = self.scope_frames(&doc, a, scope)?;
        let layer = match index_or_name(a.get("layer")) {
            Some(l) => Some(doc.find_layer(&l)?),
            None => None,
        };
        let checks: Vec<Check> = match a.get("checks").and_then(Value::as_array) {
            Some(list) => {
                list.iter().map(|v| Check::parse(v.as_str().ok_or("checks: give names")?)).collect::<Result<_, _>>()?
            }
            None => Check::ALL.to_vec(),
        };
        if checks.is_empty() {
            return Err(format!("checks is empty ({})", Check::ALL.map(Check::id).join(", ")));
        }
        let max = opt_int(a, "max")?.unwrap_or(10).clamp(1, 200) as usize;
        let (w, h) = (doc.width as i32, doc.height as i32);
        let area = |f: usize| -> Result<Option<Rect>, String> {
            match (a.get("region").and_then(Value::as_str), a.get("rect")) {
                (Some(_), Some(_)) => Err("give either region or rect, not both".into()),
                (Some(n), None) => Ok(doc.regions[doc.find_region(n)?].rect_at(f)),
                (None, Some(r)) => Ok(Some(rect_arg(r).map_err(|e| format!("rect: {e}"))?)),
                (None, None) => Ok(None),
            }
        };
        let mut total = Findings::default();
        let (mut body, mut clean) = (String::new(), Vec::new());
        for &f in &frames {
            let rect = area(f)?;
            if a.get("region").is_some() && rect.is_none() {
                continue; // the region is absent on this frame
            }
            let keys: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| match rect {
                    Some(r) if !r.contains(x, y) => TRANSPARENT,
                    _ => match layer {
                        Some(l) => doc.get(l, f, x, y),
                        None => doc.top_key(f, x, y),
                    },
                })
                .collect();
            let found = lint::lint(&keys, doc.width, doc.height, &checks, &doc.ramps, &doc.palette);
            if found.is_empty() {
                clean.push(doc.frame_label(f));
                continue;
            }
            body += &format!(
                "frame {}: {}\n{}",
                doc.frame_label_long(f),
                lint::counts(&found, &checks, !doc.ramps.is_empty()),
                lint::describe(&found, |k| doc.key_label(k), max)
            );
            total.orphans.extend(found.orphans);
            total.doubles.extend(found.doubles);
            total.jaggies.extend(found.jaggies);
            total.bands.extend(found.bands);
            for o in found.off_ramp {
                match total.off_ramp.iter_mut().find(|t| t.key == o.key) {
                    Some(t) => t.count += o.count,
                    None => total.off_ramp.push(o),
                }
            }
        }
        let what = match layer {
            Some(l) => format!("layer {:?}", doc.layers[l].name),
            None => "composite".to_string(),
        };
        let within = match (a.get("region").and_then(Value::as_str), a.get("rect")) {
            (Some(n), _) => format!(" in region {n}"),
            (None, Some(r)) => format!(" in {}", rect_arg(r)?.describe()),
            _ => String::new(),
        };
        let mut text = format!(
            "lint {} {label}, {what}{within}: {}\n",
            self.rel(&path),
            lint::counts(&total, &checks, !doc.ramps.is_empty())
        );
        text += &body;
        if !clean.is_empty() {
            text += &format!("clean: {}\n", clean.join(", "));
        }
        text += "Orphans can be deliberate (eyes, glints); doubles and jaggies are judged on 1-px lines of one key only, shading edges are not lines; banding only sees three shades with parallel edges.";
        Ok(Out::Text(text))
    }
}

pub fn tool_defs() -> Vec<Value> {
    let path =
        json!({"type": "string", "description": "document path relative to the workdir; the extension may be omitted"});
    vec![
        json!({"name": "lint", "description": "Pixel-art craft check, read-only: for one frame, several, a tag or all (default), on the visible composite or one layer, report with coordinates, grouped by kind and capped: orphans (no 4-neighbour of the same key and no diagonal one either, so steps of diagonal lines do not count), doubles (L-corners on 1-px lines where a diagonal step would do; filled areas and real box corners are not flagged), jaggies (uneven step lengths along 1-px lines such as 2-1-3; straight lines of any slope like 2-1-2-1 and smoothly bending curves pass) colors in no ramp (only when the document has ramps) and banding (three shades of one material with a band of constant width whose parallel edges step sideways; dead straight bands, thin straps and a single shade along an outline do not count, so a clean result does not prove there is none). Shades come from the document's ramps, else from color closeness; thin areas that mostly touch a shade of their material are shading edges, not lines, and single pixels in a shading transition are no orphans. The first line sums up.", "inputSchema": {"type": "object", "properties": {
        "path": path,
        "frame": {"type": ["integer", "string"], "description": "one frame (index, name or tag/n); \"all\" or omitted = all frames"},
        "frames": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "these frames"},
        "tag": {"type": "string", "description": "the frames of this tag"},
        "layer": {"type": ["integer", "string"], "description": "only this layer; default the visible composite"},
        "region": {"type": "string", "description": "only inside this region (its rect per frame)"},
        "rect": {"type": "object", "description": "only inside {x, y, w, h}", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}}},
        "checks": {"type": "array", "items": {"type": "string", "enum": ["orphans", "doubles", "jaggies", "ramps", "banding"]}, "description": "which checks; default all"},
        "max": {"type": "integer", "description": "entries listed per kind and frame; default 10"}}, "required": ["path"]}}),
    ]
}
