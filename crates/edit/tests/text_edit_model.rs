#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The model of the on-canvas text editor: insertion with colour runs, colour ranges, composition, motions
//! (including right-to-left text and formulas), clicks, clipboard and the editor's own undo.

use std::sync::Arc;

use drafftink_edit::text_edit::{Clicks, Editor, Key, KeyAction, Mods, Shown, action_for};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Color, InlineMath, TextRun};
use drafftink_typeset::{InlineMath as Math, Motion, Rgba8, Span, SpanStyle, TextLayout, TextSpec, TextStyle, Typesetter};

const RED: Color = Color::rgb(220, 30, 30);
const BLUE: Color = Color::rgb(30, 30, 220);

fn rgba(c: Color) -> Rgba8 {
    Rgba8::new(c.r, c.g, c.b, c.a)
}

/// The layout the app would draw for what the editor shows.
fn layout(ts: &mut Typesetter, shown: &Shown) -> Arc<TextLayout> {
    let mut spec = TextSpec::styled(shown.text.as_str(), TextStyle::with_size(20.0));
    spec.spans = shown
        .runs
        .spans()
        .iter()
        .map(|s| Span { range: s.range.clone(), style: SpanStyle::color(rgba(s.color)) })
        .collect();
    spec.math = Math::Auto;
    spec.reveal = shown.reveal;
    ts.text(&spec)
}

struct Rig {
    ts: Typesetter,
    ed: Editor,
}

impl Rig {
    fn new(text: &str) -> Self {
        Self { ts: Typesetter::new(), ed: Editor::new(text, &[], InlineMath::Auto) }
    }

    fn layout(&mut self) -> Arc<TextLayout> {
        let shown = self.ed.shown();
        layout(&mut self.ts, &shown)
    }

    fn key(&mut self, key: Key, mods: Mods) -> bool {
        let action = action_for(key, mods, false).expect("a bound key");
        self.act(action)
    }

    fn act(&mut self, action: KeyAction) -> bool {
        let l = self.layout();
        self.ed.apply(action, &l)
    }

    fn go(&mut self, motion: Motion, extend: bool) {
        self.act(KeyAction::Move { motion, extend });
    }

    fn caret(&self) -> usize {
        assert!(self.ed.is_collapsed(), "expected a caret, got {:?}", self.ed.anchor_focus());
        self.ed.anchor_focus().1
    }
}

const NONE: Mods = Mods { shift: false, alt: false, command: false };
const CTRL: Mods = Mods { shift: false, alt: false, command: true };

fn spans(ed: &Editor) -> Vec<(std::ops::Range<usize>, Color)> {
    ed.runs().spans().iter().map(|s| (s.range.clone(), s.color)).collect()
}

// ---- insertion and colour --------------------------------------------------------------------------------------

#[test]
fn typing_appends_and_the_caret_follows() {
    let mut r = Rig::new("");
    assert!(r.ed.insert("héllo"));
    assert_eq!(r.ed.text(), "héllo");
    assert_eq!(r.caret(), "héllo".len());
}

#[test]
fn typed_text_takes_the_colour_of_the_character_before_it() {
    let mut r = Rig::new("ab cd");
    r.ed.select_chars(3..5);
    assert!(r.ed.set_color(Some(RED)));
    r.ed.select_chars(5..5);
    r.ed.insert("ef");
    assert_eq!(spans(&r.ed), vec![(3..7, RED)]);
    // Typing before the coloured word does not colour the new text.
    r.ed.select_chars(3..3);
    r.ed.insert("X");
    assert_eq!(r.ed.text(), "ab Xcdef");
    assert_eq!(spans(&r.ed), vec![(4..8, RED)]);
}

#[test]
fn colouring_a_selection_only_changes_that_range_and_is_reported_in_characters() {
    let mut r = Rig::new("é€ word");
    r.ed.select_chars(3..7);
    assert!(r.ed.set_color(Some(BLUE)));
    let done = r.ed.finish();
    assert_eq!(done.text, "é€ word");
    assert_eq!(done.runs, vec![TextRun { start: 3, end: 7, color: Some(BLUE) }]);
    assert_eq!(r.ed.selection_color(), Some(Some(BLUE)));
    // Clearing goes back to the element's colour.
    assert!(r.ed.set_color(None));
    assert!(r.ed.finish().runs.is_empty());
}

