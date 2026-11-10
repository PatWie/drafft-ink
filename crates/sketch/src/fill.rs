//! Sketched fill patterns through roughr.
//!
//! Like the old app (`generate_fill_pattern`), the pattern is generated for the bounding box of the
//! shape and the renderer clips it to the outline. The fill gap is widened where needed so the
//! number of generated lines stays under the limits in [`crate::limits`].

use kurbo::{BezPath, Point, Rect};
use roughr::core::{FillStyle as RoughFill, OpSetType, OpType, OptionsBuilder};

use crate::input::FillStyle;
use crate::limits::{MAX_FILL_DOTS, MAX_FILL_SEGMENTS, MAX_HACHURE_LINES, MIN_FILL_GAP};

/// roughr's default hachure angle in degrees, which the old app used.
pub(crate) const DEFAULT_ANGLE: f64 = -41.0;

/// The gap actually used for a fill over `bounds`.
///
/// Lines are spaced `gap` apart, so a box needs about `(w + h) / gap` lines in the worst angle.
/// Dashes, zigzag pieces and dots add about `diagonal / gap` pieces per line.
pub(crate) fn effective_gap(style: FillStyle, bounds: Rect, gap: f64) -> f64 {
    let (w, h) = (bounds.width(), bounds.height());
    let extent = w + h;
    let diagonal = w.hypot(h);
    let mut g = gap.max(MIN_FILL_GAP);
    g = g.max(extent / MAX_HACHURE_LINES);
    let per_line_pieces = match style {
        FillStyle::Dashed | FillStyle::ZigZagLine => Some(MAX_FILL_SEGMENTS),
        FillStyle::Dots => Some(MAX_FILL_DOTS),
        _ => None,
    };
    if let Some(max_pieces) = per_line_pieces {
        g = g.max((extent * diagonal / max_pieces).sqrt());
    }
    g
}

fn rough_style(style: FillStyle) -> Option<RoughFill> {
    match style {
        FillStyle::None | FillStyle::Solid => None,
        FillStyle::Hachure => Some(RoughFill::Hachure),
        FillStyle::CrossHatch => Some(RoughFill::CrossHatch),
        FillStyle::ZigZag => Some(RoughFill::ZigZag),
        FillStyle::Dots => Some(RoughFill::Dots),
        FillStyle::Dashed => Some(RoughFill::Dashed),
        FillStyle::ZigZagLine => Some(RoughFill::ZigZagLine),
    }
}

/// The pattern lines for `style` over `bounds`, or an empty path when there is nothing to draw.
pub(crate) fn pattern(
    style: FillStyle,
    bounds: Rect,
    stroke_width: f64,
    gap: Option<f64>,
    angle: Option<f64>,
    seed: u32,
) -> BezPath {
    let mut path = BezPath::new();
    let Some(rough) = rough_style(style) else { return path };
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return path;
    }
    let gap = effective_gap(style, bounds, gap.unwrap_or(stroke_width * 4.0));
    let angle = angle.filter(|a| a.is_finite()).unwrap_or(DEFAULT_ANGLE) % 360.0;
    let ink = roughr::Srgba::new(0.0, 0.0, 0.0, 1.0);
    let options = OptionsBuilder::default()
        .seed(u64::from(seed))
        .fill_style(rough)
        .fill(ink)
        .stroke(ink)
        .fill_weight((stroke_width * 0.5) as f32)
        .hachure_gap(gap as f32)
        .hachure_angle(angle as f32)
        .build();
    let Ok(options) = options else { return path };

    // As in the old app: `rectangle` over the bounding box. The outline it also draws is thrown
    // away, but generating it first advances the random stream the same way as before, so old
    // documents keep their fill pattern for the same seed.
    let generator = roughr::generator::Generator::default();
    let drawing = generator.rectangle::<f64>(bounds.x0, bounds.y0, bounds.width(), bounds.height(), &Some(options));
    for set in &drawing.sets {
        if set.op_set_type != OpSetType::FillSketch {
            continue;
        }
        for op in &set.ops {
            let d = &op.data;
            match op.op {
                OpType::Move if d.len() >= 2 => path.move_to(Point::new(d[0], d[1])),
                OpType::LineTo if d.len() >= 2 => path.line_to(Point::new(d[0], d[1])),
                OpType::BCurveTo if d.len() >= 6 => {
                    path.curve_to(Point::new(d[0], d[1]), Point::new(d[2], d[3]), Point::new(d[4], d[5]))
                }
                _ => {}
            }
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f64, h: f64) -> Rect {
        Rect::new(0.0, 0.0, w, h)
    }

    #[test]
    fn gap_is_widened_for_huge_boxes_only() {
        assert_eq!(effective_gap(FillStyle::Hachure, rect(100.0, 100.0), 8.0), 8.0);
        let g = effective_gap(FillStyle::Hachure, rect(1.0e6, 1.0e6), 1.0);
        assert!((2.0e6 / g) <= MAX_HACHURE_LINES + 1e-6);
        let g = effective_gap(FillStyle::Dots, rect(10_000.0, 10_000.0), 1.0);
        let dots = (2.0e4 / g) * (14_142.0 / g);
        assert!(dots <= MAX_FILL_DOTS * 1.01, "{dots}");
        assert!(effective_gap(FillStyle::Hachure, rect(10.0, 10.0), 0.0) >= MIN_FILL_GAP);
        assert!(effective_gap(FillStyle::Hachure, rect(10.0, 10.0), f64::NAN) >= MIN_FILL_GAP);
    }

    #[test]
    fn every_pattern_draws_something_and_none_draws_nothing() {
        for s in [
            FillStyle::Hachure,
            FillStyle::CrossHatch,
            FillStyle::ZigZag,
            FillStyle::Dots,
            FillStyle::Dashed,
            FillStyle::ZigZagLine,
        ] {
            let p = pattern(s, rect(200.0, 100.0), 2.0, None, None, 7);
            assert!(!p.elements().is_empty(), "{s:?}");
        }
        for s in [FillStyle::None, FillStyle::Solid] {
            assert!(pattern(s, rect(200.0, 100.0), 2.0, None, None, 7).elements().is_empty());
        }
        assert!(pattern(FillStyle::Hachure, rect(0.0, 100.0), 2.0, None, None, 7).elements().is_empty());
    }

    #[test]
    fn same_seed_same_pattern() {
        let a = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, None, None, 7);
        let b = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, None, None, 7);
        let c = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, None, None, 8);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn gap_and_angle_change_the_pattern() {
        let base = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, None, None, 7);
        let wide = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, Some(20.0), None, 7);
        let steep = pattern(FillStyle::Hachure, rect(200.0, 100.0), 2.0, None, Some(45.0), 7);
        assert!(wide.elements().len() < base.elements().len());
        assert_ne!(base, steep);
    }
}
