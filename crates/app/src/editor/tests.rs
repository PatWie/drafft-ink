//! The editor draws what the commit leaves behind: same box, same wrap, same position for free text, fixed-width
//! text, shape labels (also when the shape grows), arrow labels and turned shapes. Pixel parity is in `pixels.rs`.

use drafftink_engine::interact::UiRequest;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::{LazyTypesetter, Services, SharedTypesetter};
use drafftink_engine::{Origin, Session};
use drafftink_geom::kurbo::Point;
use drafftink_gpu::TextInput;
use drafftink_scene::{Color, Element, ElementId, SceneRead, TextRun, Transform};
use drafftink_ui::TextFaces;
use serde_json::{Value, json};

use super::TextEditor;

pub(super) struct Rig {
    pub(super) s: Session,
    pub(super) ts: SharedTypesetter,
    pub(super) editor: Option<TextEditor>,
    _fakes: Fakes,
}

impl Rig {
    pub(super) fn new() -> Self {
        let fakes = Fakes::new();
        let ts = LazyTypesetter::shared();
        let s = Session::with_services(fakes.ports(), Services::sharing(ts.clone()));
        Self { s, ts, editor: None, _fakes: fakes }
    }

    pub(super) fn run(&mut self, id: &str, args: Value) -> Value {
        match self.s.execute_from(Origin::Test, id, args) {
            Ok(v) => v,
            Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
        }
    }

    /// Creates an element and returns its id.
    pub(super) fn create(&mut self, args: Value) -> ElementId {
        let out = self.run("element.create", args);
        ElementId::new(out["id"].as_str().expect("an id")).expect("a valid id")
    }

    /// `text.edit_begin` then the editor over it.
    pub(super) fn begin(&mut self, args: Value, request: UiRequest) {
        self.run("text.edit_begin", args);
        self.open(&request);
    }

    pub(super) fn open(&mut self, request: &UiRequest) {
        let s = &self.s;
        self.editor = self.ts.with(|ts| TextEditor::open(s, request, None, ts, 0.0));
        assert!(self.editor.is_some(), "an editor opened");
    }

    pub(super) fn editor(&mut self) -> &mut TextEditor {
        self.editor.as_mut().expect("an editor")
    }

    pub(super) fn type_text(&mut self, text: &str) {
        assert!(self.editor().text_input(&TextInput::Commit(text.to_owned()), 0.0));
    }

    /// The text element the editor draws, and the container it grows.
    pub(super) fn drawn(&mut self) -> (Element, Option<Element>) {
        let ts = self.ts.clone();
        let editor = self.editor.as_mut().expect("an editor");
        let a = ts.with(|ts| editor.paint(ts).clone());
        (a.text, a.grown_container)
    }

    /// Ends the edit with the editor's result.
    pub(super) fn commit(&mut self) {
        let args = self.editor().finish_args();
        self.run("text.edit_end", args);
    }

    pub(super) fn element(&self, id: &ElementId) -> Element {
        self.s.store().get(id).expect("element").clone()
    }
}

fn request(id: Option<&ElementId>) -> UiRequest {
    UiRequest::OpenTextEditor { element: id.cloned(), at: Point::ZERO, width: None }
}

fn near(a: &Transform, b: &Transform, what: &str) {
    for (name, x, y) in
        [("x", a.x, b.x), ("y", a.y, b.y), ("w", a.w, b.w), ("h", a.h, b.h), ("angle", a.angle, b.angle)]
    {
        assert!((x - y).abs() < 1e-6, "{what}: {name} is {x} in the editor and {y} committed ({a:?} / {b:?})");
    }
}

const LONG: &str = "the quick brown fox jumps over the lazy dog and keeps running";

// ---- free text -------------------------------------------------------------------------------------------------

#[test]
fn an_unchanged_text_is_drawn_where_and_as_big_as_it_is() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "hello world", "at": [100, 50] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let (drawn, container) = r.drawn();
    assert!(container.is_none());
    near(&drawn.transform, &r.element(&id).transform, "unchanged");
}

#[test]
fn typed_text_grows_the_box_exactly_as_the_commit_measures_it() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "hi", "at": [100, 50] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.type_text(" there\nsecond line with more words");
    let (drawn, _) = r.drawn();
    r.commit();
    near(&drawn.transform, &r.element(&id).transform, "typed");
}

