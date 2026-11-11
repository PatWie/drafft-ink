//! Support for the dialog tests: a fixture over the real registry, a harness that installs the fonts and
//! the theme the way the shell does, and pointer helpers.
//!
//! Every command is the real one, so a test that dispatches the recorded actions changes the session.
//! To test the "not available in this build" look of a control, build the fixture with
//! `Fixture::without(&["the.id"])`: the registry then leaves that command out.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_engine::Session;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_ui::theme;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

use crate::common::Fixture;

/// A fixture over the real registry.
pub fn fixture() -> Fixture {
    Fixture::new()
}

/// Runs a real command on the session of the fixture, refreshes the model and returns the output. A refusal
/// fails the test: it is a set-up step.
pub fn run(f: &mut Fixture, id: &str, args: Value) -> Value {
    let out = f.session.execute(id, args);
    f.refresh();
    out.unwrap_or_else(|r| panic!("set-up command {id} was refused: {} ({})", r.message, r.code))
}

/// Stores a document with `n` rectangles under `name` in the storage the fixture's session also uses, so
/// that `doc.open` with `{"stored": name}` finds it.
pub fn store_document(f: &Fixture, name: &str, n: usize) {
    let mut other = Session::with_services(f.fakes.ports(), Services::fake());
    for i in 0..n {
        let bounds = json!({"x": 100 * i, "y": 0, "w": 50, "h": 50});
        other.execute("element.create", json!({"kind": "rectangle", "bounds": bounds})).unwrap();
    }
    other.execute("doc.rename", json!({"name": name})).unwrap();
    other.execute("doc.save", json!({"to": {"stored": name}})).unwrap();
}

/// How many elements the document of the fixture's session has.
pub fn element_count(f: &mut Fixture) -> u64 {
    run(f, "element.query", json!({}))["total"].as_u64().unwrap_or(u64::MAX)
}

/// A fixture whose session has chosen a collaboration server and, when `room` is given, joined that room.
/// The commands are the real ones, so `collab.disconnect` and `collab.leave_room` are available afterwards.
pub fn connected(room: Option<&str>) -> Fixture {
    let mut f = fixture();
    run(&mut f, "collab.connect", json!({"url": "ws://127.0.0.1:3030/ws"}));
    if let Some(room) = room {
        run(&mut f, "collab.join_room", json!({"room": room}));
    }
    f
}

/// A fixture whose session is editing the formula of a new math element, which is what the equation
/// dialog is open for. Returns the fixture and the id of the element.
/// The session lays formulas out with the real typesetter (the fake has no formula engine).
pub fn editing_math(latex: &str) -> (Fixture, String) {
    let fakes = Fakes::new();
    let session = Session::with_services(fakes.ports(), Services::new());
    let mut f = Fixture::over(session, fakes);
    let made = run(&mut f, "element.create", json!({"kind": "math", "latex": latex, "at": [0, 0]}));
    let id = made["id"].as_str().unwrap_or_default().to_owned();
    run(&mut f, "text.edit_begin", json!({"id": id}));
    f.state.open_equation(latex);
    (f, id)
}

/// A harness like `common::harness`, with the fonts and the light theme installed first.
pub fn harness(f: Fixture) -> Harness<'static, Fixture> {
    sized(f, egui::vec2(1280.0, 800.0), egui::Theme::Light)
}

/// A harness with a screen size and a theme.
pub fn sized(f: Fixture, size: egui::Vec2, theme: egui::Theme) -> Harness<'static, Fixture> {
    with_frame_hook(f, size, theme, |_| {})
}

/// A harness that also runs `hook` at the start of every frame (to tell a popover where its button is).
pub fn with_frame_hook(
    f: Fixture,
    size: egui::Vec2,
    theme: egui::Theme,
    mut hook: impl FnMut(&egui::Context) + 'static,
) -> Harness<'static, Fixture> {
    Harness::builder()
        .with_size(size)
        .with_theme(theme)
        .with_context_setup(|ctx| {
            theme::install_fonts(ctx);
            theme::apply(ctx, Some("system"));
        })
        .build_ui_state(
            move |ui, f: &mut Fixture| {
                hook(ui.ctx());
                let acts = drafftink_ui::draw(ui, &f.model, &mut f.state);
                f.actions.extend(acts);
            },
            f,
        )
}

/// Presses and releases the primary button at a point.
pub fn click_at(h: &mut Harness<'static, Fixture>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run();
}

/// Clicks on empty canvas: a point no surface covers.
pub fn click_canvas(h: &mut Harness<'static, Fixture>) {
    click_at(h, Pos2::new(420.0, 420.0));
}

/// The verbs among the recorded actions.
pub fn verbs(f: &Fixture) -> Vec<drafftink_ui::Verb> {
    f.actions.iter().filter_map(|a| if let drafftink_ui::UiAction::Verb(v) = a { Some(*v) } else { None }).collect()
}

/// The host requests among the recorded actions.
pub fn hosts(f: &Fixture) -> Vec<drafftink_ui::HostRequest> {
    f.actions
        .iter()
        .filter_map(|a| if let drafftink_ui::UiAction::Host(r) = a { Some(r.clone()) } else { None })
        .collect()
}

/// The text field with this accessible name (a caption of the same name may exist).
pub fn field<'a>(h: &'a Harness<'static, Fixture>, label: &str) -> egui_kittest::Node<'a> {
    use egui_kittest::kittest::Queryable;
    let label = label.to_owned();
    h.get_by(move |n| {
        matches!(n.role(), egui::accesskit::Role::TextInput | egui::accesskit::Role::MultilineTextInput)
            && n.label().as_deref() == Some(label.as_str())
    })
}
