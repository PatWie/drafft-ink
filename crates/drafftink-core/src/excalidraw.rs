//! Excalidraw element conversion and `.excalidrawlib` library import.
//!
//! Two responsibilities live here:
//!
//! * [`element_to_shape`] converts a single Excalidraw element (as parsed JSON)
//!   into a native drafft [`Shape`]. It is shared by scene import
//!   ([`crate::canvas::CanvasDocument::from_excalidraw`]) and by library import
//!   so the two never drift apart.
//! * [`library_from_excalidrawlib`] parses an Excalidraw *library* file
//!   (`type: "excalidrawlib"`, v1 or v2) into a list of named [`LibraryItem`]s.
//!   Each item preserves Excalidraw's internal grouping: an AWS icon, for
//!   example, imports as one outer group whose children are the label text and
//!   the icon's own sub-group, mirroring how it was authored.
//!
//! # Grounding for reviewers
//!
//! CRITICAL PATH: this parser consumes untrusted file/clipboard content. It must
//! never panic on arbitrary input — unconvertible elements are skipped and a
//! non-library document returns `None`.

use crate::shapes::{
    Arrow, Ellipse, FillPattern, Freehand, Group, Line, PathStyle, Rectangle, SerializableColor,
    Shape, ShapeStyle, Sloppiness, Text,
};
use kurbo::Point;
use serde_json::Value;

/// Distance (world units) under which a poly-line's first and last point are
/// treated as coincident, closing the path.
const CLOSED_PATH_THRESHOLD: f64 = 10.0;

/// Default element size (world units) when Excalidraw omits width/height.
const DEFAULT_ELEMENT_SIZE: f64 = 100.0;

/// Default Excalidraw font size (pixels) when unspecified.
const DEFAULT_FONT_SIZE: f64 = 20.0;

/// A single reusable icon/stencil parsed from a library file: a display name
/// and one native [`Shape`] (always a [`Shape::Group`]) preserving the item's
/// internal grouping.
#[derive(Debug, Clone)]
pub struct LibraryItem {
    /// Human-readable name shown in the library UI.
    pub name: String,
    /// The item's shapes, wrapped in a single group.
    pub shape: Shape,
}

// -----------------------------------------------------------------------------
// Library parsing.
// -----------------------------------------------------------------------------

/// Parse an Excalidraw library file into named items, or `None` if the text is
/// not a recognizable `excalidrawlib` document. Supports the v2 `libraryItems`
/// layout and the legacy v1 `library` array-of-arrays layout.
pub fn library_from_excalidrawlib(json: &str) -> Option<Vec<LibraryItem>> {
    let data: Value = serde_json::from_str(json).ok()?;
    if data.get("type").and_then(Value::as_str) != Some("excalidrawlib") {
        return None;
    }

    let mut items = Vec::new();
    if let Some(v2) = data.get("libraryItems").and_then(Value::as_array) {
        for (i, entry) in v2.iter().enumerate() {
            if let Some(item) = parse_library_item_v2(entry, i) {
                items.push(item);
            }
        }
    } else if let Some(v1) = data.get("library").and_then(Value::as_array) {
        // v1: an array of element-arrays, one per item, without names.
        for (i, elements) in v1.iter().enumerate() {
            if let Some(elements) = elements.as_array() {
                if let Some(shape) = item_shape_from_elements(elements) {
                    items.push(LibraryItem {
                        name: format!("Item {}", i + 1),
                        shape,
                    });
                }
            }
        }
    }

    (!items.is_empty()).then_some(items)
}

/// Padding (world units) added around each item within its grid cell.
const GRID_CELL_PADDING: f64 = 24.0;

/// Maximum number of columns used when arranging a library as a grid.
const GRID_MAX_COLUMNS: usize = 10;

