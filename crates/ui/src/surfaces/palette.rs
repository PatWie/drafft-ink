//! The command palette: every command with `palette == true` as a row, the UI verbs too, with a fuzzy
//! filter. A row runs its command with origin `Palette`. A command that needs arguments opens the form of
//! [`crate::args_form`] first.
//!
//! The rows are data ([`rows`]); the filter is a pure function ([`filter`]); [`show`] draws them. The
//! query and the highlighted row live in [`PaletteState`](crate::state::PaletteState). The open form
//! lives in egui's temporary memory, keyed by the palette, and is dropped when the palette closes.

use drafftink_engine::Origin;
use egui::{
    Align, Align2, Context, Frame, Id, Key, Layout, Margin, Modal, Modifiers, Response, RichText, Role, ScrollArea,
    Sense, Shape, TextEdit, Ui, UiBuilder, WidgetInfo, vec2,
};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::args_form::{self, Field, FormValues};
use crate::keymap::{Pointer, VERB_BINDINGS, parse_chord, substitute};
use crate::model::{self, Model};
use crate::state::UiState;
use crate::theme::{BUTTON_RADIUS, Palette, TextSize};
use crate::widgets::{Ctl, Cx, dialog_frame, separator};

/// Width of the dialog.
const WIDTH: f32 = 540.0;
/// Height of the list before it scrolls.
const LIST_HEIGHT: f32 = 380.0;
/// The family name the UI verbs are listed under.
const UI_FAMILY: &str = "ui";

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum RowAction {
    /// Runs this command with these arguments.
    Command {
        /// The command id.
        id: String,
        /// The arguments (`{}` for none).
        args: Value,
    },
    /// Opens the arguments form of this command.
    Form {
        /// The command id.
        id: String,
    },
    /// Applies a UI verb.
    Verb(Verb),
}

/// One line of the palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The title: the command's, or the title of the binding the row stands for.
    pub title: String,
    /// The family (`ui` for the verbs).
    pub family: String,
    /// The command id, or the verb id.
    pub id: String,
    /// The first shortcut, as shown.
    pub keys: Option<String>,
    /// Why the row cannot run now. `None` when it can.
    pub refusal: Option<String>,
    /// What choosing it does.
    pub action: RowAction,
}

impl Row {
    /// True when the row can run now.
    pub fn enabled(&self) -> bool {
        self.refusal.is_none()
    }
}