#[test]
fn colouring_without_a_selection_does_nothing() {
    let mut r = Rig::new("abc");
    assert!(!r.ed.set_color(Some(RED)));
    assert!(r.ed.runs().is_empty());
}

#[test]
fn the_colour_of_a_mixed_selection_is_unknown() {
    let mut r = Rig::new("abcd");
    r.ed.select_chars(0..2);
    r.ed.set_color(Some(RED));
    r.ed.select_chars(0..4);
    assert_eq!(r.ed.selection_color(), None);
}

#[test]
fn runs_from_the_element_come_back_unchanged_when_nothing_is_edited() {
    let runs = [TextRun { start: 2, end: 4, color: Some(RED) }];
    let ed = Editor::new("abcdef", &runs, InlineMath::Auto);
    assert_eq!(ed.finish().runs, runs.to_vec());
}

#[test]
fn replacing_a_coloured_selection_keeps_the_colour_of_its_first_character() {
    let mut r = Rig::new("one two three");
    r.ed.select_chars(4..7);
    r.ed.set_color(Some(RED));
    r.ed.select_chars(4..7);
    r.ed.insert("2");
    assert_eq!(r.ed.text(), "one 2 three");
    assert_eq!(spans(&r.ed), vec![(4..5, RED)]);
}

#[test]
fn input_is_cleaned_line_breaks_tabs_and_controls() {
    let mut r = Rig::new("");
    r.ed.insert("a\r\nb\rc\td\u{7}");
    assert_eq!(r.ed.text(), "a\nb\nc    d");
    let mut r = Rig::new("");
    assert!(!r.ed.insert("\u{7}\u{1b}"));
}

// ---- deleting ---------------------------------------------------------------------------------------------------

#[test]
fn backspace_and_delete_remove_one_cluster() {
    let mut r = Rig::new("ae\u{301}b");
    // The caret is at the end; backspace removes `b`, then the whole `é` (two scalars).
    r.key(Key::Backspace, NONE);
    assert_eq!(r.ed.text(), "ae\u{301}");
    r.key(Key::Backspace, NONE);
    assert_eq!(r.ed.text(), "a");
    r.key(Key::Left, NONE);
    r.key(Key::Left, NONE);
    r.key(Key::Delete, NONE);
    assert_eq!(r.ed.text(), "");
}

#[test]
fn backspace_at_the_start_and_delete_at_the_end_do_nothing() {
    let mut r = Rig::new("ab");
    r.go(Motion::TextStart, false);
    assert!(!r.key(Key::Backspace, NONE));
    r.go(Motion::TextEnd, false);
    assert!(!r.key(Key::Delete, NONE));
    assert_eq!(r.ed.text(), "ab");
}

#[test]
fn backspace_with_a_selection_removes_the_selection() {
    let mut r = Rig::new("hello world");
    r.ed.select_chars(0..6);
    r.key(Key::Backspace, NONE);
    assert_eq!(r.ed.text(), "world");
    assert_eq!(r.caret(), 0);
}

#[test]
fn ctrl_backspace_removes_a_word() {
    let mut r = Rig::new("one two three");
    r.key(Key::Backspace, CTRL);
    assert!(r.ed.text().starts_with("one two"), "{:?}", r.ed.text());
    assert!(r.ed.text().len() < "one two three".len());
}

#[test]
fn backspace_removes_a_whole_formula() {
    let mut r = Rig::new("a $x^2$ b");
    r.ed.select_chars(7..7);
    // The caret is just after the formula's closing dollar.
    r.key(Key::Backspace, NONE);
    assert_eq!(r.ed.text(), "a  b");
}

// ---- motions ----------------------------------------------------------------------------------------------------

#[test]
fn arrows_home_and_end_move_the_caret() {
    let mut r = Rig::new("one\ntwo");
    r.go(Motion::TextStart, false);
    r.go(Motion::Right, false);
    assert_eq!(r.caret(), 1);
    r.go(Motion::Down, false);
    assert_eq!(r.caret(), 5, "down keeps the column");
    r.go(Motion::LineStart, false);
    assert_eq!(r.caret(), 4);
    r.go(Motion::LineEnd, false);
    assert_eq!(r.caret(), 7);
    r.go(Motion::Up, false);
    assert_eq!(r.caret(), 3);
}

