//! Highlight in the text being edited.
//!
//! The editor keeps what the person chose as the highlight of a run of their own ([`SpanHighlight`]), without
//! knowing the highlight of the whole element: a choice is `On(colour)` or `Off`. Writing the result back
//! (`text_bind::replace_runs`) turns a choice that only repeats the element's own highlight into "no value", so the
//! stored runs hold only what differs.

use drafftink_scene::{CharStyle, MarkerColor, SpanHighlight};

use super::Editor;
use super::history::Kind;

impl Editor {
    /// The highlight the selected characters show, given the highlight of the whole text: `Some(colour)` when they
    /// all show that one (`Some(None)`: none), and `None` when they differ. With a caret: what the next typed
    /// character gets.
    pub fn highlight_state(&self, element: Option<MarkerColor>) -> Option<Option<MarkerColor>> {
        let shown = |style: CharStyle| style.highlight.resolve(element);
        let Some(selected) = self.selected_text() else {
            return Some(shown(self.typing_style(self.focus)));
        };
        let start = self.selection().start;
        let first = shown(self.runs.style_at(start));
        selected.char_indices().all(|(i, _)| shown(self.runs.style_at(start + i)) == first).then_some(first)
    }

    /// Highlights the selected characters with `color`, or with `None` takes the highlight off them; with a caret,
    /// sets what is typed next. Returns whether anything changed. Does nothing while an input method composes.
    pub fn set_highlight(&mut self, color: Option<MarkerColor>) -> bool {
        if self.composition.is_some() {
            return false;
        }
        let choice = color.map_or(SpanHighlight::Off, SpanHighlight::On);
        if self.is_collapsed() {
            let now = self.typing_style(self.focus);
            if now.highlight == choice {
                return false;
            }
            self.pending = Some(CharStyle { highlight: choice, ..now });
            self.touch();
            return true;
        }
        let mut runs = self.runs.clone();
        runs.restyle(self.selection(), |style| CharStyle { highlight: choice, ..style });
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
}
