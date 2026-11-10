//! The keyboard shortcuts dialog (`?`). It lists what `keymap::help_rows` returns, which is built from
//! the registry and the UI verb table, so a binding cannot be missing. Under the generated groups come
//! two fixed groups: the keys of the text editor (docs/commands.md §6.2) and the pointer and touch
//! gestures (§7.3). A search field filters all rows.

use egui::{
    Align, Align2, Color32, Context, FontFamily, FontId, Id, Layout, Modal, Response, RichText, Role, ScrollArea,
    Sense, Stroke, TextEdit, Ui, WidgetInfo, vec2,
};

use crate::action::Verb;
use crate::keymap::{HelpRow, help_rows};
use crate::state::UiState;
use crate::surfaces::palette::{family_title, fuzzy_score};
use crate::theme::{self, Palette};
use crate::widgets::{Cx, dialog_frame};

/// Width of the dialog.
const WIDTH: f32 = 560.0;
/// Height of the list before it scrolls.
const LIST_HEIGHT: f32 = 420.0;
/// Heading of the group with the text editor keys.
const TEXT_GROUP: &str = "Text editing";
/// Heading of the group with the pointer gestures.
const MOUSE_GROUP: &str = "Mouse and touch";

/// What the dialog keeps in egui memory: the search text, and whether its field has been focused.
#[derive(Clone, Default)]
struct Mem {
    query: String,
    focused: bool,
}

/// Draws the dialog when `st.shortcuts_open` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let mem_id = Id::unique("shortcuts.mem");
    if !st.shortcuts_open {
        ctx.data_mut(|d| d.remove::<Mem>(mem_id));
        return;
    }
    let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(mem_id)).unwrap_or_default();
    let pal = cx.pal;
    let rows = all_rows(cx.model);
    let id = Id::unique("shortcuts");
    let area = Modal::default_area(id).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0));
    let response = Modal::new(id).area(area).frame(dialog_frame(&pal)).backdrop_color(pal.backdrop).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.label(RichText::new("Keyboard shortcuts").font(theme::hand(22.0)));
        ui.add_space(6.0);
        let query_id = Id::unique("shortcuts.query");
        let edit = TextEdit::singleline(&mut mem.query)
            .id(query_id)
            .hint_text("Search shortcuts")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6));
        // A new area lays out once before it can take focus, so ask until the field has it.
        if !mem.focused {
            ui.ctx().memory_mut(|m| m.request_focus(query_id));
        }
        mem.focused |= ui.add(edit).has_focus();
        ui.add_space(6.0);
        show_rows(ui, &pal, &rows, &mem.query);
    });
    ctx.data_mut(|d| d.insert_temp(mem_id, mem));
    // The search field has the focus, so Esc is read here too; the keymap handles it otherwise.
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if response.should_close() || esc {
        cx.verb(Verb::CloseOverlays);
    }
}

/// The generated rows of `help_rows` followed by the fixed groups.
pub fn all_rows(model: &crate::model::Model) -> Vec<HelpRow> {
    let mut rows = help_rows(model);
    rows.extend(fixed_rows(model.mac));
    rows
}

