//! Mermaid diagram import.
//!
//! Parses a subset of the [Mermaid](https://mermaid.js.org/) text syntax into
//! native drafft shapes (rectangles, ellipses, lines, arrows, text) rather than
//! rasterizing to an image. Every element of an imported diagram is therefore a
//! first-class, editable shape. Each diagram is wrapped in a single [`Group`] so
//! it can be moved, styled, or ungrouped as one unit.
//!
//! Two diagram kinds are supported, matching the constructs users most commonly
//! paste:
//!
//! * `sequenceDiagram` — participants, messages (solid/dashed arrows), notes.
//! * `graph` / `flowchart` — nodes (rectangle, rounded, stadium, circle,
//!   diamond, hexagon) and edges (solid/dotted/thick, optional arrowhead and
//!   label) laid out with a layered longest-path algorithm.
//!
//! # Grounding for reviewers
//!
//! CRITICAL PATH: this parser consumes untrusted clipboard text. It must never
//! panic on arbitrary input — malformed lines are skipped, and a text that is
//! not recognizable as a supported diagram returns `None` so the caller can fall
//! through to other paste handlers.

mod flowchart;
mod sequence;

use crate::shapes::{
    Group, Line, PathStyle, Rectangle, SerializableColor, Shape, ShapeStyle, StrokeStyle, Text,
};
use kurbo::Point;

// -----------------------------------------------------------------------------
// Shared layout constants.
//
// Every geometric magnitude used by the builders is a named constant so the
// visual proportions of imported diagrams can be tuned in one place and so no
// bare numeric literal encodes layout meaning.
// -----------------------------------------------------------------------------

/// Font size (pixels) used for all labels in imported diagrams.
pub(crate) const LABEL_FONT_SIZE: f64 = 16.0;

/// Approximate average glyph advance as a fraction of the font size. Used to
/// estimate text width for box sizing before the renderer has measured the
/// real layout. Intentionally slightly generous so text never overflows boxes.
const CHAR_WIDTH_FACTOR: f64 = 0.6;

/// Line height as a multiple of the font size (matches the text renderer).
const LINE_HEIGHT_FACTOR: f64 = 1.2;

/// Horizontal padding (pixels) added on each side of a label inside a box.
pub(crate) const BOX_PADDING_X: f64 = 20.0;

/// Vertical padding (pixels) added above and below a label inside a box.
pub(crate) const BOX_PADDING_Y: f64 = 12.0;

/// Minimum width (pixels) of any labeled box, so short labels still look boxy.
pub(crate) const MIN_BOX_WIDTH: f64 = 72.0;

/// Default stroke width (pixels) for diagram outlines and connectors.
pub(crate) const STROKE_WIDTH: f64 = 2.0;

/// Stroke color for all diagram geometry (near-black slate).
pub(crate) const STROKE_COLOR: SerializableColor = SerializableColor {
    r: 30,
    g: 41,
    b: 59,
    a: 255,
};

/// Opaque white fill used for node/participant boxes so they cleanly cover any
/// lifelines or edges drawn beneath them.
pub(crate) const BOX_FILL: SerializableColor = SerializableColor {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// Pale yellow fill used for sequence-diagram notes, echoing Mermaid's default.
pub(crate) const NOTE_FILL: SerializableColor = SerializableColor {
    r: 255,
    g: 249,
    b: 196,
    a: 255,
};

/// Error produced when the input cannot be parsed as a supported diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MermaidError {
    /// The text does not begin with a recognized diagram keyword.
    Unrecognized,
    /// The diagram keyword was recognized but no drawable content was found.
    Empty,
}

impl std::fmt::Display for MermaidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MermaidError::Unrecognized => write!(f, "not a recognized Mermaid diagram"),
            MermaidError::Empty => write!(f, "Mermaid diagram contained no drawable content"),
        }
    }
}

impl std::error::Error for MermaidError {}

/// The diagram kind detected from the header line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagramKind {
    Sequence,
    Flowchart,
}

/// Try to parse clipboard text as a supported Mermaid diagram, returning a
/// single-element vector holding one [`Shape::Group`] that contains every
/// drawable element of the diagram.
///
/// Returns `None` when the text is not a recognized diagram, so callers can
/// treat this as one option among several clipboard interpretations.
pub fn shapes_from_mermaid(text: &str) -> Option<Vec<Shape>> {
    build_group(text).ok().map(|group| vec![group])
}

