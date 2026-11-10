//! Keyboard handling for the web host: which keys the browser keeps, and the translation of DOM
//! keyboard events from the hidden textarea into `egui::Event::Key`.
//!
//! winit only listens on the canvas. While the textarea has focus the canvas gets no key events, so the
//! textarea forwards them here. The canvas also gets a `preventDefault` policy, because the shell turns
//! winit's own `preventDefault` off (it would block the browser `copy`, `cut` and `paste` events).

use crate::web::dom;

/// The facts of a keydown event that decide `preventDefault`.
pub(super) struct KeyFacts<'a> {
    pub key: &'a str,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub alt_graph: bool,
    pub is_mac: bool,
    /// The event comes from the hidden textarea. Typing keys must reach it.
    pub from_text_agent: bool,
}

/// Should the app stop the browser from acting on this keydown?
///
/// The browser keeps: function keys (F5 reload, F11, F12 devtools), `Ctrl/Cmd` + C, V, X (they raise the
/// `copy`, `paste` and `cut` events), R (reload), the tab and window keys (T, N, W, Q, L), tab switching
/// with digits, and the devtools chords (`Ctrl+Shift+C/I/J`, `Cmd+Alt+I/J/C`). Every other `Ctrl/Cmd`
/// chord belongs to the app (Save, Open, Print, Select all, Find, bookmarks and zoom keys all lose their
/// browser meaning). Plain Tab, Backspace, Delete, arrows, paging and Space would move focus, go back or
/// scroll the page, so they are prevented too.
pub(super) fn should_prevent_default(k: &KeyFacts<'_>) -> bool {
    // AltGr (Ctrl+Alt on Windows) types characters on many layouts: never block those.
    if k.alt_graph || (!k.is_mac && k.ctrl && k.alt && !k.meta) {
        return false;
    }
    // Keys the IME or a soft keyboard sends while composing.
    if matches!(k.key, "Process" | "Unidentified" | "Dead" | "Compose") {
        return false;
    }
    if is_function_key(k.key) {
        return false;
    }
    let command = if k.is_mac { k.meta } else { k.ctrl };
    if command || (k.ctrl && k.meta) {
        let lower = k.key.to_lowercase();
        let letter = lower.as_str();
        if matches!(letter, "c" | "v" | "x") {
            // Cut, copy and paste raise their own events only when the key press is not cancelled.
            return false;
        }
        if matches!(letter, "r" | "t" | "n" | "w" | "q" | "l") {
            return false;
        }
        if k.key == "Tab" || letter.len() == 1 && letter.chars().all(|c| c.is_ascii_digit()) {
            // Switch tab (Ctrl+Tab, Ctrl/Cmd+digit). Most browsers do not let a page cancel these anyway.
            return false;
        }
        let devtools_letter = matches!(letter, "c" | "i" | "j");
        if devtools_letter && ((k.ctrl && k.shift) || (k.meta && k.alt)) {
            return false;
        }
        return true;
    }
    if k.meta && !k.is_mac {
        // The Windows key chords belong to the system.
        return false;
    }
    match k.key {
        "Tab" | "Backspace" | "Delete" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "PageUp"
        | "PageDown" | "Home" | "End" => true,
        // The textarea needs a space to type one.
        " " | "Spacebar" => !k.from_text_agent,
        // The textarea would insert a line break into its (always empty) value.
        "Enter" => k.from_text_agent,
        _ => false,
    }
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// The egui key events for a key press or release that came through the textarea.
/// `key` is the DOM `key`, `code` the DOM `code`.
pub(super) fn egui_key_event(
    key: &str,
    code: &str,
    pressed: bool,
    repeat: bool,
    modifiers: egui::Modifiers,
) -> Option<egui::Event> {
    let logical = egui::Key::from_name(key);
    let physical = egui::Key::from_name(code);
    // Modifier keys and non-Latin layouts have no logical egui key: fall back to the physical one.
    let active = logical.or(physical)?;
    Some(egui::Event::Key { key: active, physical_key: physical, pressed, repeat, modifiers })
}

/// Modifiers for a keyboard event, using the platform rule for `command`.
pub(super) fn event_modifiers(is_mac: bool, event: &web_sys::KeyboardEvent) -> egui::Modifiers {
    dom::modifiers_from_key(is_mac, event)
}

/// Facts of a DOM keydown for [`should_prevent_default`].
pub(super) fn facts<'a>(
    is_mac: bool,
    key: &'a str,
    event: &web_sys::KeyboardEvent,
    from_text_agent: bool,
) -> KeyFacts<'a> {
    KeyFacts {
        key,
        ctrl: event.ctrl_key(),
        alt: event.alt_key(),
        shift: event.shift_key(),
        meta: event.meta_key(),
        alt_graph: event.get_modifier_state("AltGraph"),
        is_mac,
        from_text_agent,
    }
}
