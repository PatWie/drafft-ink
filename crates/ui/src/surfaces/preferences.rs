//! The preferences dialog (`ui.open_preferences`): the settings of docs/commands.md §5 that a person
//! changes, each as a `settings.set` or `settings.toggle` call, plus the collaboration profile
//! (`collab.set_user`) and the layout choice (UI only: automatic, touch or desktop).

use egui::{Context, Id, Modal, ScrollArea, Ui};
use serde_json::{Value, json};

use crate::state::{LayoutPreference, UiState};
use crate::surfaces::dialogs::{Close, show_modal};
use crate::theme::{TextSize, UiFontChoice};
use crate::widgets::{Ctl, Cx, section_label, separator};

const GRID_STYLES: [(&str, &str); 5] = [
    ("None", "none"),
    ("Lines", "lines"),
    ("Horizontal", "horizontal_lines"),
    ("Crosses", "crosses"),
    ("Dots", "dots"),
];
const THEMES: [(&str, &str); 3] = [("Light", "light"), ("Dark", "dark"), ("System", "system")];
const ANGLE_STEPS: [f64; 5] = [5.0, 10.0, 15.0, 30.0, 45.0];
const EXPORT_SCALES: [f64; 4] = [1.0, 2.0, 3.0, 4.0];
const EXPORT_PADDINGS: [f64; 4] = [0.0, 10.0, 20.0, 50.0];
const AUTOSAVE_SECONDS: [u64; 5] = [1, 5, 10, 30, 60];
const HISTORY_DEPTHS: [u64; 5] = [50, 100, 200, 500, 1000];
const NUDGE_STEPS: [f64; 4] = [1.0, 5.0, 10.0, 20.0];
const BOX_SELECT: [(&str, &str); 2] = [("Touching", "overlap"), ("Inside", "contain")];

/// The call that sets a key to a value.
pub(crate) fn set_args(key: &str, value: impl Into<Value>) -> Value {
    json!({ "key": key, "value": value.into() })
}

/// The call that flips a boolean key.
pub(crate) fn toggle_args(key: &str) -> Value {
    json!({ "key": key })
}

fn row_of_values(ui: &mut Ui, cx: &mut Cx<'_>, title: &str, key: &str, options: &[(String, Value)]) {
    section_label(ui, &cx.pal, title);
    let current = cx.model.setting(key).cloned();
    ui.horizontal_wrapped(|ui| {
        for (label, value) in options {
            let selected = match (&current, value) {
                (Some(Value::Number(a)), Value::Number(b)) => a.as_f64() == b.as_f64(),
                (Some(a), b) => a == b,
                (None, _) => false,
            };
            let ctl = Ctl::new("settings.set")
                .args(set_args(key, value.clone()))
                .label(format!("{title}: {label}"))
                .selected(selected);
            cx.toggle_button(ui, &ctl, label);
        }
    });
}

/// The font of menus, bars and dialogs: one button per bundled font, each in its own face.
fn font_row(ui: &mut Ui, cx: &mut Cx<'_>) {
    section_label(ui, &cx.pal, "Interface font");
    let current = UiFontChoice::of_model(cx.model);
    ui.horizontal_wrapped(|ui| {
        for choice in UiFontChoice::ALL.into_iter().filter(|c| c.available()) {
            let ctl = Ctl::new("settings.set")
                .args(set_args("ui.font", choice.setting()))
                .label(format!("Interface font: {}", choice.title()))
                .selected(current == choice);
            cx.toggle_button_font(ui, &ctl, choice.title(), choice.preview(TextSize::Body.points()));
        }
    });
}

fn strings(options: &[(&str, &str)]) -> Vec<(String, Value)> {
    options.iter().map(|(l, v)| ((*l).to_owned(), json!(v))).collect()
}

fn numbers(options: &[f64], suffix: &str) -> Vec<(String, Value)> {
    options.iter().map(|n| (format!("{n}{suffix}"), json!(n))).collect()
}

fn whole(options: &[u64], suffix: &str) -> Vec<(String, Value)> {
    options.iter().map(|n| (format!("{n}{suffix}"), json!(n))).collect()
}

fn switches(ui: &mut Ui, cx: &mut Cx<'_>, items: &[(&str, &str, bool)]) {
    ui.horizontal_wrapped(|ui| {
        for &(label, key, default) in items {
            let on = cx.model.setting(key).and_then(Value::as_bool).unwrap_or(default);
            let ctl = Ctl::new("settings.toggle").args(toggle_args(key)).label(label).selected(on);
            cx.toggle_button(ui, &ctl, label);
        }
    });
}

/// What the profile's text field holds between frames.
#[derive(Clone, Default)]
struct ProfileDraft {
    loaded: bool,
    name: String,
    sent: String,
}