#[test]
fn a_new_text_is_drawn_where_it_will_be_created() {
    let mut r = Rig::new();
    r.begin(
        json!({ "at": [300, 200] }),
        UiRequest::OpenTextEditor { element: None, at: Point::new(300.0, 200.0), width: None },
    );
    let (empty, _) = r.drawn();
    assert_eq!((empty.transform.x, empty.transform.y), (300.0, 200.0));
    r.type_text("fresh text");
    let (drawn, _) = r.drawn();
    r.commit();
    let created = r.s.store().iter().find(|e| e.text_data().is_some()).expect("the text").clone();
    near(&drawn.transform, &created.transform, "draft");
}

#[test]
fn a_dragged_out_box_keeps_its_width_and_wraps() {
    let mut r = Rig::new();
    r.begin(
        json!({ "at": [100, 100], "width": 160 }),
        UiRequest::OpenTextEditor { element: None, at: Point::new(100.0, 100.0), width: Some(160.0) },
    );
    r.type_text(LONG);
    let (drawn, _) = r.drawn();
    assert_eq!(drawn.transform.w, 160.0);
    assert!(drawn.transform.h > 40.0, "the text wrapped: {}", drawn.transform.h);
    r.commit();
    let created = r.s.store().iter().find(|e| e.text_data().is_some()).expect("the text").clone();
    near(&drawn.transform, &created.transform, "fixed width");
}

#[test]
fn a_turned_text_keeps_its_angle_and_its_top_left() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "turn", "at": [100, 50] }));
    r.run("element.rotate", json!({ "ids": [id.as_str()], "mode": "to", "angle": 35 }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.type_text(" me");
    let (drawn, _) = r.drawn();
    assert!(drawn.transform.angle.abs() > 0.05, "the text is turned: {}", drawn.transform.angle);
    r.commit();
    near(&drawn.transform, &r.element(&id).transform, "turned");
}

// ---- labels ----------------------------------------------------------------------------------------------------

fn shape_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("rectangle", json!({ "kind": "rectangle", "bounds": { "x": 0, "y": 0, "w": 160, "h": 80 } })),
        ("ellipse", json!({ "kind": "ellipse", "bounds": { "x": 0, "y": 0, "w": 160, "h": 80 } })),
        ("diamond", json!({ "kind": "diamond", "bounds": { "x": 0, "y": 0, "w": 160, "h": 80 } })),
    ]
}

#[test]
fn a_shape_label_is_drawn_as_the_commit_places_it() {
    for (name, shape) in shape_cases() {
        for text in ["ok", "two\nlines", LONG] {
            let mut r = Rig::new();
            let id = r.create(shape.clone());
            r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
            r.type_text(text);
            let (drawn, grown) = r.drawn();
            r.commit();
            let label = drafftink_edit::text_bind::label_of(r.s.store(), &id).expect("a label");
            near(&drawn.transform, &r.element(&label).transform, &format!("{name} label {text:?}"));
            let container = r.element(&id).transform;
            match grown {
                Some(g) => near(&g.transform, &container, &format!("{name} grown {text:?}")),
                None => near(&r.element(&id).transform, &Transform::new(0.0, 0.0, 160.0, 80.0), "not grown"),
            }
        }
    }
}

/// A rectangle at (0, 0, 100, 40) with an arrow from the right whose end is bound to it, and one from below that is
/// bound at both ends.
fn box_with_arrows(r: &mut Rig) -> (ElementId, ElementId, ElementId) {
    let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 0, "y": 0, "w": 100, "h": 40 } }));
    let far = r.create(json!({ "kind": "rectangle", "bounds": { "x": 300, "y": 200, "w": 60, "h": 40 } }));
    let from_right = r.create(json!({ "kind": "arrow", "points": [[300, 20], [110, 20]], "bind": "off" }));
    r.run("arrow.bind", json!({ "arrow": from_right.as_str(), "end": "end", "target": id.as_str() }));
    let between = r.create(json!({ "kind": "arrow", "points": [[50, 50], [300, 220]], "bind": "off" }));
    r.run("arrow.bind", json!({ "arrow": between.as_str(), "end": "start", "target": id.as_str() }));
    r.run("arrow.bind", json!({ "arrow": between.as_str(), "end": "end", "target": far.as_str() }));
    (id, from_right, between)
}

fn world_points(r: &Rig, id: &ElementId) -> Vec<Point> {
    drafftink_edit::linear::world_points(&r.element(id)).expect("an arrow")
}

