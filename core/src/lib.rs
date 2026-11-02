//! pixel-core: everything the pixel editor does that is not GUI.
//! Shared by the egui app and the MCP server.

pub mod animation;
pub mod aseprite;
pub mod color;
pub mod diff;
pub mod document;
pub mod draw;
pub mod export;
pub mod font;
pub mod history;
pub mod import;
pub mod io;
pub mod journal;
pub mod palettes;
pub mod recolor;
pub mod render;
pub mod seams;
pub mod tilemap;
pub mod transform;

pub use color::Rgba;
pub use document::{Document, Frame, Layer, PaletteEntry, Ramp, Rect, Region, Tag, SKIP, TRANSPARENT};
pub use history::History;
pub use render::Image;
