//! Icons drawn with the egui painter. The old app shipped 30 SVG files and an image loader; these are the
//! same pictures as strokes, so they follow the theme colour, need no asset loader and add no bytes
//! to the wasm file.
//!
//! Each icon is drawn in a 24 by 24 box that is scaled into the target rectangle.

use egui::epaint::{CubicBezierShape, PathShape, PathStroke};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

/// An icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Icon {
    Select,
    Lasso,
    Hand,
    Rectangle,
    Diamond,
    Ellipse,
    Arrow,
    Line,
    Freehand,
    Highlighter,
    Text,
    Math,
    Frame,
    Eraser,
    Laser,
    Undo,
    Redo,
    Center,
    ZoomFit,
    SnapGrid,
    SnapShapes,
    SnapAngle,
    BindArrow,
    GridNone,
    GridLines,
    GridHorizontal,
    GridCrosses,
    GridDots,
    Menu,
    Collab,
    LayerBack,
    LayerBackward,
    LayerForward,
    LayerFront,
    FlipH,
    FlipV,
    AlignLeft,
    AlignCenterX,
    AlignRight,
    AlignTop,
    AlignCenterY,
    AlignBottom,
    DistributeH,
    DistributeV,
    Group,
    Ungroup,
    Duplicate,
    Delete,
    Lock,
    Unlock,
    Plus,
    Minus,
    Close,
    Help,
    Search,
    Image,
    Wheel,
    Eyedropper,
    NoColor,
    Library,
    Sticky,
    Settings,
    Export,
    Eye,
    Panel,
    Link,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Layers,
    Convert,
    Crop,
    CropReset,
    Brush,
    Save,
    Open,
    Info,
    Keyboard,
    Stats,
    Check,
    ChevronRight,
    ChevronDown,
}

struct Pen<'a> {
    painter: &'a Painter,
    rect: Rect,
    stroke: Stroke,
    color: Color32,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        let s = self.rect.width().min(self.rect.height()) / 24.0;
        let o = self.rect.center() - vec2(12.0 * s, 12.0 * s);
        pos2(o.x + x * s, o.y + y * s)
    }
    fn scale(&self) -> f32 {
        self.rect.width().min(self.rect.height()) / 24.0
    }
    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.painter.line_segment([self.p(a.0, a.1), self.p(b.0, b.1)], self.stroke);
    }
    fn poly(&self, pts: &[(f32, f32)], closed: bool) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::Path(PathShape {
            points: pts,
            closed,
            fill: Color32::TRANSPARENT,
            stroke: PathStroke::new(self.stroke.width, self.stroke.color),
        }));
    }
    fn fill_poly(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::convex_polygon(pts, self.color, Stroke::NONE));
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let rect = Rect::from_min_max(self.p(x, y), self.p(x + w, y + h));
        self.painter.rect_stroke(rect, r * self.scale(), self.stroke, StrokeKind::Middle);
    }
    fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let rect = Rect::from_min_max(self.p(x, y), self.p(x + w, y + h));
        self.painter.rect_filled(rect, r * self.scale(), self.color);
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.painter.circle_stroke(self.p(x, y), r * self.scale(), self.stroke);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter.circle_filled(self.p(x, y), r * self.scale(), self.color);
    }
    fn curve(&self, a: (f32, f32), c1: (f32, f32), c2: (f32, f32), b: (f32, f32)) {
        self.painter.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
            [self.p(a.0, a.1), self.p(c1.0, c1.1), self.p(c2.0, c2.1), self.p(b.0, b.1)],
            false,
            Color32::TRANSPARENT,
            self.stroke,
        )));
    }
}