#[test]
fn arrows_bound_to_a_growing_shape_follow_it_while_typing_and_do_not_jump_when_the_edit_ends() {
    let mut r = Rig::new();
    let (id, from_right, between) = box_with_arrows(&mut r);
    let before = (world_points(&r, &from_right), world_points(&r, &between));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let ts = r.ts.clone();
    let editor = r.editor.as_mut().expect("an editor");
    assert!(ts.with(|ts| editor.paint(ts).following_arrows.is_empty()), "nothing grew yet, nothing follows");
    assert!(editor.hidden().iter().all(|h| *h != from_right), "the arrows stay in the document scene until then");

    r.type_text(LONG);
    let editor = r.editor.as_mut().expect("an editor");
    let live = ts.with(|ts| editor.paint(ts).clone());
    let grown = live.grown_container.as_ref().expect("the shape grew");
    assert!(grown.transform.w > 100.0 || grown.transform.h > 40.0, "{:?}", grown.transform);
    let following: Vec<ElementId> = live.following_arrows.iter().map(|a| a.id.clone()).collect();
    assert_eq!(following.len(), 2, "both arrows are drawn where the growth puts them");
    assert!(editor.hidden().contains(&id), "the shape grown on screen replaces the stored one");
    for a in [&from_right, &between] {
        assert!(editor.hidden().contains(a), "the stale arrow {a:?} is left out of the document scene");
    }
    assert_eq!(world_points(&r, &from_right), before.0, "the document has not been touched yet");

    r.commit();
    for arrow in &live.following_arrows {
        let committed = world_points(&r, &arrow.id);
        let shown = drafftink_edit::linear::world_points(arrow).expect("points");
        assert_eq!(shown.len(), committed.len());
        for (a, b) in shown.iter().zip(&committed) {
            assert!((*a - *b).hypot() < 1e-6, "{}: live {a:?}, committed {b:?}", arrow.id.as_str());
        }
    }
    assert_ne!(world_points(&r, &from_right), before.0, "the arrow did move with the shape");
}

#[test]
fn bold_and_italic_chosen_while_editing_apply_to_the_selection_and_commit_with_the_text() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "plain words here", "at": [40, 30] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let (plain, _) = r.drawn();
    r.editor().model.select_chars(6..11);
    assert!(r.editor().add_format(&json!({ "bold": true }), 1.0));
    assert!(!r.editor().add_format(&json!({ "bold": true }), 2.0), "the face is on already");
    assert_eq!(r.editor().faces().bold, Some(true));
    let (drawn, _) = r.drawn();
    assert!(
        drawn.transform.w > plain.transform.w + 1.0,
        "bold letters are wider: {} against {}",
        drawn.transform.w,
        plain.transform.w
    );
    let args = r.editor().finish_args();
    assert_eq!(args["runs"], json!([{ "start": 6, "end": 11, "bold": true }]));
    r.commit();
    let stored = r.element(&id);
    assert_eq!(
        stored.text_data().expect("text").runs,
        vec![TextRun { start: 6, end: 11, bold: true, ..Default::default() }]
    );
    near(&drawn.transform, &stored.transform, "a bold word: the editor and the commit measure alike");
}

#[test]
fn the_panel_sees_the_faces_of_the_selection_in_the_editor() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "plain words", "at": [40, 30] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.editor().model.select_chars(0..5);
    r.editor().add_format(&json!({ "bold": true }), 1.0);
    assert_eq!(r.editor().faces(), TextFaces { bold: Some(true), italic: Some(false) });
    r.editor().model.select_chars(0..11);
    assert_eq!(r.editor().faces().bold, None, "a selection that is partly bold has no state");
    r.editor().model.select_chars(6..6);
    assert_eq!(r.editor().faces().bold, Some(false), "a caret in regular text types regular text");
}

#[test]
fn a_label_that_fits_moves_no_arrow() {
    let mut r = Rig::new();
    let (id, ..) = box_with_arrows(&mut r);
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.type_text("ok");
    let ts = r.ts.clone();
    let editor = r.editor.as_mut().expect("an editor");
    let live = ts.with(|ts| editor.paint(ts).clone());
    assert!(live.grown_container.is_none() && live.following_arrows.is_empty());
    assert!(editor.hidden().iter().all(|h| r.s.store().get(h).is_none_or(|e| e.text_data().is_some())));
}

#[test]
fn a_long_label_makes_the_shape_grow_while_typing() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 0, "y": 0, "w": 100, "h": 40 } }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let (_, grown) = r.drawn();
    assert!(grown.is_none(), "an empty label fits");
    assert!(r.editor().hidden().is_empty(), "nothing of the document is hidden for a label that does not exist yet");
    r.type_text(LONG);
    let (_, grown) = r.drawn();
    let grown = grown.expect("the rectangle grows");
    assert!(grown.transform.h > 40.0);
    assert!(r.editor().hidden().contains(&id), "the container is drawn by the editor while it grows");
}

