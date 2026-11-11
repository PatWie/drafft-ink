//! Small dialogs: the confirm box, the recent documents list, the search box and the stats panel.
//! The module also holds the helpers the other dialogs (collaboration, equation) draw with.

use egui::{
    Align, Align2, Area, Button, Context, Id, Key, Layout, Modal, Order, Response, RichText, ScrollArea, TextEdit, Ui,
    vec2,
};
use serde_json::{Value, json};

use crate::action::{Call, Verb};
use crate::icons::Icon;
use crate::model::{BoundsView, Model};
use crate::state::{SearchHit, SearchResults, UiState};
use crate::theme::{self, MARGIN, Palette, SMALL_BUTTON};
use crate::widgets::{Ctl, Cx, panel_frame};

/// Space to leave above the bottom bar for panels anchored at the bottom left.
const ABOVE_BOTTOM_BAR: f32 = 64.0;
/// Width of the stats card.
const STATS_WIDTH: f32 = 190.0;
/// Tallest the list of search results grows before it scrolls.
const RESULTS_HEIGHT: f32 = 240.0;
/// The longest label of a search result row, in characters.
const RESULT_LABEL_CHARS: usize = 48;

// ---- shared helpers ----

/// What closes a dialog (the X button and a click on the backdrop).
#[derive(Debug, Clone)]
pub(crate) enum Close {
    /// The UI verb `ui.close_overlays`.
    Overlays,
    /// A command, for a dialog that is the front of a running operation (the equation editor).
    Command {
        /// Command id.
        id: &'static str,
        /// Its arguments.
        args: Value,
    },
}

fn emit_close(cx: &mut Cx<'_>, close: &Close) {
    match close {
        Close::Overlays => cx.verb(Verb::CloseOverlays),
        Close::Command { id, args } => cx.run(id, args.clone()),
    }
}

fn close_button(ui: &mut Ui, cx: &mut Cx<'_>, close: &Close) {
    match close {
        Close::Overlays => {
            cx.verb_button(ui, Verb::CloseOverlays, Icon::Close, SMALL_BUTTON, "Close", false);
        }
        Close::Command { id, args } => {
            let ctl = Ctl::new(id).args(args.clone()).label("Close");
            cx.icon_button(ui, &ctl, Icon::Close, SMALL_BUTTON);
        }
    }
}

/// The width a dialog may have on this screen.
fn fit_width(ctx: &Context, width: f32) -> f32 {
    width.min((ctx.content_rect().width() - 2.0 * MARGIN - 40.0).max(160.0))
}

/// Draws a modal dialog: a card with a hand-written title and an X, over a dimmed backdrop. A click on the
/// backdrop does what the X does.
pub(crate) fn show_modal(
    ctx: &Context,
    cx: &mut Cx<'_>,
    modal: Modal,
    title: &str,
    width: f32,
    close: &Close,
    body: impl FnOnce(&mut Ui, &mut Cx<'_>),
) {
    let pal = cx.pal;
    let width = fit_width(ctx, width);
    let out = modal.frame(crate::widgets::dialog_frame(&pal)).backdrop_color(pal.backdrop).show(ctx, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).font(theme::hand(18.0)).color(pal.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| close_button(ui, cx, close));
        });
        ui.add_space(8.0);
        body(ui, cx);
    });
    if out.backdrop_response.clicked() {
        emit_close(cx, close);
    }
}

/// A button that does not run a command (Cancel, Copy): it only changes the UI or asks the host.
pub(crate) fn plain_button(ui: &mut Ui, pal: &Palette, text: &str, primary: bool) -> Response {
    let (fill, fg) = if primary { (pal.accent, pal.on_accent) } else { (pal.button, pal.text) };
    ui.add(
        Button::new(RichText::new(text).size(13.0).color(fg))
            .fill(fill)
            .corner_radius(theme::BUTTON_RADIUS)
            .min_size(vec2(80.0, 32.0)),
    )
}

/// A single-line text field that is greyed, with the refusal as its tooltip, when `reason` is set.
pub(crate) fn text_field(
    ui: &mut Ui,
    buf: &mut String,
    hint: &str,
    reason: Option<String>,
    char_limit: Option<usize>,
    label: &str,
) -> Response {
    let mut edit = TextEdit::singleline(buf).hint_text(hint).desired_width(f32::INFINITY);
    if let Some(n) = char_limit {
        edit = edit.char_limit(n);
    }
    let resp = ui.add_enabled(reason.is_none(), edit);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, reason.is_none(), label));
    match reason {
        Some(r) => resp.on_disabled_hover_text(r),
        None => resp,
    }
}

