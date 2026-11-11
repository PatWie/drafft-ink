//! Which keys do what inside the text editor (docs/commands.md section 6.2).
//!
//! The shells translate their key events into [`Key`] and [`Mods`]; this module says what each chord means, so the
//! table is the same on every platform and can be tested without a window.

use drafftink_typeset::Motion;

use super::Face;

/// A key the editor reacts to. Letters are given as characters in lower case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Enter or Return.
    Enter,
    /// F2: show the source of the formula at the caret.
    F2,
    /// A letter key.
    Letter(char),
}

/// Modifier keys held with a [`Key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    /// Shift.
    pub shift: bool,
    /// Alt (Option on a Mac).
    pub alt: bool,
    /// Ctrl, or Cmd on a Mac: the key the platform uses for shortcuts.
    pub command: bool,
}

/// What a chord asks the editor to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Insert a line break.
    Newline,
    /// Move the caret, or the selection's end when `extend`.
    Move {
        /// Where to.
        motion: Motion,
        /// Shift was held.
        extend: bool,
    },
    /// Delete backwards (a word with `word`) or the selection.
    Backspace {
        /// By word.
        word: bool,
    },
    /// Delete forwards (a word with `word`) or the selection.
    Delete {
        /// By word.
        word: bool,
    },
    /// Select everything.
    SelectAll,
    /// The editor's undo.
    Undo,
    /// The editor's redo.
    Redo,
    /// Make the selection bold or italic, or take it away when all of it is (with a caret: the style of what is
    /// typed next).
    Toggle(Face),
    /// Show the source of the formula at the caret.
    RevealFormula,
    /// Turn typesetting of `$...$` on or off for this text.
    ToggleFormulas,
}

