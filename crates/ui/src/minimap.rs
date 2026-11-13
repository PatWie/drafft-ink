//! The minimap: what it draws and how a pointer on it moves the view. No egui widgets here, only data and maths, so
//! the tests can check them without a window (`surfaces/minimap.rs` draws and reads the pointer).
//!
//! * [`Scene`] is the document reduced to [`Mark`]s: the box of each element with the colour it is drawn in. It is
//!   built once per document change and never per frame. A document with more than [`MAX_MARKS`] elements is merged
//!   into a coarse grid first, so the cost of a frame has an upper bound whatever the size of the document.
//! * [`Mapping`] is the one place that turns world units into points of the card and back.
//! * [`Grab`] is a press or drag on the card: where it puts the camera.
//!
//! The camera is the one of `view.get` ([`ViewView`]): `scroll` is the world point at the middle of the viewport and
//! `zoom` is screen points per world unit.

use drafftink_scene::{Color, Element, ElementKind, SceneRead};
use egui::{Color32, Pos2, Rect, pos2};

use crate::colors::to_egui;
use crate::model::ViewView;

/// A document with more elements than this is merged into a grid before it is drawn.
pub const MAX_MARKS: usize = 2048;
/// The most marks of a merged document that are drawn one by one: the elements bigger than a few cells (frames,
/// backgrounds), which a cell cannot stand for.
const MAX_LARGE: usize = 512;
/// Columns and rows of the grid a large document is merged into. A cell is a few points on the card.
const GRID: (usize, usize) = (48, 32);
/// Text and handwriting are drawn as a block of their colour at this share of full strength, so that a line of text
/// reads as grey ink and not as a solid bar.
const INK_STRENGTH: f64 = 0.55;
/// The narrowest world extent the mapping accepts, so that an empty extent cannot divide by zero.
const MIN_SPAN: f64 = 1.0;
/// The zoom a wheel asks for per point scrolled: a notch of 50 points is `e^0.1`, about the 1.1 of a canvas wheel.
const ZOOM_PER_POINT: f64 = 0.002;
/// The most the zoom changes in one frame, up or down.
const MAX_ZOOM_STEP: f64 = 2.0;

/// A rectangle in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRect {
    /// Left and top.
    pub min: [f64; 2],
    /// Right and bottom.
    pub max: [f64; 2],
}

impl WorldRect {
    /// The rectangle through two corners, in any order. `None` when a coordinate is not a finite number.
    pub fn new(a: [f64; 2], b: [f64; 2]) -> Option<Self> {
        let finite = a.iter().chain(&b).all(|v| v.is_finite());
        finite.then(|| Self { min: [a[0].min(b[0]), a[1].min(b[1])], max: [a[0].max(b[0]), a[1].max(b[1])] })
    }

    /// The rectangle of this size around a point.
    pub fn around(center: [f64; 2], size: [f64; 2]) -> Self {
        let half = [size[0].abs() / 2.0, size[1].abs() / 2.0];
        Self { min: [center[0] - half[0], center[1] - half[1]], max: [center[0] + half[0], center[1] + half[1]] }
    }

    /// The smallest rectangle that holds both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    /// Width and height.
    pub fn size(self) -> [f64; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }

    /// The middle.
    pub fn center(self) -> [f64; 2] {
        [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0]
    }

    /// True when the point is inside or on the edge.
    pub fn contains(self, p: [f64; 2]) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }
}

/// The part of the world the camera shows.
pub fn visible(view: &ViewView) -> WorldRect {
    let zoom = if view.zoom > 0.0 { view.zoom } else { 1.0 };
    WorldRect::around(view.scroll, [view.width / zoom, view.height / zoom])
}

/// A colour as the canvas draws it in the light theme and in the dark theme, worked out once so that a frame needs
/// no conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    light: Color32,
    dark: Color32,
}

impl Tint {
    /// Images show as this grey: the picture itself is not worth decoding for a card this small.
    const PICTURE: Tint = Tint {
        light: Color32::from_rgba_premultiplied(72, 72, 76, 150),
        dark: Color32::from_rgba_premultiplied(54, 54, 58, 150),
    };

