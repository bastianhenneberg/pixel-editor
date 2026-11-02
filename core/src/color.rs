//! RGBA colors and hex parsing.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba([0, 0, 0, 0]);

    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba([r, g, b, a])
    }

    /// Parse `#rgb`, `#rrggbb` or `#rrggbbaa` (leading `#` optional).
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim();
        let h = t.strip_prefix('#').unwrap_or(t);
        let expanded: String = match h.len() {
            3 => h.chars().flat_map(|c| [c, c]).chain("ff".chars()).collect(),
            6 => format!("{h}ff"),
            8 => h.to_string(),
            _ => return Err(format!("bad color {text:?}: expected #rgb, #rrggbb or #rrggbbaa")),
        };
        let mut out = [0u8; 4];
        for (i, v) in out.iter_mut().enumerate() {
            *v = u8::from_str_radix(&expanded[i * 2..i * 2 + 2], 16)
                .map_err(|_| format!("bad color {text:?}: not hex"))?;
        }
        Ok(Rgba(out))
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    pub fn a(self) -> u8 {
        self.0[3]
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Straight-alpha "source over" blend of `src` (scaled by `opacity`) onto `dst`.
pub fn blend_over(dst: [u8; 4], src: [u8; 4], opacity: f32) -> [u8; 4] {
    let sa = src[3] as f32 / 255.0 * opacity.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return dst;
    }
    let da = dst[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let c = (src[i] as f32 * sa + dst[i] as f32 * da * (1.0 - sa)) / oa;
        out[i] = c.round().clamp(0.0, 255.0) as u8;
    }
    out[3] = (oa * 255.0).round() as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        assert_eq!(Rgba::parse("#fff").unwrap(), Rgba([255, 255, 255, 255]));
        assert_eq!(Rgba::parse("#1a1c2c").unwrap(), Rgba([0x1a, 0x1c, 0x2c, 255]));
        assert_eq!(Rgba::parse("1a1c2c80").unwrap(), Rgba([0x1a, 0x1c, 0x2c, 0x80]));
        assert!(Rgba::parse("#12345").is_err());
        assert!(Rgba::parse("#gggggg").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        for s in ["#1a1c2c", "#ff000080"] {
            assert_eq!(Rgba::parse(s).unwrap().to_hex(), s);
        }
    }

    #[test]
    fn blend() {
        assert_eq!(blend_over([0, 0, 0, 0], [10, 20, 30, 255], 1.0), [10, 20, 30, 255]);
        assert_eq!(blend_over([255, 0, 0, 255], [0, 0, 255, 255], 0.0), [255, 0, 0, 255]);
        let half = blend_over([255, 255, 255, 255], [0, 0, 0, 255], 0.5);
        assert_eq!(half, [128, 128, 128, 255]);
    }
}
