//! Built-in palettes (same keys as pixelart-studio's `.pxs` palettes).

pub const BUILTIN: &[(&str, &[(char, &str)])] = &[
    // Sweetie 16 by GrafxKid, https://lospec.com/palette-list/sweetie-16
    // (published for free use; no formal license stated)
    (
        "sweetie16",
        &[
            ('0', "#1a1c2c"),
            ('1', "#5d275d"),
            ('2', "#b13e53"),
            ('3', "#ef7d57"),
            ('4', "#ffcd75"),
            ('5', "#a7f070"),
            ('6', "#38b764"),
            ('7', "#257179"),
            ('8', "#29366f"),
            ('9', "#3b5dc9"),
            ('a', "#41a6f6"),
            ('b', "#73eff7"),
            ('c', "#f4f4f4"),
            ('d', "#94b0c2"),
            ('e', "#566c86"),
            ('f', "#333c57"),
        ],
    ),
    // PICO-8 fantasy console by Lexaloffle Games, standard indices from the
    // PICO-8 manual, https://www.lexaloffle.com/pico-8.php (no formal license stated)
    (
        "pico8",
        &[
            ('0', "#000000"),
            ('1', "#1d2b53"),
            ('2', "#7e2553"),
            ('3', "#008751"),
            ('4', "#ab5236"),
            ('5', "#5f574f"),
            ('6', "#c2c3c7"),
            ('7', "#fff1e8"),
            ('8', "#ff004d"),
            ('9', "#ffa300"),
            ('a', "#ffec27"),
            ('b', "#00e436"),
            ('c', "#29adff"),
            ('d', "#83769c"),
            ('e', "#ff77a8"),
            ('f', "#ffccaa"),
        ],
    ),
    // Original Game Boy greens, dark to light: common community approximation
    // of the DMG screen (no single author, no license stated)
    ("gameboy", &[('0', "#0f380f"), ('1', "#306230"), ('2', "#8bac0f"), ('3', "#9bbc0f")]),
    // 1-bit, self-made
    ("mono", &[('0', "#000000"), ('1', "#ffffff")]),
];

pub fn names() -> Vec<&'static str> {
    BUILTIN.iter().map(|(n, _)| *n).collect()
}

pub fn get(name: &str) -> Option<&'static [(char, &'static str)]> {
    BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)
}