/// The fixed rows, with `Mod` spelled for the platform.
fn fixed_rows(mac: bool) -> Vec<HelpRow> {
    let m = |rest: &str| if mac { format!("⌘{rest}") } else { format!("Ctrl+{rest}") };
    let alt = |rest: &str| if mac { format!("⌥{rest}") } else { format!("Alt+{rest}") };
    let shift = |rest: &str| if mac { format!("⇧{rest}") } else { format!("Shift+{rest}") };
    let row = |group: &str, title: &str, keys: Vec<String>| HelpRow {
        title: title.to_owned(),
        group: group.to_owned(),
        keys,
    };
    let s = |t: &str| t.to_owned();
    vec![
        row(TEXT_GROUP, "New line", vec![s("Enter")]),
        row(TEXT_GROUP, "Finish editing", vec![m("Enter"), s("Esc")]),
        row(TEXT_GROUP, "Select all text", vec![m("A")]),
        row(TEXT_GROUP, "Cut, copy and paste text", vec![m("X"), m("C"), m("V")]),
        row(TEXT_GROUP, "Undo and redo typing", vec![m("Z"), m(&shift("Z"))]),
        row(TEXT_GROUP, "Move by character", vec![s("←"), s("→")]),
        row(TEXT_GROUP, "Move by word", vec![if mac { alt("←") } else { m("←") }, if mac { alt("→") } else { m("→") }]),
        row(TEXT_GROUP, "Extend the selection", vec![shift("←"), shift("→")]),
        row(TEXT_GROUP, "Delete by word", vec![if mac { alt("Backspace") } else { m("Backspace") }]),
        row(MOUSE_GROUP, "Pan", vec![s("Wheel"), s("Space + drag"), s("Middle button + drag")]),
        row(MOUSE_GROUP, "Zoom", vec![m("Wheel"), s("Pinch")]),
        row(MOUSE_GROUP, "Pan and zoom on a touch screen", vec![s("Two fingers")]),
        row(MOUSE_GROUP, "Open the context menu", vec![s("Right click")]),
        row(MOUSE_GROUP, "Add to the selection", vec![shift("Click")]),
        row(MOUSE_GROUP, "Duplicate while moving", vec![alt("Drag")]),
        row(MOUSE_GROUP, "Keep the aspect ratio while resizing", vec![shift("Drag")]),
        row(MOUSE_GROUP, "Enter a group, or write a text", vec![s("Double click")]),
    ]
}

/// True when a row matches every word of the query: in its title, its group or one of its keys.
fn matches(row: &HelpRow, query: &str) -> bool {
    query.split_whitespace().all(|word| {
        fuzzy_score(word, &row.title).is_some()
            || fuzzy_score(word, &family_title(&row.group)).is_some()
            || row.keys.iter().any(|k| k.to_lowercase().contains(&word.to_lowercase()))
    })
}

/// The rows that match, in groups in order of first appearance, as `(heading, rows)`.
pub fn grouped<'a>(rows: &'a [HelpRow], query: &str) -> Vec<(String, Vec<&'a HelpRow>)> {
    let mut groups: Vec<(String, Vec<&HelpRow>)> = Vec::new();
    for row in rows.iter().filter(|r| matches(r, query)) {
        let heading = family_title(&row.group);
        match groups.iter_mut().find(|(h, _)| *h == heading) {
            Some((_, list)) => list.push(row),
            None => groups.push((heading, vec![row])),
        }
    }
    groups
}

fn show_rows(ui: &mut Ui, pal: &Palette, rows: &[HelpRow], query: &str) {
    let groups = grouped(rows, query);
    if groups.is_empty() {
        ui.label(RichText::new("No shortcut matches.").color(pal.muted));
        return;
    }
    ScrollArea::vertical().max_height(LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for (heading, list) in groups {
            ui.add_space(8.0);
            ui.label(RichText::new(heading).font(theme::hand(15.0)).color(pal.muted));
            for row in list {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&row.title).size(13.5).color(pal.text)).selectable(false));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        key_caps(ui, pal, &row.keys.join(" / "));
                    });
                });
            }
        }
    });
}

// ---- key caps ----

/// Splits a chord label into its caps: `Ctrl+Shift+Z` into `Ctrl`, `Shift`, `Z`; `⌘⇧Z` into `⌘`, `⇧`,
/// `Z`; `Ctrl++` into `Ctrl`, `+`.
pub(crate) fn split_chord(label: &str) -> Vec<String> {
    const WORDS: [&str; 3] = ["Ctrl+", "Alt+", "Shift+"];
    const SYMBOLS: [char; 4] = ['⌃', '⌥', '⇧', '⌘'];
    let mut caps = Vec::new();
    let mut rest = label;
    'outer: loop {
        for word in WORDS {
            if let Some(r) = rest.strip_prefix(word) {
                caps.push(word.trim_end_matches('+').to_owned());
                rest = r;
                continue 'outer;
            }
        }
        if let Some(c) = rest.chars().next().filter(|c| SYMBOLS.contains(c)) {
            caps.push(c.to_string());
            rest = &rest[c.len_utf8()..];
            continue;
        }
        break;
    }
    if !rest.is_empty() {
        caps.push(rest.to_owned());
    }
    caps
}