    /// A stored colour at some strength (the element's opacity), in both themes.
    fn of(color: Color, strength: f64) -> Self {
        let alpha = (f64::from(color.a) * strength.clamp(0.0, 1.0)).round() as u8;
        let faded = Color::rgba(color.r, color.g, color.b, alpha);
        Self { light: to_egui(faded), dark: to_egui(faded.for_dark_theme()) }
    }

    /// The colour for a theme.
    pub fn pick(self, dark: bool) -> Color32 {
        if dark { self.dark } else { self.light }
    }
}

/// How one element is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    /// A filled box: a shape with a fill, a text, a picture, a pen stroke.
    Fill(Tint),
    /// An outline: a shape or frame without a fill, so that what lies inside it stays visible.
    Outline(Tint),
    /// A line or arrow: the segment from its first to its last point.
    Line {
        /// The colour.
        tint: Tint,
        /// The first point, in world units.
        from: [f64; 2],
        /// The last point, in world units.
        to: [f64; 2],
    },
}

impl Look {
    fn tint(self) -> Tint {
        match self {
            Look::Fill(tint) | Look::Outline(tint) | Look::Line { tint, .. } => tint,
        }
    }
}

/// One element, or one cell of merged elements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    /// The box in world units: the axis-aligned bounds of the rotated box.
    pub bounds: WorldRect,
    /// How it is drawn.
    pub look: Look,
}

impl Mark {
    /// The mark of an element, or `None` for one without usable bounds.
    fn of(element: &Element) -> Option<Self> {
        let r = element.transform.obox().aabb();
        let bounds = WorldRect::new([r.x0, r.y0], [r.x1, r.y1])?;
        Some(Self { bounds, look: look_of(element) })
    }
}

fn look_of(element: &Element) -> Look {
    let style = &element.style;
    let stroke = Tint::of(style.stroke_color, style.opacity);
    let ink = Tint::of(style.stroke_color, style.opacity * INK_STRENGTH);
    let filled = style.fill_color.filter(|fill| fill.a > 0).map(|fill| Tint::of(fill, style.opacity));
    match &element.kind {
        ElementKind::Image(_) => Look::Fill(Tint::PICTURE),
        ElementKind::Text(_) | ElementKind::Math(_) => Look::Fill(ink),
        ElementKind::Freehand(_) => Look::Fill(filled.unwrap_or(ink)),
        ElementKind::Line(data) | ElementKind::Arrow(data) => {
            let ends = data.points.first().zip(data.points.last()).filter(|_| data.points.len() > 1);
            match ends.map(|(a, b)| (element.transform.local_to_world(*a), element.transform.local_to_world(*b))) {
                Some((a, b)) => Look::Line { tint: stroke, from: [a.x, a.y], to: [b.x, b.y] },
                None => Look::Fill(ink),
            }
        }
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse | ElementKind::Frame(_) => {
            filled.map_or(Look::Outline(stroke), Look::Fill)
        }
    }
}

/// The document as the minimap draws it.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    marks: Vec<Mark>,
    content: Option<WorldRect>,
    elements: usize,
}

impl Scene {
    /// Reduces the live elements of a store, back to front. A text bound to a shape is left out (the shape stands
    /// for it). More than [`MAX_MARKS`] elements are merged into a grid.
    pub fn build<R: SceneRead>(store: &R) -> Self {
        let mut marks: Vec<Mark> = store.iter().filter(|e| e.container_id().is_none()).filter_map(Mark::of).collect();
        let elements = marks.len();
        let content = marks.iter().map(|m| m.bounds).reduce(WorldRect::union);
        if let Some(content) = content.filter(|_| elements > MAX_MARKS) {
            marks = merge(marks, content);
        }
        Self { marks, content, elements }
    }

    /// What to draw, back to front.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// The box around everything, `None` for an empty document.
    pub fn content(&self) -> Option<WorldRect> {
        self.content
    }

    /// How many elements the scene stands for: more than [`Scene::marks`] when a large document was merged.
    pub fn elements(&self) -> usize {
        self.elements
    }

    /// What the card shows: the content and the part of the world the camera shows. Panning off the content moves
    /// the camera's rectangle across the card, and zooming far out shrinks the content, so the rectangle is always on
    /// the card.
    pub fn extent(&self, view: &ViewView) -> WorldRect {
        let seen = visible(view);
        self.content.map_or(seen, |content| content.union(seen))
    }
}

