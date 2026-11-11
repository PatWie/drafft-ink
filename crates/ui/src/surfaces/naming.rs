//! The naming dialogs: rename (document, tab, library item, frame) and Save As.
//!
//! Rename ends in `doc.rename`, `tab.rename`, `library.rename` or `frame.rename`. Save As asks for the
//! document name first: Save runs `doc.rename` when the name changed and then hands the app a
//! [`HostRequest::PickSaveTarget`] (the app calls `doc.save` with `to` afterwards). A refusal (an empty
//! name) keeps the dialog open next to its toast.

use egui::{Context, Id, Key, Modal, RichText};
use serde_json::{Value, json};

use crate::action::{HostRequest, UiAction};
use crate::state::{RenameState, RenameTarget, UiState};
use crate::surfaces::dialogs::{Close, plain_button, show_modal, text_field};
use crate::theme::TextSize;
use crate::widgets::{Ctl, Cx};

/// The longest name the engine takes.
const MAX_NAME_CHARS: usize = 256;

/// The command and arguments that apply a rename.
pub(crate) fn rename_call(target: &RenameTarget, text: &str) -> (&'static str, Value) {
    let name = text.trim();
    match target {
        RenameTarget::Document => ("doc.rename", json!({ "name": name })),
        RenameTarget::Tab(tab) => ("tab.rename", json!({ "tab": tab, "name": name })),
        RenameTarget::LibraryItem(item) => ("library.rename", json!({ "item": item, "name": name })),
        RenameTarget::Frame(frame) => ("frame.rename", json!({ "frame": frame, "name": name })),
    }
}

/// The title of the dialog.
fn title_of(target: &RenameTarget) -> &'static str {
    match target {
        RenameTarget::Document => "Rename the document",
        RenameTarget::Tab(_) => "Rename the tab",
        RenameTarget::LibraryItem(_) => "Rename the library item",
        RenameTarget::Frame(_) => "Rename the frame",
    }
}

/// The name a rename starts from.
fn current_name(cx: &Cx<'_>, target: &RenameTarget) -> String {
    match target {
        RenameTarget::Document => cx.model.tabs.iter().find(|t| t.active).map(|t| t.name.clone()).unwrap_or_default(),
        RenameTarget::Tab(id) => cx.model.tabs.iter().find(|t| &t.id == id).map(|t| t.name.clone()).unwrap_or_default(),
        RenameTarget::LibraryItem(id) => {
            cx.model.library.iter().find(|i| &i.id == id).map(|i| i.name.clone()).unwrap_or_default()
        }
        RenameTarget::Frame(_) => String::new(),
    }
}

/// Draws the dialogs.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    rename(ctx, cx, st);
    save_as(ctx, cx, st);
}

fn rename(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(state) = st.rename.as_mut() else {
        return;
    };
    if !state.focused && state.text.is_empty() {
        state.text = current_name(cx, &state.target);
    }
    let pal = cx.pal;
    let RenameState { target, text, focused } = state;
    let title = title_of(target);
    show_modal(ctx, cx, Modal::new(Id::unique("rename_dialog")), title, 380.0, &Close::Overlays, |ui, cx| {
        let field = text_field(ui, text, "Name", None, Some(MAX_NAME_CHARS), "Name");
        if !*focused {
            field.request_focus();
            *focused = true;
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(10.0);
        let (id, args) = rename_call(target, text);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
            let ok = Ctl::new(id).args(args.clone()).label("Rename");
            let clicked = cx.text_button(ui, &ok, "Rename", true).clicked();
            if enter && !clicked {
                cx.run(id, args.clone());
            }
        });
    });
}

fn save_as(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(state) = st.save_as.as_mut() else {
        return;
    };
    let current = current_name(cx, &RenameTarget::Document);
    let text = state.text.get_or_insert_with(|| current.clone());
    let focused = &mut state.focused;
    let pal = cx.pal;
    show_modal(ctx, cx, Modal::new(Id::unique("save_as_dialog")), "Save as", 380.0, &Close::Overlays, |ui, cx| {
        let field = text_field(ui, text, "Document name", None, Some(MAX_NAME_CHARS), "Document name");
        if !*focused {
            field.request_focus();
            *focused = true;
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(4.0);
        ui.label(
            RichText::new("You choose the place to save it next.").size(TextSize::Small.points()).color(pal.muted),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
            let name = text.trim().to_owned();
            let changed = name != current.trim();
            // An unchanged name goes straight to the picker. A changed one is renamed first, and the
            // rename's success hands the picker request on (`UiState::after_command`).
            let clicked = if changed {
                let ok = Ctl::new("doc.rename").args(json!({ "name": name })).label("Choose a place");
                cx.text_button(ui, &ok, "Choose a place…", true).clicked()
            } else {
                let r = plain_button(ui, &pal, "Choose a place…", true);
                if r.clicked() {
                    cx.emit(UiAction::Host(HostRequest::PickSaveTarget));
                    cx.verb(crate::action::Verb::CloseOverlays);
                }
                r.clicked()
            };
            if enter && !clicked {
                if changed {
                    cx.run("doc.rename", json!({ "name": name }));
                } else {
                    cx.emit(UiAction::Host(HostRequest::PickSaveTarget));
                    cx.verb(crate::action::Verb::CloseOverlays);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_target_has_its_command_and_the_name_is_trimmed() {
        assert_eq!(rename_call(&RenameTarget::Document, "  Plan "), ("doc.rename", json!({"name": "Plan"})));
        assert_eq!(
            rename_call(&RenameTarget::Tab("t2".into()), "A"),
            ("tab.rename", json!({"tab": "t2", "name": "A"}))
        );
        assert_eq!(
            rename_call(&RenameTarget::LibraryItem("i1".into()), "Box"),
            ("library.rename", json!({"item": "i1", "name": "Box"}))
        );
        assert_eq!(
            rename_call(&RenameTarget::Frame("f1".into()), "Part"),
            ("frame.rename", json!({"frame": "f1", "name": "Part"}))
        );
    }
}