/// Draws the dialog.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let key = Id::unique("preferences_profile");
    if !st.preferences_open {
        ctx.data_mut(|d| d.remove_temp::<ProfileDraft>(key));
        return;
    }
    let mut draft: ProfileDraft = ctx.data_mut(|d| d.get_temp(key)).unwrap_or_default();
    if !draft.loaded {
        draft.loaded = true;
        draft.name = cx.model.setting_str("collab.user_name").unwrap_or_default().to_owned();
        draft.sent = draft.name.clone();
    }
    let layout = &mut st.layout;
    let height = (ctx.content_rect().height() - 140.0).max(200.0);
    show_modal(
        ctx,
        cx,
        Modal::new(Id::unique("preferences_dialog")),
        "Preferences",
        460.0,
        &Close::Overlays,
        |ui, cx| {
            ScrollArea::vertical().max_height(height).show(ui, |ui| body(ui, cx, layout, &mut draft));
        },
    );
    ctx.data_mut(|d| d.insert_temp(key, draft));
}

fn body(ui: &mut Ui, cx: &mut Cx<'_>, layout: &mut LayoutPreference, draft: &mut ProfileDraft) {
    let pal = cx.pal;
    row_of_values(ui, cx, "Theme", "theme", &strings(&THEMES));
    font_row(ui, cx);
    section_label(ui, &pal, "Layout");
    ui.horizontal(|ui| {
        for choice in LayoutPreference::ALL {
            if cx.choice(ui, choice.title(), *layout == choice, None).clicked() {
                *layout = choice;
            }
        }
    });
    separator(ui, &pal);

    row_of_values(ui, cx, "Grid", "view.grid_style", &strings(&GRID_STYLES));
    switches(ui, cx, &[("Minimap", "view.minimap", false)]);
    section_label(ui, &pal, "Snapping");
    switches(
        ui,
        cx,
        &[
            ("Snap to grid", "snap.grid", false),
            ("Smart guides", "snap.smart_guides", false),
            ("Angle snap", "snap.angle", false),
        ],
    );
    row_of_values(ui, cx, "Angle step", "snap.angle_step", &numbers(&ANGLE_STEPS, " deg"));
    row_of_values(ui, cx, "Arrow keys move by", "input.nudge_step", &numbers(&NUDGE_STEPS, ""));
    row_of_values(ui, cx, "Box select takes what is", "input.box_select", &strings(&BOX_SELECT));
    section_label(ui, &pal, "Arrows and frames");
    switches(
        ui,
        cx,
        &[
            ("Arrows bind to shapes", "arrow.binding", true),
            ("Snap to side midpoints", "arrow.midpoint_snap", false),
            ("Clip frame contents", "frame.clip", true),
            ("Frame names", "frame.show_names", true),
            ("Frame outline", "frame.outline", true),
        ],
    );
    separator(ui, &pal);

    row_of_values(ui, cx, "Export scale", "export.scale", &numbers(&EXPORT_SCALES, "x"));
    row_of_values(ui, cx, "Export padding", "export.padding", &numbers(&EXPORT_PADDINGS, ""));
    section_label(ui, &pal, "Export options");
    switches(
        ui,
        cx,
        &[
            ("Background", "export.background", true),
            ("Dark", "export.dark", false),
            ("Embed scene", "export.embed_scene", true),
        ],
    );
    separator(ui, &pal);

    section_label(ui, &pal, "Saving and history");
    switches(
        ui,
        cx,
        &[("Autosave", "autosave.enabled", true), ("Copy for Excalidraw too", "clipboard.excalidraw_compat", true)],
    );
    row_of_values(ui, cx, "Autosave every", "autosave.interval_s", &whole(&AUTOSAVE_SECONDS, " s"));
    row_of_values(ui, cx, "Undo steps kept", "history.depth", &whole(&HISTORY_DEPTHS, ""));
    separator(ui, &pal);

    section_label(ui, &pal, "Collaboration");
    crate::surfaces::collab::profile(ui, cx, &mut draft.name, &mut draft.sent);
    agents(ui, cx);
}

/// The MCP server over HTTP: a switch, and where to connect once it runs. Only where the app can serve one.
fn agents(ui: &mut Ui, cx: &mut Cx<'_>) {
    let Some(server) = cx.model.agent_server.clone() else { return };
    let pal = cx.pal;
    separator(ui, &pal);
    section_label(ui, &pal, "Agents");
    switches(ui, cx, &[("Serve MCP over HTTP", "agent.mcp_http", false)]);
    match (&server.endpoint, &server.problem) {
        (Some(endpoint), _) => {
            ui.label(format!("Listening on {endpoint} (this computer only)."));
            if let Some(file) = &server.token_file {
                ui.label(format!("Clients send the token in {file} as a bearer token. docs/agents.md has setups."));
            }
        }
        (None, Some(problem)) => {
            ui.colored_label(pal.danger, problem);
        }
        (None, None) => {
            ui.label("Off. When on, MCP clients on this computer can draw here with the token the app writes to a private file.");
        }
    }
}
