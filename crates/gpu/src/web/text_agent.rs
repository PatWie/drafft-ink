//! The hidden `<textarea>` ("text agent") that gets IME composition, soft keyboards and, while it has
//! focus, the keyboard events winit cannot see (winit only listens on the canvas).
//!
//! eframe's `text_agent.rs` is the model; this version differs in three ways. It keeps the textarea
//! empty and takes committed text from `InputEvent.data` instead of diffing the value, so there is no
//! value state to get out of sync. It also reports every event to the app as `ShellEvent::TextInput`.
//! And it is focused only while the app asks for it (`set_text_input`), because focusing a textarea
//! raises the on-screen keyboard on phones.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{CompositionEvent, FocusEvent, HtmlCanvasElement, HtmlTextAreaElement, InputEvent, KeyboardEvent};

use super::dom::{self, ListenOptions, Listener};
use super::keys;
use crate::events::{ShellEvent, TextInput};
use crate::host::HostSink;

/// State shared between the textarea listeners, the pointer listeners and [`TextAgent`].
pub(super) struct Core {
    pub(super) textarea: HtmlTextAreaElement,
    canvas: HtmlCanvasElement,
    sink: HostSink,
    is_mac: bool,
    /// The app asked for text input and has not withdrawn the request.
    wanted: Cell<bool>,
    composing: Cell<bool>,
    /// The text of the last composition commit. Some browsers send an `insertText` input event for it
    /// as well. The next key press clears this.
    just_committed: RefCell<Option<String>>,
    /// Keys the textarea reported as down, to release them when the window loses focus or Ctrl/Cmd
    /// comes up (Chromium sends no keyup for the letter of Ctrl+A).
    down: RefCell<HashSet<egui::Key>>,
    /// We are moving focus ourselves: do not report the textarea blur as the window losing focus.
    moving_focus: Cell<bool>,
    last_rect: Cell<Option<egui::Rect>>,
}

pub(super) struct TextAgent {
    core: Rc<Core>,
    listeners: Vec<Listener>,
}

impl TextAgent {
    pub(super) fn attach(canvas: &HtmlCanvasElement, sink: HostSink, is_mac: bool) -> Result<Self, String> {
        let document = dom::document()?;
        let textarea = document
            .create_element("textarea")
            .map_err(|e| dom::js_error(&e))?
            .dyn_into::<HtmlTextAreaElement>()
            .map_err(|_| "textarea element has the wrong type".to_owned())?;
        for (name, value) in [
            ("autocapitalize", "off"),
            ("autocomplete", "off"),
            ("autocorrect", "off"),
            ("spellcheck", "false"),
            ("rows", "1"),
            ("tabindex", "-1"),
            ("aria-label", "Text input"),
            ("data-drafftink", "text-agent"),
        ] {
            textarea.set_attribute(name, value).map_err(|e| dom::js_error(&e))?;
        }
        let style = textarea.style();
        for (name, value) in [
            ("position", "fixed"),
            ("left", "0px"),
            ("top", "0px"),
            ("width", "1px"),
            ("height", "1px"),
            ("margin", "0"),
            ("padding", "0"),
            ("border", "none"),
            ("outline", "none"),
            ("resize", "none"),
            ("overflow", "hidden"),
            ("opacity", "0"),
            ("background", "transparent"),
            ("color", "transparent"),
            ("caret-color", "transparent"),
            ("pointer-events", "none"),
            // 16px or more stops iOS Safari from zooming the page when the field takes focus.
            ("font-size", "16px"),
        ] {
            style.set_property(name, value).map_err(|e| dom::js_error(&e))?;
        }
        document.body().ok_or("document has no body")?.append_child(&textarea).map_err(|e| dom::js_error(&e))?;

        let core = Rc::new(Core {
            textarea,
            canvas: canvas.clone(),
            sink,
            is_mac,
            wanted: Cell::new(false),
            composing: Cell::new(false),
            just_committed: RefCell::new(None),
            down: RefCell::new(HashSet::new()),
            moving_focus: Cell::new(false),
            last_rect: Cell::new(None),
        });
        let listeners = Self::listen(&core)?;
        Ok(Self { core, listeners })
    }

