//! The on-canvas text editor.
//!
//! While a text, a label or a draft is being edited, the editor owns the content (`docs/exceptions.md` E16). The
//! model is `drafftink_edit::text_edit::Editor`: buffer, colour runs, selection, motions, composition and the
//! editor's own undo. This module wires it to the app: it finds out where the text sits ([`subject`]), turns
//! egui and shell events into model calls ([`keys`]), and describes what to draw. The text itself is drawn by the
//! renderer from the element the commit would leave behind, so the editor and the committed text are the same
//! picture; the caret, the selection and the composition underline are the overlay's.
//!
//! The result goes back in one `text.edit_end`.

mod keys;
mod subject;

use drafftink_canvas::Camera;
use drafftink_edit::style::StyleProps;
use drafftink_edit::text_bind::TextProps;
use drafftink_edit::text_edit::{Clicks, Editor, Face};
use drafftink_engine::Session;
use drafftink_engine::interact::UiRequest;
use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_gpu::TextInput;
use drafftink_render::{TextEdit as TextEditOverlay, Theme};
use drafftink_scene::{Color, ElementId, InlineMath};
use drafftink_typeset::Typesetter;
use drafftink_ui::TextFaces;
use serde_json::{Value, json};

pub(crate) use keys::{EditorKey, KeyRoute, route};

#[cfg(test)]
mod pixels;
#[cfg(test)]
mod tests;
pub(crate) use subject::Arranged;
use subject::Subject;

/// How long the caret stays on, and off, in milliseconds.
const BLINK_HALF_PERIOD_MS: f64 = 530.0;

/// The text being edited.
pub(crate) struct TextEditor {
    pub(super) model: Editor,
    subject: Subject,
    /// Style changes chosen while editing that apply to the whole element (font, size, alignment, opacity).
    props: StyleProps,
    /// The same as the typed text properties; what the layout uses.
    text_props: TextProps,
    props_revision: u64,
    initial_math: InlineMath,
    clicks: Clicks,
    /// When the caret last moved or changed state, on the app clock; the blink counts from it.
    blink_from_ms: f64,
    /// What the layout was made for.
    arranged: Option<(Key, Arranged)>,
    /// Where the pointer went down, to tell the selection drag from a click elsewhere.
    pressing: bool,
}

/// What an arrangement depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    revision: u64,
    props: u64,
}

impl TextEditor {
    /// An editor for what `request` names, or `None` when the element is gone. `caret_hint` is a world point that
    /// puts the caret there when it lies on the text (a double click), and `now_ms` the app clock.
    pub(crate) fn open(
        s: &Session,
        request: &UiRequest,
        caret_hint: Option<Point>,
        ts: &mut Typesetter,
        now_ms: f64,
    ) -> Option<Self> {
        let (subject, initial) = Subject::open(s, request)?;
        let model = Editor::new(&initial.text, &initial.runs, initial.math);
        let mut editor = Self {
            model,
            subject,
            props: StyleProps::default(),
            text_props: TextProps::default(),
            props_revision: 0,
            initial_math: initial.math,
            clicks: Clicks::default(),
            blink_from_ms: now_ms,
            arranged: None,
            pressing: false,
        };
        if let Some(world) = caret_hint {
            editor.place_caret(world, ts);
        }
        Some(editor)
    }

    fn place_caret(&mut self, world: Point, ts: &mut Typesetter) {
        let arranged = self.arrange(ts).clone();
        let local = arranged.to_local(world);
        if arranged.layout.bounds().inflate(2.0, 2.0).contains(local) {
            self.model.press(&arranged.layout, local, 1, false);
            self.model.release();
        }
    }

    /// The laid-out text, made again when the content or a style changed.
    fn arrange(&mut self, ts: &mut Typesetter) -> &Arranged {
        let key = Key { revision: self.model.revision(), props: self.props_revision };
        if self.arranged.as_ref().is_none_or(|(k, _)| *k != key) {
            let shown = self.model.shown();
            let arranged = self.subject.arrange(&shown, self.model.inline_math(), &self.text_props, ts);
            self.arranged = Some((key, arranged));
        }
        match &self.arranged {
            Some((_, a)) => a,
            None => unreachable!("the arrangement was just made"),
        }
    }

    /// The document elements the scene leaves out while the editor draws them.
    pub(crate) fn hidden(&self) -> Vec<ElementId> {
        self.subject.hidden(self.arranged.as_ref().map(|(_, a)| a))
    }