#[test]
fn shift_extends_and_a_plain_move_collapses() {
    let mut r = Rig::new("abcdef");
    r.go(Motion::TextStart, false);
    r.go(Motion::Right, true);
    r.go(Motion::Right, true);
    assert_eq!(r.ed.selection(), 0..2);
    assert_eq!(r.ed.anchor_focus(), (0, 2));
    r.go(Motion::Right, false);
    assert!(r.ed.is_collapsed());
}

#[test]
fn word_motions_stop_at_words() {
    let mut r = Rig::new("alpha beta gamma");
    r.go(Motion::TextStart, false);
    r.go(Motion::WordRight, false);
    let first = r.caret();
    assert!((5..=6).contains(&first), "{first}");
    r.go(Motion::WordRight, false);
    assert!(r.caret() > first);
    r.go(Motion::WordLeft, false);
    assert_eq!(r.caret(), first);
}

#[test]
fn select_all_and_the_text_ends() {
    let mut r = Rig::new("abc\ndef");
    r.key(Key::Letter('a'), CTRL);
    assert_eq!(r.ed.selection(), 0..7);
    r.go(Motion::ParagraphStart, false);
    assert_eq!(r.caret(), 4);
}

/// Hebrew: visual order is the reverse of logical order in a right-to-left paragraph.
const HEBREW: &str = "\u{5e9}\u{5dc}\u{5d5}\u{5dd}";

#[test]
fn in_right_to_left_text_the_left_arrow_moves_forward_in_the_text() {
    let mut r = Rig::new(HEBREW);
    r.go(Motion::TextStart, false);
    assert_eq!(r.caret(), 0);
    r.go(Motion::Left, false);
    let after_left = r.caret();
    r.go(Motion::TextStart, false);
    r.go(Motion::Right, false);
    let after_right = r.caret();
    // Whichever way the paragraph runs, the two arrows go opposite ways and each moves one letter (2 bytes).
    assert_ne!(after_left, after_right);
    assert_eq!(after_left.max(after_right), 2);
    assert_eq!(after_left.min(after_right), 0);
}

#[test]
fn backspace_in_right_to_left_text_deletes_the_letter_before_the_caret_in_logical_order() {
    let mut r = Rig::new(HEBREW);
    r.key(Key::Backspace, NONE);
    assert_eq!(r.ed.text(), "\u{5e9}\u{5dc}\u{5d5}");
    r.go(Motion::TextStart, false);
    r.key(Key::Delete, NONE);
    assert_eq!(r.ed.text(), "\u{5dc}\u{5d5}");
}

#[test]
fn mixed_direction_text_stays_whole_under_arrow_walks() {
    let text = format!("abc {HEBREW} def");
    let mut r = Rig::new(&text);
    r.go(Motion::TextStart, false);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..40 {
        r.go(Motion::Right, false);
        seen.insert(r.caret());
    }
    assert!(seen.iter().all(|&i| text.is_char_boundary(i)));
    assert!(seen.len() >= text.chars().count() / 2, "the caret visited {} places", seen.len());
}

#[test]
fn a_formula_is_one_step_for_the_arrows() {
    let mut r = Rig::new("a$x^2$b");
    r.go(Motion::TextStart, false);
    r.go(Motion::Right, false);
    assert_eq!(r.caret(), 1);
    r.go(Motion::Right, false);
    assert_eq!(r.caret(), 6, "one step crosses the whole formula");
    r.go(Motion::Left, false);
    assert_eq!(r.caret(), 1);
}

#[test]
fn the_formula_shows_its_source_only_while_the_caret_is_inside() {
    let mut r = Rig::new("a$x^2$b");
    r.ed.select_chars(4..4);
    let inside = r.ed.shown();
    assert_eq!(inside.reveal, Some(2));
    let l = layout(&mut r.ts, &inside);
    assert!(l.formulas.is_empty(), "the formula is plain text while the caret is in it");
    r.ed.select_chars(1..1);
    let outside = r.ed.shown();
    assert_eq!(outside.reveal, None);
    assert_eq!(layout(&mut r.ts, &outside).formulas.len(), 1);
    // Moving the caret out of the source types the formula again.
    r.ed.select_chars(4..4);
    r.go(Motion::TextEnd, false);
    assert_eq!(r.ed.shown().reveal, None);
}

