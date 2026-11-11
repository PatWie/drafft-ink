//! Output: paths in local coordinates plus instructions for drawing them.

use kurbo::{BezPath, Rect};

/// How to stroke a path. Join and cap are the renderer's defaults (round join, butt cap), as in
/// the old app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    /// Line width in local units.
    pub width: f64,
    /// Dash and gap lengths, `None` for a solid line.
    pub dash: Option<[f64; 2]>,
}

/// How to fill a path. Colours are the caller's: "fill colour" is the element's fill colour and
/// "stroke colour" its stroke colour.
#[derive(Debug, Clone, PartialEq)]
pub enum FillKind {
    /// Fill the path (non-zero rule) with the fill colour.
    Solid,
    /// Fill the path with the fill colour at `alpha` times its own opacity. This is the pale
    /// background under hatch lines (the old app: 0.15).
    Tint {
        /// Opacity factor, 0 to 1.
        alpha: f64,
    },
    /// Fill the path with the stroke colour. Used for filled arrowheads and pressure outlines.
    Ink,
    /// The path is a set of lines. Stroke them with the fill colour at width `line_width`, clipped
    /// to `clip`.
    Hatch {
        /// Width of the lines.
        line_width: f64,
        /// The region the lines are clipped to.
        clip: BezPath,
    },
}

/// The sketched shape. Draw `fills` first, in order, then `strokes`, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SketchGeometry {
    /// Paths to stroke, in drawing order.
    pub strokes: Vec<(BezPath, StrokeStyle)>,
    /// Paths to fill, in drawing order.
    pub fills: Vec<(BezPath, FillKind)>,
}

impl SketchGeometry {
    /// True when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty() && self.fills.is_empty()
    }

    /// Tight bounds of all control points of all paths, `None` when empty. Stroke width is not
    /// included. Hatch clip paths are not counted separately: they are the fill outline.
    pub fn bounds(&self) -> Option<Rect> {
        let mut out: Option<Rect> = None;
        let paths = self.strokes.iter().map(|(p, _)| p).chain(self.fills.iter().map(|(p, _)| p));
        for p in paths {
            if p.elements().is_empty() {
                continue;
            }
            let b = p.control_box();
            out = Some(out.map_or(b, |o| o.union(b)));
        }
        out
    }

    /// Number of path elements over all paths. Useful for budgets and tests.
    pub fn element_count(&self) -> usize {
        let s: usize = self.strokes.iter().map(|(p, _)| p.elements().len()).sum();
        let f: usize = self
            .fills
            .iter()
            .map(|(p, k)| {
                p.elements().len()
                    + match k {
                        FillKind::Hatch { clip, .. } => clip.elements().len(),
                        _ => 0,
                    }
            })
            .sum();
        s + f
    }

    /// Drop every path that contains a non-finite coordinate. Called last by [`sketch`](crate::sketch)
    /// as a guard, so a caller never sees NaN.
    pub(crate) fn drop_non_finite(&mut self) {
        self.strokes.retain(|(p, s)| finite_path(p) && s.width.is_finite());
        self.fills.retain(|(p, k)| {
            finite_path(p)
                && match k {
                    FillKind::Hatch { clip, line_width } => finite_path(clip) && line_width.is_finite(),
                    _ => true,
                }
        });
    }
}

fn finite_path(p: &BezPath) -> bool {
    p.elements().iter().all(|el| {
        let pts: &[kurbo::Point] = match el {
            kurbo::PathEl::MoveTo(a) | kurbo::PathEl::LineTo(a) => std::slice::from_ref(a),
            kurbo::PathEl::QuadTo(a, b) => &[*a, *b],
            kurbo::PathEl::CurveTo(a, b, c) => &[*a, *b, *c],
            kurbo::PathEl::ClosePath => &[],
        };
        pts.iter().all(|q| q.x.is_finite() && q.y.is_finite())
    })
}
