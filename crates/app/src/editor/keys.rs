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
}