/// What `key` with `mods` does. `mac` is true on macOS, where Cmd moves by line and Alt by word.
///
/// Copy, cut and paste are not here: the shell delivers them as clipboard events. `Esc` and `Mod+Enter` end the
/// edit and are the app's. Every other `Mod` chord does nothing, as in the old app.
pub fn action_for(key: Key, mods: Mods, mac: bool) -> Option<KeyAction> {
    let Mods { shift, alt, command } = mods;
    // Moving by word: Alt on a Mac, Ctrl elsewhere (and Alt too). Cmd on a Mac goes to the ends of the line.
    let word = alt || (command && !mac);
    let to_edge = command && mac;
    let mv = |motion| Some(KeyAction::Move { motion, extend: shift });
    match key {
        Key::Left if to_edge => mv(Motion::LineStart),
        Key::Right if to_edge => mv(Motion::LineEnd),
        Key::Left if word => mv(Motion::WordLeft),
        Key::Right if word => mv(Motion::WordRight),
        Key::Left => mv(Motion::Left),
        Key::Right => mv(Motion::Right),
        Key::Up if to_edge => mv(Motion::TextStart),
        Key::Down if to_edge => mv(Motion::TextEnd),
        Key::Up if word => mv(Motion::ParagraphStart),
        Key::Down if word => mv(Motion::ParagraphEnd),
        Key::Up => mv(Motion::Up),
        Key::Down => mv(Motion::Down),
        Key::Home if command => mv(Motion::TextStart),
        Key::End if command => mv(Motion::TextEnd),
        Key::Home => mv(Motion::LineStart),
        Key::End => mv(Motion::LineEnd),
        Key::Backspace => Some(KeyAction::Backspace { word: word || to_edge }),
        Key::Delete => Some(KeyAction::Delete { word: word || to_edge }),
        Key::Enter if !command => Some(KeyAction::Newline),
        Key::F2 if !command => Some(KeyAction::RevealFormula),
        Key::Letter('a') if command && !shift && !alt => Some(KeyAction::SelectAll),
        Key::Letter('z') if command && !alt => Some(if shift { KeyAction::Redo } else { KeyAction::Undo }),
        Key::Letter('y') if command && !shift && !alt => Some(KeyAction::Redo),
        Key::Letter('b') if command && !shift && !alt => Some(KeyAction::Toggle(Face::Bold)),
        Key::Letter('i') if command && !shift && !alt => Some(KeyAction::Toggle(Face::Italic)),
        Key::Letter('m') if command && shift && !alt => Some(KeyAction::ToggleFormulas),
        Key::Enter | Key::F2 | Key::Letter(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, command: false };

    #[test]
    fn plain_and_shifted_arrows_move_and_extend() {
        assert_eq!(action_for(Key::Left, NONE, false), Some(KeyAction::Move { motion: Motion::Left, extend: false }));
        let shift = Mods { shift: true, ..NONE };
        assert_eq!(action_for(Key::Down, shift, false), Some(KeyAction::Move { motion: Motion::Down, extend: true }));
    }

    #[test]
    fn words_are_alt_on_a_mac_and_ctrl_elsewhere() {
        let ctrl = Mods { command: true, ..NONE };
        let alt = Mods { alt: true, ..NONE };
        let word = Some(KeyAction::Move { motion: Motion::WordLeft, extend: false });
        assert_eq!(action_for(Key::Left, ctrl, false), word);
        assert_eq!(action_for(Key::Left, alt, true), word);
        assert_eq!(
            action_for(Key::Left, ctrl, true),
            Some(KeyAction::Move { motion: Motion::LineStart, extend: false })
        );
        assert_eq!(action_for(Key::Backspace, ctrl, false), Some(KeyAction::Backspace { word: true }));
        assert_eq!(action_for(Key::Delete, NONE, false), Some(KeyAction::Delete { word: false }));
    }

    #[test]
    fn home_end_and_the_ends_of_the_text() {
        let ctrl = Mods { command: true, ..NONE };
        assert_eq!(
            action_for(Key::Home, NONE, false),
            Some(KeyAction::Move { motion: Motion::LineStart, extend: false })
        );
        assert_eq!(action_for(Key::End, ctrl, false), Some(KeyAction::Move { motion: Motion::TextEnd, extend: false }));
    }

    #[test]
    fn editing_chords() {
        let ctrl = Mods { command: true, ..NONE };
        let ctrl_shift = Mods { command: true, shift: true, alt: false };
        assert_eq!(action_for(Key::Letter('a'), ctrl, false), Some(KeyAction::SelectAll));
        assert_eq!(action_for(Key::Letter('z'), ctrl, false), Some(KeyAction::Undo));
        assert_eq!(action_for(Key::Letter('z'), ctrl_shift, false), Some(KeyAction::Redo));
        assert_eq!(action_for(Key::Letter('y'), ctrl, false), Some(KeyAction::Redo));
        assert_eq!(action_for(Key::Letter('m'), ctrl_shift, false), Some(KeyAction::ToggleFormulas));
        assert_eq!(action_for(Key::Letter('b'), ctrl, false), Some(KeyAction::Toggle(Face::Bold)));
        assert_eq!(action_for(Key::Letter('i'), ctrl, true), Some(KeyAction::Toggle(Face::Italic)));
        assert_eq!(action_for(Key::Enter, NONE, false), Some(KeyAction::Newline));
        assert_eq!(action_for(Key::F2, NONE, false), Some(KeyAction::RevealFormula));
    }

    #[test]
    fn other_command_chords_do_nothing() {
        let ctrl = Mods { command: true, ..NONE };
        assert_eq!(action_for(Key::Letter('c'), ctrl, false), None);
        assert_eq!(action_for(Key::Letter('b'), Mods { shift: true, ..ctrl }, false), None);
        assert_eq!(action_for(Key::Letter('i'), Mods { alt: true, ..ctrl }, false), None);
        assert_eq!(action_for(Key::Letter('b'), NONE, false), None, "a plain b is typed text");
        assert_eq!(action_for(Key::Enter, ctrl, false), None);
        assert_eq!(action_for(Key::Letter('a'), NONE, false), None);
    }
}