/// True when Enter was pressed in this frame (used with `lost_focus` of a single-line field).
pub(crate) fn enter_pressed(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(Key::Enter))
}

fn muted(pal: &Palette, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(12.0).color(pal.muted)
}

/// Draws the dialogs of this file.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    confirm(ctx, cx, st);
    recent(ctx, cx, st);
    search(ctx, cx, st);
    stats(ctx, cx, st);
}

// ---- confirm ----

/// The refused call again, with the user's decision filled in as `on_unsaved`.
pub(crate) fn unsaved_call(call: &Call, decision: &str) -> Call {
    let mut args = match &call.args {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    args.insert("on_unsaved".to_owned(), Value::from(decision));
    Call::new(call.origin, call.id.clone(), Value::Object(args))
}

fn confirm(ctx: &Context, cx: &mut Cx<'_>, st: &UiState) {
    let Some(c) = st.confirm.as_ref() else {
        return;
    };
    let pal = cx.pal;
    let (call, message) = (&c.call, &c.message);
    let id = Id::unique("confirm_dialog");
    show_modal(ctx, cx, Modal::new(id), "Unsaved changes", 380.0, &Close::Overlays, |ui, cx| {
        ui.label(RichText::new(message).size(13.0).color(pal.text));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let discard = unsaved_call(call, "discard");
            let save = unsaved_call(call, "save");
            // The repeat comes from the same surface as the call that was refused.
            cx.with_origin(call.origin, |cx| {
                let ctl = Ctl::new(&discard.id).args(discard.args.clone());
                if cx.text_button(ui, &ctl, "Discard changes", false).clicked() {
                    cx.verb(Verb::CloseOverlays);
                }
                let ctl = Ctl::new(&save.id).args(save.args.clone());
                if cx.text_button(ui, &ctl, "Save first", true).clicked() {
                    cx.verb(Verb::CloseOverlays);
                }
            });
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(Verb::CloseOverlays);
            }
        });
    });
}

// ---- recent documents ----

/// One entry of the `doc.recent` setting, ready to show.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recent {
    /// The file reference exactly as stored; it is the `source` of `doc.open`.
    pub source: Value,
    /// The short name shown in the list.
    pub name: String,
    /// The full description: the control's name and tooltip.
    pub full: String,
}

/// Reads the `doc.recent` setting. Entries that are not a file reference are skipped.
pub(crate) fn recent_entries(setting: Option<&Value>) -> Vec<Recent> {
    let Some(list) = setting.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter().filter_map(recent_entry).collect()
}

fn recent_entry(v: &Value) -> Option<Recent> {
    let obj = v.as_object()?;
    let text = |key: &str| obj.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
    let (name, full) = if let Some(path) = text("path") {
        (path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned(), path.to_owned())
    } else if let Some(stored) = text("stored") {
        (stored.to_owned(), stored.to_owned())
    } else if let Some(file) = text("download") {
        (file.to_owned(), file.to_owned())
    } else if text("handle").is_some() {
        ("Opened file".to_owned(), "Opened file (browser handle)".to_owned())
    } else {
        return None;
    };
    Some(Recent { source: v.clone(), name, full })
}

/// The arguments of `doc.open` for a recent entry.
pub(crate) fn open_recent_args(source: &Value) -> Value {
    json!({ "source": source, "mode": "replace" })
}

fn recent(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.recent_open {
        return;
    }
    let pal = cx.pal;
    let entries = recent_entries(cx.model.setting("doc.recent"));
    let max_h = (ctx.content_rect().height() * 0.6).max(120.0);
    let mut picked = false;
    show_modal(
        ctx,
        cx,
        Modal::new(Id::unique("recent_dialog")),
        "Recent documents",
        360.0,
        &Close::Overlays,
        |ui, cx| {
            if entries.is_empty() {
                ui.label(muted(&pal, "No recent documents yet."));
                return;
            }
            ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                for e in &entries {
                    let ctl = Ctl::new("doc.open").args(open_recent_args(&e.source)).label(&e.full);
                    if cx.menu_item(ui, &ctl, &e.name).clicked() {
                        picked = true;
                    }
                }
            });
        },
    );
    if picked {
        // Not `ui.close_overlays`: a refusal (unsaved changes) opens the confirm box, which must stay.
        st.recent_open = false;
    }
}

