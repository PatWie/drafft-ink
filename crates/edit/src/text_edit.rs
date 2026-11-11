//! The model of the on-canvas text editor: the text, its colours, the selection, motions, the editor's own undo
//! and the composition of an input method (docs/exceptions.md E16).
//!
//! The editor owns the content while the person types; the document hears about it once, through
//! `text.edit_end`. Nothing here knows about windows, key codes or egui. A shell translates its events into
//! [`Key`], [`Mods`], text, clipboard strings and pointer positions, lays the text out with the typesetter
//! (so the editor looks exactly like the committed text), and hands the [`TextLayout`] back for every call that
//! needs geometry: motions, clicks, deleting by cluster.
//!
//! * Indexes are bytes of the text as written (formulas as `$...$`), the typesetter's source indexes.
//! * A formula is one unit for the caret. The editor shows the source of the formula the caret is inside, and the
//!   typeset formula otherwise ([`Shown::reveal`]).
//! * Typed text takes the colour of the character before it. [`Editor::set_color`] colours the selection.
//! * Composition text is shown at the caret and joins the text when the input method commits.

mod clicks;
mod history;
mod ime;
mod keys;
mod runs;

use std::ops::Range;

use drafftink_geom::kurbo::Point;
use drafftink_scene::limits::MAX_TEXT_CHARS;
use drafftink_scene::{Color, InlineMath, TextRun};
use drafftink_typeset::{Motion, TextLayout, TextSelection, formula_ranges};

pub use clicks::Clicks;
pub use keys::{Key, KeyAction, Mods, action_for};
pub use runs::{ColorRuns, ColorSpan};

use history::{History, Kind, Snapshot};
use ime::Composition;

/// What to lay out and draw: the text with the composition in it, and where the caret is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The text, with the composition inserted at the caret.
    pub text: String,
    /// The colour of its characters.
    pub runs: ColorRuns,
    /// The selection in `text`, anchor first. Equal ends are a caret. While a composition runs it is the caret.
    pub selection: (usize, usize),
    /// The bytes of `text` that belong to the composition (drawn underlined).
    pub composition: Option<Range<usize>>,
    /// `TextSpec::reveal` for the layout: the caret is inside a formula, so its source is shown.
    pub reveal: Option<usize>,
}

impl Shown {
    /// The colours as the scene stores them (characters).
    pub fn text_runs(&self) -> Vec<TextRun> {
        self.runs.to_text_runs(&self.text)
    }
}

/// The result of an edit session: what `text.edit_end` receives.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    /// The final text.
    pub text: String,
    /// The final colour runs, in characters.
    pub runs: Vec<TextRun>,
    /// Whether `$...$` is typeset.
    pub inline_math: InlineMath,
}

/// The editing state of one text.
#[derive(Debug, Clone)]
pub struct Editor {
    text: String,
    runs: ColorRuns,
    anchor: usize,
    focus: usize,
    math: InlineMath,
    composition: Option<Composition>,
    history: History,
    /// The selection as the typesetter built it, for the layout with this id (keeps the column of Up and Down).
    cached: Option<(u64, TextSelection)>,
    dragging: bool,
    revision: u64,
}

impl Editor {
    /// An editor over `text` with its colour `runs` (characters). The caret is at the end.
    pub fn new(text: &str, runs: &[TextRun], math: InlineMath) -> Self {
        let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
        let end = text.len();
        Self {
            runs: ColorRuns::from_text_runs(&text, runs),
            text,
            anchor: end,
            focus: end,
            math,
            composition: None,
            history: History::default(),
            cached: None,
            dragging: false,
            revision: 0,
        }
    }

    /// Changes whenever anything the person can see changed: text, colours, selection, composition.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The text without the composition.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The colours of the text without the composition.
    pub fn runs(&self) -> &ColorRuns {
        &self.runs
    }

    /// Whether `$...$` is typeset.
    pub fn inline_math(&self) -> InlineMath {
        self.math
    }