/// Merges the marks of a large document: the small ones into the cells of a grid over the content (the last one in a
/// cell is the topmost and gives the cell its colour), the large ones kept as they are up to [`MAX_LARGE`].
fn merge(marks: Vec<Mark>, content: WorldRect) -> Vec<Mark> {
    let (columns, rows) = GRID;
    let [width, height] = content.size();
    let (cell_w, cell_h) = (width.max(MIN_SPAN) / columns as f64, height.max(MIN_SPAN) / rows as f64);
    let mut cells: Vec<Option<Tint>> = vec![None; columns * rows];
    let mut kept = Vec::new();
    for mark in marks {
        let [w, h] = mark.bounds.size();
        if (w > 2.0 * cell_w || h > 2.0 * cell_h) && kept.len() < MAX_LARGE {
            kept.push(mark);
            continue;
        }
        let [x, y] = mark.bounds.center();
        // A float to usize cast saturates, so a coordinate left of the content lands in column 0.
        let column = (((x - content.min[0]) / cell_w) as usize).min(columns - 1);
        let row = (((y - content.min[1]) / cell_h) as usize).min(rows - 1);
        cells[row * columns + column] = Some(mark.look.tint());
    }
    let cell_marks = cells.iter().enumerate().filter_map(|(i, tint)| {
        let (column, row) = (i % columns, i / columns);
        let min = [content.min[0] + column as f64 * cell_w, content.min[1] + row as f64 * cell_h];
        let bounds = WorldRect { min, max: [min[0] + cell_w, min[1] + cell_h] };
        tint.map(|tint| Mark { bounds, look: Look::Fill(tint) })
    });
    kept.extend(cell_marks);
    kept
}

/// What a scene was built from: the tab, the version of its elements and how many there are. A scene is stale when
/// this differs.
pub type Stamp = (u32, Option<u64>, usize);

/// What the model keeps for the minimap: the reduced document and the camera. Empty, and costing nothing, while the
/// minimap is off.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The minimap is switched on (the setting `view.minimap`).
    pub on: bool,
    /// The document.
    pub scene: Scene,
    /// The camera: the app's live one while it tells it, else the session's.
    pub camera: ViewView,
    stamp: Option<Stamp>,
}

impl Snapshot {
    /// Brings the snapshot up to date. Off: drops everything. On: follows the camera every time, and builds the scene
    /// again only when `stamp` differs from the one it was built at (`build` is not called otherwise). While a gesture
    /// runs (`dragging`: a drag, a resize, a text edit) the scene of a large document is left as it was, because the
    /// document changes every frame and the rebuild is a pass over all of it; the gesture's end changes the stamp
    /// once more and the scene catches up in the frame after it.
    pub fn sync(&mut self, on: bool, stamp: Stamp, camera: ViewView, dragging: bool, build: impl FnOnce() -> Scene) {
        if !on {
            if self.on {
                *self = Self::default();
            }
            return;
        }
        self.on = true;
        self.camera = camera;
        let large = self.scene.elements() > MAX_MARKS;
        if self.stamp != Some(stamp) && !(dragging && large) {
            self.scene = build();
            self.stamp = Some(stamp);
        }
    }
}

/// World units to points of the card and back: the extent fitted into the area, centred, keeping the proportions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    /// The world point shown at the middle of the area.
    anchor: [f64; 2],
    /// Points per world unit.
    scale: f64,
    /// The middle of the area, in points.
    centre: Pos2,
}

impl Mapping {
    /// Fits `world` into `area`. `None` for an area without room or an extent that is not a number.
    pub fn fit(world: WorldRect, area: Rect) -> Option<Self> {
        let [w, h] = world.size();
        let [x, y] = world.center();
        let scale = (f64::from(area.width()) / w.max(MIN_SPAN)).min(f64::from(area.height()) / h.max(MIN_SPAN));
        // `f64::max` skips a NaN, so the extent itself must be checked.
        let usable = [w, h, x, y, scale].iter().all(|v| v.is_finite()) && scale > 0.0;
        usable.then(|| Self { anchor: [x, y], scale, centre: area.center() })
    }

