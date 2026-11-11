//! The text editor drawn over the canvas while a text, a label or a draft is being edited.
//!
//! The engine only knows that an edit runs (`Session::text_edit`). Typing, the caret, selection and IME
//! belong to the editor (docs/exceptions.md E16); here that is an egui multi-line field placed where the text
//! will be. The result goes back in one `text.edit_end`, which is one undo step.

use drafftink_canvas::Camera;
use drafftink_edit::text_bind::label_of;
use drafftink_engine::Session;
use drafftink_engine::interact::UiRequest;
use drafftink_geom::kurbo::{Point, Size};
use drafftink_scene::{ElementId, ElementKind, FontFamily, SceneRead};
use drafftink_ui::theme;
use egui::{Align2, Color32, Id, Key, Modifiers, Order, TextEdit};
use serde_json::{Value, json};

/// What an editor wants after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorEvent {
    /// Keep going.
    None,
    /// The person finished (Esc or Ctrl/Cmd+Enter).
    Finish,
}

/// The text being edited and how to draw it.
#[derive(Debug)]
pub(crate) struct TextEditor {
    buffer: String,
    /// Top-left of the text in world units.
    at: Point,
    /// A shape or arrow label is centred on `at`.
    centred: bool,
    family: &'static str,
    size: f64,
    /// The fixed width of a dragged-out text box, in world units. `None`: the field follows the text.
    width: Option<f64>,
    /// Style changes chosen while editing, applied with the result.
    style: Option<Value>,
    focused: bool,
    /// Where the field was drawn last frame, in egui points.
    rect: Option<egui::Rect>,
}

/// The egui font family name of a canvas font.
fn family_name(f: FontFamily) -> &'static str {
    match f {
        FontFamily::GelPenSerif => theme::PREVIEW_SERIF,
        FontFamily::VanillaExtract => theme::PREVIEW_VANILLA,
        _ => theme::HAND_FONT,
    }
}

impl TextEditor {
    /// An editor for what `request` names, or `None` when the element is gone.
    pub(crate) fn open(s: &Session, request: &UiRequest) -> Option<Self> {
        let UiRequest::OpenTextEditor { element, at, width } = request else { return None };
        let store = s.store();
        let Some(id) = element else {
            let mut editor = Self::new(String::new(), *at, false, "gelpen", 20.0);
            editor.width = *width;
            return Some(editor);
        };
        let el = store.get(id)?;
        let (target, centred) = match &el.kind {
            ElementKind::Text(_) => (el, false),
            _ => match label_of(store, id).and_then(|l| store.get(&l)) {
                Some(label) => (label, true),
                None => (el, true),
            },
        };
        let (text, family, size) = match &target.kind {
            ElementKind::Text(t) => (t.content.clone(), family_name(t.font.family), t.font.size),
            _ => (String::new(), theme::HAND_FONT, 20.0),
        };
        let t = &target.transform;
        let at = if centred { Point::new(t.x + t.w / 2.0, t.y + t.h / 2.0) } else { Point::new(t.x, t.y) };
        Some(Self::new(text, at, centred, family, size))
    }

    fn new(buffer: String, at: Point, centred: bool, family: &'static str, size: f64) -> Self {
        Self { buffer, at, centred, family, size, width: None, style: None, focused: false, rect: None }
    }

    /// The field's rectangle in the last frame.
    pub(crate) fn rect(&self) -> Option<egui::Rect> {
        self.rect
    }

    /// Remembers a style the person chose while editing (merged over earlier choices).
    pub(crate) fn add_style(&mut self, props: &Value) {
        let Some(new) = props.as_object() else { return };
        let merged = self.style.get_or_insert_with(|| json!({}));
        if let Some(map) = merged.as_object_mut() {
            map.extend(new.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }

    /// Arguments of `text.edit_end` with what was typed.
    pub(crate) fn finish_args(&self) -> Value {
        let mut args = json!({ "commit": true, "text": self.buffer });
        if let (Some(style), Some(map)) = (&self.style, args.as_object_mut()) {
            map.insert("style".to_owned(), style.clone());
        }
        args
    }

    /// Draws the field and reports whether the person finished. `size` is the canvas in points.
    pub(crate) fn show(&mut self, ctx: &egui::Context, camera: Camera, size: Size) -> EditorEvent {
        // Before the field is drawn: egui's own text field gives up focus on Esc.
        let finish = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::COMMAND, Key::Enter)
        });
        if finish {
            return EditorEvent::Finish;
        }
        let zoom = camera.zoom;
        let to_screen = |p: Point| {
            egui::pos2(
                ((p.x - camera.scroll.x) * zoom + size.width / 2.0) as f32,
                ((p.y - camera.scroll.y) * zoom + size.height / 2.0) as f32,
            )
        };
        let pos = to_screen(self.at);
        let font = theme::named(self.family, (self.size * zoom) as f32);
        let width = self.width.map_or_else(|| (self.size * zoom * 8.0).max(120.0), |w| w * zoom) as f32;
        let anchor = if self.centred { Align2::CENTER_CENTER } else { Align2::LEFT_TOP };
        let inner = egui::Area::new(Id::unique("drafftink-text-editor"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .pivot(anchor)
            .show(ctx, |ui| {
                let visuals = ui.visuals().clone();
                let out = TextEdit::multiline(&mut self.buffer)
                    .font(font)
                    .frame(egui::Frame::NONE)
                    .desired_rows(1)
                    .desired_width(width)
                    .align(anchor)
                    .text_color(visuals.text_color())
                    .lock_focus(true)
                    .show(ui);
                if !self.focused {
                    out.response.request_focus();
                    self.focused = true;
                }
                out.response
            });
        self.rect = Some(inner.response.rect.expand(4.0));
        let underline = inner.response.rect;
        ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("drafftink-text-editor-frame")))
            .rect_stroke(
                underline.expand(3.0),
                3.0,
                egui::Stroke::new(1.0, Color32::from_rgb(0x3b, 0x82, 0xf6)),
                egui::StrokeKind::Outside,
            );
        EditorEvent::None
    }
}

/// The element a text edit session belongs to, for the hidden list of the composer.
pub(crate) fn hidden_by_edit(s: &Session) -> Vec<ElementId> {
    let Some(edit) = s.text_edit.as_ref() else { return Vec::new() };
    let Some(id) = edit.element.as_ref() else { return Vec::new() };
    let store = s.store();
    match store.get(id).map(|e| &e.kind) {
        Some(ElementKind::Text(_)) => vec![id.clone()],
        Some(_) => label_of(store, id).into_iter().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_carries_the_text_and_the_styles_chosen_while_editing() {
        let mut e = TextEditor::new("hello".into(), Point::new(1.0, 2.0), false, "gelpen", 20.0);
        assert_eq!(e.finish_args(), json!({ "commit": true, "text": "hello" }));
        e.add_style(&json!({ "stroke_color": "red-600" }));
        e.add_style(&json!({ "font_size": 28 }));
        assert_eq!(
            e.finish_args(),
            json!({ "commit": true, "text": "hello", "style": { "stroke_color": "red-600", "font_size": 28 } })
        );
        e.add_style(&json!(5));
        assert!(e.finish_args()["style"].is_object());
    }
}