    /// Whether an input method is composing.
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }

    /// The selected bytes, start before end.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    /// The selection as `(anchor, focus)`.
    pub fn anchor_focus(&self) -> (usize, usize) {
        (self.anchor, self.focus)
    }

    /// Whether the selection is a caret.
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }

    /// The selected text.
    pub fn selected_text(&self) -> Option<&str> {
        self.text.get(self.selection()).filter(|s| !s.is_empty())
    }

    /// Steps the editor's undo can go back.
    pub fn undo_steps(&self) -> usize {
        self.history.undo_len()
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), runs: self.runs.clone(), anchor: self.anchor, focus: self.focus }
    }

    fn restore(&mut self, s: Snapshot) {
        self.text = s.text;
        self.runs = s.runs;
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.cached = None;
        self.composition = None;
        self.touch();
    }

    fn boundary_before(&self, mut i: usize) -> usize {
        i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    // ---- what to draw ------------------------------------------------------------------------------------

    /// The text to lay out and the caret in it.
    pub fn shown(&self) -> Shown {
        let Some(c) = self.composition.as_ref() else {
            let reveal = self.reveal_for(&self.text, self.focus);
            return Shown {
                text: self.text.clone(),
                runs: self.runs.clone(),
                selection: (self.anchor, self.focus),
                composition: None,
                reveal,
            };
        };
        let at = self.focus.min(self.text.len());
        let mut text = String::with_capacity(self.text.len() + c.text.len());
        text.push_str(self.text.get(..at).unwrap_or_default());
        text.push_str(&c.text);
        text.push_str(self.text.get(at..).unwrap_or_default());
        let mut runs = self.runs.clone();
        runs.replace(at..at, c.text.len(), self.color_before(at));
        let caret = c.caret(at);
        let reveal = self.reveal_for(&text, caret);
        Shown { text, runs, selection: (caret, caret), composition: Some(at..at + c.text.len()), reveal }
    }

    /// The `reveal` of the layout when the caret is at `caret` in `text`: strictly inside a formula.
    fn reveal_for(&self, text: &str, caret: usize) -> Option<usize> {
        if self.math != InlineMath::Auto {
            return None;
        }
        formula_ranges(text).into_iter().find(|f| f.start < caret && caret < f.end).map(|f| f.start + 1)
    }

    /// The selection on `layout`, which was made from [`Self::shown`].
    pub fn typeset_selection(&mut self, layout: &TextLayout) -> TextSelection {
        if let Some((id, sel)) = &self.cached
            && *id == layout.id
        {
            return sel.clone();
        }
        let (a, f) = self.composition.as_ref().map_or((self.anchor, self.focus), |c| {
            let caret = c.caret(self.focus);
            (caret, caret)
        });
        let sel = layout.selection_from_range(a, f);
        if self.composition.is_none() {
            self.cached = Some((layout.id, sel.clone()));
        }
        sel
    }

    // ---- editing -----------------------------------------------------------------------------------------

    /// The colour a character typed at `at` gets: that of the character before it.
    fn color_before(&self, at: usize) -> Option<Color> {
        at.checked_sub(1).and_then(|i| self.runs.color_at(i))
    }

    /// Replaces `range` by `with`. The new text takes the colour of the first replaced character, or else of the one
    /// before the range. The caret ends after it.
    fn replace(&mut self, range: Range<usize>, with: &str, kind: Kind) {
        let joins = !with.chars().any(char::is_whitespace);
        let before = self.snapshot();
        self.history.record(before, kind, joins);
        let color = if range.is_empty() { self.color_before(range.start) } else { self.runs.color_at(range.start) };
        self.text.replace_range(range.clone(), with);
        self.runs.replace(range.clone(), with.len(), color);
        self.anchor = range.start + with.len();
        self.focus = self.anchor;
        self.cached = None;
        self.history.settle(self.focus);
        self.touch();
    }

    /// Inserts typed or pasted text in place of the selection.
    pub fn insert(&mut self, text: &str) -> bool {
        let clean = clean_input(text);
        let selected = self.selected_text().map_or(0, |s| s.chars().count());
        let room = MAX_TEXT_CHARS.saturating_sub(self.text.chars().count().saturating_sub(selected));
        let clean: String = clean.chars().take(room).collect();
        if clean.is_empty() {
            return false;
        }
        self.composition = None;
        let range = self.selection();
        self.replace(range, &clean, Kind::Typing);
        true
    }

    /// Deletes the selection. Returns false when it is a caret.
    fn delete_selection(&mut self, kind: Kind) -> bool {
        if self.is_collapsed() {
            return false;
        }
        let range = self.selection();
        self.replace(range, "", kind);
        true
    }

    /// Backspace or Delete: the selection, or else one cluster (or word) in front of or behind the caret in the
    /// logical order of the text. Right-to-left text moves the caret the other way on screen, and the deletion
    /// follows the text, not the screen.
    fn delete_step(&mut self, layout: &TextLayout, forward: bool, word: bool) -> bool {
        if !self.is_collapsed() {
            return self.delete_selection(Kind::Deleting);
        }
        let (left, right) = if word { (Motion::WordLeft, Motion::WordRight) } else { (Motion::Left, Motion::Right) };
        let sel = self.typeset_selection(layout).collapse();
        let to = |m| self.boundary_before(layout.move_selection(&sel, m, false).focus().index);
        let (a, b) = (to(left), to(right));
        let caret = self.focus;
        let target = if forward {
            [a, b].into_iter().filter(|&i| i > caret).min()
        } else {
            [a, b].into_iter().filter(|&i| i < caret).max()
        };
        let Some(target) = target else { return false };
        self.replace(caret.min(target)..caret.max(target), "", Kind::Deleting);
        true
    }

    /// Takes the selection of the typesetter as ours.
    fn adopt(&mut self, layout: &TextLayout, sel: TextSelection) {
        self.anchor = self.boundary_before(sel.anchor().index);
        self.focus = self.boundary_before(sel.focus().index);
        self.cached = Some((layout.id, sel));
        self.history.break_run();
        self.touch();
    }

    /// Runs the action of a key. Returns whether anything the person can see changed.
    pub fn apply(&mut self, action: KeyAction, layout: &TextLayout) -> bool {
        if self.composition.is_some() {
            // The input method owns the keys while it composes.
            return false;
        }
        match action {
            KeyAction::Newline => self.insert("\n"),
            KeyAction::Move { motion, extend } => {
                let sel = self.typeset_selection(layout);
                let moved = layout.move_selection(&sel, motion, extend);
                let before = (self.anchor, self.focus);
                self.adopt(layout, moved);
                before != (self.anchor, self.focus)
            }
            KeyAction::Backspace { word } => self.delete_step(layout, false, word),
            KeyAction::Delete { word } => self.delete_step(layout, true, word),
            KeyAction::SelectAll => {
                self.select_all();
                true
            }
            KeyAction::Undo => self.undo(),
            KeyAction::Redo => self.redo(),
            KeyAction::RevealFormula => self.reveal_formula(layout),
            KeyAction::ToggleFormulas => {
                self.math = match self.math {
                    InlineMath::Auto => InlineMath::Off,
                    InlineMath::Off => InlineMath::Auto,
                };
                self.touch();
                true
            }
        }
    }

    /// Selects all the text.
    pub fn select_all(&mut self) {
        self.composition = None;
        self.anchor = 0;
        self.focus = self.text.len();
        self.cached = None;
        self.history.break_run();
        self.touch();
    }

    /// Selects the characters `chars` (clamped).
    pub fn select_chars(&mut self, chars: Range<usize>) {
        let byte = |c: usize| self.text.char_indices().nth(c).map_or(self.text.len(), |(i, _)| i);
        let (a, b) = (byte(chars.start), byte(chars.end));
        self.anchor = a;
        self.focus = b;
        self.cached = None;
        self.history.break_run();
        self.touch();
    }

    /// Shows the source of the formula at the caret and puts the caret in it.
    fn reveal_formula(&mut self, layout: &TextLayout) -> bool {
        let Some(at) = layout.formula_at_index(self.focus).map(|f| layout.reveal_index(f)) else { return false };
        self.anchor = at;
        self.focus = at;
        self.cached = None;
        self.history.break_run();
        self.touch();
        true
    }

    // ---- colour ------------------------------------------------------------------------------------------

    /// Gives the selected characters `color` (`None`: the colour of the element). Returns false, and does nothing,
    /// when no character is selected.
    pub fn set_color(&mut self, color: Option<Color>) -> bool {
        if self.is_collapsed() || self.composition.is_some() {
            return false;
        }
        let range = self.selection();
        let mut runs = self.runs.clone();
        runs.paint(range, color);
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

    /// The colour of the selected characters when they all have the same: `Some(None)` is the element's colour.
    pub fn selection_color(&self) -> Option<Option<Color>> {
        let selected = self.selected_text()?;
        let start = self.selection().start;
        let first = self.runs.color_at(start);
        selected.char_indices().all(|(i, _)| self.runs.color_at(start + i) == first).then_some(first)
    }

    // ---- clipboard ---------------------------------------------------------------------------------------

    /// The text to copy.
    pub fn copy(&self) -> Option<String> {
        self.selected_text().map(str::to_owned)
    }

    /// Removes the selection and returns it for the clipboard.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.composition = None;
        self.delete_selection(Kind::Other).then_some(text)
    }

    /// Pastes `text` in place of the selection.
    pub fn paste(&mut self, text: &str) -> bool {
        let clean = clean_input(text);
        if clean.is_empty() {
            return false;
        }
        // A paste is a step of its own, however it looks.
        self.history.break_run();
        let changed = self.insert(&clean);
        self.history.break_run();
        changed
    }

    // ---- undo --------------------------------------------------------------------------------------------

    /// The editor's own undo.
    pub fn undo(&mut self) -> bool {
        let now = self.snapshot();
        match self.history.undo(now) {
            Some(back) => {
                self.restore(back);
                true
            }
            None => false,
        }
    }

    /// The editor's own redo.
    pub fn redo(&mut self) -> bool {
        let now = self.snapshot();
        match self.history.redo(now) {
            Some(forward) => {
                self.restore(forward);
                true
            }
            None => false,
        }
    }

    // ---- composition -------------------------------------------------------------------------------------

    /// The input method shows `text` (empty: it gave up). `cursor` is its own cursor, in bytes of `text`.
    pub fn preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        let text: String = clean_input(text);
        if text.is_empty() {
            self.cancel_composition();
            return;
        }
        if self.composition.is_none() {
            // The composition replaces the selection from the start, as in every text field.
            self.delete_selection(Kind::Other);
        }
        self.composition = Some(Composition::new(text, cursor));
        self.touch();
    }

    /// The input method commits `text`: it takes the place of the composition (or of the selection).
    pub fn commit(&mut self, text: &str) -> bool {
        let had = self.composition.take().is_some();
        if had {
            self.touch();
        }
        self.insert(text) || had
    }

    /// The input method gave up: the composition goes away.
    pub fn cancel_composition(&mut self) {
        if self.composition.take().is_some() {
            self.touch();
        }
    }

    // ---- the mouse ---------------------------------------------------------------------------------------

    /// A press at `at` (layout space) that was the `clicks`-th of a series: 1 sets the caret (or extends the
    /// selection with `extend`), 2 selects a word or shows a formula's source, 3 selects the paragraph.
    pub fn press(&mut self, layout: &TextLayout, at: Point, clicks: u32, extend: bool) -> bool {
        if self.composition.is_some() {
            return false;
        }
        let before = (self.anchor, self.focus);
        if clicks == 2
            && let Some(at) = layout.formula_at_point(at).map(|f| layout.reveal_index(f))
        {
            // The typeset formula cannot hold a caret inside; the editor puts it there and the next layout shows
            // the source.
            self.anchor = at;
            self.focus = at;
            self.cached = None;
            self.history.break_run();
            self.dragging = false;
            self.touch();
            return before != (self.anchor, self.focus);
        }
        let sel = match clicks {
            0 | 1 if extend => layout.shift_click(&self.typeset_selection(layout), at),
            0 | 1 => layout.selection_at_point(at),
            2 => layout.word_at_point(at),
            _ => layout.paragraph_at_point(at),
        };
        self.adopt(layout, sel);
        self.dragging = true;
        before != (self.anchor, self.focus)
    }

    /// The pointer moved with the button down: the selection's end follows it.
    pub fn drag(&mut self, layout: &TextLayout, at: Point) -> bool {
        if !self.dragging || self.composition.is_some() {
            return false;
        }
        let before = (self.anchor, self.focus);
        let sel = layout.extend_to_point(&self.typeset_selection(layout), at);
        self.adopt(layout, sel);
        before != (self.anchor, self.focus)
    }

    /// The button went up.
    pub fn release(&mut self) {
        self.dragging = false;
    }

    // ---- the end -----------------------------------------------------------------------------------------

    /// What `text.edit_end` receives. A composition that is still running is not part of it.
    pub fn finish(&self) -> Finished {
        Finished { text: self.text.clone(), runs: self.runs.to_text_runs(&self.text), inline_math: self.math }
    }
}

/// Typed or pasted text as the editor takes it: line breaks are `\n`, a tab is four spaces, other control
/// characters are dropped.
fn clean_input(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' => out.push_str("    "),
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}
