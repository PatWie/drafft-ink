//! Fixtures shared by the tests of the `group`, `frame` and `arrange` families.

use drafftink_edit::zorder;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    Binding, BindMode, Element, ElementId, ElementKind, FrameData, GroupId, LinearData, SceneRead, SceneTx, StoreError,
    TextData,
};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

pub(crate) fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

pub(crate) fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// `n` rectangles `r0`.. of 40x40 at x = 100 * i, in z-order, nothing selected.
pub(crate) fn rects(n: usize) -> (Session, Vec<ElementId>) {
    let mut s = testing::headless_session();
    let ids = (0..n).map(|i| named(&mut s, &format!("r{i}"), 100.0 * i as f64, 0.0, 40.0, 40.0)).collect();
    (s, ids)
}

/// Adds a rectangle with a fixed id on top.
pub(crate) fn named(s: &mut Session, id: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    add_kind(s, id, ElementKind::Rectangle, x, y, w, h)
}

pub(crate) fn add_kind(s: &mut Session, id: &str, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    let id = ElementId::new(id).unwrap();
    let ret = id.clone();
    setup(s, |tx| {
        let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
        let mut e = Element::new(id, kind, z);
        e.transform = drafftink_scene::Transform::new(x, y, w, h);
        tx.insert(e)
    });
    ret
}

/// Edits the document as test setup: the change reaches the replicated store but is no undo step, so that
/// undo in a test goes back to this state.
pub(crate) fn setup(s: &mut Session, f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<(), StoreError>) {
    let doc = &mut s.tab_mut().doc;
    doc.edit::<(), StoreError>(f).unwrap();
    doc.commit_untracked().unwrap();
}

/// Locks an element.
pub(crate) fn lock(s: &mut Session, id: &ElementId) {
    setup(s, |tx| tx.update(id, |e| e.locked = true));
}

pub(crate) fn frame(s: &mut Session, id: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    add_kind(s, id, ElementKind::Frame(FrameData { name: None, clip: true }), x, y, w, h)
}

/// A text that is the bound label of `container`.
pub(crate) fn label(s: &mut Session, id: &str, container: &ElementId) -> ElementId {
    let mut t = TextData::new("label");
    t.container_id = Some(container.clone());
    let (x, y) = s.element(container).map_or((0.0, 0.0), |e| (e.transform.x, e.transform.y));
    add_kind(s, id, ElementKind::Text(t), x + 5.0, y + 5.0, 30.0, 20.0)
}

/// An arrow from (x, y) to (x + dx, y) with its start bound to `target`.
pub(crate) fn arrow_bound_to(s: &mut Session, id: &str, target: &ElementId, x: f64, y: f64, dx: f64) -> ElementId {
    let mut l = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(dx, 0.0)]);
    l.start_binding = Some(Binding { target: target.clone(), mode: BindMode::Inside, fixed_point: [0.5, 0.5] });
    add_kind(s, id, ElementKind::Arrow(l), x, y, dx, 0.0)
}

pub(crate) fn select(s: &mut Session, ids: &[&ElementId]) {
    s.tab_mut().observed.selection = ids.iter().map(|i| (*i).clone()).collect();
}

/// The live ids in z-order, back to front.
pub(crate) fn order(s: &Session) -> Vec<String> {
    zorder::order(s.store())
        .into_iter()
        .filter(|i| s.element(i).is_some())
        .map(|i| i.as_str().to_owned())
        .collect()
}

pub(crate) fn get<'a>(s: &'a Session, id: &ElementId) -> &'a Element {
    s.element(id).unwrap_or_else(|| panic!("{id} is not live"))
}

pub(crate) fn groups_of(s: &Session, id: &ElementId) -> Vec<GroupId> {
    get(s, id).group_ids.iter().cloned().collect()
}

pub(crate) fn x_of(s: &Session, id: &ElementId) -> f64 {
    get(s, id).transform.x
}

pub(crate) fn y_of(s: &Session, id: &ElementId) -> f64 {
    get(s, id).transform.y
}

pub(crate) fn json_ids(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect()
}

pub(crate) fn live_count(s: &Session) -> usize {
    s.store().iter().count()
}

/// `{"ids": [...]}` for the given ids.
pub(crate) fn ids_arg(ids: &[&ElementId]) -> Value {
    json!({ "ids": ids.iter().map(|i| i.as_str()).collect::<Vec<_>>() })
}