    fn listen(core: &Rc<Core>) -> Result<Vec<Listener>, String> {
        let target: &web_sys::EventTarget = core.textarea.as_ref();
        let opts = ListenOptions::default();
        let mut out = Vec::new();

        let c = Rc::clone(core);
        out.push(Listener::new(target, "compositionstart", opts, move |_: CompositionEvent| c.composition_start())?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "compositionupdate", opts, move |e: CompositionEvent| {
            c.composition_update(e.data().unwrap_or_default());
        })?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "compositionend", opts, move |e: CompositionEvent| {
            c.composition_end(e.data().unwrap_or_default());
        })?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "beforeinput", opts, move |e: InputEvent| c.before_input(&e))?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "input", opts, move |e: InputEvent| c.input(&e))?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "keydown", opts, move |e: KeyboardEvent| c.key(&e, true))?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "keyup", opts, move |e: KeyboardEvent| c.key(&e, false))?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "focus", opts, move |_: FocusEvent| {
            c.sink.egui(egui::Event::WindowFocused(true))
        })?);
        let c = Rc::clone(core);
        out.push(Listener::new(target, "blur", opts, move |e: FocusEvent| c.blur(&e))?);
        Ok(out)
    }

    pub(super) fn core(&self) -> Rc<Core> {
        Rc::clone(&self.core)
    }

    /// See [`super::WebHost::set_text_input`].
    pub(super) fn set(&self, caret_css: Option<egui::Rect>) {
        self.core.set(caret_css);
    }

    pub(super) fn has_focus(&self) -> bool {
        dom::is_active(&self.core.textarea)
    }
}

impl Drop for TextAgent {
    fn drop(&mut self) {
        // Listeners first: removing a focused element fires `blur`, which must not reach the sink.
        self.listeners.clear();
        self.core.textarea.remove();
    }
}

impl Core {
    fn text_input(&self, event: TextInput) {
        self.sink.app(ShellEvent::TextInput(event));
    }

    fn clear_value(&self) {
        self.textarea.set_value("");
    }

    fn composition_start(&self) {
        self.composing.set(true);
        self.just_committed.borrow_mut().take();
        #[allow(deprecated)] // egui no longer uses `Enabled`, but integrations still report it.
        self.sink.egui(egui::Event::Ime(egui::ImeEvent::Enabled));
    }

    fn composition_update(&self, text: String) {
        self.composing.set(true);
        self.sink.egui(egui::Event::Ime(egui::ImeEvent::Preedit { text: text.clone(), active_range_chars: None }));
        self.text_input(TextInput::Preedit { text, cursor: None });
    }