/// The family as a heading: capitalised, and `ui` as "Interface".
pub(crate) fn family_title(family: &str) -> String {
    if family == UI_FAMILY {
        return "Interface".to_owned();
    }
    let mut chars = family.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Every row of the palette: one per titled binding of a palette command, one for a palette command
/// without titled bindings, one per variant (a named call without keys), then one per UI verb. Arguments with `$cursor` placeholders lose them,
/// because the palette has no pointer.
pub fn rows(model: &Model) -> Vec<Row> {
    let mut out = Vec::new();
    for cmd in model.commands.iter().filter(|c| c.palette) {
        let refusal = cmd.available.as_ref().err().map(|r| r.message.clone());
        let titled: Vec<_> = cmd.shortcuts.iter().filter_map(|b| b.title.map(|t| (t, b))).collect();
        let make = |title: &str, shortcut_args: &Value, action: RowAction| Row {
            title: title.to_owned(),
            family: cmd.family.to_owned(),
            id: cmd.id.to_owned(),
            keys: model.shortcut_label(cmd.id, shortcut_args),
            refusal: refusal.clone(),
            action,
        };
        if titled.is_empty() {
            let action = if args_form::requires_args(cmd.id) {
                RowAction::Form { id: cmd.id.to_owned() }
            } else {
                RowAction::Command { id: cmd.id.to_owned(), args: json!({}) }
            };
            out.push(make(cmd.title, &json!({}), action));
        } else {
            for (title, binding) in titled {
                let args = match &binding.args {
                    Value::Null => json!({}),
                    a => substitute(a, &Pointer::default()),
                };
                out.push(make(title, &binding.args, RowAction::Command { id: cmd.id.to_owned(), args }));
            }
        }
        for variant in &cmd.variants {
            let action = RowAction::Command { id: cmd.id.to_owned(), args: variant.args.clone() };
            out.push(make(variant.title, &variant.args, action));
        }
    }
    for verb in Verb::ALL {
        let keys = VERB_BINDINGS
            .iter()
            .find(|b| b.verb == verb)
            .and_then(|b| b.keys.iter().find_map(|k| parse_chord(k)))
            .map(|c| c.label(model.mac));
        out.push(Row {
            title: verb.title().to_owned(),
            family: UI_FAMILY.to_owned(),
            id: verb.id().to_owned(),
            keys,
            refusal: None,
            action: RowAction::Verb(verb),
        });
    }
    out
}

/// How well `query` matches `text`: `None` when its letters (white space ignored, case ignored) do not
/// appear in `text` in order. Higher is better: runs of consecutive letters, matches at the start of a
/// word and an early first letter score more, gaps cost a little, and a plain substring gets a bonus.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0_i32;
    let mut next = 0;
    let mut prev: Option<usize> = None;
    for qc in &q {
        let pos = next + t.get(next..)?.iter().position(|c| c == qc)?;
        score += 10;
        if pos == 0 || t.get(pos - 1).is_some_and(|c| !c.is_alphanumeric()) {
            score += 12;
        }
        score -= match prev {
            Some(p) if p + 1 == pos => {
                score += 15;
                0
            }
            Some(p) => i32::try_from(pos - p - 1).unwrap_or(5).min(5),
            None => i32::try_from(pos).unwrap_or(10).min(10),
        };
        prev = Some(pos);
        next = pos + 1;
    }
    let q_text: String = q.iter().collect();
    let t_text: String = t.iter().collect();
    if t_text.contains(&q_text) {
        score += 20;
    }
    Some(score)
}

/// The score of a row for a query: every white-space separated word must match the title, the family
/// or the id, and the best field counts. A title match weighs more.
fn row_score(row: &Row, query: &str) -> Option<i32> {
    query
        .split_whitespace()
        .map(|word| {
            [fuzzy_score(word, &row.title).map(|s| s + 8), fuzzy_score(word, &row.family), fuzzy_score(word, &row.id)]
                .into_iter()
                .flatten()
                .max()
        })
        .sum()
}

/// The indexes of the rows to show, in display order. An empty query lists everything grouped by family
/// (the families in the order of their first row, the UI verbs last). Otherwise the matches are sorted
/// by score, best first, and ties keep the order of `rows`.
pub fn filter(rows: &[Row], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        let mut families: Vec<&str> = Vec::new();
        for r in rows.iter().filter(|r| r.family != UI_FAMILY) {
            if !families.contains(&r.family.as_str()) {
                families.push(&r.family);
            }
        }
        families.push(UI_FAMILY);
        let rank = |r: &Row| families.iter().position(|f| *f == r.family).unwrap_or(usize::MAX);
        let mut all: Vec<usize> = (0..rows.len()).collect();
        all.sort_by_key(|&i| rows.get(i).map_or(usize::MAX, rank));
        return all;
    }
    let mut scored: Vec<(usize, i32)> =
        rows.iter().enumerate().filter_map(|(i, r)| row_score(r, query).map(|s| (i, s))).collect();
    scored.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// An open arguments form.
#[derive(Clone)]
struct FormSession {
    id: String,
    title: String,
    summary: String,
    schema: Value,
    fields: Vec<Field>,
    values: FormValues,
    example: Option<Value>,
}

impl FormSession {
    fn open(model: &Model, id: &str) -> Option<Self> {
        let schema = model::args_schema(id)?;
        let fields = args_form::fields(&schema);
        let example = model::example_args(id);
        let values = args_form::initial_values(&fields, example.as_ref());
        let cmd = model.command(id)?;
        Some(Self {
            id: id.to_owned(),
            title: cmd.title.to_owned(),
            summary: cmd.summary.to_owned(),
            schema,
            fields,
            values,
            example,
        })
    }
}

