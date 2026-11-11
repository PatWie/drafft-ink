//! A whole frame through `UiShell::frame`: keys and widgets, all actions ran, idle frames request
//! no repaint, the theme follows the setting, toasts animate only while visible.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use drafftink_engine::Session;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_ui::{FrameInput, FrameResult, UiShell};
use egui::{Context, Event, FullOutput, Key, Modifiers, Pos2, RawInput, Rect, ViewportId, vec2};
use serde_json::json;

struct Rig {
    ctx: Context,
    shell: UiShell,
    session: Session,
    time: f64,
}

impl Rig {
    fn new() -> Self {
        let fakes = Fakes::new();
        Self {
            ctx: Context::default(),
            shell: UiShell::new(false),
            session: Session::with_services(fakes.ports(), Services::fake()),
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<Event>) -> (FrameResult, FullOutput) {
        self.time += 0.1;
        let raw = RawInput {
            events,
            time: Some(self.time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let mut result = None;
        let (shell, session) = (&mut self.shell, &mut self.session);
        let mut out = self.ctx.run_ui(raw, |ui| result = Some(shell.frame(ui, session, FrameInput::default())));
        out.textures_delta.clear();
        (result.expect("the frame ran"), out)
    }

    fn repaint_delay(out: &FullOutput) -> Duration {
        out.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay)
    }
}

fn key(k: Key, m: Modifiers) -> Event {
    Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: m }
}

#[test]
fn a_frame_executes_every_key_action_in_order() {
    let mut rig = Rig::new();
    rig.frame(vec![]);
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let (res, _) = rig.frame(vec![key(Key::R, Modifiers::NONE), key(Key::Z, ctrl), key(Key::A, Modifiers::NONE)]);
    let ids: Vec<&str> = res.report.ran.iter().map(|e| e.call.id.as_str()).collect();
    assert_eq!(ids, ["tool.set", "history.undo", "tool.set"]);
    // The undo was refused; the toast says why, and the tool changes went through.
    assert_eq!(rig.session.active_tool(), drafftink_engine::state::Tool::Arrow);
    assert_eq!(rig.shell.state.toasts.len(), 1);
    assert!(rig.shell.state.toasts[0].text.contains("nothing to undo"), "{}", rig.shell.state.toasts[0].text);
}

#[test]
fn the_model_follows_the_session_after_a_frame() {
    let mut rig = Rig::new();
    rig.frame(vec![]);
    rig.frame(vec![key(Key::O, Modifiers::NONE)]);
    assert_eq!(rig.shell.model.tool, drafftink_engine::state::Tool::Ellipse);
}

#[test]
fn idle_frames_request_no_repaint() {
    let mut rig = Rig::new();
    // Let the first frames settle (fonts, sizing passes).
    for _ in 0..4 {
        rig.frame(vec![]);
    }
    let (_, out) = rig.frame(vec![]);
    assert_eq!(Rig::repaint_delay(&out), Duration::MAX, "a quiet frame must not ask for another one");
    let (_, out) = rig.frame(vec![]);
    assert_eq!(Rig::repaint_delay(&out), Duration::MAX);
}

#[test]
fn a_toast_schedules_one_repaint_for_the_fade_and_none_afterwards() {
    let mut rig = Rig::new();
    for _ in 0..3 {
        rig.frame(vec![]);
    }
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let (_, out) = rig.frame(vec![key(Key::Z, ctrl)]);
    assert!(!rig.shell.state.toasts.is_empty());
    assert_eq!(Rig::repaint_delay(&out), Duration::ZERO, "a command ran: one frame to show the new state");
    // The toast area appears (egui animates a new area for a few frames), then the UI waits for the fade.
    for _ in 0..6 {
        rig.frame(vec![]);
    }
    let (_, out) = rig.frame(vec![]);
    let d = Rig::repaint_delay(&out);
    assert!(d <= Duration::from_secs(5) && d > Duration::ZERO, "{d:?}");
    // Long after the toast faded: no repaint request at all.
    rig.time += 30.0;
    rig.frame(vec![]);
    let (_, out) = rig.frame(vec![]);
    assert!(rig.shell.state.toasts.is_empty());
    assert_eq!(Rig::repaint_delay(&out), Duration::MAX);
}

#[test]
fn the_theme_setting_drives_the_egui_theme() {
    let mut rig = Rig::new();
    rig.session.execute("settings.set", json!({"key": "theme", "value": "dark"})).unwrap();
    rig.frame(vec![]);
    rig.frame(vec![]);
    assert_eq!(rig.ctx.theme(), egui::Theme::Dark);
    rig.session.execute("settings.set", json!({"key": "theme", "value": "light"})).unwrap();
    rig.frame(vec![]);
    rig.frame(vec![]);
    assert_eq!(rig.ctx.theme(), egui::Theme::Light);
}

#[test]
fn modal_is_reported_while_a_dialog_is_open() {
    let mut rig = Rig::new();
    let (res, _) = rig.frame(vec![]);
    assert!(!res.modal);
    let alt_slash = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let (_, _) = rig.frame(vec![Event::Key {
        key: Key::Slash,
        physical_key: Some(Key::Slash),
        pressed: true,
        repeat: false,
        modifiers: alt_slash,
    }]);
    let (res, _) = rig.frame(vec![]);
    assert!(res.modal, "Mod+/ opens the palette");
    assert!(rig.shell.state.palette.is_some());
}