    /// The point of the card that shows a world point.
    pub fn to_card(&self, p: [f64; 2]) -> Pos2 {
        let axis = |centre: f32, p: f64, anchor: f64| (f64::from(centre) + (p - anchor) * self.scale) as f32;
        pos2(axis(self.centre.x, p[0], self.anchor[0]), axis(self.centre.y, p[1], self.anchor[1]))
    }

    /// The world point under a point of the card.
    pub fn to_world(&self, p: Pos2) -> [f64; 2] {
        let axis = |p: f32, centre: f32, anchor: f64| anchor + (f64::from(p) - f64::from(centre)) / self.scale;
        [axis(p.x, self.centre.x, self.anchor[0]), axis(p.y, self.centre.y, self.anchor[1])]
    }

    /// The rectangle of the card that shows a world rectangle.
    pub fn rect_to_card(&self, r: WorldRect) -> Rect {
        Rect::from_min_max(self.to_card(r.min), self.to_card(r.max))
    }
}

/// A press and the drag after it. The mapping stays as it was at the press: the extent grows with the camera's
/// rectangle, and a mapping that followed it would slide the content under the pointer while it drags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    mapping: Mapping,
    card: Rect,
    /// The camera centre minus the world point that was pressed, when the press was inside the camera's rectangle: the
    /// rectangle then keeps its place under the pointer. Zero otherwise: the rectangle centres on the pointer.
    offset: [f64; 2],
    target: [f64; 2],
}

impl Grab {
    /// A press at `pointer` on `card`, which is drawn with `mapping`, while the camera is `view`. The target is the
    /// camera's own centre until [`Grab::aim`] moves it.
    pub fn begin(mapping: Mapping, card: Rect, view: &ViewView, pointer: Pos2) -> Self {
        let at = mapping.to_world(card.clamp(pointer));
        let offset =
            if visible(view).contains(at) { [view.scroll[0] - at[0], view.scroll[1] - at[1]] } else { [0.0; 2] };
        Self { mapping, card, offset, target: view.scroll }
    }

    /// Moves the target to where `pointer` puts the camera (a pointer off the card counts as on its edge) and returns
    /// it.
    pub fn aim(&mut self, pointer: Pos2) -> [f64; 2] {
        let at = self.mapping.to_world(self.card.clamp(pointer));
        self.target = [at[0] + self.offset[0], at[1] + self.offset[1]];
        self.target
    }

    /// The camera centre the pointer asked for last.
    pub fn target(&self) -> [f64; 2] {
        self.target
    }

    /// The mapping the card is drawn with while the grab lasts.
    pub fn mapping(&self) -> Mapping {
        self.mapping
    }
}

/// The factor `view.zoom_at` takes for a frame of input over the card: a wheel that scrolled `scroll` points up
/// (negative: down) and a pinch or Ctrl+wheel that scaled by `pinch` (egui's `zoom_delta`, 1 for none). `None` when
/// the zoom would not change or a value is not a number.
pub fn zoom_factor(scroll: f32, pinch: f32) -> Option<f64> {
    let (scroll, pinch) = (f64::from(scroll), f64::from(pinch));
    let limit = MAX_ZOOM_STEP.ln();
    let factor = (scroll * ZOOM_PER_POINT).clamp(-limit, limit).exp() * pinch;
    (factor.is_finite() && factor != 1.0 && pinch > 0.0).then(|| factor.clamp(1.0 / MAX_ZOOM_STEP, MAX_ZOOM_STEP))
}

#[cfg(test)]
mod tests {
    use egui::vec2;

    use super::*;

    fn view(scroll: [f64; 2], zoom: f64) -> ViewView {
        ViewView { scroll, zoom, width: 800.0, height: 600.0 }
    }