#[test]
fn an_existing_label_is_edited_in_place_and_hidden_while_the_editor_draws_it() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 10, "y": 20, "w": 200, "h": 100 } }));
    r.run("element.relabel", json!({ "id": id.as_str(), "text": "first" }));
    let label = drafftink_edit::text_bind::label_of(r.s.store(), &id).expect("a label");
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    assert_eq!(r.editor().hidden(), vec![label.clone()]);
    let (drawn, _) = r.drawn();
    near(&drawn.transform, &r.element(&label).transform, "existing label");
    r.type_text(" and more");
    let (drawn, _) = r.drawn();
    r.commit();
    near(&drawn.transform, &r.element(&label).transform, "edited label");
}

#[test]
fn an_arrow_label_sits_on_the_arrow_like_the_committed_one() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "arrow", "points": [[0, 0], [300, 100]] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.type_text("a label for the arrow");
    let (drawn, grown) = r.drawn();
    assert!(grown.is_none(), "an arrow does not grow");
    r.commit();
    let label = drafftink_edit::text_bind::label_of(r.s.store(), &id).expect("a label");
    near(&drawn.transform, &r.element(&label).transform, "arrow label");
}

#[test]
fn a_turned_shape_turns_its_label() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 0, "y": 0, "w": 160, "h": 80 } }));
    r.run("element.rotate", json!({ "ids": [id.as_str()], "mode": "to", "angle": 40 }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.type_text("turned label");
    let (drawn, _) = r.drawn();
    r.commit();
    let label = drafftink_edit::text_bind::label_of(r.s.store(), &id).expect("a label");
    near(&drawn.transform, &r.element(&label).transform, "turned label");
    assert!(drawn.transform.angle.abs() > 0.05, "the label is turned: {}", drawn.transform.angle);
}

// ---- style and colour --------------------------------------------------------------------------------------------

#[test]
fn a_colour_chosen_with_a_selection_colours_that_range_and_goes_with_the_result() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "red blue", "at": [0, 0] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    r.editor().model.select_chars(4..8);
    assert!(r.editor().add_style(&json!({ "stroke_color": "#0000ff" }), 0.0));
    let args = r.editor().finish_args();
    assert_eq!(args["runs"], json!([{ "start": 4, "end": 8, "color": "#0000ff" }]));
    assert!(args.get("style").is_none(), "the element's own colour is unchanged");
    r.commit();
    let runs = r.element(&id).text_data().expect("text").runs.clone();
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].start, runs[0].end, runs[0].color), (4, 8, Some(Color::rgb(0, 0, 255))));
    // The editor drew the colour before the commit.
    r.run("history.undo", json!({}));
}

#[test]
fn a_colour_with_nothing_selected_and_other_styles_apply_to_the_whole_text() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "all of it", "at": [0, 0] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let before = r.drawn().0.transform;
    assert!(r.editor().add_style(&json!({ "stroke_color": "#ff0000", "font_size": 36 }), 0.0));
    let (drawn, _) = r.drawn();
    assert_eq!(drawn.style.stroke_color, Color::rgb(255, 0, 0));
    assert!(drawn.transform.w > before.w * 1.5, "the bigger font shows at once");
    let args = r.editor().finish_args();
    assert_eq!(args["style"]["font_size"], json!(36.0));
    r.commit();
    near(&drawn.transform, &r.element(&id).transform, "restyled");
    assert_eq!(r.element(&id).style.stroke_color, Color::rgb(255, 0, 0));
}

#[test]
fn turning_formulas_off_is_sent_only_when_it_changed() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "a $x$ b", "at": [0, 0] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    assert!(r.editor().finish_args().get("inline_math").is_none());
    let key = drafftink_edit::text_edit::Key::Letter('m');
    let mods = drafftink_edit::text_edit::Mods { shift: true, alt: false, command: true };
    let route =
        super::keys::route(egui::Key::M, egui::Modifiers { command: true, shift: true, ..Default::default() }, false);
    assert!(matches!(route, Some(super::KeyRoute::Editor(_))), "{key:?} {mods:?}");
    let Some(super::KeyRoute::Editor(k)) = route else { return };
    let ts = r.ts.clone();
    assert!(ts.with(|ts| r.editor().key(k, false, ts, 0.0)));
    assert_eq!(r.editor().finish_args()["inline_math"], json!("off"));
}

// ---- the caret and the input method --------------------------------------------------------------------------------

#[test]
fn the_caret_blinks_by_the_clock_and_the_next_change_is_known() {
    let mut r = Rig::new();
    r.begin(json!({ "at": [0, 0] }), UiRequest::OpenTextEditor { element: None, at: Point::ZERO, width: None });
    let e = r.editor();
    assert!(e.caret_on(0.0) && e.caret_on(529.0));
    assert!(!e.caret_on(531.0) && !e.caret_on(1059.0));
    assert!(e.caret_on(1061.0));
    assert_eq!(e.next_blink_ms(100.0), 530.0);
    assert_eq!(e.next_blink_ms(530.0), 1060.0);
}