/// Arrange library items into a uniform grid for display on a canvas.
///
/// Each item's group is cloned and translated so its bounding box is centered
/// in a grid cell sized to the largest item. Items are placed row-major. The
/// returned shapes are ready to add to a fresh document (e.g. a "library" tab).
pub fn library_layout_grid(items: &[LibraryItem]) -> Vec<Shape> {
    if items.is_empty() {
        return Vec::new();
    }

    // Cell size is driven by the largest item so every icon fits its cell.
    let mut cell_w: f64 = 0.0;
    let mut cell_h: f64 = 0.0;
    for item in items {
        let b = item.shape.bounds();
        cell_w = cell_w.max(b.width());
        cell_h = cell_h.max(b.height());
    }
    cell_w += 2.0 * GRID_CELL_PADDING;
    cell_h += 2.0 * GRID_CELL_PADDING;

    // Choose a roughly square column count, capped for a browsable width.
    let columns = (items.len() as f64).sqrt().ceil() as usize;
    let columns = columns.clamp(1, GRID_MAX_COLUMNS);

    let mut shapes = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let row = i / columns;
        let col = i % columns;
        let cell_center = Point::new(
            col as f64 * cell_w + cell_w / 2.0,
            row as f64 * cell_h + cell_h / 2.0,
        );

        let mut shape = item.shape.clone();
        let bounds = shape.bounds();
        let current_center =
            Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let offset = kurbo::Vec2::new(
            cell_center.x - current_center.x,
            cell_center.y - current_center.y,
        );
        shape.transform(kurbo::Affine::translate(offset));
        shapes.push(shape);
    }
    shapes
}

/// Parse a v2 `libraryItems` entry `{ name, elements: [...] }`.
fn parse_library_item_v2(entry: &Value, index: usize) -> Option<LibraryItem> {
    let elements = entry.get("elements").and_then(Value::as_array)?;
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Item {}", index + 1));
    let shape = item_shape_from_elements(elements)?;
    Some(LibraryItem { name, shape })
}

/// Convert a library item's elements into one grouped [`Shape`], preserving the
/// Excalidraw group hierarchy. Returns `None` if nothing is convertible.
fn item_shape_from_elements(elements: &[Value]) -> Option<Shape> {
    // Convert each element, carrying its group path (innermost -> outermost).
    let mut entries: Vec<(Shape, Vec<String>)> = Vec::new();
    for elem in elements {
        if let Some(shape) = element_to_shape(elem) {
            entries.push((shape, element_group_ids(elem)));
        }
    }
    if entries.is_empty() {
        return None;
    }

    let children = assemble_groups(entries);
    // A library item is always a single group so it moves/edits as one unit.
    match children.len() {
        0 => None,
        1 => Some(match children.into_iter().next().unwrap() {
            group @ Shape::Group(_) => group,
            other => Shape::Group(Group::new(vec![other])),
        }),
        _ => Some(Shape::Group(Group::new(children))),
    }
}

