//! Text bound to a container shape, sized as Excalidraw does (research/parts/02c §3).
//!
//! The container (rectangle, ellipse, diamond, arrow label) gives the text a maximum width and
//! height. The text wraps at that width, the container grows when the wrapped text is taller than
//! it, and the text sits inside by the vertical alignment. Everything here is in the container's
//! own frame (origin at its top-left, no rotation); the caller rotates the result with the
//! container.
//!
//! Ported from Excalidraw `textElement.ts` (MIT); see ATTRIBUTION.md.

use std::sync::Arc;

use kurbo::{Point, Rect, Size};

use crate::layout::TextLayout;

/// Padding between a container's border and its text, per side. Excalidraw's `BOUND_TEXT_PADDING`.
pub const BOUND_TEXT_PADDING: f64 = 5.0;

/// The shape that holds the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// Rectangle (and rounded rectangle).
    Rectangle,
    /// Ellipse: the text fits the inscribed rectangle.
    Ellipse,
    /// Diamond: the text fits the inscribed rectangle.
    Diamond,
    /// An arrow label. The container box is the arrow's bounding box. Arrows never grow.
    Arrow,
}

/// Vertical placement of the text inside the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VerticalAlign {
    /// Top of the inner box.
    Top,
    /// Centred (Excalidraw's default for containers).
    #[default]
    Middle,
    /// Bottom of the inner box.
    Bottom,
}

/// A container: its kind and the size of its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Container {
    /// Kind.
    pub kind: ContainerKind,
    /// Width and height of the container's box.
    pub size: Size,
}

/// JavaScript `Math.round`: halves go up.
fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

impl Container {
    /// A container of `kind` and `size`.
    pub fn new(kind: ContainerKind, width: f64, height: f64) -> Self {
        Self { kind, size: Size::new(width, height) }
    }

    /// Widest the text may be (the wrap width). For arrows `font_size` sets the floor.
    pub fn max_text_width(&self, font_size: f64) -> f64 {
        let p2 = 2.0 * BOUND_TEXT_PADDING;
        let w = self.size.width;
        let v = match self.kind {
            ContainerKind::Rectangle => w - p2,
            ContainerKind::Ellipse => js_round(w / 2.0 * std::f64::consts::SQRT_2) - p2,
            ContainerKind::Diamond => js_round(w / 2.0) - p2,
            ContainerKind::Arrow => (0.7 * w).max(font_size * 11.0),
        };
        v.max(1.0)
    }

    /// Tallest the text may be before the container has to grow. Arrows return the text height
    /// (they never constrain it); pass it as `text_height`.
    pub fn max_text_height(&self, text_height: f64) -> f64 {
        let p2 = 2.0 * BOUND_TEXT_PADDING;
        let h = self.size.height;
        match self.kind {
            ContainerKind::Rectangle => h - p2,
            ContainerKind::Ellipse => js_round(h / 2.0 * std::f64::consts::SQRT_2) - p2,
            ContainerKind::Diamond => js_round(h / 2.0) - p2,
            ContainerKind::Arrow => {
                if h - 16.0 * BOUND_TEXT_PADDING > 0.0 {
                    h
                } else {
                    text_height
                }
            }
        }
        .max(0.0)
    }

    /// Top-left of the inner box, relative to the container's top-left.
    pub fn inner_origin(&self) -> Point {
        let p = BOUND_TEXT_PADDING;
        let (w, h) = (self.size.width, self.size.height);
        match self.kind {
            ContainerKind::Rectangle | ContainerKind::Arrow => Point::new(p, p),
            ContainerKind::Ellipse => {
                let k = 1.0 - std::f64::consts::SQRT_2 / 2.0;
                Point::new(p + w / 2.0 * k, p + h / 2.0 * k)
            }
            ContainerKind::Diamond => Point::new(p + w / 4.0, p + h / 4.0),
        }
    }

    /// The container dimension that leaves `text_dim` room for text: the inverse of
    /// [`Self::max_text_width`] and [`Self::max_text_height`].
    pub fn dimension_for_text(kind: ContainerKind, text_dim: f64) -> f64 {
        let d = text_dim.ceil();
        let p2 = 2.0 * BOUND_TEXT_PADDING;
        match kind {
            ContainerKind::Rectangle => d + p2,
            ContainerKind::Ellipse => js_round((d + p2) / std::f64::consts::SQRT_2 * 2.0),
            ContainerKind::Diamond => 2.0 * (d + p2),
            ContainerKind::Arrow => d + p2 * 8.0,
        }
    }
}

/// The result of laying text into a container.
#[derive(Clone, Debug)]
pub struct BoundText {
    /// The wrapped text. Its box is [`Self::wrap_width`] wide (lines are aligned inside it).
    pub layout: Arc<TextLayout>,
    /// Top-left of the text box, relative to the container's top-left.
    pub origin: Point,
    /// The width the text wrapped at.
    pub wrap_width: f64,
    /// The inner box the text may use, in container coordinates, after any growth.
    pub inner: Rect,
    /// The container size after growth. Equal to the input when the text fits. The container
    /// grows downward (its top stays); the caller moves it if another edge must stay fixed.
    pub container_size: Size,
}

impl BoundText {
    /// Whether the text made the container grow.
    pub fn grew(&self, original: Size) -> bool {
        self.container_size != original
    }

    /// Smallest container height that holds this text (what resize must not go below).
    pub fn min_container_height(&self, kind: ContainerKind) -> f64 {
        Container::dimension_for_text(kind, self.layout.size.height)
    }
}
