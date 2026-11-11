//! The routing function and the dispatcher: one place maps actions to commands, every action of a frame
//! runs, refusals become toasts with the text unchanged.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Fixture;
use drafftink_engine::testing::{add_rect, select};
use drafftink_engine::{Origin, Refusal};
use drafftink_ui::action::{CommandSink, Ran};
use drafftink_ui::{Call, HostRequest, Routed, Scrub, ScrubPhase, UiAction, UiState, Verb, dispatch, route};
use serde_json::{Value, json};

/// A sink that records what it was asked to run and answers from a script.
#[derive(Default)]
struct Recorder {
    calls: Vec<Call>,
    refuse: Vec<(&'static str, &'static str, &'static str)>,
}

impl CommandSink for Recorder {
    fn run(&mut self, origin: Origin, id: &str, args: Value) -> Result<Value, Refusal> {
        self.calls.push(Call::new(origin, id, args));
        match self.refuse.iter().find(|(c, _, _)| *c == id) {
            Some((_, code, msg)) => Err(Refusal::custom(code, id, *msg)),
            None => Ok(json!({"gesture": "g1"})),
        }
    }
}

#[test]
fn route_maps_each_kind_of_action() {
    let st = UiState::new();
    let call = Call::new(Origin::Ui, "history.undo", json!({}));
    assert_eq!(route(&UiAction::Command(call.clone()), &st), Routed::Command(call));
    assert_eq!(route(&UiAction::Verb(Verb::OpenPalette), &st), Routed::Local(Verb::OpenPalette));
    assert_eq!(route(&UiAction::Host(HostRequest::PickImage), &st), Routed::Host(HostRequest::PickImage));
}

#[test]
fn a_scrub_becomes_the_gesture_commands_with_the_running_gesture_id() {
    let mut st = UiState::new();
    let args = json!({"props": {"opacity": 0.5}});
    let begin = UiAction::Scrub(Scrub { phase: ScrubPhase::Begin, command: "style.set", args: args.clone() });
    let Routed::Command(c) = route(&begin, &st) else { panic!("not a command") };
    assert_eq!(c.id, "gesture.begin");
    assert_eq!(c.args, json!({"command": "style.set", "args": args}));

    // Run through a recorder that answers `g1`, the UI state remembers it.
    let mut rec = Recorder::default();
    dispatch(&mut rec, &mut st, vec![begin], 0.0);
    assert_eq!(st.scrub_gesture(), Some("g1"));
    let update = UiAction::Scrub(Scrub {
        phase: ScrubPhase::Update,
        command: "style.set",
        args: json!({"props": {"opacity": 0.3}}),
    });
    let Routed::Command(c) = route(&update, &st) else { panic!() };
    assert_eq!(
        (c.id.as_str(), &c.args),
        ("gesture.update", &json!({"gesture": "g1", "args": {"props": {"opacity": 0.3}}}))
    );
    let end = UiAction::Scrub(Scrub {
        phase: ScrubPhase::End,
        command: "style.set",
        args: json!({"props": {"opacity": 0.3}}),
    });
    dispatch(&mut rec, &mut st, vec![update, end], 0.0);
    assert_eq!(st.scrub_gesture(), None, "the end of the drag forgets the gesture");
    let ids: Vec<&str> = rec.calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["gesture.begin", "gesture.update", "gesture.end"]);
}

#[test]
fn a_drag_of_the_opacity_slider_is_one_undo_step_on_a_real_session() {
    let mut f = Fixture::new();
    let a = add_rect(&mut f.session, 0.0, 0.0, 100.0, 60.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    let steps = |f: &Fixture| f.session.tab().doc.undo_depth();
    let before = steps(&f);
    let frame = |opacity: f64, phase| {
        UiAction::Scrub(Scrub { phase, command: "style.set", args: json!({"props": {"opacity": opacity}}) })
    };
    f.actions = vec![frame(0.9, ScrubPhase::Begin)];
    f.dispatch();
    for o in [0.8, 0.6, 0.4] {
        f.actions = vec![frame(o, ScrubPhase::Update)];
        let r = f.dispatch();
        assert!(r.refusals().next().is_none(), "{:?}", r.ran);
    }
    f.actions = vec![frame(0.4, ScrubPhase::End)];
    f.dispatch();
    assert_eq!(steps(&f), before + 1, "a whole drag is one undo step");
    assert_eq!(f.model.style.num_value("opacity"), Some(0.4));
}

#[test]
fn all_actions_of_a_frame_run_in_order() {
    let mut st = UiState::new();
    let mut rec = Recorder::default();
    let actions = vec![
        UiAction::run("tool.set", json!({"tool": "arrow"})),
        UiAction::Verb(Verb::OpenShortcuts),
        UiAction::run("view.zoom_in", json!({})),
        UiAction::run("history.undo", json!({})),
    ];
    let report = dispatch(&mut rec, &mut st, actions, 0.0);
    let ids: Vec<&str> = rec.calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["tool.set", "view.zoom_in", "history.undo"], "the old UI ran only the first action of a frame");
    assert_eq!(report.ran.len(), 3);
    assert!(st.shortcuts_open, "the verb in the middle was applied too");
}