// ---- search ----

/// The arguments of `element.query` for a search text.
pub(crate) fn query_args(text: &str) -> Value {
    json!({ "filter": { "text": text.trim() } })
}

#[derive(Clone, Default)]
struct SearchDraft {
    text: String,
    focused: bool,
}

/// The text of a result row: what the element says (or its name, or its id) and its kind.
pub(crate) fn result_row_text(hit: &SearchHit) -> String {
    let flat = hit.label.split_whitespace().collect::<Vec<_>>().join(" ");
    let label: String = if flat.chars().count() > RESULT_LABEL_CHARS {
        flat.chars().take(RESULT_LABEL_CHARS).chain(std::iter::once('~')).collect()
    } else {
        flat
    };
    format!("{label} ({})", hit.kind)
}

/// The line above the result list.
pub(crate) fn results_summary(found: &SearchResults) -> String {
    match (found.total, found.hits.len()) {
        (0, _) => format!("Nothing contains \"{}\".", found.query),
        (1, _) => "1 element found.".to_owned(),
        (total, shown) if total > shown => format!("{total} elements found, showing the first {shown}."),
        (total, _) => format!("{total} elements found."),
    }
}

/// The list of what the last search found. Choosing a row selects the element and centres the view on it.
fn results(ui: &mut Ui, cx: &mut Cx<'_>, found: &SearchResults) {
    ui.add_space(6.0);
    ui.label(muted(&cx.pal, results_summary(found)));
    ScrollArea::vertical().max_height(RESULTS_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
        for hit in &found.hits {
            let text = result_row_text(hit);
            let ctl = Ctl::new("selection.select").args(json!({ "ids": [hit.id] })).label(&text);
            // The row runs `selection.select`; the view then moves to the element.
            if cx.menu_item(ui, &ctl, &text).clicked() {
                if let Some(point) = hit.center {
                    cx.run("view.center", json!({ "on": "point", "point": point }));
                }
                cx.verb(Verb::CloseOverlays);
            }
        }
    });
}

fn search(ctx: &Context, cx: &mut Cx<'_>, st: &UiState) {
    let key = Id::unique("search_draft");
    if !st.search_open {
        ctx.data_mut(|d| d.remove_temp::<SearchDraft>(key));
        return;
    }
    let pal = cx.pal;
    let mut draft: SearchDraft = ctx.data_mut(|d| d.get_temp(key)).unwrap_or_default();
    let reason = cx.model.refusal_text("element.query");
    let id = Id::unique("search_dialog");
    let modal = Modal::new(id).area(Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 96.0)));
    show_modal(ctx, cx, modal, "Search", 420.0, &Close::Overlays, |ui, cx| {
        let resp = text_field(ui, &mut draft.text, "Find elements by their text", reason.clone(), None, "Search text");
        if reason.is_none() && !draft.focused {
            resp.request_focus();
            draft.focused = true;
        }
        ui.add_space(6.0);
        match &reason {
            Some(r) => {
                ui.label(RichText::new(r).size(12.0).color(pal.danger));
            }
            None => {
                ui.label(muted(&pal, "Press Enter to search."));
            }
        }
        if reason.is_none() && resp.lost_focus() && enter_pressed(ui) && !draft.text.trim().is_empty() {
            cx.run("element.query", query_args(&draft.text));
            // Stay in the field, so the search can be refined.
            resp.request_focus();
        }
        if let Some(found) = &st.search_results {
            results(ui, cx, found);
        }
    });
    ctx.data_mut(|d| d.insert_temp(key, draft));
}

// ---- stats ----

/// The four fields of the bounds editor: the caption, the accessible name and the argument of
/// `element.set_bounds`.
const BOUNDS_FIELDS: [(&str, &str, &str); 4] = [
    ("X", "Selection X", "x"),
    ("Y", "Selection Y", "y"),
    ("W", "Selection width", "w"),
    ("H", "Selection height", "h"),
];
/// Width of one bounds field.
const BOUNDS_FIELD_WIDTH: f32 = 52.0;

/// A coordinate as the field shows it: at most two decimals, no trailing zeros.
pub(crate) fn bounds_text(v: f64) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    // Never "-0".
    format!("{}", if rounded == 0.0 { 0.0 } else { rounded })
}

