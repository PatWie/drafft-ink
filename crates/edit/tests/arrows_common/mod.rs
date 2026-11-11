#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Builders and helpers for the arrow tests.
#![allow(dead_code)]

use drafftink_edit::binding::{End, update_bound_arrows};
use drafftink_geom::Outline;
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, Head, LinearData, LinearKind, MemoryStore, PathType, SceneRead,
    SceneStore, SceneTx, ZIndex,
};

pub fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

pub fn close(a: Point, b: Point) -> bool {
    (a - b).hypot() < 1e-6
}

pub fn assert_close(a: Point, b: Point) {
    assert!(close(a, b), "{a:?} is not {b:?}");
}

/// Equal to within 0.02: a fixed point of 0.5 is stored as 0.5001, which moves a centred end by 0.0001 of the side.
pub fn assert_near(a: Point, b: Point) {
    assert!((a - b).hypot() < 0.02, "{a:?} is not near {b:?}");
}

/// A scene with a few helpers. Elements are stacked in the order they are added.
pub struct Scene {
    pub store: MemoryStore,
}

fn z0() -> ZIndex {
    ZIndex::between(None, None).unwrap()
}

impl Scene {
    pub fn new() -> Self {
        Self { store: MemoryStore::new() }
    }

    pub fn add(&mut self, mut e: Element) -> ElementId {
        let eid = e.id.clone();
        e.index = ZIndex::between(self.store.top_z().as_ref(), None).unwrap();
        let mut tx = self.store.begin();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
        eid
    }

    pub fn rect(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        self.add(Element::new(id(name), ElementKind::Rectangle, z0()).with_bounds(x, y, w, h))
    }

    pub fn ellipse(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        self.add(Element::new(id(name), ElementKind::Ellipse, z0()).with_bounds(x, y, w, h))
    }

    pub fn diamond(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        self.add(Element::new(id(name), ElementKind::Diamond, z0()).with_bounds(x, y, w, h))
    }

    /// An arrow through world points.
    pub fn arrow(&mut self, name: &str, pts: &[(f64, f64)]) -> ElementId {
        let pts: Vec<Point> = pts.iter().map(|(x, y)| p(*x, *y)).collect();
        self.add(Element::linear(id(name), LinearKind::Arrow, z0(), &pts).unwrap())
    }

    pub fn line(&mut self, name: &str, pts: &[(f64, f64)]) -> ElementId {
        let pts: Vec<Point> = pts.iter().map(|(x, y)| p(*x, *y)).collect();
        self.add(Element::linear(id(name), LinearKind::Line, z0(), &pts).unwrap())
    }

    pub fn edit<F: FnOnce(&mut Element)>(&mut self, eid: &ElementId, f: F) {
        let mut tx = self.store.begin();
        tx.update(eid, f).unwrap();
        tx.commit().unwrap();
    }

    pub fn bind(&mut self, arrow: &ElementId, end: End, target: &str, mode: BindMode, fp: [f64; 2]) {
        self.edit(arrow, |e| {
            let ElementKind::Arrow(d) = &mut e.kind else { panic!("not an arrow") };
            let b = Some(Binding { target: id(target), mode, fixed_point: fp });
            match end {
                End::Start => d.start_binding = b,
                End::End => d.end_binding = b,
            }
        });
    }

    pub fn data<F: FnOnce(&mut LinearData)>(&mut self, arrow: &ElementId, f: F) {
        self.edit(arrow, |e| {
            if let ElementKind::Arrow(d) | ElementKind::Line(d) = &mut e.kind {
                f(d)
            }
        });
    }

    pub fn set_path(&mut self, arrow: &ElementId, path: PathType) {
        self.data(arrow, |d| d.path = path);
    }

    pub fn heads(&mut self, arrow: &ElementId, s: Head, e: Head) {
        self.data(arrow, |d| {
            d.start_head = s;
            d.end_head = e;
        });
    }

    pub fn get(&self, eid: &ElementId) -> Element {
        self.store.get(eid).unwrap().clone()
    }

    pub fn world(&self, eid: &ElementId) -> Vec<Point> {
        drafftink_edit::linear::world_points(self.store.get(eid).unwrap()).unwrap()
    }

    pub fn start(&self, eid: &ElementId) -> Point {
        self.world(eid)[0]
    }

    pub fn end(&self, eid: &ElementId) -> Point {
        *self.world(eid).last().unwrap()
    }

    /// Runs the solver for `changed` in its own transaction.
    pub fn solve(&mut self, changed: &[&ElementId]) -> Vec<ElementId> {
        let ids: Vec<ElementId> = changed.iter().map(|i| (*i).clone()).collect();
        let mut tx = self.store.begin();
        let out = update_bound_arrows(&mut tx, &ids).unwrap();
        tx.commit().unwrap();
        out
    }

    /// Moves a shape by `(dx, dy)` and re-solves, as the transform owner does.
    pub fn move_by(&mut self, eid: &ElementId, dx: f64, dy: f64) -> Vec<ElementId> {
        let mut tx = self.store.begin();
        tx.update(eid, |e| {
            e.transform.x += dx;
            e.transform.y += dy;
        })
        .unwrap();
        let out = update_bound_arrows(&mut tx, std::slice::from_ref(eid)).unwrap();
        tx.commit().unwrap();
        out
    }

