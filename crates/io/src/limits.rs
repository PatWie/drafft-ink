//! Hard limits on input handled by this crate. Scene limits (elements, points, text length) are in
//! `drafftink_scene::limits` and apply on top.

/// Largest text/JSON input a parser accepts, in bytes.
pub const MAX_INPUT_BYTES: usize = 512 * 1024 * 1024;
/// Largest size a gzip stream may expand to, in bytes. Stops decompression bombs.
pub const MAX_INFLATED_BYTES: usize = 1024 * 1024 * 1024;
/// Most warnings an [`crate::ImportReport`] keeps.
pub const MAX_WARNINGS: usize = 200;
/// Most items one library file may hold.
pub const MAX_LIBRARY_ITEMS: usize = 10_000;
/// Most nodes and edges in an imported diagram.
pub const MAX_DIAGRAM_ITEMS: usize = 10_000;
/// Most text lines a diagram parser reads.
pub const MAX_DIAGRAM_LINES: usize = 100_000;
/// Largest PNG a text-chunk reader scans, in bytes.
pub const MAX_PNG_BYTES: usize = 512 * 1024 * 1024;
/// Largest scene JSON a PNG or SVG text chunk may carry, in bytes, after inflation.
pub const MAX_EMBEDDED_SCENE_BYTES: usize = 256 * 1024 * 1024;