/// The arguments of `element.set_bounds` for an edited field, or `None` when the text is not a number
/// the command accepts or does not change the value the field shows. Only the edited value is sent, so
/// the rest of the box stays as it is.
pub(crate) fn bounds_args(key: &str, text: &str, now: BoundsView) -> Option<Value> {
    let v: f64 = text.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
    let current = match key {
        "x" => now.x,
        "y" => now.y,
        "w" => now.w,
        _ => now.h,
    };
    let size = matches!(key, "w" | "h");
    ((v - current).abs() >= 0.005 && (!size || v > 0.0)).then(|| json!({ key: v }))
}

/// What the bounds fields hold while the panel is open.
#[derive(Clone, Default)]
struct BoundsDraft {
    /// The bounds the texts were made from. A different bounds of the model reloads the texts.
    shown: Option<BoundsView>,
    text: [String; 4],
}

impl BoundsDraft {
    fn load(b: BoundsView) -> Self {
        Self { shown: Some(b), text: [b.x, b.y, b.w, b.h].map(bounds_text) }
    }
}

/// Position and size of the selection, edited with `element.set_bounds`. A value is sent when a field
/// loses focus (Enter or a click elsewhere), never while typing; Esc drops the edit.
fn bounds_editor(ui: &mut Ui, cx: &mut Cx<'_>, now: BoundsView) {
    let pal = cx.pal;
    let key = Id::unique("stats_bounds_draft");
    let mut draft: BoundsDraft = ui.data_mut(|d| d.get_temp(key)).unwrap_or_default();
    if draft.shown != Some(now) {
        draft = BoundsDraft::load(now);
    }
    let reason = cx.refusal("element.set_bounds");
    let escaped = ui.input(|i| i.key_pressed(Key::Escape));
    let mut reload = false;
    ui.add_space(4.0);
    egui::Grid::new(Id::unique("stats_bounds_grid")).num_columns(4).spacing(vec2(6.0, 3.0)).show(ui, |ui| {
        for (i, ((caption, label, arg), text)) in BOUNDS_FIELDS.iter().zip(draft.text.iter_mut()).enumerate() {
            ui.label(muted(&pal, *caption));
            let resp = ui
                .allocate_ui(vec2(BOUNDS_FIELD_WIDTH, 20.0), |ui| {
                    text_field(ui, text, "", reason.clone(), Some(12), label)
                })
                .inner;
            if reason.is_none() && resp.lost_focus() {
                if !escaped && let Some(args) = bounds_args(arg, text, now) {
                    cx.run("element.set_bounds", args);
                }
                reload = true;
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if reload {
        draft.shown = None;
    }
    ui.data_mut(|d| d.insert_temp(key, draft));
}

/// The rows of the stats panel: a name and the value, read from the model.
pub(crate) fn stats_rows(m: &Model) -> Vec<(&'static str, String)> {
    let label = |l: &Option<String>| l.clone().unwrap_or_else(|| "nothing".to_owned());
    vec![
        ("Selected", m.selection_count.to_string()),
        ("Tool", m.tool.name().to_owned()),
        ("Zoom", format!("{:.0}%", m.zoom_percent)),
        ("Undo", label(&m.history.undo_label)),
        ("Redo", label(&m.history.redo_label)),
        ("View only", if m.view_only { "yes" } else { "no" }.to_owned()),
        ("Tabs", m.tabs.len().to_string()),
    ]
}

fn stats(ctx: &Context, cx: &mut Cx<'_>, st: &UiState) {
    if !st.stats_open || st.zen {
        return;
    }
    let pal = cx.pal;
    let rows = stats_rows(cx.model);
    Area::new(Id::unique("stats_panel"))
        .anchor(Align2::LEFT_BOTTOM, vec2(MARGIN, -ABOVE_BOTTOM_BAR))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.set_width(STATS_WIDTH);
                ui.label(RichText::new("Stats").font(theme::hand(14.0)).color(pal.muted));
                egui::Grid::new(Id::unique("stats_grid")).num_columns(2).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
                    for (name, value) in &rows {
                        ui.label(muted(&pal, *name));
                        ui.label(RichText::new(value).size(12.0).color(pal.text));
                        ui.end_row();
                    }
                });
                if let Some(bounds) = cx.model.selection_bounds {
                    bounds_editor(ui, cx, bounds);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use drafftink_engine::Origin;

    use super::*;

    #[test]
    fn unsaved_call_merges_the_decision_and_keeps_origin() {
        let call = Call::new(Origin::Shortcut, "doc.new", json!({"in_new_tab": false}));
        let again = unsaved_call(&call, "discard");
        assert_eq!(again.id, "doc.new");
        assert_eq!(again.origin, Origin::Shortcut);
        assert_eq!(again.args, json!({"in_new_tab": false, "on_unsaved": "discard"}));
        let none = Call::new(Origin::Ui, "doc.new", Value::Null);
        assert_eq!(unsaved_call(&none, "save").args, json!({"on_unsaved": "save"}));
    }

    #[test]
    fn recent_entries_read_every_file_reference_and_skip_junk() {
        let v = json!([
            {"path": "/home/me/plan.drafftink"},
            {"path": "C:\\docs\\b.json"},
            {"stored": "Sketch"},
            {"download": "out.drafftink"},
            {"handle": "h1"},
            {"nope": 1},
            7,
            {"path": ""},
            null
        ]);
        let got = recent_entries(Some(&v));
        let names: Vec<_> = got.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["plan.drafftink", "b.json", "Sketch", "out.drafftink", "Opened file"]);
        assert_eq!(got[0].source, json!({"path": "/home/me/plan.drafftink"}));
        assert_eq!(got[0].full, "/home/me/plan.drafftink");
        assert!(recent_entries(Some(&json!("not a list"))).is_empty());
        assert!(recent_entries(None).is_empty());
    }

    #[test]
    fn query_and_open_arguments() {
        assert_eq!(query_args("  box "), json!({"filter": {"text": "box"}}));
        let src = json!({"stored": "A"});
        assert_eq!(open_recent_args(&src), json!({"source": {"stored": "A"}, "mode": "replace"}));
    }

    #[test]
    fn bounds_text_and_arguments() {
        let now = BoundsView { x: 10.0, y: 20.0, w: 100.0, h: 60.0 };
        assert_eq!(bounds_text(10.0), "10");
        assert_eq!(bounds_text(10.126), "10.13");
        assert_eq!(bounds_text(-0.004), "0");
        assert_eq!(bounds_args("x", " 12 ", now), Some(json!({"x": 12.0})));
        assert_eq!(bounds_args("x", "10", now), None);
        assert_eq!(bounds_args("x", "10.001", now), None, "below the precision the field shows");
        assert_eq!(bounds_args("y", "-3", now), Some(json!({"y": -3.0})), "positions may be negative");
        assert_eq!(bounds_args("w", "0", now), None);
        assert_eq!(bounds_args("h", "-1", now), None);
        assert_eq!(bounds_args("h", "61", now), Some(json!({"h": 61.0})));
        assert_eq!(bounds_args("w", "1e999", now), None);
        assert_eq!(bounds_args("w", "NaN", now), None);
    }

    #[test]
    fn result_rows_and_summary() {
        let hit = |label: &str| SearchHit { id: "e1".into(), kind: "text".into(), label: label.into(), center: None };
        assert_eq!(result_row_text(&hit("Login\n  box")), "Login box (text)");
        let long = "x".repeat(60);
        assert_eq!(result_row_text(&hit(&long)), format!("{}~ (text)", "x".repeat(RESULT_LABEL_CHARS)));
        let found = |total, n| SearchResults { query: "q".into(), total, hits: vec![hit("a"); n] };
        assert_eq!(results_summary(&found(0, 0)), "Nothing contains \"q\".");
        assert_eq!(results_summary(&found(1, 1)), "1 element found.");
        assert_eq!(results_summary(&found(7, 7)), "7 elements found.");
        assert_eq!(results_summary(&found(250, 100)), "250 elements found, showing the first 100.");
    }

    #[test]
    fn stats_rows_show_the_model() {
        let mut m = Model::new(false);
        m.selection_count = 3;
        m.zoom_percent = 149.6;
        m.view_only = true;
        m.history.undo_label = Some("Move".to_owned());
        let rows = stats_rows(&m);
        let get = |k: &str| rows.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("Selected"), Some("3"));
        assert_eq!(get("Zoom"), Some("150%"));
        assert_eq!(get("View only"), Some("yes"));
        assert_eq!(get("Undo"), Some("Move"));
        assert_eq!(get("Redo"), Some("nothing"));
        assert_eq!(get("Tool"), Some("select"));
        assert_eq!(get("Tabs"), Some("0"));
    }
}