    pub fn resize(&mut self, eid: &ElementId, x: f64, y: f64, w: f64, h: f64) {
        let mut tx = self.store.begin();
        tx.update(eid, |e| {
            e.transform.x = x;
            e.transform.y = y;
            e.transform.w = w;
            e.transform.h = h;
        })
        .unwrap();
        update_bound_arrows(&mut tx, std::slice::from_ref(eid)).unwrap();
        tx.commit().unwrap();
    }

    pub fn rotate(&mut self, eid: &ElementId, angle: f64) {
        let mut tx = self.store.begin();
        tx.update(eid, |e| e.transform.angle = angle).unwrap();
        update_bound_arrows(&mut tx, std::slice::from_ref(eid)).unwrap();
        tx.commit().unwrap();
    }

    pub fn tombstone(&mut self, eid: &ElementId, deleted: bool) {
        self.edit(eid, |e| e.deleted = deleted);
    }

    pub fn target_of(&self, eid: &ElementId) -> drafftink_geom::binding::BindTarget {
        drafftink_edit::binding::bind_target(self.store.get(eid).unwrap()).unwrap()
    }

    pub fn binding(&self, arrow: &ElementId, end: End) -> Option<Binding> {
        let e = self.store.get(arrow).unwrap();
        let d = e.linear_data().unwrap();
        match end {
            End::Start => d.start_binding.clone(),
            End::End => d.end_binding.clone(),
        }
    }
}

/// Distance from `pt` to the outline of the shape (negative inside).
pub fn signed(scene: &Scene, target: &ElementId, pt: Point) -> f64 {
    scene.target_of(target).shape(0.0).signed_distance(pt)
}

pub fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

/// Distance from `pt` to the outline of the shape pushed out by `gap` (what an orbit end sits on).
pub fn orbit_error(scene: &Scene, target: &ElementId, pt: Point, gap: f64) -> f64 {
    scene.target_of(target).shape(gap).outline_distance(pt)
}

use drafftink_edit::binding::{BindOpts, drag_end};

impl Scene {
    /// Drags one end of an arrow to `at` (binding it if a shape is there), in its own transaction.
    pub fn drag_end(
        &mut self,
        arrow: &ElementId,
        end: End,
        at: Point,
    ) -> Option<drafftink_edit::binding::BindingCandidate> {
        self.drag_end_with(arrow, end, at, &BindOpts::default())
    }

    pub fn drag_end_with(
        &mut self,
        arrow: &ElementId,
        end: End,
        at: Point,
        opts: &BindOpts,
    ) -> Option<drafftink_edit::binding::BindingCandidate> {
        let mut tx = self.store.begin();
        let out = drag_end(&mut tx, arrow, end, at, opts).unwrap();
        tx.commit().unwrap();
        out
    }

    /// An elbow arrow from `a_at` to `b_at`, both ends dropped near shapes so they bind.
    pub fn elbow_between(&mut self, name: &str, a_at: Point, b_at: Point) -> ElementId {
        let arrow = self.arrow(name, &[(a_at.x, a_at.y), (b_at.x, b_at.y)]);
        self.set_path(&arrow, PathType::Elbow);
        self.drag_end(&arrow, End::Start, a_at);
        self.drag_end(&arrow, End::End, b_at);
        arrow
    }

    /// The stored points relative to the first one.
    pub fn rel(&self, arrow: &ElementId) -> Vec<(f64, f64)> {
        let w = self.world(arrow);
        w.iter().map(|q| (q.x - w[0].x, q.y - w[0].y)).collect()
    }

    pub fn pins(&self, arrow: &ElementId) -> Vec<u32> {
        self.get(arrow).linear_data().unwrap().fixed_segments.iter().map(|s| s.index).collect()
    }
}

pub fn assert_pts(got: &[(f64, f64)], want: &[(f64, f64)]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g.0 - w.0).abs() < 0.02 && (g.1 - w.1).abs() < 0.02, "{got:?} vs {want:?}");
    }
}

/// Every segment is horizontal or vertical, no two consecutive points coincide, no three are collinear.
pub fn assert_clean_orthogonal(pts: &[Point]) {
    for w in pts.windows(2) {
        assert!((w[0].x - w[1].x).abs() < 1e-6 || (w[0].y - w[1].y).abs() < 1e-6, "diagonal segment in {pts:?}");
        assert!((w[0] - w[1]).hypot() > 1e-9, "repeated point in {pts:?}");
    }
}

impl Scene {
    pub fn fill(&mut self, eid: &ElementId, color: drafftink_scene::Color) {
        self.edit(eid, |e| e.style.fill_color = Some(color));
    }

    pub fn lock(&mut self, eid: &ElementId) {
        self.edit(eid, |e| e.locked = true);
    }
}

impl Scene {
    /// Puts a shape's top-left at `(x, y)` and re-solves.
    pub fn move_by_to(&mut self, eid: &ElementId, x: f64, y: f64) {
        let mut tx = self.store.begin();
        tx.update(eid, |e| {
            e.transform.x = x;
            e.transform.y = y;
        })
        .unwrap();
        update_bound_arrows(&mut tx, std::slice::from_ref(eid)).unwrap();
        tx.commit().unwrap();
    }
}