#[test]
fn f2_and_a_double_click_open_the_source_of_a_formula() {
    let mut r = Rig::new("a$x^2$b");
    r.ed.select_chars(1..1);
    let l = r.layout();
    assert!(l.formula_at_index(1).is_some());
    assert!(r.key(Key::F2, NONE));
    assert_eq!(r.caret(), 2);
    assert_eq!(r.ed.shown().reveal, Some(2));
    // A double click on the typeset formula does the same.
    let mut r = Rig::new("a$x^2$b");
    let l = r.layout();
    let f = &l.formulas[0];
    let mid = Point::new((f.rect.x0 + f.rect.x1) / 2.0, (f.rect.y0 + f.rect.y1) / 2.0);
    r.ed.press(&l, mid, 2, false);
    assert_eq!(r.caret(), 2);
    assert!(r.ed.shown().reveal.is_some());
}

#[test]
fn turning_formulas_off_shows_every_dollar_as_text() {
    let mut r = Rig::new("a$x$b");
    r.ed.select_chars(0..0);
    assert!(r.act(KeyAction::ToggleFormulas));
    assert_eq!(r.ed.inline_math(), InlineMath::Off);
    assert_eq!(r.ed.finish().inline_math, InlineMath::Off);
    assert_eq!(r.ed.shown().reveal, None);
}

// ---- the mouse --------------------------------------------------------------------------------------------------

fn centre_of_line(l: &TextLayout, n: usize) -> f64 {
    let line = &l.lines[n];
    f64::from(line.top + line.bottom) / 2.0
}

#[test]
fn a_click_sets_the_caret_and_a_drag_selects() {
    let mut r = Rig::new("hello world");
    let l = r.layout();
    let y = centre_of_line(&l, 0);
    let a = l.caret_point(l.selection_at_index(2, drafftink_typeset::Affinity::Downstream).focus());
    let b = l.caret_point(l.selection_at_index(8, drafftink_typeset::Affinity::Downstream).focus());
    assert!(r.ed.press(&l, Point::new(a.x, y), 1, false));
    assert_eq!(r.caret(), 2);
    assert!(r.ed.drag(&l, Point::new(b.x, y)));
    assert_eq!(r.ed.selection(), 2..8);
    r.ed.release();
    assert!(!r.ed.drag(&l, Point::new(0.0, y)), "no selection change without the button down");
}

#[test]
fn double_click_selects_a_word_and_triple_click_the_paragraph() {
    let mut r = Rig::new("hello world\nnext");
    let l = r.layout();
    let y = centre_of_line(&l, 0);
    let at = l.caret_point(l.selection_at_index(7, drafftink_typeset::Affinity::Downstream).focus());
    r.ed.press(&l, Point::new(at.x, y), 2, false);
    assert_eq!(r.ed.selected_text(), Some("world"));
    r.ed.press(&l, Point::new(at.x, y), 3, false);
    assert!(r.ed.selected_text().is_some_and(|s| s.starts_with("hello world")), "{:?}", r.ed.selected_text());
    assert!(!r.ed.selected_text().unwrap().contains("next"));
}

#[test]
fn shift_click_keeps_the_anchor() {
    let mut r = Rig::new("abcdefgh");
    r.ed.select_chars(2..2);
    let l = r.layout();
    let y = centre_of_line(&l, 0);
    let to = l.caret_point(l.selection_at_index(6, drafftink_typeset::Affinity::Downstream).focus());
    r.ed.press(&l, Point::new(to.x, y), 1, true);
    assert_eq!(r.ed.anchor_focus(), (2, 6));
}

#[test]
fn clicks_are_counted_by_time_and_place() {
    let mut c = Clicks::default();
    let p = Point::new(3.0, 3.0);
    assert_eq!((c.press(p, 0.0), c.press(p, 100.0), c.press(p, 200.0)), (1, 2, 3));
}

// ---- clipboard --------------------------------------------------------------------------------------------------