    fn card() -> Rect {
        Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0))
    }

    fn wide_world() -> Mapping {
        Mapping::fit(WorldRect { min: [0.0, 0.0], max: [2000.0, 1000.0] }, card()).unwrap()
    }

    #[test]
    fn the_visible_part_is_centred_on_the_scroll_and_sized_by_the_zoom() {
        let seen = visible(&view([10.0, 20.0], 2.0));
        assert_eq!(seen, WorldRect { min: [-190.0, -130.0], max: [210.0, 170.0] });
        assert_eq!(visible(&view([0.0, 0.0], 0.0)).size(), [800.0, 600.0], "a zoom that is not positive reads as 1");
    }

    #[test]
    fn a_rectangle_needs_finite_corners_and_orders_them() {
        assert_eq!(WorldRect::new([5.0, 6.0], [1.0, 2.0]), Some(WorldRect { min: [1.0, 2.0], max: [5.0, 6.0] }));
        assert_eq!(WorldRect::new([f64::NAN, 0.0], [1.0, 1.0]), None);
        assert_eq!(WorldRect::new([0.0, f64::INFINITY], [1.0, 1.0]), None);
    }

    #[test]
    fn the_extent_fits_the_card_keeps_the_proportions_and_is_centred() {
        let m = Mapping::fit(WorldRect { min: [0.0, 0.0], max: [400.0, 100.0] }, card()).unwrap();
        // 200 points for 400 units, so 0.5 a unit; the 50 point high strip is centred in the 100 point card.
        assert_eq!(m.to_card([0.0, 0.0]), pos2(100.0, 125.0));
        assert_eq!(m.to_card([400.0, 100.0]), pos2(300.0, 175.0));
        let tall = Mapping::fit(WorldRect { min: [0.0, 0.0], max: [100.0, 400.0] }, card()).unwrap();
        // 100 points for 400 units: 0.25 a unit, so the world is 25 points wide and sits in the middle of the 200.
        assert_eq!(tall.to_card([0.0, 0.0]), pos2(187.5, 100.0), "limited by the height, centred across");
    }

    #[test]
    fn world_and_card_points_convert_both_ways() {
        let world = WorldRect { min: [-1000.0, -500.0], max: [3000.0, 1500.0] };
        let m = Mapping::fit(world, card()).unwrap();
        for p in [[0.0, 0.0], [-1000.0, -500.0], [2999.0, 1499.5], [12.25, -77.5]] {
            let back = m.to_world(m.to_card(p));
            assert!((back[0] - p[0]).abs() < 0.1 && (back[1] - p[1]).abs() < 0.1, "{p:?} came back as {back:?}");
        }
        assert_eq!(m.to_card(world.center()), card().center(), "the middle of the world is the middle of the card");
    }

    #[test]
    fn a_degenerate_extent_or_card_has_no_mapping() {
        let point = WorldRect { min: [5.0, 5.0], max: [5.0, 5.0] };
        assert!(Mapping::fit(point, card()).is_some(), "a point is given a minimum span");
        assert!(Mapping::fit(point, Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, 50.0))).is_none());
        let nan = WorldRect { min: [f64::NAN, 0.0], max: [1.0, 1.0] };
        assert!(Mapping::fit(nan, card()).is_none());
    }

    #[test]
    fn a_press_outside_the_rectangle_centres_the_camera_on_the_pointer() {
        let m = wide_world();
        let v = view([400.0, 300.0], 1.0); // sees x 0..800, y 0..600
        let far = m.to_card([1500.0, 800.0]);
        let mut grab = Grab::begin(m, card(), &v, far);
        let target = grab.aim(far);
        assert!((target[0] - 1500.0).abs() < 0.1 && (target[1] - 800.0).abs() < 0.1, "{target:?}");
        assert_eq!(grab.target(), target);
    }

    #[test]
    fn a_press_inside_the_rectangle_keeps_its_place_under_the_pointer() {
        let m = wide_world();
        let v = view([400.0, 300.0], 1.0);
        let press = m.to_card([600.0, 300.0]); // inside the rectangle, right of its centre
        let mut grab = Grab::begin(m, card(), &v, press);
        let at_press = grab.aim(press);
        assert!((at_press[0] - 400.0).abs() < 0.1 && (at_press[1] - 300.0).abs() < 0.1, "no jump: {at_press:?}");
        // The card shows 2000 units in 200 points: 10 units a point.
        let moved = grab.aim(press + vec2(20.0, 0.0));
        assert!((moved[0] - 600.0).abs() < 0.5 && (moved[1] - 300.0).abs() < 0.1, "{moved:?}");
    }

    #[test]
    fn dragging_off_the_card_holds_the_pointer_on_its_edge() {
        let mut grab = Grab::begin(wide_world(), card(), &view([400.0, 300.0], 1.0), card().center());
        let edge = grab.aim(pos2(card().right(), card().center().y));
        let beyond = grab.aim(pos2(card().right() + 500.0, card().center().y));
        assert_eq!(edge, beyond, "past the edge is the edge");
    }

    #[test]
    fn the_wheel_zooms_in_going_up_and_out_going_down_and_is_bounded() {
        let notch = zoom_factor(50.0, 1.0).unwrap();
        assert!((notch - 1.105).abs() < 0.01, "a 50 point notch is about 1.1: {notch}");
        let down = zoom_factor(-50.0, 1.0).unwrap();
        assert!((down * notch - 1.0).abs() < 1e-9, "down undoes up");
        assert_eq!(zoom_factor(1.0e6, 1.0), Some(MAX_ZOOM_STEP));
        assert_eq!(zoom_factor(-1.0e6, 1.0), Some(1.0 / MAX_ZOOM_STEP));
        assert_eq!(zoom_factor(0.0, 1.0), None, "no movement, no zoom");
        assert_eq!(zoom_factor(f32::NAN, 1.0), None);
    }

    #[test]
    fn a_pinch_scales_the_zoom_by_its_own_factor() {
        assert!((zoom_factor(0.0, 1.25).unwrap() - 1.25).abs() < 1e-6);
        assert_eq!(zoom_factor(0.0, 0.0), None, "a pinch is never zero or negative");
        assert_eq!(zoom_factor(0.0, -1.0), None);
        assert_eq!(zoom_factor(0.0, f32::INFINITY), None);
    }

    #[test]
    fn the_snapshot_builds_when_on_and_when_the_stamp_changes_and_is_empty_when_off() {
        let mut snap = Snapshot::default();
        let mut builds = 0;
        let mut sync = |snap: &mut Snapshot, on: bool, stamp: Stamp, camera: ViewView| {
            snap.sync(on, stamp, camera, false, || {
                builds += 1;
                Scene::default()
            });
        };
        sync(&mut snap, false, (0, Some(1), 0), ViewView::default());
        sync(&mut snap, true, (0, Some(1), 0), ViewView::default());
        sync(&mut snap, true, (0, Some(1), 0), view([9.0, 9.0], 1.0));
        assert_eq!(snap.camera.scroll, [9.0, 9.0], "the camera follows every time");
        sync(&mut snap, true, (0, Some(2), 0), ViewView::default());
        sync(&mut snap, true, (1, Some(2), 0), ViewView::default());
        sync(&mut snap, false, (1, Some(2), 0), ViewView::default());
        assert!(!snap.on && snap.scene.marks().is_empty(), "off drops the scene");
        sync(&mut snap, true, (1, Some(2), 0), ViewView::default());
        // Off built nothing, the same stamp built once, a new revision and a new tab built again, and so did on again.
        assert_eq!(builds, 4);
    }

    #[test]
    fn a_large_scene_waits_for_the_end_of_a_gesture_and_a_small_one_follows_it() {
        let large = Scene { marks: Vec::new(), content: None, elements: MAX_MARKS + 1 };
        let mut snap = Snapshot::default();
        snap.sync(true, (0, Some(1), 0), ViewView::default(), false, || large.clone());
        let mut builds = 0;
        let mut during = |snap: &mut Snapshot, stamp: Stamp, dragging: bool| {
            snap.sync(true, stamp, ViewView::default(), dragging, || {
                builds += 1;
                Scene::default()
            });
        };
        during(&mut snap, (0, Some(2), 0), true);
        during(&mut snap, (0, Some(3), 0), true);
        assert_eq!(snap.scene.elements(), MAX_MARKS + 1, "the big picture stays while the gesture runs");
        during(&mut snap, (0, Some(4), 0), false);
        assert_eq!(snap.scene.elements(), 0, "and catches up when it ends");

        let mut small = Snapshot::default();
        during(&mut small, (0, Some(1), 0), false);
        during(&mut small, (0, Some(2), 0), true);
        assert_eq!(builds, 3, "a small scene is rebuilt on every change, gesture or not");
    }
}
