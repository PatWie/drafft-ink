//! egui key events as the editor model's keys.

use drafftink_edit::text_edit::{Key, KeyAction, Mods, action_for};

/// A key press for the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorKey {
    key: Key,
    mods: Mods,
}

/// What the app does with a key press while a text is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRoute {
    /// Esc, or Ctrl/Cmd+Enter: the edit ends.
    Finish,
    /// The editor takes it.
    Editor(EditorKey),
}

impl EditorKey {
    /// What the model does for this key.
    pub(crate) fn action(self, mac: bool) -> Option<KeyAction> {
        action_for(self.key, self.mods, mac)
    }
}

/// Where a key press goes. `None`: not the editor's (a function key, a letter chord that does nothing here), so
/// the rest of the app may see it.
pub(crate) fn route(key: egui::Key, m: egui::Modifiers, mac: bool) -> Option<KeyRoute> {
    use egui::Key as K;
    if key == K::Escape && m.is_none() {
        return Some(KeyRoute::Finish);
    }
    if key == K::Enter && m.command {
        return Some(KeyRoute::Finish);
    }
    let named = match key {
        K::ArrowLeft => Key::Left,
        K::ArrowRight => Key::Right,
        K::ArrowUp => Key::Up,
        K::ArrowDown => Key::Down,
        K::Home => Key::Home,
        K::End => Key::End,
        K::Backspace => Key::Backspace,
        K::Delete => Key::Delete,
        K::Enter => Key::Enter,
        K::F2 => Key::F2,
        K::A => Key::Letter('a'),
        K::B => Key::Letter('b'),
        K::I => Key::Letter('i'),
        K::Y => Key::Letter('y'),
        K::Z => Key::Letter('z'),
        K::M => Key::Letter('m'),
        _ => return None,
    };
    let mods = Mods { shift: m.shift, alt: m.alt, command: m.command };
    let key = EditorKey { key: named, mods };
    key.action(mac).map(|_| KeyRoute::Editor(key))
}

/// What the editor took from the key events of one frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Taken {
    /// Esc or Mod+Enter was pressed: the edit ends after the keys are applied.
    pub finish: bool,
    /// The editing keys, in the order they were pressed.
    pub keys: Vec<EditorKey>,
}

/// Takes the editor's key presses out of a frame's events and leaves the rest to the app.
///
/// A frame can hold the whole of what a person typed and the key that ends the edit (a slow frame, a fast typist).
/// The letters before that key belong to the text: with the edit over, the app's keymap must not read them as tool
/// shortcuts, so they go too. Keys after it are the app's, and so is every chord with Ctrl, Cmd or Alt.
pub(crate) fn take_keys(events: &mut Vec<egui::Event>, mac: bool) -> Taken {
    let mut taken = Taken::default();
    let mut kept = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = &event else {
            kept.push(event);
            continue;
        };
        match route(*key, *modifiers, mac) {
            Some(KeyRoute::Finish) => {
                taken.finish = true;
                kept.retain(|e| !is_typing(e));
            }
            Some(KeyRoute::Editor(k)) => taken.keys.push(k),
            None => kept.push(event),
        }
    }
    *events = kept;
    taken
}

/// A key press or release that types a character: no Ctrl, Cmd or Alt held.
fn is_typing(event: &egui::Event) -> bool {
    matches!(event, egui::Event::Key { modifiers, .. } if !modifiers.command && !modifiers.alt && !modifiers.ctrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> egui::Modifiers {
        egui::Modifiers::COMMAND
    }

    #[test]
    fn esc_and_mod_enter_end_the_edit() {
        assert_eq!(route(egui::Key::Escape, egui::Modifiers::NONE, false), Some(KeyRoute::Finish));
        assert_eq!(route(egui::Key::Enter, cmd(), false), Some(KeyRoute::Finish));
    }

    #[test]
    fn editing_keys_go_to_the_editor_and_other_chords_pass() {
        assert!(matches!(route(egui::Key::ArrowLeft, egui::Modifiers::NONE, false), Some(KeyRoute::Editor(_))));
        assert!(matches!(route(egui::Key::Z, cmd(), false), Some(KeyRoute::Editor(_))));
        assert!(matches!(route(egui::Key::B, cmd(), false), Some(KeyRoute::Editor(_))), "Mod+B is bold in a text");
        assert!(matches!(route(egui::Key::I, cmd(), true), Some(KeyRoute::Editor(_))), "Mod+I is italic in a text");
        assert_eq!(route(egui::Key::B, egui::Modifiers::NONE, false), None, "a plain b is typed text");
        assert_eq!(route(egui::Key::K, cmd(), false), None, "other chords do nothing in a text");
        assert_eq!(route(egui::Key::S, cmd(), false), None, "Mod+S is the app's");
        assert_eq!(route(egui::Key::Z, egui::Modifiers::NONE, false), None, "a plain letter is typed text");
    }

    fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn letters_typed_before_the_end_of_the_edit_in_one_frame_are_not_shortcuts() {
        // "hello" and Ctrl+Enter in one frame: a slow frame batches them. Only the editor's keys and the typing
        // before the end are taken; what comes after the end, and a chord, stays for the keymap.
        let mut events = vec![
            press(egui::Key::H, egui::Modifiers::NONE),
            press(egui::Key::E, egui::Modifiers::NONE),
            press(egui::Key::S, cmd()),
            press(egui::Key::Enter, cmd()),
            press(egui::Key::T, egui::Modifiers::NONE),
            egui::Event::Text("x".to_owned()),
        ];
        let taken = take_keys(&mut events, false);
        assert!(taken.finish);
        assert!(taken.keys.is_empty());
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(&events[0], egui::Event::Key { key: egui::Key::S, .. }), "Mod+S is the app's");
        assert!(matches!(&events[1], egui::Event::Key { key: egui::Key::T, .. }), "a key after the end is the app's");
        assert!(matches!(&events[2], egui::Event::Text(_)));
    }

    #[test]
    fn without_the_end_of_the_edit_the_typed_letters_stay_for_the_frame_to_ignore() {
        let mut events =
            vec![press(egui::Key::H, egui::Modifiers::NONE), press(egui::Key::ArrowLeft, egui::Modifiers::NONE)];
        let taken = take_keys(&mut events, false);
        assert!(!taken.finish);
        assert_eq!(taken.keys.len(), 1);
        assert_eq!(events.len(), 1, "the editing key is taken, the letter is not");
    }
}