#[test]
fn copy_cut_and_paste_work_inside_the_text() {
    let mut r = Rig::new("hello world");
    r.ed.select_chars(0..5);
    assert_eq!(r.ed.copy().as_deref(), Some("hello"));
    assert_eq!(r.ed.cut().as_deref(), Some("hello"));
    assert_eq!(r.ed.text(), " world");
    r.ed.select_chars(6..6);
    assert!(r.ed.paste("!\r\n"));
    assert_eq!(r.ed.text(), " world!\n");
    assert_eq!(r.ed.cut(), None, "nothing selected");
    assert_eq!(r.ed.copy(), None);
}

#[test]
fn a_paste_is_one_undo_step() {
    let mut r = Rig::new("");
    r.ed.insert("ab");
    r.ed.paste("cd ef");
    r.ed.insert("g");
    assert_eq!(r.ed.text(), "abcd efg");
    r.ed.undo();
    assert_eq!(r.ed.text(), "abcd ef");
    r.ed.undo();
    assert_eq!(r.ed.text(), "ab");
}

// ---- the editor's own undo --------------------------------------------------------------------------------------

#[test]
fn undo_goes_back_by_runs_of_typing_and_redo_returns() {
    let mut r = Rig::new("");
    for c in ["h", "i", " ", "y", "o"] {
        r.ed.insert(c);
    }
    assert_eq!(r.ed.text(), "hi yo");
    // A space starts a new step, and the word after it joins that step.
    assert!(r.ed.undo());
    assert_eq!(r.ed.text(), "hi");
    assert!(r.ed.undo());
    assert_eq!(r.ed.text(), "");
    assert!(r.ed.redo());
    assert_eq!(r.ed.text(), "hi");
    assert!(r.ed.redo());
    assert_eq!(r.ed.text(), "hi yo");
    assert!(r.ed.undo() && r.ed.undo());
    assert_eq!(r.ed.text(), "");
    assert!(!r.ed.undo(), "nothing left");
}

#[test]
fn a_new_edit_clears_redo_and_undo_restores_colours_and_the_caret() {
    let mut r = Rig::new("abc def");
    r.ed.select_chars(4..7);
    r.ed.set_color(Some(RED));
    r.ed.select_chars(7..7);
    r.ed.insert("!");
    assert_eq!(spans(&r.ed), vec![(4..8, RED)]);
    r.ed.undo();
    assert_eq!(r.ed.text(), "abc def");
    assert_eq!(spans(&r.ed), vec![(4..7, RED)]);
    r.ed.undo();
    assert!(r.ed.runs().is_empty());
    r.ed.redo();
    r.ed.insert("?");
    assert!(!r.ed.redo(), "an edit after an undo drops the redo steps");
}

#[test]
fn undo_count_is_bounded() {
    let mut r = Rig::new("");
    for _ in 0..500 {
        r.ed.insert("a b ");
    }
    assert!(r.ed.undo_steps() <= 200);
}

// ---- composition ------------------------------------------------------------------------------------------------

#[test]
fn a_preedit_is_shown_at_the_caret_and_leaves_the_text_alone() {
    let mut r = Rig::new("ab");
    r.ed.select_chars(1..1);
    r.ed.preedit("ni", Some((2, 2)));
    assert!(r.ed.is_composing());
    assert_eq!(r.ed.text(), "ab");
    let shown = r.ed.shown();
    assert_eq!(shown.text, "anib");
    assert_eq!(shown.composition, Some(1..3));
    assert_eq!(shown.selection, (3, 3));
    r.ed.preedit("ni h", None);
    assert_eq!(r.ed.shown().selection, (5, 5));
}

#[test]
fn a_commit_replaces_the_preedit() {
    let mut r = Rig::new("ab");
    r.ed.select_chars(1..1);
    r.ed.preedit("ni", None);
    assert!(r.ed.commit("你"));
    assert!(!r.ed.is_composing());
    assert_eq!(r.ed.text(), "a你b");
    assert_eq!(r.caret(), 1 + "你".len());
}

