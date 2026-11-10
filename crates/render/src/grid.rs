//! The background grid, bounded in cost at every zoom.
//!
//! The old renderer emitted one cell per `grid_size` in world space, which is about 520,000 cells when zoomed out
//! to 0.1 on a 1080p screen. Here the spacing adapts: it doubles (`base * 2^k`, so the lines stay on the base
//! grid that snapping uses) until cells are at least [`MIN_CELL_PX`] apart on screen and the whole grid has at
//! most [`MAX_LINES`] lines or [`MAX_POINTS`] points. The grid is one path and one draw call in device space.

use drafftink_scene::Color;
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::Fill;
use vello::Scene;

use crate::theme::Theme;
use crate::view::View;

/// Closest two grid cells may be on screen, in device pixels.
pub const MIN_CELL_PX: f64 = 8.0;
/// Most lines (both directions together) a line grid draws.
pub const MAX_LINES: usize = 400;
/// Most points a dot or cross grid draws.
pub const MAX_POINTS: usize = 6_000;
/// The base spacing of the old app, in world units.
pub const DEFAULT_SPACING: f64 = 20.0;

/// How the grid looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum GridStyle {
    /// No grid.
    #[default]
    None,
    /// Horizontal and vertical lines.
    Lines,
    /// Horizontal lines only (ruled paper).
    HorizontalLines,
    /// A small plus at each crossing.
    Crosses,
    /// A dot at each crossing.
    Dots,
}

/// A grid request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    /// Look.
    pub style: GridStyle,
    /// Base spacing in world units.
    pub spacing: f64,
    /// Colour override; `None` uses the old app's grey for the style.
    pub color: Option<Color>,
}

impl Default for GridSpec {
    fn default() -> Self {
        Self { style: GridStyle::None, spacing: DEFAULT_SPACING, color: None }
    }
}

impl GridSpec {
    /// A grid of `style` at the default spacing.
    pub fn new(style: GridStyle) -> Self {
        Self { style, ..Self::default() }
    }
}

/// What a grid draw did, for tests and the profiler.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GridStats {
    /// The spacing used, in world units (a power-of-two multiple of the base).
    pub spacing: f64,
    /// Vertical plus horizontal lines.
    pub lines: usize,
    /// Dots or crosses.
    pub points: usize,
}

fn counts(view: &View, spacing: f64, style: GridStyle) -> (usize, usize) {
    let w = view.world_rect();
    // The draw loops run from floor(x0 / s) to ceil(x1 / s) inclusive: at most ceil(width / s) + 2 columns.
    let cols = ((w.width() / spacing).ceil() + 2.0).clamp(0.0, 1.0e12) as usize;
    let rows = ((w.height() / spacing).ceil() + 2.0).clamp(0.0, 1.0e12) as usize;
    match style {
        GridStyle::None => (0, 0),
        GridStyle::Lines => (cols + rows, 0),
        GridStyle::HorizontalLines => (rows, 0),
        GridStyle::Crosses | GridStyle::Dots => (0, cols.saturating_mul(rows)),
    }
}

/// The spacing the grid uses for this view: the base times a power of two, as small as the limits allow.
/// Returns `None` for no grid, an invalid view or an unusable base spacing.
pub fn grid_spacing(view: &View, spec: &GridSpec) -> Option<f64> {
    if spec.style == GridStyle::None || !view.is_valid() || !spec.spacing.is_finite() || spec.spacing <= 0.0 {
        return None;
    }
    let mut s = spec.spacing;
    for _ in 0..80 {
        let (lines, points) = counts(view, s, spec.style);
        if s * view.scale() >= MIN_CELL_PX && lines <= MAX_LINES && points <= MAX_POINTS {
            return Some(s);
        }
        s *= 2.0;
    }
    None
}

fn base_color(spec: &GridSpec) -> Color {
    spec.color.unwrap_or(match spec.style {
        GridStyle::Lines | GridStyle::HorizontalLines | GridStyle::None => Color::rgba(200, 200, 200, 100),
        GridStyle::Crosses => Color::rgba(180, 180, 180, 60),
        GridStyle::Dots => Color::rgba(160, 160, 160, 70),
    })
}

/// Draws the grid into `scene` in device space (identity transform). At most one path is drawn.
pub fn paint_grid(scene: &mut Scene, view: &View, spec: &GridSpec, theme: Theme) -> GridStats {
    let Some(spacing) = grid_spacing(view, spec) else { return GridStats::default() };
    let world = view.world_rect();
    let to_dev = view.world_to_device();
    let color = peniko::Color::from(theme.map(base_color(spec)));
    let d = view.dpi.max(1.0e-3);
    let first_col = (world.x0 / spacing).floor() as i64;
    let last_col = (world.x1 / spacing).ceil() as i64;
    let first_row = (world.y0 / spacing).floor() as i64;
    let last_row = (world.y1 / spacing).ceil() as i64;
    let dev = view.device_size();
    // Lines sit on half pixels so a one-pixel line is crisp.
    let snap = |v: f64| v.floor() + 0.5;
    let mut path = BezPath::new();
    let mut stats = GridStats { spacing, ..GridStats::default() };
    match spec.style {
        GridStyle::None => return GridStats::default(),
        GridStyle::Lines | GridStyle::HorizontalLines => {
            if spec.style == GridStyle::Lines {
                for c in first_col..=last_col {
                    let x = snap((to_dev * Point::new(c as f64 * spacing, 0.0)).x);
                    path.move_to(Point::new(x, 0.0));
                    path.line_to(Point::new(x, dev.height));
                    stats.lines += 1;
                }
            }
            for r in first_row..=last_row {
                let y = snap((to_dev * Point::new(0.0, r as f64 * spacing)).y);
                path.move_to(Point::new(0.0, y));
                path.line_to(Point::new(dev.width, y));
                stats.lines += 1;
            }
            scene.stroke(&kurbo::Stroke::new(d.max(1.0)), Affine::IDENTITY, color, None, &path);
        }
        GridStyle::Crosses => {
            let arm = 3.0 * d;
            for c in first_col..=last_col {
                let x = snap((to_dev * Point::new(c as f64 * spacing, 0.0)).x);
                for r in first_row..=last_row {
                    let y = snap((to_dev * Point::new(0.0, r as f64 * spacing)).y);
                    path.move_to(Point::new(x - arm, y));
                    path.line_to(Point::new(x + arm, y));
                    path.move_to(Point::new(x, y - arm));
                    path.line_to(Point::new(x, y + arm));
                    stats.points += 1;
                }
            }
            scene.stroke(&kurbo::Stroke::new(d.max(1.0)), Affine::IDENTITY, color, None, &path);
        }
        GridStyle::Dots => {
            let half = 1.5 * d;
            for c in first_col..=last_col {
                let x = (to_dev * Point::new(c as f64 * spacing, 0.0)).x;
                for r in first_row..=last_row {
                    let y = (to_dev * Point::new(0.0, r as f64 * spacing)).y;
                    let rect = Rect::new(x - half, y - half, x + half, y + half);
                    path.move_to(Point::new(rect.x0, rect.y0));
                    path.line_to(Point::new(rect.x1, rect.y0));
                    path.line_to(Point::new(rect.x1, rect.y1));
                    path.line_to(Point::new(rect.x0, rect.y1));
                    path.close_path();
                    stats.points += 1;
                }
            }
            scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &path);
        }
    }
    stats
}