#[test]
fn a_refusal_becomes_a_toast_with_its_text_unchanged() {
    let mut st = UiState::new();
    let mut rec = Recorder {
        refuse: vec![("history.undo", "nothing_to_undo", "There is nothing to undo in this tab.")],
        ..Default::default()
    };
    dispatch(&mut rec, &mut st, vec![UiAction::run("history.undo", json!({}))], 3.0);
    assert_eq!(st.toasts.len(), 1);
    assert_eq!(st.toasts[0].text, "There is nothing to undo in this tab.");
    assert_eq!(st.toasts[0].code, "nothing_to_undo");
}

#[test]
fn a_quiet_refusal_from_a_shortcut_is_not_toasted_but_from_a_button_it_is() {
    let mut rec =
        Recorder { refuse: vec![("group.create", "no_target", "Nothing is selected.")], ..Default::default() };
    let mut st = UiState::new();
    dispatch(&mut rec, &mut st, vec![UiAction::run_from(Origin::Shortcut, "group.create", json!({}))], 0.0);
    assert!(st.toasts.is_empty(), "docs/commands.md §1.5: no_target from a shortcut is silent");
    dispatch(&mut rec, &mut st, vec![UiAction::run("group.create", json!({}))], 0.0);
    assert_eq!(st.toasts.len(), 1);
}

#[test]
fn a_refusal_with_a_suggestion_offers_it_as_a_button() {
    let mut st = UiState::new();
    struct Sink;
    impl CommandSink for Sink {
        fn run(&mut self, _: Origin, id: &str, _: Value) -> Result<Value, Refusal> {
            Err(Refusal::custom("unknown_theme", id, "Unknown theme.").suggesting("style.list_themes", None))
        }
    }
    dispatch(&mut Sink, &mut st, vec![UiAction::run("style.apply_theme", json!({"name": "x"}))], 0.0);
    let s = st.toasts[0].suggestion.as_ref().expect("suggestion");
    assert_eq!((s.id.as_str(), &s.args), ("style.list_themes", &json!({})));
}

#[test]
fn unsaved_changes_asks_and_no_save_target_opens_the_picker() {
    let mut st = UiState::new();
    let mut rec = Recorder {
        refuse: vec![
            ("doc.new", "unsaved_changes", "This document has unsaved changes."),
            ("doc.save", "no_save_target", "No save location yet."),
        ],
        ..Default::default()
    };
    let r = dispatch(&mut rec, &mut st, vec![UiAction::run("doc.new", json!({}))], 0.0);
    assert!(st.toasts.is_empty());
    let confirm = st.confirm.as_ref().expect("a confirm box");
    assert_eq!(confirm.call.id, "doc.new");
    assert_eq!(confirm.message, "This document has unsaved changes.");
    assert!(r.host.is_empty());

    let r = dispatch(&mut rec, &mut st, vec![UiAction::run("doc.save", json!({}))], 0.0);
    assert_eq!(r.host, vec![HostRequest::PickSaveTarget]);
    assert!(st.toasts.is_empty());
}

#[test]
fn identical_toasts_are_not_stacked_and_old_ones_expire() {
    let mut st = UiState::new();
    let mut rec =
        Recorder { refuse: vec![("history.undo", "nothing_to_undo", "Nothing to undo.")], ..Default::default() };
    for t in [0.0, 0.5, 1.0] {
        dispatch(&mut rec, &mut st, vec![UiAction::run("history.undo", json!({}))], t);
    }
    assert_eq!(st.toasts.len(), 1);
    assert!(st.toasts[0].alpha(1.0).is_some());
    st.expire_toasts(20.0);
    assert!(st.toasts.is_empty());
}

#[test]
fn the_dispatcher_records_the_origin_it_was_given() {
    let mut f = Fixture::new();
    f.actions = vec![UiAction::run_from(Origin::Palette, "tool.set", json!({"tool": "arrow"}))];
    f.dispatch();
    let recent = f.session.ring().recent(5, false, None);
    let last = recent.last().expect("ring entry");
    assert_eq!(last.id, "tool.set");
    assert_eq!(last.source, Origin::Palette);
}

#[test]
fn every_executed_call_carries_its_result() {
    let mut f = Fixture::new();
    f.actions = vec![UiAction::run("tool.set", json!({"tool": "arrow"})), UiAction::run("history.undo", json!({}))];
    let r = f.dispatch();
    let results: Vec<(&str, bool)> =
        r.ran.iter().map(|Ran { call, result }| (call.id.as_str(), result.is_ok())).collect();
    assert_eq!(results, [("tool.set", true), ("history.undo", false)]);
    assert_eq!(f.model.tool, drafftink_engine::state::Tool::Arrow);
}