#[test]
fn typing_shows_the_caret_solid_again() {
    let mut r = Rig::new();
    r.begin(json!({ "at": [0, 0] }), UiRequest::OpenTextEditor { element: None, at: Point::ZERO, width: None });
    assert!(!r.editor().caret_on(700.0));
    assert!(r.editor().text_input(&TextInput::Commit("a".into()), 700.0));
    assert!(r.editor().caret_on(701.0));
}

#[test]
fn a_composition_is_drawn_at_the_caret_and_the_candidate_window_follows_it() {
    use drafftink_canvas::Camera;
    use drafftink_geom::kurbo::Size;
    let mut r = Rig::new();
    r.begin(
        json!({ "at": [50, 60] }),
        UiRequest::OpenTextEditor { element: None, at: Point::new(50.0, 60.0), width: None },
    );
    r.type_text("ab");
    let camera = Camera { scroll: Point::new(0.0, 0.0), zoom: 2.0 };
    let canvas = Size::new(800.0, 600.0);
    let ts = r.ts.clone();
    let before = ts.with(|ts| r.editor().caret_on_screen(ts, camera, canvas)).expect("a caret");
    assert!(r.editor().text_input(&TextInput::Preedit { text: "nihao".into(), cursor: Some((5, 5)) }, 0.0));
    let after = ts.with(|ts| r.editor().caret_on_screen(ts, camera, canvas)).expect("a caret");
    assert!(after.x0 > before.x0 + 20.0, "the caret is after the preedit: {before:?} {after:?}");
    assert!(r.editor().text_input(&TextInput::Commit("你好".into()), 0.0));
    let (drawn, _) = r.drawn();
    assert_eq!(drawn.text_data().expect("text").content, "ab你好");
    assert!(!r.editor().text_input(&TextInput::Cancel, 0.0), "nothing was composing");
}

#[test]
fn only_the_text_and_its_margin_take_a_press() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "click me", "at": [100, 100] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let t = r.element(&id).transform;
    let ts = r.ts.clone();
    let editor = r.editor.as_mut().expect("an editor");
    assert!(ts.with(|ts| editor.hit(ts, Point::new(t.x + t.w / 2.0, t.y + t.h / 2.0), 0.0)));
    assert!(ts.with(|ts| editor.hit(ts, Point::new(t.x - 3.0, t.y + 2.0), 6.0)));
    assert!(!ts.with(|ts| editor.hit(ts, Point::new(t.x + t.w + 50.0, t.y + 2.0), 6.0)));
    assert!(!ts.with(|ts| editor.hit(ts, Point::new(f64::NAN, 0.0), 6.0)));
}

#[test]
fn a_double_click_puts_the_caret_where_the_text_was_hit() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "hello world", "at": [100, 100] }));
    r.run("text.edit_begin", json!({ "id": id.as_str() }));
    let t = r.element(&id).transform;
    let hint = Point::new(t.x + 1.0, t.y + t.h / 2.0);
    let s = &r.s;
    let editor = r.ts.with(|ts| TextEditor::open(s, &request(Some(&id)), Some(hint), ts, 0.0)).expect("editor");
    assert_eq!(editor.model.anchor_focus(), (0, 0), "the caret is at the start, where the click was");
}

#[test]
fn drag_selects_and_copy_and_cut_use_the_selection() {
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "hello world", "at": [100, 100] }));
    r.begin(json!({ "id": id.as_str() }), request(Some(&id)));
    let t = r.element(&id).transform;
    let (y, ts) = (t.y + t.h / 2.0, r.ts.clone());
    let editor = r.editor.as_mut().expect("an editor");
    ts.with(|ts| editor.press(ts, Point::new(t.x + 1.0, y), Point::new(5.0, 5.0), false, 0.0));
    assert!(ts.with(|ts| editor.drag(ts, Point::new(t.x + t.w * 0.4, y), 1.0)));
    editor.release();
    let selected = editor.copy(false, 2.0).expect("a selection");
    assert!(!selected.is_empty() && "hello world".starts_with(&selected), "{selected:?}");
    assert_eq!(editor.copy(true, 3.0).as_deref(), Some(selected.as_str()));
    assert_eq!(editor.model.text().len(), "hello world".len() - selected.len());
    assert!(editor.paste(&selected, 4.0));
    assert_eq!(editor.model.text(), "hello world");
}