    fn composition_end(&self, text: String) {
        self.composing.set(false);
        if text.is_empty() {
            // An empty preedit tells egui the composition was dismissed.
            self.sink.egui(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
            self.text_input(TextInput::Cancel);
        } else {
            *self.just_committed.borrow_mut() = Some(text.clone());
            self.sink.egui(egui::Event::Ime(egui::ImeEvent::Commit(text.clone())));
            self.text_input(TextInput::Commit(text));
        }
        #[allow(deprecated)]
        self.sink.egui(egui::Event::Ime(egui::ImeEvent::Disabled));
        self.clear_value();
    }

    /// Soft keyboards (Android Gboard) report Backspace as keydown 229 plus a delete `beforeinput`, with
    /// nothing to delete in our empty textarea. Turn that into the key press egui expects. A desktop
    /// Backspace never gets here: its keydown is cancelled, which cancels the `beforeinput`.
    fn before_input(&self, event: &InputEvent) {
        if self.composing.get() || event.is_composing() {
            return;
        }
        let key = match event.input_type().as_str() {
            "deleteContentBackward" => egui::Key::Backspace,
            "deleteContentForward" => egui::Key::Delete,
            _ => return,
        };
        let modifiers = egui::Modifiers::NONE;
        for pressed in [true, false] {
            self.sink.egui(egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
        }
    }

    fn input(&self, event: &InputEvent) {
        let kind = event.input_type();
        let composing = self.composing.get() || event.is_composing();
        if !composing
            && matches!(kind.as_str(), "insertText" | "insertReplacementText")
            && let Some(text) = event.data().filter(|t| !t.is_empty() && !t.chars().all(char::is_control))
        {
            let duplicate = self.just_committed.borrow_mut().take().is_some_and(|c| c == text);
            if !duplicate {
                self.sink.egui(egui::Event::Text(text.clone()));
                self.text_input(TextInput::Commit(text));
            }
        }
        // Composition text stays in the value until `compositionend`, which clears it.
        if !composing {
            self.clear_value();
        }
    }

    fn key(&self, event: &KeyboardEvent, pressed: bool) {
        // During composition the IME owns the keys (Chromium sends keyCode 229, Firefox/Safari set `isComposing`).
        if event.is_composing() || event.key_code() == 229 {
            return;
        }
        let key = event.key();
        let code = event.code();
        let modifiers = keys::event_modifiers(self.is_mac, event);
        if pressed {
            self.just_committed.borrow_mut().take();
            if keys::should_prevent_default(&keys::facts(self.is_mac, &key, event, true)) {
                event.prevent_default();
            }
        }
        if let Some(egui_event) = keys::egui_key_event(&key, &code, pressed, pressed && event.repeat(), modifiers) {
            if let egui::Event::Key { key: egui_key, .. } = &egui_event {
                let mut down = self.down.borrow_mut();
                if pressed {
                    down.insert(*egui_key);
                } else {
                    down.remove(egui_key);
                }
            }
            self.sink.egui(egui_event);
        }
        if !pressed && (key == "Control" || key == "Meta") {
            // Chromium sends no keyup for the letter of Ctrl+A, Cmd+C, ...: release what is still down.
            self.release_all_keys(modifiers);
        }
    }

    fn release_all_keys(&self, modifiers: egui::Modifiers) {
        let keys: Vec<egui::Key> = self.down.borrow_mut().drain().collect();
        for key in keys {
            self.sink.egui(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
        }
    }

    fn blur(&self, event: &FocusEvent) {
        self.release_all_keys(egui::Modifiers::NONE);
        self.clear_value();
        if self.composing.replace(false) {
            self.text_input(TextInput::Cancel);
        }
        let canvas: &web_sys::Element = self.canvas.as_ref();
        let to_canvas =
            event.related_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|el| el == *canvas);
        if !self.moving_focus.get() && !to_canvas {
            self.sink.egui(egui::Event::WindowFocused(false));
        }
    }

    fn focus_textarea(&self) {
        if !dom::is_active(&self.textarea) {
            dom::focus_without_scroll(&self.textarea);
        }
    }

    fn set(&self, caret_css: Option<egui::Rect>) {
        let Some(rect) = caret_css else {
            self.wanted.set(false);
            if dom::is_active(&self.textarea) {
                self.moving_focus.set(true);
                if let Err(e) = self.textarea.blur() {
                    log::warn!("textarea blur failed: {}", dom::js_error(&e));
                }
                // Give the key events back to the canvas (and winit).
                dom::focus_without_scroll(&self.canvas);
                self.moving_focus.set(false);
            }
            self.clear_value();
            return;
        };
        // Position first: the browser anchors the IME candidate window to the focused element.
        self.place(rect);
        // Focus only when the request starts. Re-focusing on every frame would bring the soft keyboard
        // back after the user dismissed it.
        if !self.wanted.replace(true) {
            self.focus_textarea();
        }
    }

    fn place(&self, rect: egui::Rect) {
        if self.last_rect.get() == Some(rect) {
            return;
        }
        self.last_rect.set(Some(rect));
        let bounds = self.canvas.get_bounding_client_rect();
        // Keep the textarea inside the canvas: focusing an element outside scrolls the page.
        let x = f64::from(rect.min.x).clamp(0.0, bounds.width().max(0.0));
        let y = f64::from(rect.min.y).clamp(0.0, bounds.height().max(0.0));
        let height = f64::from(rect.height()).clamp(1.0, 256.0);
        let style = self.textarea.style();
        let set = |name: &str, value: String| {
            if let Err(e) = style.set_property(name, &value) {
                log::warn!("textarea style {name}: {}", dom::js_error(&e));
            }
        };
        set("left", format!("{}px", bounds.left() + x));
        set("top", format!("{}px", bounds.top() + y));
        set("height", format!("{height}px"));
    }

    /// Called on pointer release (a user gesture): iOS only raises the keyboard for a focus call made
    /// inside a gesture handler, and a tap on the canvas moved the focus away from the textarea.
    pub(super) fn refocus_if_wanted(&self) {
        if self.wanted.get() {
            self.focus_textarea();
        }
    }

    /// Pointer down: keep the textarea focused while the app wants text input, else give the canvas focus.
    pub(super) fn focus_for_pointer_down(&self) {
        if self.wanted.get() {
            self.focus_textarea();
        } else {
            dom::focus_without_scroll(&self.canvas);
        }
    }
}
