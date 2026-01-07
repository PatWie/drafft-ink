//! Line shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Line as KurboLine, Point, Rect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Path style for lines and arrows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathStyle {
    /// Straight line segments (sharp corners).
    #[default]
    Direct,
    /// Smooth bezier curves through points.
    Flowing,
    /// Right-angle connectors (elbow).
    Angular,
}

/// A line segment or polyline with optional bezier smoothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub(crate) id: ShapeId,
    /// Start point.
    pub start: Point,
    /// End point.
    pub end: Point,
    /// Intermediate points (for polylines/curves).
    #[serde(default)]
    pub intermediate_points: Vec<Point>,
    /// Path style (Direct, Flowing, Angular).
    #[serde(default)]
    pub path_style: PathStyle,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Line {
    /// Create a new line.
    pub fn new(start: Point, end: Point) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            intermediate_points: Vec::new(),
            path_style: PathStyle::Direct,
            style: ShapeStyle::default(),
        }
    }

    /// Reconstruct a line with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(
        id: ShapeId,
        start: Point,
        end: Point,
        intermediate_points: Vec<Point>,
        path_style: PathStyle,
        style: ShapeStyle,
    ) -> Self {
        Self {
            id,
            start,
            end,
            intermediate_points,
            path_style,
            style,
        }
    }

    /// Create a polyline from multiple points.
    pub fn from_points(points: Vec<Point>, path_style: PathStyle) -> Self {
        let start = points.first().copied().unwrap_or(Point::ZERO);
        let end = points.last().copied().unwrap_or(Point::ZERO);
        let intermediate_points = if points.len() > 2 {
            points[1..points.len() - 1].to_vec()
        } else {
            Vec::new()
        };
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            intermediate_points,
            path_style,
            style: ShapeStyle::default(),
        }
    }

    /// Get all points including start, intermediate, and end.
    pub fn all_points(&self) -> Vec<Point> {
        let mut pts = vec![self.start];
        pts.extend(&self.intermediate_points);
        pts.push(self.end);
        pts
    }

    /// Get the length of the line.
    pub fn length(&self) -> f64 {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Get the midpoint of the line.
    pub fn midpoint(&self) -> Point {
        Point::new(
            (self.start.x + self.end.x) / 2.0,
            (self.start.y + self.end.y) / 2.0,
        )
    }

    /// Get as a kurbo Line.
    pub fn as_kurbo(&self) -> KurboLine {
        KurboLine::new(self.start, self.end)
    }
}

impl ShapeTrait for Line {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        let points = self.all_points();
        let (min_x, max_x) = points.iter().fold((f64::MAX, f64::MIN), |(mn, mx), p| {
            (mn.min(p.x), mx.max(p.x))
        });
        let (min_y, max_y) = points.iter().fold((f64::MAX, f64::MIN), |(mn, mx), p| {
            (mn.min(p.y), mx.max(p.y))
        });
        Rect::new(min_x, min_y, max_x, max_y)
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        // Distance from point to line segment
        let line_vec = kurbo::Vec2::new(self.end.x - self.start.x, self.end.y - self.start.y);
        let point_vec = kurbo::Vec2::new(point.x - self.start.x, point.y - self.start.y);

        let line_len_sq = line_vec.hypot2();
        if line_len_sq < f64::EPSILON {
            // Line is a point
            let dist = point_vec.hypot();
            return dist <= tolerance;
        }

        // Project point onto line, clamped to segment
        let t = (point_vec.dot(line_vec) / line_len_sq).clamp(0.0, 1.0);
        let projection = Point::new(self.start.x + t * line_vec.x, self.start.y + t * line_vec.y);

        let dist = ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt();
        dist <= tolerance + self.style.stroke_width / 2.0
    }

    fn to_path(&self) -> BezPath {
        let mut path = BezPath::new();

        if self.start == self.end {
            return path;
        }

        // Get points to draw
        let points = match self.path_style {
            PathStyle::Angular if self.intermediate_points.is_empty() => {
                // Compute elbow path dynamically
                let elbow_pts = crate::elbow::compute_elbow_path(self.start, self.end);
                let mut pts = vec![self.start];
                pts.extend(elbow_pts);
                pts.push(self.end);
                pts
            }
            _ => self.all_points(),
        };

        if points.len() < 2 {
            return path;
        }

        path.move_to(points[0]);

        match self.path_style {
            PathStyle::Direct | PathStyle::Angular => {
                for p in &points[1..] {
                    path.line_to(*p);
                }
            }
            PathStyle::Flowing => {
                // Catmull-Rom spline converted to cubic bezier
                let tension = 0.5;
                for i in 0..points.len() - 1 {
                    let p0 = points[if i == 0 { 0 } else { i - 1 }];
                    let p1 = points[i];
                    let p2 = points[i + 1];
                    let p3 = points[if i + 2 >= points.len() {
                        points.len() - 1
                    } else {
                        i + 2
                    }];

                    let t1x = (p2.x - p0.x) * tension;
                    let t1y = (p2.y - p0.y) * tension;
                    let t2x = (p3.x - p1.x) * tension;
                    let t2y = (p3.y - p1.y) * tension;

                    let cp1 = Point::new(p1.x + t1x / 3.0, p1.y + t1y / 3.0);
                    let cp2 = Point::new(p2.x - t2x / 3.0, p2.y - t2y / 3.0);

                    path.curve_to(cp1, cp2, p2);
                }
            }
        }
        path
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        self.start = affine * self.start;
        self.end = affine * self.end;
        for p in &mut self.intermediate_points {
            *p = affine * *p;
        }
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_creation() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!((line.length() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_midpoint() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let mid = line.midpoint();
        assert!((mid.x - 50.0).abs() < f64::EPSILON);
        assert!((mid.y - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test_on_line() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(line.hit_test(Point::new(50.0, 0.0), 1.0));
        assert!(line.hit_test(Point::new(50.0, 2.0), 5.0));
        assert!(!line.hit_test(Point::new(50.0, 20.0), 5.0));
    }

    #[test]
    fn test_hit_test_endpoints() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(line.hit_test(Point::new(0.0, 0.0), 1.0));
        assert!(line.hit_test(Point::new(100.0, 0.0), 1.0));
    }

    #[test]
    fn test_bounds() {
        let line = Line::new(Point::new(10.0, 20.0), Point::new(50.0, 80.0));
        let bounds = line.bounds();
        assert!((bounds.x0 - 10.0).abs() < f64::EPSILON);
        assert!((bounds.y0 - 20.0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 50.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 80.0).abs() < f64::EPSILON);
    }
}