/// What the palette keeps in egui memory between frames.
#[derive(Clone, Default)]
struct Mem {
    /// The query field has been focused for this view.
    focused: bool,
    form: Option<FormSession>,
}

/// What happened in a frame that closes the palette or changes its view.
enum Outcome {
    /// Run the chosen action, then close.
    Run(RowAction),
    /// Close with the verb (Esc, a click on the backdrop).
    Close,
    /// The form already emitted its command: close.
    Finished,
}

/// Draws the palette when `st.palette` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let mem_id = Id::unique("palette.mem");
    if st.palette.is_none() {
        ctx.data_mut(|d| d.remove::<Mem>(mem_id));
        return;
    }
    let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(mem_id)).unwrap_or_default();
    let outcome = draw(ctx, cx, st, &mut mem);
    ctx.data_mut(|d| d.insert_temp(mem_id, mem));
    match outcome {
        Some(Outcome::Close) => cx.verb(Verb::CloseOverlays),
        Some(Outcome::Run(action)) => finish(cx, st, action),
        Some(Outcome::Finished) => st.palette = None,
        None => {}
    }
}

/// Runs the chosen action and closes the palette.
fn finish(cx: &mut Cx<'_>, st: &mut UiState, action: RowAction) {
    match action {
        RowAction::Command { id, args } => cx.with_origin(Origin::Palette, |cx| cx.run(&id, args)),
        RowAction::Verb(v) => cx.verb(v),
        // The form is opened by `draw`, which never reports it as an outcome.
        RowAction::Form { .. } => return,
    }
    st.palette = None;
}

fn draw(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState, mem: &mut Mem) -> Option<Outcome> {
    let pal = cx.pal;
    let all = rows(cx.model);
    let state = st.palette.as_mut()?;
    let mut outcome = None;
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        outcome = Some(Outcome::Close);
    }
    // The query field has the focus, so the keys that move through the list are read here, before the
    // text field sees them.
    let listed = filter(&all, &state.query);
    let mut moved = false;
    let mut chosen: Option<usize> = None;
    if mem.form.is_none() && !listed.is_empty() {
        let last = listed.len() - 1;
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        if down {
            state.selected = if state.selected >= last { 0 } else { state.selected + 1 };
            moved = true;
        }
        if up {
            state.selected = if state.selected == 0 { last } else { state.selected - 1 };
            moved = true;
        }
        if enter {
            chosen = Some(state.selected.min(last));
        }
    }

    let id = Id::unique("palette");
    let area = Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 80.0));
    let response = Modal::new(id).area(area).frame(dialog_frame(&pal)).backdrop_color(pal.backdrop).show(ctx, |ui| {
        ui.set_width(WIDTH);
        let mut inner = None;
        if let Some(form) = mem.form.as_mut() {
            match show_form(ui, cx, form) {
                FormEvent::Back => mem.form = None,
                FormEvent::Done => inner = Some(Outcome::Finished),
                FormEvent::Nothing => {}
            }
            if mem.form.is_none() {
                mem.focused = false;
            }
        } else {
            let picked = show_list(ui, &pal, state, &all, mem, moved, chosen);
            if let Some(index) = picked {
                inner = open_row(cx.model, &all, index, mem);
            }
        }
        inner
    });
    let close = response.should_close();
    outcome.or(response.inner).or(close.then_some(Outcome::Close))
}

/// Acts on a chosen row. A form row opens its form and returns nothing; the others are the outcome.
fn open_row(model: &Model, all: &[Row], index: usize, mem: &mut Mem) -> Option<Outcome> {
    let row = all.get(index)?;
    if !row.enabled() {
        return None;
    }
    match &row.action {
        RowAction::Form { id } => match FormSession::open(model, id) {
            Some(form) => {
                mem.form = Some(form);
                None
            }
            // No schema to build a form from: run it without arguments and let it refuse in words.
            None => Some(Outcome::Run(RowAction::Command { id: id.clone(), args: json!({}) })),
        },
        action => Some(Outcome::Run(action.clone())),
    }
}

