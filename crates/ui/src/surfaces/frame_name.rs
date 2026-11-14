//! The field that renames a frame in place: over its name label, in the label's font, with the name selected.
//!
//! A double-click on the label asks for it (`UiRequest::OpenFrameNameEditor`; `UiState::open_frame_name`). Enter or a
//! click away commits through `frame.rename`, Esc cancels. The canvas keeps its pointer while the field is open.

use drafftink_scene::FrameData;
use egui::{Align2, Area, Context, Frame, Id, Key, Margin, Order, Stroke, TextEdit, vec2};

use crate::state::{RenameTarget, UiState};
use crate::surfaces::dialogs::focus_and_select_all;
use crate::surfaces::naming::rename_call;
use crate::theme::{self, Palette};
use crate::widgets::Cx;

/// The longest name the engine takes.
const MAX_NAME_CHARS: usize = 256;
/// Space between the text and the border of the field, in points.
const PAD: i8 = 2;

/// Draws the field when `st.frame_name` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(edit) = st.frame_name.as_mut() else {
        return;
    };
    let pal = cx.pal;
    // The camera the canvas draws with, so that the field stays on its label while the view moves.
    let camera = cx.model.live_camera.unwrap_or(cx.model.view);
    let corner = camera.world_to_screen(edit.at);
    let zoom = if camera.zoom > 0.0 { camera.zoom } else { 1.0 };
    // The label is cut at the frame's width on screen and never narrower than the minimum.
    let width = ((edit.width * zoom) as f32).max(FrameData::LABEL_MIN_WIDTH as f32);
    // The text of the field starts where the label's does; its bottom edge is where the label's is.
    let anchor = corner + vec2(-f32::from(PAD) - 1.0, -(FrameData::LABEL_GAP as f32) + f32::from(PAD) + 1.0);
    let escaped = ctx.input(|i| i.key_pressed(Key::Escape));
    let mut commit = false;
    Area::new(Id::unique("frame_name_field"))
        .order(Order::Foreground)
        .fixed_pos(anchor)
        .pivot(Align2::LEFT_BOTTOM)
        .show(ctx, |ui| {
            field_frame(&pal).show(ui, |ui| {
                let field = ui.add(
                    TextEdit::singleline(&mut edit.text)
                        .id(Id::unique("frame_name_text"))
                        .font(theme::hand(FrameData::LABEL_SIZE as f32))
                        .text_color(pal.text)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .char_limit(MAX_NAME_CHARS)
                        .desired_width(width),
                );
                field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Frame name"));
                if !edit.focused {
                    focus_and_select_all(ui, &field, &edit.text);
                    edit.focused = true;
                }
                // The field has the keyboard until Enter or a click away takes it, which commits. Esc takes it
                // too and cancels (below), so a lost focus alone does not say which of them it was.
                if field.has_focus() {
                    edit.had_focus = true;
                } else if std::mem::take(&mut edit.had_focus) {
                    commit = true;
                }
            });
        });
    if escaped {
        // The keymap does not see Esc while a text field has the keyboard, so it is read here. It ends the edit at
        // once, so that the focus Esc takes from the field is never read as a commit.
        st.close_overlays();
    } else if commit {
        finish(cx, st);
    }
}

/// Ends the edit: a name that did not change closes the field with no call, any other runs `frame.rename`.
fn finish(cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(edit) = st.frame_name.as_ref() else {
        return;
    };
    if edit.text.trim() == edit.opened_with.trim() {
        st.frame_name = None;
        return;
    }
    let (id, args) = rename_call(&RenameTarget::Frame(edit.frame.clone()), &edit.text);
    cx.run(id, args);
}

/// The look of the field: the panel's colour with the accent as its border, so that it reads as an edit over the
/// drawing.
fn field_frame(pal: &Palette) -> Frame {
    Frame::new()
        .fill(pal.panel)
        .stroke(Stroke::new(1.0, pal.accent))
        .corner_radius(theme::BUTTON_RADIUS)
        .inner_margin(Margin::same(PAD))
}