    /// Changes whenever the picture of the editor changes (not the caret blink).
    pub(crate) fn picture_key(&self) -> (u64, u64) {
        (self.model.revision(), self.props_revision)
    }

    /// What the renderer draws, made for the current content.
    pub(crate) fn paint(&mut self, ts: &mut Typesetter) -> &Arranged {
        self.arrange(ts)
    }

    /// The caret and the selection for the overlay, with the caret on or off by the blink.
    pub(crate) fn overlay(&mut self, ts: &mut Typesetter, theme: Theme, now_ms: f64) -> TextEditOverlay {
        let caret_on = self.caret_on(now_ms);
        let shown = self.model.shown();
        let arranged = self.arrange(ts).clone();
        let selection = self.model.typeset_selection(&arranged.layout);
        let color = peniko_color(theme.map(arranged.text.style.stroke_color));
        let edit = TextEditOverlay::new(arranged.to_world, &arranged.layout, &selection, caret_on, color);
        match shown.composition {
            Some(range) => edit.with_composition(&arranged.layout, range),
            None => edit,
        }
    }

    /// Whether the caret shows at `now_ms`.
    pub(crate) fn caret_on(&self, now_ms: f64) -> bool {
        let elapsed = (now_ms - self.blink_from_ms).max(0.0);
        ((elapsed / BLINK_HALF_PERIOD_MS) as u64).is_multiple_of(2)
    }

    /// When the caret next changes state, on the app clock.
    pub(crate) fn next_blink_ms(&self, now_ms: f64) -> f64 {
        let elapsed = (now_ms - self.blink_from_ms).max(0.0);
        self.blink_from_ms + ((elapsed / BLINK_HALF_PERIOD_MS).floor() + 1.0) * BLINK_HALF_PERIOD_MS
    }

    /// The caret has just moved or the text changed: show it solid again.
    fn keep_caret_on(&mut self, now_ms: f64) {
        self.blink_from_ms = now_ms;
    }

    /// The caret as a rectangle in canvas points, for the input method's candidate window.
    pub(crate) fn caret_on_screen(&mut self, ts: &mut Typesetter, camera: Camera, canvas: Size) -> Option<Rect> {
        let arranged = self.arrange(ts).clone();
        let selection = self.model.typeset_selection(&arranged.layout);
        let caret = arranged.layout.caret_rect(selection.focus(), 0.0);
        let to_screen = |p: Point| {
            let w = arranged.to_world * p;
            Point::new(
                (w.x - camera.scroll.x) * camera.zoom + canvas.width / 2.0,
                (w.y - camera.scroll.y) * camera.zoom + canvas.height / 2.0,
            )
        };
        let (a, b) = (to_screen(Point::new(caret.x0, caret.y0)), to_screen(Point::new(caret.x0 + 1.0, caret.y1)));
        Some(Rect::from_points(a, b)).filter(|r| r.is_finite())
    }

    /// Whether a world point is on the text or close to it.
    pub(crate) fn hit(&mut self, ts: &mut Typesetter, world: Point, margin: f64) -> bool {
        let arranged = self.arrange(ts);
        let local = arranged.to_local(world);
        let slack = if margin.is_finite() { margin.max(0.0) } else { 0.0 };
        arranged.layout.box_size.to_rect().inflate(slack, slack).contains(local)
            || arranged.layout.bounds().inflate(slack, slack).contains(local)
    }

    // ---- input -------------------------------------------------------------------------------------------