/// The query field, the list, and a key hint. Returns the index (into `all`) of the row the user chose.
fn show_list(
    ui: &mut Ui,
    pal: &Palette,
    state: &mut crate::state::PaletteState,
    all: &[Row],
    mem: &mut Mem,
    moved: bool,
    chosen: Option<usize>,
) -> Option<usize> {
    let query_id = Id::unique("palette.query");
    ui.label(RichText::new("Command palette").font(TextSize::Display.hand()));
    ui.add_space(6.0);
    let edit = TextEdit::singleline(&mut state.query)
        .id(query_id)
        .hint_text("Type a command, a family or a shortcut name")
        .font(TextSize::Label.font())
        .desired_width(f32::INFINITY)
        .margin(Margin::symmetric(8, 6));
    // A new area lays out once before it can take focus, so ask until the field has it.
    if !mem.focused {
        ui.ctx().memory_mut(|m| m.request_focus(query_id));
    }
    let query = ui.add(edit);
    mem.focused |= query.has_focus();
    let query_changed = query.changed();
    if query_changed {
        state.selected = 0;
    }
    ui.add_space(6.0);

    let listed = filter(all, &state.query);
    state.selected = state.selected.min(listed.len().saturating_sub(1));
    let grouped = state.query.trim().is_empty();
    let mut picked = chosen.and_then(|i| listed.get(i).copied());

    if listed.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("No command matches.").color(pal.muted));
    }
    ScrollArea::vertical().max_height(LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let mut heading: Option<&str> = None;
        for (position, &index) in listed.iter().enumerate() {
            let Some(row) = all.get(index) else {
                continue;
            };
            if grouped && heading != Some(row.family.as_str()) {
                heading = Some(&row.family);
                ui.add_space(6.0);
                ui.label(RichText::new(family_title(&row.family)).font(TextSize::Label.hand()).color(pal.muted));
            }
            let is_selected = position == state.selected;
            let resp = show_row(ui, pal, row, is_selected, !grouped);
            if is_selected && moved {
                resp.scroll_to_me(Some(Align::Center));
            }
            if resp.clicked() && row.enabled() {
                state.selected = position;
                picked = Some(index);
            }
        }
    });
    ui.add_space(6.0);
    separator(ui, pal);
    ui.add_space(4.0);
    ui.label(
        RichText::new("Up and Down to move, Enter to run, Esc to close")
            .size(TextSize::Caption.points())
            .color(pal.muted),
    );
    picked
}

/// One row: the title, the shortcut on the right, and under a refused row the refusal text.
fn show_row(ui: &mut Ui, pal: &Palette, row: &Row, selected: bool, show_family: bool) -> Response {
    let width = ui.available_width();
    let background = ui.painter().add(Shape::Noop);
    let enabled = row.enabled();
    let text_color = if enabled { pal.text } else { pal.muted };
    let resp = ui
        .scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            ui.set_width(width);
            Frame::new().inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(width - 20.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&row.title).size(TextSize::Label.points()).color(text_color))
                            .selectable(false),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(keys) = &row.keys {
                            super::shortcuts::key_caps(ui, pal, keys);
                        } else if show_family {
                            let family = family_title(&row.family);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(family).size(TextSize::Caption.points()).color(pal.muted),
                                )
                                .selectable(false),
                            );
                        }
                    });
                });
                if let Some(reason) = &row.refusal {
                    ui.add(
                        egui::Label::new(RichText::new(reason).size(TextSize::Caption.points()).color(pal.muted))
                            .selectable(false),
                    );
                }
            });
        })
        .response;
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, row.title.as_str()));
    let fill = if selected {
        pal.selected
    } else if resp.hovered() && enabled {
        pal.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().set(background, Shape::rect_filled(resp.rect, BUTTON_RADIUS, fill));
    resp
}

/// What the form asks the palette to do.
enum FormEvent {
    Nothing,
    Back,
    Done,
}

