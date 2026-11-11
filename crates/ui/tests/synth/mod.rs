//! Support for the dialog tests: a session whose registry also holds stand-ins for commands the engine
//! does not register yet (`collab.*`, `doc.set_background`, `doc.open`, `element.query`, `text.edit_end`),
//! a harness that installs the fonts and the theme the way the shell does, and pointer helpers.
//!
//! The stand-ins accept any arguments and do nothing, so a test sees the `Call` the UI produced and the
//! controls are enabled. They refuse in a view-only document, which gives a second, robust way to test
//! the greyed state. The real families come first in the registry: when the engine registers one of
//! these ids for real, the real command wins and the tests keep working.
#![allow(dead_code)]

use std::sync::Arc;

use drafftink_engine::cmd::FAMILIES;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_engine::spec::{CommandSpec, Facts, Gate, Typed};
use drafftink_engine::{Refusal, Registry, Session};
use drafftink_ui::theme;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

use crate::common::Fixture;

fn accept(_: &mut Session, _: Value) -> Result<Value, Refusal> {
    Ok(json!({}))
}

const FACTS: Facts = Facts { view_only: Gate::Refused, ..Facts::READ_ONLY };

macro_rules! stand_in {
    ($id:literal, $family:literal, $title:literal) => {
        CommandSpec::new($id, $family, $title, "A stand-in for the UI tests.", &Typed::new(accept), FACTS)
    };
}

const STAND_INS: &[CommandSpec] = &[
    stand_in!("collab.connect", "collab", "Connect to server"),
    stand_in!("collab.disconnect", "collab", "Disconnect"),
    stand_in!("collab.join_room", "collab", "Join room"),
    stand_in!("collab.leave_room", "collab", "Leave room"),
    stand_in!("collab.set_user", "collab", "Set name and colour"),
    stand_in!("doc.set_background", "doc", "Set background colour"),
    stand_in!("doc.open", "doc", "Open document"),
    stand_in!("element.query", "element", "Find elements"),
    stand_in!("text.edit_end", "text", "Finish editing text"),
];

/// A fixture over a registry with the stand-ins.
pub fn fixture() -> Fixture {
    let mut families: Vec<&'static [CommandSpec]> = FAMILIES.to_vec();
    families.push(STAND_INS);
    let fakes = Fakes::new();
    let mut session = Session::with_registry(fakes.ports(), Arc::new(Registry::from_families(&families)));
    session.services = Services::fake();
    Fixture::over(session, fakes)
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
    click_at(h, Pos2::new(640.0, 760.0));
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
