//! Composition: the text an input method shows before it is final.
//!
//! The text is not in the buffer until the input method commits it. While it runs, the editor shows it at the
//! caret (underlined) and gives it to the layout like any other text.

use std::ops::Range;

/// The preedit of an input method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Composition {
    /// The preedit text.
    pub(super) text: String,
    /// The input method's own cursor inside `text`, in bytes. `None` puts the caret after the text.
    pub(super) cursor: Option<Range<usize>>,
}

impl Composition {
    /// A preedit; a cursor range that does not lie on character boundaries of `text` is dropped.
    pub(super) fn new(text: String, cursor: Option<(usize, usize)>) -> Self {
        let cursor = cursor
            .map(|(a, b)| a.min(b)..a.max(b))
            .filter(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && r.end <= text.len());
        Self { text, cursor }
    }

    /// Where the caret goes inside the shown text when the preedit starts at `at`.
    pub(super) fn caret(&self, at: usize) -> usize {
        at + self.cursor.as_ref().map_or(self.text.len(), |c| c.end)
    }
}