    /// A key. Returns whether the picture changed.
    pub(crate) fn key(&mut self, key: EditorKey, mac: bool, ts: &mut Typesetter, now_ms: f64) -> bool {
        let Some(action) = key.action(mac) else { return false };
        let layout = self.arrange(ts).layout.clone();
        let changed = self.model.apply(action, &layout);
        if changed {
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// Text from the input method or the keyboard.
    pub(crate) fn text_input(&mut self, input: &TextInput, now_ms: f64) -> bool {
        let changed = match input {
            TextInput::Preedit { text, cursor } => {
                self.model.preedit(text, *cursor);
                true
            }
            TextInput::Commit(text) => self.model.commit(text),
            TextInput::Cancel => {
                let was = self.model.is_composing();
                self.model.cancel_composition();
                was
            }
        };
        if changed {
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// Pasted text.
    pub(crate) fn paste(&mut self, text: &str, now_ms: f64) -> bool {
        let changed = self.model.paste(text);
        if changed {
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// The selected text for the clipboard; with `cut` it is removed.
    pub(crate) fn copy(&mut self, cut: bool, now_ms: f64) -> Option<String> {
        if cut {
            let text = self.model.cut();
            if text.is_some() {
                self.keep_caret_on(now_ms);
            }
            text
        } else {
            self.model.copy()
        }
    }

    /// A press of the primary button at a world point. `pos` is where it is on the screen, for counting clicks.
    pub(crate) fn press(&mut self, ts: &mut Typesetter, world: Point, pos: Point, shift: bool, now_ms: f64) {
        let clicks = self.clicks.press(pos, now_ms);
        let arranged = self.arrange(ts).clone();
        self.model.press(&arranged.layout, arranged.to_local(world), clicks, shift);
        self.pressing = true;
        self.keep_caret_on(now_ms);
    }

    /// The pointer moved with the button down.
    pub(crate) fn drag(&mut self, ts: &mut Typesetter, world: Point, now_ms: f64) -> bool {
        if !self.pressing {
            return false;
        }
        let arranged = self.arrange(ts).clone();
        let changed = self.model.drag(&arranged.layout, arranged.to_local(world));
        if changed {
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// The button went up.
    pub(crate) fn release(&mut self) {
        self.pressing = false;
        self.model.release();
    }

    /// Whether the button is down in the text.
    pub(crate) fn is_pressing(&self) -> bool {
        self.pressing
    }

    // ---- style -------------------------------------------------------------------------------------------

    /// A style the person chose while editing. A colour goes to the selected characters when some are selected;
    /// everything else (and a colour with nothing selected) applies to the whole text and goes with the result.
    /// Returns whether the picture changed.
    pub(crate) fn add_style(&mut self, props: &Value, now_ms: f64) -> bool {
        let Ok(mut chosen) = serde_json::from_value::<StyleProps>(props.clone()) else { return false };
        let mut changed = false;
        if let Some(color) = chosen.stroke_color.take() {
            if self.model.is_collapsed() {
                self.props.stroke_color = Some(color);
                changed = true;
            } else {
                changed |= self.model.set_color(Some(color.0));
            }
        }
        if chosen != StyleProps::default() {
            merge(&mut self.props, chosen);
            changed = true;
        }
        if changed {
            self.text_props = TextProps::from_style(&self.props).unwrap_or_default();
            self.props_revision += 1;
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// Bold or italic chosen in the style panel while editing: the arguments of `text.format`. They apply to the
    /// selected characters, or to what is typed next. Returns whether the picture changed.
    pub(crate) fn add_format(&mut self, args: &Value, now_ms: f64) -> bool {
        let mut changed = false;
        for (key, face) in [("bold", Face::Bold), ("italic", Face::Italic)] {
            if let Some(on) = args.get(key).and_then(Value::as_bool) {
                changed |= self.model.set_face(face, on);
            }
        }
        if changed {
            self.keep_caret_on(now_ms);
        }
        changed
    }

    /// Bold and italic at the selection, for the style panel.
    pub(crate) fn faces(&self) -> TextFaces {
        TextFaces { bold: self.model.face_state(Face::Bold), italic: self.model.face_state(Face::Italic) }
    }

    // ---- the end -----------------------------------------------------------------------------------------

    /// Arguments of `text.edit_end`.
    pub(crate) fn finish_args(&self) -> Value {
        let done = self.model.finish();
        let mut args = json!({ "commit": true, "text": done.text, "runs": done.runs });
        if let Some(map) = args.as_object_mut() {
            if self.props != StyleProps::default()
                && let Ok(style) = serde_json::to_value(&self.props)
            {
                map.insert("style".to_owned(), style);
            }
            if done.inline_math != self.initial_math {
                let mode = match done.inline_math {
                    InlineMath::Auto => "auto",
                    InlineMath::Off => "off",
                };
                map.insert("inline_math".to_owned(), json!(mode));
            }
        }
        args
    }
}

/// `new` over `old`: what `new` sets wins.
fn merge(old: &mut StyleProps, new: StyleProps) {
    macro_rules! take {
        ($($f:ident),*) => { $( if new.$f.is_some() { old.$f = new.$f; } )* };
    }
    take!(stroke_color, opacity, font_family, font_weight, font_size, text_align, vertical_align, line_height);
}

fn peniko_color(c: Color) -> vello::peniko::Color {
    vello::peniko::Color::from_rgba8(c.r, c.g, c.b, c.a)
}
