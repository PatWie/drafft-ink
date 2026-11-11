//! Bold and italic in the text being edited.

use drafftink_scene::CharStyle;

use super::Editor;
use super::history::Kind;

/// A face a stretch of text can have on top of the element's own font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// The heavy face.
    Bold,
    /// The italic face.
    Italic,
}

impl Face {
    /// Whether `style` has this face.
    pub fn of(self, style: CharStyle) -> bool {
        match self {
            Face::Bold => style.bold,
            Face::Italic => style.italic,
        }
    }

    /// `style` with this face switched on or off.
    pub fn set(self, style: CharStyle, on: bool) -> CharStyle {
        match self {
            Face::Bold => CharStyle { bold: on, ..style },
            Face::Italic => CharStyle { italic: on, ..style },
        }
    }
}

impl Editor {
    /// Whether `face` is on for the whole selection (`Some(true)`), off for all of it (`Some(false)`), or mixed
    /// (`None`). With a caret: whether the next typed character has it.
    pub fn face_state(&self, face: Face) -> Option<bool> {
        let Some(selected) = self.selected_text() else {
            return Some(face.of(self.typing_style(self.focus)));
        };
        let start = self.selection().start;
        let first = face.of(self.runs.style_at(start));
        selected.char_indices().all(|(i, _)| face.of(self.runs.style_at(start + i)) == first).then_some(first)
    }

    /// Switches `face` on or off for the selected characters; with a caret, for what is typed next. Returns whether
    /// anything changed. Does nothing while an input method composes.
    pub fn set_face(&mut self, face: Face, on: bool) -> bool {
        if self.composition.is_some() {
            return false;
        }
        if self.is_collapsed() {
            let now = self.typing_style(self.focus);
            if face.of(now) == on {
                return false;
            }
            self.pending = Some(face.set(now, on));
            self.touch();
            return true;
        }
        let mut runs = self.runs.clone();
        runs.restyle(self.selection(), |style| face.set(style, on));
        if runs == self.runs {
            return false;
        }
        let before = self.snapshot();
        self.history.record(before, Kind::Other, false);
        self.runs = runs;
        self.history.break_run();
        self.touch();
        true
    }

    /// `Mod+B` and `Mod+I`: the face goes off when all of the selection has it, and on otherwise.
    pub fn toggle_face(&mut self, face: Face) -> bool {
        let on = self.face_state(face) != Some(true);
        self.set_face(face, on)
    }
}