/// Read an element's `groupIds` as owned strings (empty when ungrouped).
fn element_group_ids(elem: &Value) -> Vec<String> {
    elem.get("groupIds")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Reconstruct nested groups from Excalidraw group paths.
///
/// Each entry is a shape plus its `groupIds` ordered innermost -> outermost.
/// Elements sharing the same outermost id form one group; the group's contents
/// are assembled recursively after peeling that outermost id. A group that
/// would contain a single child is collapsed to that child, so the hierarchy
/// carries no redundant single-element wrappers.
fn assemble_groups(entries: Vec<(Shape, Vec<String>)>) -> Vec<Shape> {
    let mut handled = vec![false; entries.len()];
    let mut out = Vec::new();

    for i in 0..entries.len() {
        if handled[i] {
            continue;
        }
        let Some(outer) = entries[i].1.last().cloned() else {
            // Ungrouped element: emit as a standalone shape at this level.
            handled[i] = true;
            out.push(entries[i].0.clone());
            continue;
        };

        // Gather every remaining element in the same outermost group, peeling
        // that id so the recursion sees the next level in.
        let mut bucket: Vec<(Shape, Vec<String>)> = Vec::new();
        for j in i..entries.len() {
            if handled[j] || entries[j].1.last() != Some(&outer) {
                continue;
            }
            handled[j] = true;
            let mut peeled = entries[j].1.clone();
            peeled.pop();
            bucket.push((entries[j].0.clone(), peeled));
        }

        let mut children = assemble_groups(bucket);
        if children.len() == 1 {
            out.push(children.pop().unwrap());
        } else {
            out.push(Shape::Group(Group::new(children)));
        }
    }

    out
}

// -----------------------------------------------------------------------------
// Element conversion (shared with scene import).
// -----------------------------------------------------------------------------

/// Convert a single Excalidraw element into a native shape, or `None` if it is
/// deleted or of an unsupported type.
pub fn element_to_shape(elem: &Value) -> Option<Shape> {
    if elem
        .get("isDeleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    let elem_type = elem.get("type").and_then(Value::as_str).unwrap_or("");
    let x = elem.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = elem.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let style = parse_style(elem);

    match elem_type {
        "rectangle" | "diamond" => Some(convert_rectangle(elem, x, y, style)),
        "ellipse" => Some(convert_ellipse(elem, x, y, style)),
        "freedraw" => convert_freedraw(elem, x, y, style),
        "line" => convert_line(elem, x, y, style),
        "arrow" => convert_arrow(elem, x, y, style),
        "text" => Some(convert_text(elem, x, y, style)),
        _ => None,
    }
}

/// Parse the shared style (stroke, fill, roughness, opacity) of an element.
fn parse_style(elem: &Value) -> ShapeStyle {
    let stroke_color = parse_color(
        elem.get("strokeColor")
            .and_then(Value::as_str)
            .unwrap_or("#000000"),
    );
    let bg_color = elem
        .get("backgroundColor")
        .and_then(Value::as_str)
        .unwrap_or("transparent");
    let fill_color = (bg_color != "transparent").then(|| parse_color(bg_color));

    let stroke_width = elem
        .get("strokeWidth")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    let sloppiness = match elem.get("roughness").and_then(Value::as_i64).unwrap_or(1) {
        0 => Sloppiness::Architect,
        1 => Sloppiness::Artist,
        _ => Sloppiness::Cartoonist,
    };
    let fill_pattern = match elem
        .get("fillStyle")
        .and_then(Value::as_str)
        .unwrap_or("solid")
    {
        "hachure" => FillPattern::Hachure,
        "cross-hatch" => FillPattern::CrossHatch,
        "zigzag" => FillPattern::ZigZag,
        _ => FillPattern::Solid,
    };

    ShapeStyle {
        stroke_color,
        stroke_width,
        fill_color,
        fill_pattern,
        sloppiness,
        seed: elem.get("seed").and_then(Value::as_u64).unwrap_or(0) as u32,
        opacity: elem
            .get("opacity")
            .and_then(Value::as_f64)
            // Excalidraw stores opacity as 0..100; normalize to 0..1.
            .map(|v| if v > 1.0 { v / 100.0 } else { v })
            .unwrap_or(1.0),
    }
}

/// Convert a `rectangle` or `diamond` element (diamond rendered as a rectangle).
fn convert_rectangle(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Shape {
    let width = elem
        .get("width")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_ELEMENT_SIZE);
    let height = elem
        .get("height")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_ELEMENT_SIZE);
    let mut rect = Rectangle::new(Point::new(x, y), width, height);
    rect.style = style;
    if elem.get("roundness").map(|r| !r.is_null()).unwrap_or(false) {
        rect.corner_radius = Rectangle::DEFAULT_ADAPTIVE_RADIUS
            .min(width / 4.0)
            .min(height / 4.0);
    }
    Shape::Rectangle(rect)
}

/// Convert an `ellipse` element.
fn convert_ellipse(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Shape {
    let width = elem
        .get("width")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_ELEMENT_SIZE);
    let height = elem
        .get("height")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_ELEMENT_SIZE);
    let center = Point::new(x + width / 2.0, y + height / 2.0);
    let mut ellipse = Ellipse::new(center, width / 2.0, height / 2.0);
    ellipse.style = style;
    Shape::Ellipse(ellipse)
}

