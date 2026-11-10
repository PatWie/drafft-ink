//! Helpers for tests of the engine and of the families: sessions over the fake ports, and a few
//! elements to act on. Nothing here is used by the app.

use drafftink_scene::{Element, ElementId, ElementKind, SceneRead, SceneTx, StoreError, Transform};

use crate::ports::fake::Fakes;
use crate::session::Session;

/// A session with every fake port, and the fakes to inspect them.
pub fn session() -> (Session, Fakes) {
    let fakes = Fakes::new();
    (Session::new(fakes.ports()), fakes)
}

/// A session with only a clock and ids: no clipboard, storage, files, renderer or network.
pub fn headless_session() -> Session {
    Session::new(Fakes::new().headless_ports())
}

/// Adds a rectangle on top and returns its id. The change is a normal journaled commit, not an undo step.
pub fn add_rect(s: &mut Session, x: f64, y: f64, w: f64, h: f64) -> Result<ElementId, StoreError> {
    add(s, ElementKind::Rectangle, x, y, w, h)
}

/// Adds an element of any kind on top and returns its id.
pub fn add(s: &mut Session, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> Result<ElementId, StoreError> {
    let id = s.ports.ids.element_id();
    let seed = s.ports.ids.seed();
    let ret = id.clone();
    s.tab_mut().doc.edit::<(), StoreError>(|tx| {
        let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
        let mut e = Element::new(id, kind, z);
        e.transform = Transform::new(x, y, w, h);
        e.style.seed = seed;
        tx.insert(e)
    })?;
    s.tab_mut().doc.forget_journal();
    Ok(ret)
}

/// Selects the elements.
pub fn select(s: &mut Session, ids: &[ElementId]) {
    s.tab_mut().observed.selection = ids.iter().cloned().collect();
}

/// A session with three rectangles at known places, the first one selected. The fixture of
/// `hostile_params_never_panic` and a good start for a family's tests.
pub fn session_with_shapes() -> Result<(Session, Vec<ElementId>), StoreError> {
    let mut s = headless_session();
    let a = add_rect(&mut s, 0.0, 0.0, 100.0, 60.0)?;
    let b = add_rect(&mut s, 200.0, 50.0, 80.0, 80.0)?;
    let c = add_rect(&mut s, -150.0, 300.0, 40.0, 40.0)?;
    select(&mut s, std::slice::from_ref(&a));
    Ok((s, vec![a, b, c]))
}

/// A richer fixture for registry-wide tests: rectangles, an ellipse, a named diamond, an arrow, a text
/// and a frame, with two elements selected. Ids are `e1`..`e8`.
pub fn rich_session() -> Result<Session, StoreError> {
    use drafftink_geom::kurbo::Point;
    use drafftink_scene::{LinearData, TextData};
    let (mut s, ids) = session_with_shapes()?;
    add(&mut s, ElementKind::Ellipse, 300.0, 300.0, 60.0, 40.0)?;
    let d = add(&mut s, ElementKind::Diamond, -300.0, -100.0, 50.0, 50.0)?;
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(&d, |e| e.name = Some("diamond".to_owned())))?;
    add(
        &mut s,
        ElementKind::Arrow(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(120.0, 40.0)])),
        20.0,
        200.0,
        120.0,
        40.0,
    )?;
    add(&mut s, ElementKind::Text(TextData::new("hello")), 40.0, -80.0, 60.0, 20.0)?;
    add(
        &mut s,
        ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true }),
        -50.0,
        -50.0,
        400.0,
        500.0,
    )?;
    s.tab_mut().doc.forget_journal();
    if let Some(second) = ids.get(1) {
        s.tab_mut().observed.selection.insert(second.clone());
    }
    Ok(s)
}