/// Parse `text` and assemble the diagram's shapes into one group.
fn build_group(text: &str) -> Result<Shape, MermaidError> {
    let body = strip_code_fence(text);
    let kind = detect_kind(body).ok_or(MermaidError::Unrecognized)?;

    let children = match kind {
        DiagramKind::Sequence => sequence::build(body),
        DiagramKind::Flowchart => flowchart::build(body),
    };

    if children.is_empty() {
        return Err(MermaidError::Empty);
    }
    Ok(Shape::Group(Group::new(children)))
}

/// Remove a surrounding Markdown code fence (```mermaid ... ```), if present,
/// and return the inner diagram text. Leaves un-fenced input untouched.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the remainder of the opening fence line (e.g. the "mermaid" tag).
    let after_first_line = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or("");
    // Drop the trailing closing fence.
    after_first_line
        .trim_end()
        .strip_suffix("```")
        .unwrap_or(after_first_line)
        .trim()
}

/// Inspect the first meaningful line to decide which diagram builder to use.
fn detect_kind(body: &str) -> Option<DiagramKind> {
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))?;

    // Sequence diagrams open with the `sequenceDiagram` keyword.
    if first.starts_with("sequenceDiagram") {
        return Some(DiagramKind::Sequence);
    }
    // Flowcharts open with either `graph` or `flowchart`, followed by a
    // direction token on the same line.
    if first.starts_with("graph") || first.starts_with("flowchart") {
        return Some(DiagramKind::Flowchart);
    }
    None
}

// -----------------------------------------------------------------------------
// Shared shape-building helpers.
// -----------------------------------------------------------------------------

/// Estimate the rendered `(width, height)` of a possibly multi-line label at the
/// given font size. This is only an approximation used for box sizing; the text
/// renderer computes the exact layout at draw time.
pub(crate) fn measure_text(label: &str, font_size: f64) -> (f64, f64) {
    let normalized = normalize_label(label);
    let max_line_chars = normalized
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let line_count = normalized.lines().count().max(1);
    let width = max_line_chars as f64 * font_size * CHAR_WIDTH_FACTOR;
    let height = line_count as f64 * font_size * LINE_HEIGHT_FACTOR;
    (width, height)
}

/// Convert Mermaid label conventions into plain multi-line text: `<br>` and
/// `<br/>` become newlines, and surrounding quotes are stripped.
pub(crate) fn normalize_label(label: &str) -> String {
    let unquoted = label.trim().trim_matches('"');
    unquoted
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n")
}

/// Build the default stroke/fill style for diagram geometry.
pub(crate) fn base_style(fill: Option<SerializableColor>) -> ShapeStyle {
    ShapeStyle {
        stroke_color: STROKE_COLOR,
        stroke_width: STROKE_WIDTH,
        fill_color: fill,
        ..ShapeStyle::default()
    }
}

/// Create a rectangle shape at `(x, y)` with the given size, corner radius and
/// optional fill.
pub(crate) fn rect_shape(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    corner_radius: f64,
    fill: Option<SerializableColor>,
) -> Rectangle {
    let mut rect = Rectangle::new(Point::new(x, y), width, height);
    rect.corner_radius = corner_radius;
    rect.style = base_style(fill);
    rect
}

/// Create a centered text shape whose bounding box is centered on `center`.
pub(crate) fn centered_text(center: Point, label: &str) -> Text {
    let content = normalize_label(label);
    let (w, h) = measure_text(&content, LABEL_FONT_SIZE);
    let position = Point::new(center.x - w / 2.0, center.y - h / 2.0);
    let mut text = Text::new(position, content).with_font_size(LABEL_FONT_SIZE);
    text.style = base_style(None);
    text
}