/// Read a `points` array of `[dx, dy]` pairs, translated by the element origin.
fn read_points(elem: &Value, x: f64, y: f64) -> Vec<Point> {
    elem.get("points")
        .and_then(Value::as_array)
        .map(|pts| {
            pts.iter()
                .filter_map(Value::as_array)
                .filter_map(|arr| {
                    let px = arr.first().and_then(Value::as_f64)?;
                    let py = arr.get(1).and_then(Value::as_f64)?;
                    Some(Point::new(x + px, y + py))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a poly-line's endpoints are close enough to be a closed shape.
fn is_closed_path(points: &[Point]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    (first.x - last.x).abs() < CLOSED_PATH_THRESHOLD
        && (first.y - last.y).abs() < CLOSED_PATH_THRESHOLD
}

/// Convert a `freedraw` element into a freehand stroke.
fn convert_freedraw(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Option<Shape> {
    let points = read_points(elem, x, y);
    if points.is_empty() {
        return None;
    }
    let closed = is_closed_path(&points);
    let mut freehand = Freehand::from_points(points);
    freehand.style = style;
    freehand.closed = closed;
    Some(Shape::Freehand(freehand))
}

/// Convert a `line` element into a (possibly closed, possibly curved) poly-line.
fn convert_line(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Option<Shape> {
    let points = read_points(elem, x, y);
    if points.len() < 2 {
        return None;
    }
    let path_style = if has_roundness(elem) {
        PathStyle::Flowing
    } else {
        PathStyle::Direct
    };
    let closed = is_closed_path(&points);
    let mut line = Line::from_points(points, path_style);
    line.style = style;
    line.closed = closed;
    Some(Shape::Line(line))
}

/// Convert an `arrow` element, honoring elbowed/curved path styles.
fn convert_arrow(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Option<Shape> {
    let points = read_points(elem, x, y);
    if points.len() < 2 {
        return None;
    }
    let path_style = if elem
        .get("elbowed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        PathStyle::Angular
    } else if has_roundness(elem) {
        PathStyle::Flowing
    } else {
        PathStyle::Direct
    };
    let mut arrow = Arrow::from_points(points, path_style);
    arrow.style = style;
    Some(Shape::Arrow(arrow))
}

/// Convert a `text` element.
fn convert_text(elem: &Value, x: f64, y: f64, style: ShapeStyle) -> Shape {
    let content = elem
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let font_size = elem
        .get("fontSize")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_FONT_SIZE);
    let mut text = Text::new(Point::new(x, y), content);
    text.font_size = font_size;
    text.style = style;
    Shape::Text(text)
}

/// Whether an element declares non-null `roundness` (curved rendering).
fn has_roundness(elem: &Value) -> bool {
    elem.get("roundness").map(|r| !r.is_null()).unwrap_or(false)
}

/// Parse an Excalidraw color string (`transparent` or `#rgb`/`#rrggbb`/
/// `#rrggbbaa`) into a serializable color, defaulting to black.
pub(crate) fn parse_color(color: &str) -> SerializableColor {
    if color == "transparent" {
        return SerializableColor::transparent();
    }
    if let Some(hex) = color.strip_prefix('#') {
        let hex = hex.trim();
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17;
                return SerializableColor::new(r, g, b, 255);
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                return SerializableColor::new(r, g, b, 255);
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
                return SerializableColor::new(r, g, b, a);
            }
            _ => {}
        }
    }
    SerializableColor::black()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One library item mirroring the AWS "CloudSearch" icon's group structure:
    /// a text label directly in the outer group, plus an icon sub-group.
    const LIB: &str = r#"{
      "type": "excalidrawlib",
      "version": 2,
      "libraryItems": [
        {
          "name": "CloudSearch",
          "elements": [
            {"type":"text","x":0,"y":0,"text":"CloudSearch","groupIds":["OUTER"]},
            {"type":"rectangle","x":0,"y":0,"width":40,"height":40,"groupIds":["ICON_A","ICON","OUTER"]},
            {"type":"ellipse","x":5,"y":5,"width":10,"height":10,"groupIds":["ICON_B","ICON","OUTER"]},
            {"type":"line","x":0,"y":0,"points":[[0,0],[10,10]],"groupIds":["ICON_B","ICON","OUTER"]}
          ]
        }
      ]
    }"#;

    fn as_group(shape: &Shape) -> &Group {
        match shape {
            Shape::Group(g) => g,
            other => panic!("expected group, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_library_json() {
        assert!(library_from_excalidrawlib("{}").is_none());
        assert!(library_from_excalidrawlib("not json").is_none());
        assert!(library_from_excalidrawlib(r#"{"type":"excalidraw"}"#).is_none());
    }

    #[test]
    fn parses_item_name_and_wraps_in_one_group() {
        let items = library_from_excalidrawlib(LIB).expect("library parses");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "CloudSearch");
        assert!(matches!(items[0].shape, Shape::Group(_)));
    }

    #[test]
    fn preserves_icon_and_text_as_separate_groups() {
        let items = library_from_excalidrawlib(LIB).unwrap();
        let outer = as_group(&items[0].shape);
        // The outer group has two members: the text label and the icon group.
        assert_eq!(outer.children().len(), 2);

        let has_text = outer.children().iter().any(|c| matches!(c, Shape::Text(_)));
        let icon_group = outer
            .children()
            .iter()
            .find_map(|c| match c {
                Shape::Group(g) => Some(g),
                _ => None,
            })
            .expect("icon sub-group present");
        assert!(has_text, "text label is a direct child of the outer group");
        // The icon sub-group holds the rectangle and the ICON_B sub-group.
        assert_eq!(icon_group.children().len(), 2);
    }

    #[test]
    fn unnamed_items_get_a_fallback_name() {
        let lib = r#"{"type":"excalidrawlib","libraryItems":[
            {"elements":[{"type":"rectangle","x":0,"y":0,"width":10,"height":10}]}
        ]}"#;
        let items = library_from_excalidrawlib(lib).unwrap();
        assert_eq!(items[0].name, "Item 1");
    }

    #[test]
    fn v1_array_of_arrays_layout_is_supported() {
        let lib = r#"{"type":"excalidrawlib","version":1,"library":[
            [{"type":"rectangle","x":0,"y":0,"width":10,"height":10}],
            [{"type":"ellipse","x":0,"y":0,"width":10,"height":10}]
        ]}"#;
        let items = library_from_excalidrawlib(lib).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Item 1");
    }

    #[test]
    fn single_shape_item_is_still_wrapped_in_a_group() {
        let lib = r#"{"type":"excalidrawlib","libraryItems":[
            {"name":"Box","elements":[{"type":"rectangle","x":0,"y":0,"width":10,"height":10}]}
        ]}"#;
        let items = library_from_excalidrawlib(lib).unwrap();
        let group = as_group(&items[0].shape);
        assert_eq!(group.children().len(), 1);
        assert!(matches!(group.children()[0], Shape::Rectangle(_)));
    }

    #[test]
    fn grid_layout_separates_items_without_overlap() {
        let items = library_from_excalidrawlib(LIB).unwrap();
        // Duplicate the single item a few times to exercise the grid.
        let many: Vec<LibraryItem> = (0..5).map(|_| items[0].clone()).collect();
        let placed = library_layout_grid(&many);
        assert_eq!(placed.len(), 5);
        // No two placed items share the same center (they occupy distinct cells).
        let centers: Vec<(i64, i64)> = placed
            .iter()
            .map(|s| {
                let b = s.bounds();
                (
                    ((b.x0 + b.x1) / 2.0).round() as i64,
                    ((b.y0 + b.y1) / 2.0).round() as i64,
                )
            })
            .collect();
        let mut unique = centers.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), centers.len(), "each item gets its own cell");
    }

    #[test]
    fn empty_grid_is_empty() {
        assert!(library_layout_grid(&[]).is_empty());
    }

    #[test]
    fn element_converter_maps_core_types() {
        let rect = element_to_shape(&serde_json::json!({
            "type":"rectangle","x":1.0,"y":2.0,"width":30.0,"height":40.0
        }))
        .unwrap();
        assert!(matches!(rect, Shape::Rectangle(_)));
        let bounds = rect.bounds();
        assert!((bounds.x0 - 1.0).abs() < 1e-9);

        assert!(element_to_shape(&serde_json::json!({"type":"image"})).is_none());
        assert!(
            element_to_shape(&serde_json::json!({"type":"rectangle","isDeleted":true})).is_none()
        );
    }
}
