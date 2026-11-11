//! The UI reads what to do after a command from the registration (`CommandSpec::host`), not from a list of ids.
//! These tests drive `UiState::after_command` with the commands the surfaces run, so a command that loses its
//! host fact breaks one of them.

use drafftink_engine::spec::{HostEffect, RefusalToast};
use drafftink_engine::{Origin, Refusal, Registry};
use serde_json::{Value, json};

use crate::action::{Call, HostRequest};
use crate::state::{RenameTarget, UiState};

fn run(state: &mut UiState, id: &str, args: Value, result: &Result<Value, Refusal>) -> Vec<HostRequest> {
    let mut host = Vec::new();
    state.after_command(&Call::new(Origin::Ui, id, args), result, 0.0, &mut host);
    host
}

fn effect_of(id: &str) -> HostEffect {
    Registry::global().find(id).map(|s| s.host.effect).unwrap_or(HostEffect::None)
}

#[test]
fn the_rename_commands_of_the_dialogs_close_them_and_a_refusal_keeps_them_open() {
    for (target, id) in [
        (RenameTarget::Document, "doc.rename"),
        (RenameTarget::Tab("t1".into()), "tab.rename"),
        (RenameTarget::LibraryItem("i1".into()), "library.rename"),
        (RenameTarget::Frame("f1".into()), "frame.rename"),
    ] {
        let mut st = UiState::new();
        st.open_rename(target.clone(), "old");
        let refused = Err(Refusal::custom("empty_name", id, "The name cannot be empty."));
        run(&mut st, id, json!({}), &refused);
        assert!(st.rename.is_some(), "{id}: a refusal keeps the dialog open");
        run(&mut st, id, json!({}), &Ok(json!({})));
        assert!(st.rename.is_none(), "{id}: success closes the dialog; it needs the host effect in its registration");
    }
}

#[test]
fn a_rename_of_the_document_goes_on_to_the_save_picker_when_save_as_asked_for_it() {
    let mut st = UiState::new();
    st.save_as = Some(Default::default());
    let host = run(&mut st, "doc.rename", json!({}), &Ok(json!({})));
    assert_eq!(host, [HostRequest::PickSaveTarget]);
    // Another rename command does not.
    st.save_as = Some(Default::default());
    let host = run(&mut st, "tab.rename", json!({}), &Ok(json!({})));
    assert!(host.is_empty());
}

#[test]
fn scrub_gestures_follow_the_gesture_commands_and_lost_ones_are_silent() {
    let mut st = UiState::new();
    run(&mut st, "gesture.begin", json!({}), &Ok(json!({ "gesture": "g1" })));
    assert_eq!(st.scrub_gesture(), Some("g1"));
    let lost = Err(Refusal::custom("no_gesture", "gesture.update", "There is no running gesture."));
    run(&mut st, "gesture.update", json!({}), &lost);
    assert_eq!(st.scrub_gesture(), None, "a refused update ends the scrub");
    assert!(st.toasts.is_empty(), "and says nothing");
    // A refusal of a begin is a toast, like any other command's.
    let refused = Err(Refusal::custom("view_only", "gesture.begin", "The document is view-only."));
    run(&mut st, "gesture.begin", json!({}), &refused);
    assert_eq!(st.toasts.len(), 1);
}

#[test]
fn the_effects_the_surfaces_rely_on_are_declared_on_their_commands() {
    for (id, effect) in [
        ("doc.export", HostEffect::CompletesExport),
        ("library.add", HostEffect::CompletesLibraryAdd),
        ("element.convert", HostEffect::ShowsConvertPopup),
        ("element.set_link", HostEffect::CompletesLinkEdit),
        ("element.open_link", HostEffect::ReturnsUrlToOpen),
        ("element.query", HostEffect::ShowsSearchResults),
        ("text.edit_end", HostEffect::EndsEquationEdit),
        ("gesture.end", HostEffect::EndsGesture),
        ("gesture.cancel", HostEffect::EndsGesture),
    ] {
        assert_eq!(effect_of(id), effect, "{id}");
    }
    for id in ["gesture.update", "gesture.end", "gesture.cancel"] {
        assert_eq!(Registry::global().find(id).map(|s| s.host.refusal), Some(RefusalToast::Silent), "{id}");
    }
}