/// Draws `icon` into `rect` with `color`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let pen = Pen { painter, rect, stroke: Stroke::new((1.7 * s).max(1.0), color), color };
    match icon {
        Icon::Select => pen.poly(
            &[(6.0, 3.5), (6.0, 18.5), (10.0, 14.8), (13.2, 21.0), (15.8, 19.7), (12.8, 13.6), (18.0, 13.6)],
            true,
        ),
        Icon::Lasso => {
            pen.curve((6.0, 9.5), (3.0, 4.5), (16.0, 2.5), (19.0, 8.0));
            pen.curve((19.0, 8.0), (22.0, 13.0), (14.0, 17.0), (9.5, 15.5));
            pen.curve((9.5, 15.5), (5.5, 14.5), (4.5, 12.0), (6.0, 9.5));
            pen.curve((9.5, 15.5), (10.5, 18.5), (9.0, 20.5), (6.5, 20.0));
        }
        Icon::Hand => {
            pen.poly(&[(8.0, 12.0), (8.0, 6.0)], false);
            pen.poly(&[(11.0, 11.0), (11.0, 4.5)], false);
            pen.poly(&[(14.0, 11.0), (14.0, 5.5)], false);
            pen.poly(&[(17.0, 12.0), (17.0, 8.0)], false);
            pen.poly(
                &[
                    (8.0, 12.0),
                    (5.5, 10.0),
                    (4.8, 11.5),
                    (8.5, 18.0),
                    (11.0, 20.5),
                    (15.0, 20.5),
                    (17.5, 17.5),
                    (17.0, 12.0),
                ],
                false,
            );
        }
        Icon::Rectangle => pen.rect(4.0, 6.0, 16.0, 12.0, 1.5),
        Icon::Diamond => pen.poly(&[(12.0, 3.5), (21.0, 12.0), (12.0, 20.5), (3.0, 12.0)], true),
        Icon::Ellipse => {
            let pts: Vec<(f32, f32)> = (0..40)
                .map(|i| {
                    let a = i as f32 / 40.0 * std::f32::consts::TAU;
                    (12.0 + 8.5 * a.cos(), 12.0 + 6.5 * a.sin())
                })
                .collect();
            pen.poly(&pts, true);
        }
        Icon::Arrow => {
            pen.line((4.5, 19.0), (19.0, 5.0));
            pen.poly(&[(11.0, 5.0), (19.0, 5.0), (19.0, 13.0)], false);
        }
        Icon::Line => pen.line((4.5, 19.0), (19.5, 5.0)),
        Icon::Freehand => {
            pen.curve((3.5, 17.0), (7.0, 5.0), (10.0, 22.0), (13.0, 12.0));
            pen.curve((13.0, 12.0), (16.0, 3.0), (18.0, 8.0), (20.5, 9.0));
        }
        Icon::Highlighter => {
            pen.fill_poly(&[(14.5, 4.0), (20.0, 9.5), (11.0, 18.5), (5.5, 13.0)]);
            pen.line((4.0, 20.5), (12.0, 20.5));
        }
        Icon::Text => {
            pen.line((6.0, 6.0), (18.0, 6.0));
            pen.line((12.0, 6.0), (12.0, 19.5));
            pen.line((9.0, 19.5), (15.0, 19.5));
        }
        Icon::Math => {
            pen.poly(&[(18.5, 5.0), (7.0, 5.0), (12.5, 12.0), (7.0, 19.0), (18.5, 19.0)], false);
        }
        Icon::Frame => {
            pen.line((8.0, 3.5), (8.0, 20.5));
            pen.line((16.0, 3.5), (16.0, 20.5));
            pen.line((3.5, 8.0), (20.5, 8.0));
            pen.line((3.5, 16.0), (20.5, 16.0));
        }
        Icon::Eraser => {
            pen.poly(&[(4.5, 15.0), (13.0, 6.5), (19.5, 13.0), (13.0, 19.5), (9.0, 19.5)], true);
            pen.line((8.8, 10.7), (15.3, 17.2));
            pen.line((13.0, 19.5), (20.0, 19.5));
        }
        Icon::Laser => {
            pen.dot(15.5, 8.5, 2.2);
            pen.line((4.0, 20.0), (12.5, 11.5));
            pen.line((15.5, 3.5), (15.5, 4.8));
            pen.line((20.5, 8.5), (19.2, 8.5));
            pen.line((19.0, 5.0), (18.0, 6.0));
        }
        Icon::Undo => {
            pen.curve((8.0, 8.0), (14.0, 5.0), (20.0, 9.0), (17.0, 16.0));
            pen.poly(&[(11.0, 4.0), (7.5, 8.0), (11.5, 11.0)], false);
        }
        Icon::Redo => {
            pen.curve((16.0, 8.0), (10.0, 5.0), (4.0, 9.0), (7.0, 16.0));
            pen.poly(&[(13.0, 4.0), (16.5, 8.0), (12.5, 11.0)], false);
        }
        Icon::Center => {
            pen.circle(12.0, 12.0, 5.5);
            pen.dot(12.0, 12.0, 1.6);
            pen.line((12.0, 2.5), (12.0, 5.0));
            pen.line((12.0, 19.0), (12.0, 21.5));
            pen.line((2.5, 12.0), (5.0, 12.0));
            pen.line((19.0, 12.0), (21.5, 12.0));
        }
        Icon::ZoomFit => {
            pen.poly(&[(3.5, 9.0), (3.5, 3.5), (9.0, 3.5)], false);
            pen.poly(&[(15.0, 3.5), (20.5, 3.5), (20.5, 9.0)], false);
            pen.poly(&[(20.5, 15.0), (20.5, 20.5), (15.0, 20.5)], false);
            pen.poly(&[(9.0, 20.5), (3.5, 20.5), (3.5, 15.0)], false);
            pen.rect(8.5, 8.5, 7.0, 7.0, 1.0);
        }
        Icon::SnapGrid => {
            for v in [5.0, 12.0, 19.0] {
                pen.line((v, 4.0), (v, 20.0));
                pen.line((4.0, v), (20.0, v));
            }
            pen.dot(12.0, 12.0, 2.2);
        }
        Icon::SnapShapes => {
            pen.rect(3.5, 4.5, 8.0, 8.0, 1.0);
            pen.rect(13.0, 12.5, 7.5, 7.0, 1.0);
            pen.line((3.5, 21.5), (21.0, 21.5));
        }
        Icon::SnapAngle => {
            pen.line((5.0, 19.0), (19.5, 19.0));
            pen.line((5.0, 19.0), (17.0, 7.0));
            pen.curve((11.0, 19.0), (11.0, 16.5), (12.5, 15.5), (13.5, 14.5));
        }
        Icon::BindArrow => {
            // An arrow that ends on the edge of a box, with a dot where it attaches.
            pen.rect(12.5, 5.5, 8.0, 13.0, 1.0);
            pen.line((3.5, 12.0), (11.0, 12.0));
            pen.poly(&[(8.0, 9.0), (11.0, 12.0), (8.0, 15.0)], false);
            pen.dot(12.5, 12.0, 1.8);
        }
        Icon::GridNone => {
            pen.dot(5.0, 5.0, 0.8);
            pen.dot(19.0, 19.0, 0.8);
            pen.line((4.0, 20.0), (20.0, 4.0));
        }
        Icon::GridLines => {
            for v in [6.0, 12.0, 18.0] {
                pen.line((v, 4.0), (v, 20.0));
                pen.line((4.0, v), (20.0, v));
            }
        }
        Icon::GridHorizontal => {
            for v in [6.0, 12.0, 18.0] {
                pen.line((4.0, v), (20.0, v));
            }
        }
        Icon::GridCrosses => {
            for x in [6.0, 12.0, 18.0] {
                for y in [6.0, 12.0, 18.0] {
                    pen.line((x - 1.6, y), (x + 1.6, y));
                    pen.line((x, y - 1.6), (x, y + 1.6));
                }
            }
        }
        Icon::GridDots => {
            for x in [6.0, 12.0, 18.0] {
                for y in [6.0, 12.0, 18.0] {
                    pen.dot(x, y, 1.1);
                }
            }
        }
        Icon::Menu => {
            for y in [6.5, 12.0, 17.5] {
                pen.line((4.5, y), (19.5, y));
            }
        }
        Icon::Collab => {
            pen.circle(9.0, 8.5, 3.2);
            pen.curve((3.0, 20.0), (3.0, 13.0), (15.0, 13.0), (15.0, 20.0));
            pen.circle(17.0, 9.5, 2.5);
            pen.curve((16.5, 14.0), (20.0, 13.5), (21.5, 16.5), (21.5, 19.0));
        }
        Icon::LayerBack => {
            pen.rect(7.0, 7.0, 13.0, 13.0, 1.5);
            pen.fill_rect(3.5, 3.5, 9.0, 9.0, 1.5);
        }
        Icon::LayerBackward => {
            pen.rect(9.0, 9.0, 11.0, 11.0, 1.5);
            pen.rect(4.0, 4.0, 11.0, 11.0, 1.5);
        }
        Icon::LayerForward => {
            pen.rect(4.0, 4.0, 11.0, 11.0, 1.5);
            pen.rect(9.0, 9.0, 11.0, 11.0, 1.5);
            pen.fill_rect(10.5, 10.5, 8.0, 8.0, 1.0);
        }
        Icon::LayerFront => {
            pen.rect(3.5, 3.5, 13.0, 13.0, 1.5);
            pen.fill_rect(11.5, 11.5, 9.0, 9.0, 1.5);
        }
        Icon::FlipH => {
            pen.line((12.0, 3.0), (12.0, 21.0));
            pen.poly(&[(9.0, 6.0), (3.5, 18.0), (9.0, 18.0)], true);
            pen.poly(&[(15.0, 6.0), (20.5, 18.0), (15.0, 18.0)], true);
        }
        Icon::FlipV => {
            pen.line((3.0, 12.0), (21.0, 12.0));
            pen.poly(&[(6.0, 9.0), (18.0, 3.5), (18.0, 9.0)], true);
            pen.poly(&[(6.0, 15.0), (18.0, 20.5), (18.0, 15.0)], true);
        }
        Icon::AlignLeft => {
            pen.line((4.0, 3.0), (4.0, 21.0));
            pen.rect(7.5, 5.5, 11.0, 5.0, 1.0);
            pen.rect(7.5, 13.5, 6.0, 5.0, 1.0);
        }
        Icon::AlignCenterX => {
            pen.line((12.0, 3.0), (12.0, 21.0));
            pen.rect(5.0, 5.5, 14.0, 5.0, 1.0);
            pen.rect(8.0, 13.5, 8.0, 5.0, 1.0);
        }
        Icon::AlignRight => {
            pen.line((20.0, 3.0), (20.0, 21.0));
            pen.rect(5.5, 5.5, 11.0, 5.0, 1.0);
            pen.rect(10.5, 13.5, 6.0, 5.0, 1.0);
        }
        Icon::AlignTop => {
            pen.line((3.0, 4.0), (21.0, 4.0));
            pen.rect(5.5, 7.5, 5.0, 11.0, 1.0);
            pen.rect(13.5, 7.5, 5.0, 6.0, 1.0);
        }
        Icon::AlignCenterY => {
            pen.line((3.0, 12.0), (21.0, 12.0));
            pen.rect(5.5, 5.0, 5.0, 14.0, 1.0);
            pen.rect(13.5, 8.0, 5.0, 8.0, 1.0);
        }
        Icon::AlignBottom => {
            pen.line((3.0, 20.0), (21.0, 20.0));
            pen.rect(5.5, 5.5, 5.0, 11.0, 1.0);
            pen.rect(13.5, 10.5, 5.0, 6.0, 1.0);
        }
        Icon::DistributeH => {
            pen.line((3.5, 3.0), (3.5, 21.0));
            pen.line((20.5, 3.0), (20.5, 21.0));
            pen.rect(9.5, 6.5, 5.0, 11.0, 1.0);
        }
        Icon::DistributeV => {
            pen.line((3.0, 3.5), (21.0, 3.5));
            pen.line((3.0, 20.5), (21.0, 20.5));
            pen.rect(6.5, 9.5, 11.0, 5.0, 1.0);
        }
        Icon::Group => {
            pen.rect(3.5, 3.5, 17.0, 17.0, 2.0);
            pen.rect(7.0, 7.0, 5.0, 5.0, 1.0);
            pen.rect(12.5, 12.5, 5.0, 5.0, 1.0);
        }
        Icon::Ungroup => {
            pen.rect(3.5, 3.5, 8.0, 8.0, 1.0);
            pen.rect(12.5, 12.5, 8.0, 8.0, 1.0);
        }
        Icon::Duplicate => {
            pen.rect(8.5, 8.5, 12.0, 12.0, 1.5);
            pen.poly(&[(15.5, 5.5), (15.5, 3.5), (3.5, 3.5), (3.5, 15.5), (5.5, 15.5)], false);
        }
        Icon::Delete => {
            pen.line((4.0, 6.5), (20.0, 6.5));
            pen.poly(&[(9.0, 6.5), (9.0, 4.0), (15.0, 4.0), (15.0, 6.5)], false);
            pen.poly(&[(6.0, 6.5), (7.0, 20.5), (17.0, 20.5), (18.0, 6.5)], false);
            pen.line((10.0, 10.0), (10.0, 17.0));
            pen.line((14.0, 10.0), (14.0, 17.0));
        }
        Icon::Lock => {
            pen.rect(5.5, 10.5, 13.0, 10.0, 1.5);
            pen.curve((8.5, 10.5), (8.5, 2.5), (15.5, 2.5), (15.5, 10.5));
        }
        Icon::Unlock => {
            pen.rect(5.5, 10.5, 13.0, 10.0, 1.5);
            pen.curve((8.5, 10.5), (8.5, 2.5), (15.5, 2.5), (15.5, 6.0));
        }
        Icon::Plus => {
            pen.line((12.0, 5.0), (12.0, 19.0));
            pen.line((5.0, 12.0), (19.0, 12.0));
        }
        Icon::Minus => pen.line((5.0, 12.0), (19.0, 12.0)),
        Icon::Close => {
            pen.line((6.0, 6.0), (18.0, 18.0));
            pen.line((18.0, 6.0), (6.0, 18.0));
        }
        Icon::Help => {
            pen.circle(12.0, 12.0, 9.0);
            pen.curve((9.0, 9.5), (9.0, 5.5), (15.0, 5.5), (15.0, 9.5));
            pen.curve((15.0, 9.5), (15.0, 12.0), (12.0, 12.0), (12.0, 14.5));
            pen.dot(12.0, 17.5, 1.0);
        }
        Icon::Search => {
            pen.circle(10.5, 10.5, 6.0);
            pen.line((15.0, 15.0), (20.5, 20.5));
        }
        Icon::Crop => {
            // Two crop marks: an L from the top-left and an L from the bottom-right.
            pen.poly(&[(7.0, 3.0), (7.0, 17.0), (21.0, 17.0)], false);
            pen.poly(&[(3.0, 7.0), (17.0, 7.0), (17.0, 21.0)], false);
        }
        Icon::CropReset => {
            // The crop marks, crossed out by a slash.
            pen.poly(&[(7.0, 3.0), (7.0, 17.0), (21.0, 17.0)], false);
            pen.poly(&[(3.0, 7.0), (17.0, 7.0), (17.0, 21.0)], false);
            pen.line((4.0, 20.0), (20.0, 4.0));
        }
        Icon::Image => {
            pen.rect(3.5, 5.0, 17.0, 14.0, 1.5);
            pen.poly(&[(4.5, 17.0), (9.5, 11.5), (13.0, 15.0), (15.5, 12.5), (19.5, 17.0)], false);
            pen.dot(15.5, 9.0, 1.4);
        }
        Icon::Wheel => {
            for i in 0..24 {
                let a0 = i as f32 / 24.0 * std::f32::consts::TAU;
                let a1 = (i + 1) as f32 / 24.0 * std::f32::consts::TAU;
                let c = hue(i as f32 / 24.0);
                let pts = vec![
                    pen.p(12.0, 12.0),
                    pen.p(12.0 + 11.0 * a0.cos(), 12.0 + 11.0 * a0.sin()),
                    pen.p(12.0 + 11.0 * a1.cos(), 12.0 + 11.0 * a1.sin()),
                ];
                painter.add(Shape::convex_polygon(pts, c, Stroke::NONE));
            }
        }
        Icon::Eyedropper => {
            // The tube from the tip at the lower left to the bulb at the upper right.
            pen.poly(&[(5.8, 14.8), (8.2, 17.2), (15.2, 10.2), (12.8, 7.8)], true);
            pen.line((6.0, 17.0), (3.5, 20.5));
            pen.circle(16.6, 7.4, 3.2);
        }
        Icon::NoColor => {
            pen.circle(12.0, 12.0, 9.0);
            pen.line((5.5, 18.5), (18.5, 5.5));
        }
        Icon::Library => {
            pen.rect(4.0, 4.0, 4.5, 16.0, 1.0);
            pen.rect(10.0, 4.0, 4.5, 16.0, 1.0);
            pen.poly(&[(16.5, 6.5), (20.5, 5.5), (22.0, 18.5), (17.5, 19.5)], true);
        }
        Icon::Sticky => {
            pen.poly(&[(4.5, 4.5), (19.5, 4.5), (19.5, 14.5), (14.5, 19.5), (4.5, 19.5)], true);
            pen.poly(&[(14.5, 19.5), (14.5, 14.5), (19.5, 14.5)], false);
        }
        Icon::Settings => {
            pen.line((4.0, 7.0), (20.0, 7.0));
            pen.line((4.0, 12.0), (20.0, 12.0));
            pen.line((4.0, 17.0), (20.0, 17.0));
            pen.dot(9.0, 7.0, 2.2);
            pen.dot(15.0, 12.0, 2.2);
            pen.dot(8.0, 17.0, 2.2);
        }
        Icon::Export => {
            pen.poly(&[(5.0, 14.0), (5.0, 20.0), (19.0, 20.0), (19.0, 14.0)], false);
            pen.line((12.0, 15.0), (12.0, 4.0));
            pen.poly(&[(7.5, 8.5), (12.0, 4.0), (16.5, 8.5)], false);
        }
        Icon::Eye => {
            pen.curve((2.5, 12.0), (7.0, 4.5), (17.0, 4.5), (21.5, 12.0));
            pen.curve((2.5, 12.0), (7.0, 19.5), (17.0, 19.5), (21.5, 12.0));
            pen.dot(12.0, 12.0, 2.6);
        }
        Icon::Link => {
            pen.curve((10.0, 14.0), (7.0, 17.0), (4.5, 14.5), (7.0, 12.0));
            pen.curve((7.0, 12.0), (9.0, 10.0), (10.0, 9.0), (12.0, 10.5));
            pen.curve((14.0, 10.0), (17.0, 7.0), (19.5, 9.5), (17.0, 12.0));
            pen.curve((17.0, 12.0), (15.0, 14.0), (14.0, 15.0), (12.0, 13.5));
        }
        Icon::Panel => {
            pen.rect(3.5, 4.5, 17.0, 15.0, 1.5);
            pen.line((9.5, 4.5), (9.5, 19.5));
        }
        Icon::Cut => {
            pen.circle(7.0, 17.5, 2.6);
            pen.circle(17.0, 17.5, 2.6);
            pen.line((8.6, 15.4), (16.5, 4.0));
            pen.line((15.4, 15.4), (7.5, 4.0));
        }
        Icon::Copy => {
            pen.rect(9.0, 9.0, 11.0, 11.0, 1.5);
            pen.poly(&[(15.0, 6.0), (15.0, 5.5), (5.5, 5.5), (5.5, 15.0), (6.0, 15.0)], false);
        }
        Icon::Paste => {
            pen.rect(5.0, 5.5, 14.0, 15.0, 1.5);
            pen.rect(9.0, 3.0, 6.0, 4.5, 1.0);
        }
        Icon::SelectAll => {
            pen.poly(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)], false);
            pen.poly(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)], false);
            pen.poly(&[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)], false);
            pen.poly(&[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)], false);
            pen.rect(9.0, 9.0, 6.0, 6.0, 1.0);
        }
        Icon::Layers => {
            pen.poly(&[(12.0, 4.0), (21.0, 9.0), (12.0, 14.0), (3.0, 9.0)], true);
            pen.poly(&[(3.0, 13.0), (12.0, 18.0), (21.0, 13.0)], false);
        }
        Icon::Convert => {
            pen.line((4.0, 8.0), (19.5, 8.0));
            pen.poly(&[(16.0, 4.5), (19.5, 8.0), (16.0, 11.5)], false);
            pen.line((20.0, 16.0), (4.5, 16.0));
            pen.poly(&[(8.0, 12.5), (4.5, 16.0), (8.0, 19.5)], false);
        }
        Icon::Brush => {
            pen.line((18.5, 4.5), (11.0, 12.0));
            pen.curve((11.0, 12.0), (8.0, 11.0), (5.5, 13.5), (5.0, 19.0));
            pen.curve((5.0, 19.0), (10.5, 18.5), (13.5, 16.5), (12.5, 13.0));
        }
        Icon::Save => {
            pen.rect(4.0, 4.0, 16.0, 16.0, 2.0);
            pen.rect(8.0, 4.0, 7.0, 5.0, 0.5);
            pen.rect(7.0, 13.0, 10.0, 7.0, 0.5);
        }
        Icon::Open => {
            pen.poly(&[(3.5, 18.5), (3.5, 6.0), (9.0, 6.0), (11.0, 8.5), (19.5, 8.5), (19.5, 18.5)], true);
            pen.line((3.5, 18.5), (6.0, 12.5));
            pen.line((6.0, 12.5), (21.0, 12.5));
        }
        Icon::Info => {
            pen.circle(12.0, 12.0, 8.5);
            pen.line((12.0, 11.0), (12.0, 16.5));
            pen.dot(12.0, 7.8, 1.1);
        }
        Icon::Keyboard => {
            pen.rect(3.0, 6.5, 18.0, 11.0, 1.5);
            pen.dot(7.0, 10.0, 0.9);
            pen.dot(11.0, 10.0, 0.9);
            pen.dot(15.0, 10.0, 0.9);
            pen.dot(18.0, 10.0, 0.9);
            pen.line((8.0, 14.5), (16.0, 14.5));
        }
        Icon::Stats => {
            pen.line((6.0, 19.0), (6.0, 12.0));
            pen.line((12.0, 19.0), (12.0, 6.0));
            pen.line((18.0, 19.0), (18.0, 10.0));
        }
        Icon::Check => pen.poly(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)], false),
        Icon::ChevronRight => pen.poly(&[(9.0, 5.5), (15.5, 12.0), (9.0, 18.5)], false),
        Icon::ChevronDown => pen.poly(&[(5.5, 9.0), (12.0, 15.5), (18.5, 9.0)], false),
    }
}

/// A fully saturated colour for a hue from 0 to 1.
pub fn hue(h: f32) -> Color32 {
    let h6 = (h.fract() * 6.0).max(0.0);
    let f = h6.fract();
    let (r, g, b) = match h6 as u32 {
        0 => (1.0, f, 0.0),
        1 => (1.0 - f, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, 1.0 - f, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - f),
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}
