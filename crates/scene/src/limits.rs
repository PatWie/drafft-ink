//! Hard limits applied to every document, so that hostile input cannot exhaust memory or stall a loop.

/// Most elements in one document, tombstones included.
pub const MAX_ELEMENTS: usize = 1_000_000;
/// Most points in one line, arrow or freehand stroke.
pub const MAX_POINTS: usize = 200_000;
/// Most characters in one text element.
pub const MAX_TEXT_CHARS: usize = 1_000_000;
/// Most characters in one LaTeX source.
pub const MAX_LATEX_CHARS: usize = 100_000;
/// Longest element name or frame name, in characters.
pub const MAX_NAME_CHARS: usize = 256;
/// Longest hyperlink, in bytes.
pub const MAX_LINK_BYTES: usize = 2048;
/// Deepest group nesting on one element.
pub const MAX_GROUP_DEPTH: usize = 16;
/// Most keys in an element's `custom` map.
pub const MAX_CUSTOM_KEYS: usize = 256;
/// Largest image blob, in bytes.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Most blobs in one document.
pub const MAX_FILES: usize = 10_000;
/// Largest coordinate or size magnitude, in world units.
pub const MAX_COORD: f64 = 1.0e9;
/// Largest stroke width, in world units.
pub const MAX_STROKE_WIDTH: f64 = 1000.0;
/// Largest font size, in world units.
pub const MAX_FONT_SIZE: f64 = 10_000.0;
/// Largest corner radius, in world units.
pub const MAX_CORNER_RADIUS: f64 = 1.0e6;
/// Largest `fixed_point` ratio magnitude of a binding.
pub const MAX_FIXED_POINT: f64 = 100.0;
/// Tolerance when comparing a linear element's box with the bounding box of its points.
pub const BOX_TOLERANCE: f64 = 1.0e-4;
