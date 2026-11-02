//! Animated GIF per animation, with exact colors and real transparency.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use super::anims;
use crate::document::Document;
use crate::render::{composite, Image};

pub fn export(doc: &Document, out_dir: &Path, stem: &str, scale: u32) -> Result<Vec<PathBuf>, String> {
    let scale = scale.clamp(1, 64);
    let mut files = Vec::new();
    for a in anims(doc) {
        let frames: Vec<(Image, u32)> = a.steps.iter().map(|(f, ms)| (composite(doc, *f).scaled(scale), *ms)).collect();
        let path = out_dir.join(format!("{stem}_{}.gif", a.name));
        let bytes = encode(&frames, a.looping)?;
        crate::io::write_atomic(&path, &bytes)?;
        files.push(path);
    }
    Ok(files)
}

/// Encode frames; alpha < 128 becomes transparent (index 0). Uses one
/// global palette when the animation has at most 255 colors, otherwise
/// falls back to per-frame quantisation.
pub fn encode(frames: &[(Image, u32)], looping: bool) -> Result<Vec<u8>, String> {
    let (w, h) = (frames[0].0.width, frames[0].0.height);
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err("image too large for GIF".into());
    }
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut exact = true;
    'scan: for (img, _) in frames {
        for p in img.pixels.as_chunks::<4>().0 {
            if p[3] >= 128 {
                let c = [p[0], p[1], p[2]];
                if !colors.contains(&c) {
                    if colors.len() == 255 {
                        exact = false;
                        break 'scan;
                    }
                    colors.push(c);
                }
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut global = vec![0u8, 0, 0];
        if exact {
            global.extend(colors.iter().flatten());
        }
        let mut enc = gif::Encoder::new(&mut out, w as u16, h as u16, if exact { &global } else { &[] })
            .map_err(|e| e.to_string())?;
        enc.set_repeat(if looping { gif::Repeat::Infinite } else { gif::Repeat::Finite(0) })
            .map_err(|e| e.to_string())?;
        for (img, ms) in frames {
            let mut frame = if exact {
                let buf: Vec<u8> = img
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| {
                        if p[3] >= 128 {
                            colors.iter().position(|c| *c == [p[0], p[1], p[2]]).unwrap() as u8 + 1
                        } else {
                            0
                        }
                    })
                    .collect();
                gif::Frame {
                    width: w as u16,
                    height: h as u16,
                    buffer: Cow::Owned(buf),
                    transparent: Some(0),
                    ..Default::default()
                }
            } else {
                let mut px = img.pixels.clone();
                for p in px.as_chunks_mut::<4>().0 {
                    p[3] = if p[3] >= 128 { 255 } else { 0 };
                }
                gif::Frame::from_rgba_speed(w as u16, h as u16, &mut px, 10)
            };
            frame.delay = ((*ms + 5) / 10).max(1) as u16;
            frame.dispose = gif::DisposalMethod::Background;
            enc.write_frame(&frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_with_exact_colors() {
        let mut a = Image::new(2, 1);
        a.put(0, 0, [10, 20, 30, 255]);
        let mut b = Image::new(2, 1);
        b.put(1, 0, [200, 100, 50, 255]);
        let bytes = encode(&[(a, 120), (b, 250)], true).unwrap();
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(&bytes[..]).unwrap();
        let f0 = dec.read_next_frame().unwrap().unwrap().clone();
        assert_eq!(f0.delay, 12);
        assert_eq!(&f0.buffer[..], &[10, 20, 30, 255, 0, 0, 0, 0]);
        let f1 = dec.read_next_frame().unwrap().unwrap();
        assert_eq!(f1.delay, 25);
        assert_eq!(&f1.buffer[4..], &[200, 100, 50, 255]);
    }
}