/// The arguments form of a command: heading, fields, the inline error, Back and Run.
fn show_form(ui: &mut Ui, cx: &mut Cx<'_>, form: &mut FormSession) -> FormEvent {
    let pal = cx.pal;
    let mut event = FormEvent::Nothing;
    if ui.add(egui::Button::new("Back").frame(false)).clicked() {
        event = FormEvent::Back;
    }
    ui.label(RichText::new(&form.title).font(TextSize::Display.hand()));
    if !form.summary.is_empty() {
        ui.label(RichText::new(&form.summary).size(TextSize::Small.points()).color(pal.muted));
    }
    let refusal = cx.refusal(&form.id);
    if let Some(reason) = &refusal {
        ui.label(RichText::new(reason).size(TextSize::Small.points()).color(pal.danger));
    }
    ui.add_space(4.0);
    let mut entered = false;
    ScrollArea::vertical().max_height(LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
        entered = args_form::show_fields(ui, &pal, &form.fields, &mut form.values, form.example.as_ref());
    });
    let built = args_form::build_args(&form.schema, &form.values);
    ui.add_space(8.0);
    if let Err(message) = &built {
        ui.label(RichText::new(message).size(TextSize::Small.points()).color(pal.danger));
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| match &built {
        Ok(args) => {
            let ctl = Ctl::new(&form.id).args(args.clone());
            let resp = cx.with_origin(Origin::Palette, |cx| cx.text_button(ui, &ctl, "Run", true));
            if resp.clicked() {
                event = FormEvent::Done;
            } else if entered && refusal.is_none() {
                cx.with_origin(Origin::Palette, |cx| cx.run(&form.id, args.clone()));
                event = FormEvent::Done;
            }
        }
        Err(_) => {
            ui.add_enabled(false, egui::Button::new("Run"));
        }
    });
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_needs_the_letters_in_order_and_ignores_case_and_spaces() {
        assert!(fuzzy_score("zfit", "Zoom to fit everything").is_some());
        assert!(fuzzy_score("ZOOM FIT", "zoom to fit").is_some());
        assert!(fuzzy_score("tiz", "zoom to fit").is_none());
        assert!(fuzzy_score("x", "").is_none());
        assert_eq!(fuzzy_score("  ", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_prefixes_runs_and_word_starts() {
        let prefix = fuzzy_score("und", "Undo").unwrap();
        let scattered = fuzzy_score("und", "Send to under").unwrap();
        assert!(prefix > scattered, "{prefix} > {scattered}");
        let run = fuzzy_score("fit", "zoom to fit").unwrap();
        let gaps = fuzzy_score("fit", "find it tomorrow").unwrap();
        assert!(run > gaps, "{run} > {gaps}");
    }

    #[test]
    fn family_titles_are_capitalised() {
        assert_eq!(family_title("arrange"), "Arrange");
        assert_eq!(family_title("ui"), "Interface");
        assert_eq!(family_title(""), "");
    }

    fn row(title: &str, family: &str, id: &str) -> Row {
        Row {
            title: title.to_owned(),
            family: family.to_owned(),
            id: id.to_owned(),
            keys: None,
            refusal: None,
            action: RowAction::Command { id: id.to_owned(), args: json!({}) },
        }
    }

    #[test]
    fn filter_groups_by_family_when_empty_and_ranks_when_typing() {
        let rows = vec![
            row("Undo", "history", "history.undo"),
            row("Open", "ui", "ui.open"),
            row("Redo", "history", "history.redo"),
            row("Zoom in", "view", "view.zoom_in"),
        ];
        assert_eq!(filter(&rows, ""), [0, 2, 3, 1], "families in first-seen order, ui last");
        assert_eq!(filter(&rows, "zoom"), [3]);
        assert_eq!(filter(&rows, "history"), [0, 2], "the family matches too");
        assert_eq!(filter(&rows, "view.zoom"), [3], "the id matches too");
        assert!(filter(&rows, "qqq").is_empty());
        assert_eq!(filter(&rows, "hist undo"), [0], "every word must match");
    }
}