#[test]
fn cancelling_a_composition_leaves_no_trace() {
    let mut r = Rig::new("ab");
    r.ed.select_chars(1..1);
    r.ed.insert("x");
    let steps = r.ed.undo_steps();
    r.ed.preedit("zh", None);
    r.ed.cancel_composition();
    assert!(!r.ed.is_composing());
    assert_eq!((r.ed.text(), r.ed.undo_steps()), ("axb", steps));
    // An empty preedit is a cancel.
    r.ed.preedit("zh", None);
    r.ed.preedit("", None);
    assert!(!r.ed.is_composing());
}

#[test]
fn a_selection_is_replaced_when_the_composition_starts() {
    let mut r = Rig::new("hello");
    r.ed.select_chars(1..4);
    r.ed.preedit("n", None);
    assert_eq!(r.ed.text(), "ho");
    r.ed.commit("你");
    assert_eq!(r.ed.text(), "h你o");
}

#[test]
fn keys_and_the_mouse_wait_while_the_input_method_composes() {
    let mut r = Rig::new("ab");
    r.ed.preedit("x", None);
    assert!(!r.key(Key::Backspace, NONE));
    assert_eq!(r.ed.text(), "ab");
    let l = r.layout();
    assert!(!r.ed.press(&l, Point::new(0.0, 5.0), 1, false));
}

#[test]
fn composed_text_takes_the_colour_before_it_and_finish_ignores_a_running_composition() {
    let mut r = Rig::new("ab");
    r.ed.select_chars(0..2);
    r.ed.set_color(Some(RED));
    r.ed.select_chars(2..2);
    r.ed.preedit("ni", None);
    assert_eq!(r.ed.shown().runs.spans().iter().map(|s| s.range.clone()).collect::<Vec<_>>(), vec![0..4]);
    let done = r.ed.finish();
    assert_eq!(done.text, "ab");
    assert_eq!(done.runs, vec![TextRun { start: 0, end: 2, color: Some(RED) }]);
}

#[test]
fn a_cursor_range_off_a_character_boundary_is_ignored() {
    let mut r = Rig::new("");
    r.ed.preedit("你好", Some((1, 2)));
    assert_eq!(r.ed.shown().selection, (6, 6));
    r.ed.preedit("你好", Some((0, 3)));
    assert_eq!(r.ed.shown().selection, (3, 3));
}

// ---- revision and hostile input ---------------------------------------------------------------------------------

#[test]
fn the_revision_moves_when_something_visible_changes() {
    let mut r = Rig::new("abc");
    let v0 = r.ed.revision();
    r.go(Motion::Left, false);
    let v1 = r.ed.revision();
    assert_ne!(v0, v1);
    r.ed.insert("x");
    assert_ne!(v1, r.ed.revision());
}

#[test]
fn hostile_sequences_never_panic_and_keep_the_model_consistent() {
    let mut r = Rig::new("a$x$ é\u{301}\n你");
    let actions = [
        KeyAction::Backspace { word: true },
        KeyAction::Delete { word: false },
        KeyAction::Move { motion: Motion::Up, extend: true },
        KeyAction::Move { motion: Motion::WordLeft, extend: false },
        KeyAction::Newline,
        KeyAction::Undo,
        KeyAction::Redo,
        KeyAction::SelectAll,
        KeyAction::RevealFormula,
    ];
    for i in 0..400usize {
        let a = actions[(i * 7 + i / 3) % actions.len()];
        r.act(a);
        match i % 5 {
            0 => {
                r.ed.insert("$é$");
            }
            1 => r.ed.preedit("你", Some((0, 99))),
            2 => {
                r.ed.commit("a");
            }
            3 => {
                let l = r.layout();
                r.ed.press(&l, Point::new(f64::NAN, f64::INFINITY), (i % 4) as u32, i % 2 == 0);
                r.ed.drag(&l, Point::new(1e12, -1e12));
            }
            _ => {
                r.ed.set_color(Some(RED));
            }
        }
        let (a, f) = r.ed.anchor_focus();
        let t = r.ed.text();
        assert!(a <= t.len() && f <= t.len() && t.is_char_boundary(a) && t.is_char_boundary(f), "step {i}");
        assert!(r.ed.runs().spans().iter().all(|s| s.range.end <= t.len() && t.is_char_boundary(s.range.start)));
        let shown = r.ed.shown();
        assert!(shown.selection.1 <= shown.text.len());
    }
}
