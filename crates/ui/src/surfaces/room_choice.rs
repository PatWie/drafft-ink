//! The question before a canvas or a file goes into a shared room: add it to the room, where the others cannot undo
//! it, or keep it apart in a new tab.
//!
//! Two things open it. A room link entered on a tab that has content is refused as `local_content`, and a document
//! dropped on a tab in a room is held back by the app (a dropped document would otherwise open a new tab and leave the
//! room untouched). Both answers repeat the held call with the decision filled in as an argument, so the command layer
//! keeps its safe default and never asks anything itself.

use egui::{Context, Id, Modal, RichText};
use serde_json::{Value, json};

use crate::action::Call;
use crate::state::{RoomChoice, RoomChoiceKind, UiState};
use crate::surfaces::dialogs::{Close, plain_button, show_modal};
use crate::theme::TextSize;
use crate::widgets::{Ctl, Cx};

/// The commands that join a room from a room link.
const JOINS: [&str; 2] = ["collab.join_room", "collab.connect"];
/// Most file names the question lists.
const NAMES_SHOWN: usize = 3;

/// The choice a refusal asks for, if it is one. Only a call of the person that left the decision open: one that
/// already says `on_local`, or that opens the file in a mode, is answered as it is.
pub(crate) fn asked_by(call: &Call, code: &str) -> Option<RoomChoiceKind> {
    match code {
        "local_content" if JOINS.contains(&call.id.as_str()) && call.args.get("room").is_some() => {
            call.args.get("on_local").is_none().then_some(RoomChoiceKind::JoinWithContent)
        }
        "in_room" if call.id == "doc.open" => {
            let replacing = call.args.get("mode").is_none_or(|m| m == "replace");
            replacing.then(|| RoomChoiceKind::DocumentInRoom { names: Vec::new() })
        }
        _ => None,
    }
}

/// `call` with one more argument.
fn with_arg(call: &Call, key: &str, value: &str) -> Call {
    let mut args = match &call.args {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    args.insert(key.to_owned(), json!(value));
    Call::new(call.origin, call.id.clone(), Value::Object(args))
}

/// The argument that says where a document goes: `documents` of `doc.drop`, `mode` of `doc.open`.
fn placement_key(call: &Call) -> &'static str {
    if call.id == "doc.drop" { "documents" } else { "mode" }
}

/// What each answer runs, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Answers {
    /// Put it into the room.
    pub add: Vec<Call>,
    /// Keep it apart, in a new tab.
    pub apart: Vec<Call>,
}

/// The calls of both answers for a held call.
pub(crate) fn answers(choice: &RoomChoice) -> Answers {
    let call = &choice.call;
    match choice.kind {
        RoomChoiceKind::JoinWithContent => Answers {
            add: vec![with_arg(call, "on_local", "merge")],
            // The room opens in a tab of its own, which starts empty, so nothing is merged.
            apart: vec![Call::new(call.origin, "tab.new", json!({})), call.clone()],
        },
        RoomChoiceKind::DocumentInRoom { .. } => {
            let key = placement_key(call);
            Answers { add: vec![with_arg(call, key, "merge")], apart: vec![with_arg(call, key, "new_tab")] }
        }
    }
}

/// The question and the one-line consequence under it.
pub(crate) fn texts(kind: &RoomChoiceKind) -> (&'static str, String, &'static str) {
    match kind {
        RoomChoiceKind::JoinWithContent => (
            "Join the room",
            "Add this canvas to the room, or open the room in a new tab?".to_owned(),
            "Adding puts everything on this tab into the room, and the others cannot undo it. Joining also clears the undo history of this tab.",
        ),
        RoomChoiceKind::DocumentInRoom { names } => {
            let what = match names.as_slice() {
                [] => "this file".to_owned(),
                [one] => format!("`{one}`"),
                many => {
                    let shown: Vec<_> = many.iter().take(NAMES_SHOWN).map(|n| format!("`{n}`")).collect();
                    let more = many.len().saturating_sub(NAMES_SHOWN);
                    if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
                }
            };
            (
                "Add to this room?",
                format!("Add {what} to this room, or open it in a new tab?"),
                "Adding puts it on the canvas for everyone in the room, in one step you can undo here.",
            )
        }
    }
}

/// Draws the question while one is pending. The default answer is the one that touches nobody else's work for a
/// room link (a new tab) and the one the person meant for a file they dropped here (add it).
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(choice) = st.room_choice.clone() else {
        return;
    };
    let pal = cx.pal;
    let (title, question, consequence) = texts(&choice.kind);
    let Answers { add, apart } = answers(&choice);
    let add_default = choice.kind.adds_by_default();
    let (add_label, apart_label) = match choice.kind {
        RoomChoiceKind::JoinWithContent => ("Add this canvas to the room", "Open in a new tab"),
        RoomChoiceKind::DocumentInRoom { .. } => ("Add to this room", "Open in a new tab"),
    };
    let mut answered = false;
    show_modal(ctx, cx, Modal::new(Id::unique("room_choice_dialog")), title, 400.0, &Close::Overlays, |ui, cx| {
        ui.label(RichText::new(question).size(TextSize::Body.points()).color(pal.text));
        ui.add_space(4.0);
        ui.label(RichText::new(consequence).size(TextSize::Small.points()).color(pal.muted));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            // The repeat comes from the same surface as the call that was held.
            cx.with_origin(choice.call.origin, |cx| {
                let buttons = [(add_label, &add, add_default), (apart_label, &apart, !add_default)];
                // The default answer comes first, so it is the one under the thumb.
                for (label, calls, primary) in
                    buttons.into_iter().filter(|b| b.2).chain(buttons.into_iter().filter(|b| !b.2))
                {
                    let Some((first, rest)) = calls.split_first() else { continue };
                    let ctl = Ctl::new(&first.id).args(first.args.clone()).label(label);
                    if cx.text_button(ui, &ctl, label, primary).clicked() {
                        for next in rest {
                            cx.run(&next.id, next.args.clone());
                        }
                        answered = true;
                    }
                }
            });
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                answered = true;
            }
        });
    });
    if answered {
        st.room_choice = None;
    }
}