/// Create a straight connector line between two points with the given stroke
/// style (solid/dashed/dotted).
pub(crate) fn line_shape(start: Point, end: Point, stroke_style: StrokeStyle) -> Line {
    let mut line = Line::new(start, end);
    line.stroke_style = stroke_style;
    line.path_style = PathStyle::Direct;
    line.style = base_style(None);
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_fence_removes_mermaid_fence() {
        let input = "```mermaid\nsequenceDiagram\n    A->>B: hi\n```";
        assert_eq!(strip_code_fence(input), "sequenceDiagram\n    A->>B: hi");
    }

    #[test]
    fn strip_code_fence_leaves_plain_text() {
        let input = "sequenceDiagram\n    A->>B: hi";
        assert_eq!(strip_code_fence(input), input);
    }

    #[test]
    fn detect_kind_recognizes_both_diagram_types() {
        assert_eq!(
            detect_kind("sequenceDiagram\n"),
            Some(DiagramKind::Sequence)
        );
        assert_eq!(detect_kind("graph LR\n"), Some(DiagramKind::Flowchart));
        assert_eq!(detect_kind("flowchart TD\n"), Some(DiagramKind::Flowchart));
        assert_eq!(detect_kind("pie title x\n"), None);
    }

    #[test]
    fn detect_kind_skips_comments_and_blanks() {
        let body = "%% a comment\n\nsequenceDiagram\n";
        assert_eq!(detect_kind(body), Some(DiagramKind::Sequence));
    }

    #[test]
    fn unrecognized_input_returns_none() {
        assert!(shapes_from_mermaid("just some prose").is_none());
        assert!(shapes_from_mermaid("").is_none());
    }

    #[test]
    fn measure_text_handles_multiline() {
        let (w1, h1) = measure_text("ab", LABEL_FONT_SIZE);
        let (w2, h2) = measure_text("ab\ncdef", LABEL_FONT_SIZE);
        assert!(w2 > w1, "wider line should measure wider");
        assert!(h2 > h1, "two lines should measure taller");
    }

    #[test]
    fn normalize_label_converts_breaks_and_quotes() {
        assert_eq!(normalize_label("\"a<br>b\""), "a\nb");
    }

    /// The two diagrams from the feature request, used to lock in the top-level
    /// contract: one diagram imports as exactly one editable group.
    const SEQUENCE_EXAMPLE: &str = "sequenceDiagram\n\
        participant C as Customer\n\
        participant CA as Cashier\n\
        C->>CA: place an order\n\
        CA-->>C: order confirmed (paid)\n\
        Note over C: waiting\n";

    const FLOWCHART_EXAMPLE: &str = "graph LR\n\
        C[Cashier] -- \"Order (make this)\" --> S[Kitchen]\n\
        S -- \"Report (done / burnt)\" --> C\n";

    /// A diagram must import as a single group so it moves/edits as one unit.
    fn assert_single_group(text: &str) -> Group {
        let shapes = shapes_from_mermaid(text).expect("diagram should parse");
        assert_eq!(
            shapes.len(),
            1,
            "a diagram must import as exactly one shape"
        );
        match shapes.into_iter().next().unwrap() {
            Shape::Group(group) => group,
            other => panic!("expected a group, got {other:?}"),
        }
    }

    #[test]
    fn sequence_imports_as_one_editable_group() {
        let group = assert_single_group(SEQUENCE_EXAMPLE);
        // The group must hold native, individually editable shapes, not an image.
        assert!(group.children().len() > 1);
        assert!(
            group
                .children()
                .iter()
                .all(|s| !matches!(s, Shape::Image(_)))
        );
        // At least one arrow (a message) and one text (a label) must be present.
        assert!(
            group
                .children()
                .iter()
                .any(|s| matches!(s, Shape::Arrow(_)))
        );
        assert!(group.children().iter().any(|s| matches!(s, Shape::Text(_))));
    }

    #[test]
    fn flowchart_imports_as_one_editable_group() {
        let group = assert_single_group(FLOWCHART_EXAMPLE);
        assert!(group.children().len() > 1);
        assert!(
            group
                .children()
                .iter()
                .all(|s| !matches!(s, Shape::Image(_)))
        );
        assert!(
            group
                .children()
                .iter()
                .any(|s| matches!(s, Shape::Arrow(_)))
        );
    }

    #[test]
    fn fenced_diagram_is_recognized() {
        let fenced = format!("```mermaid\n{SEQUENCE_EXAMPLE}```");
        assert!(shapes_from_mermaid(&fenced).is_some());
    }
}