/// How a cap is written. The bundled fonts have no glyph for the Mac modifier symbols `⌘ ⌥ ⌃`, so the cap
/// spells them out (`Cmd`, `Opt`, `Ctrl`); `⇧` and the arrows exist in the monospace font only.
fn cap_face(cap: &str) -> (&str, FontFamily) {
    match cap {
        "⌘" => ("Cmd", FontFamily::Proportional),
        "⌥" => ("Opt", FontFamily::Proportional),
        "⌃" => ("Ctrl", FontFamily::Proportional),
        c if c.chars().any(|ch| matches!(ch, '⇧' | '←' | '→' | '↑' | '↓')) => (c, FontFamily::Monospace),
        c => (c, FontFamily::Proportional),
    }
}

/// Draws chords as small rounded key caps, right to left: call it inside a `right_to_left` layout. Several
/// chords are written `A / B`. Each chord is one accessible node named with its label (`Ctrl+Z`).
pub(crate) fn key_caps(ui: &mut Ui, pal: &Palette, labels: &str) {
    let chords: Vec<&str> = labels.split(" / ").collect();
    for (n, chord) in chords.iter().enumerate().rev() {
        if n + 1 < chords.len() {
            ui.label(RichText::new("/").size(11.0).color(pal.muted));
        }
        chord_caps(ui, pal, chord);
    }
}

fn chord_caps(ui: &mut Ui, pal: &Palette, chord: &str) -> Response {
    let pad = vec2(6.0, 2.0);
    let gap = 3.0;
    let galleys: Vec<_> = split_chord(chord)
        .iter()
        .map(|cap| {
            let (text, family) = cap_face(cap);
            ui.painter().layout_no_wrap(text.to_owned(), FontId::new(11.0, family), pal.text)
        })
        .collect();
    let height = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max) + 2.0 * pad.y;
    let width: f32 =
        galleys.iter().map(|g| g.size().x + 2.0 * pad.x).sum::<f32>() + gap * galleys.len().saturating_sub(1) as f32;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let mut x = rect.left();
        for galley in galleys {
            let cap_width = galley.size().x + 2.0 * pad.x;
            let cap = egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(cap_width, height));
            ui.painter().rect(cap, 4, pal.button, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
            let at = cap.center() - galley.size() / 2.0;
            ui.painter().galley(at, galley, Color32::PLACEHOLDER);
            x += cap_width + gap;
        }
    }
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, chord));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_split_into_caps() {
        assert_eq!(split_chord("Ctrl+Shift+Z"), ["Ctrl", "Shift", "Z"]);
        assert_eq!(split_chord("⌘⇧Z"), ["⌘", "⇧", "Z"]);
        assert_eq!(split_chord("Ctrl++"), ["Ctrl", "+"]);
        assert_eq!(split_chord("Esc"), ["Esc"]);
        assert_eq!(split_chord("Alt+PgUp"), ["Alt", "PgUp"]);
        assert_eq!(split_chord("Shift+←"), ["Shift", "←"]);
        assert!(split_chord("").is_empty());
    }

    #[test]
    fn mac_symbols_are_spelled_out_on_the_caps() {
        assert_eq!(cap_face("⌘").0, "Cmd");
        assert_eq!(cap_face("⇧").1, FontFamily::Monospace);
        assert_eq!(cap_face("Z"), ("Z", FontFamily::Proportional));
    }
}
