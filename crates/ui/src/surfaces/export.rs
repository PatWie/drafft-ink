//! The export dialog (`ui.open_export`): format, scope, scale, background, dark theme and the embedded
//! scene. Export runs `doc.export` with a download name; Copy runs `clipboard.copy_as`. The options start
//! from the `export.*` settings; changing them here changes only this export (the preferences change the
//! settings).

use drafftink_engine::spec::HostEffect;
use egui::{Context, Id, Modal, RichText};
use serde_json::{Value, json};

use crate::model::{Model, arg_doc};
use crate::state::{ExportFormat, ExportState, UiState};
use crate::surfaces::dialogs::{Close, plain_button, show_modal};
use crate::theme::TextSize;
use crate::widgets::{Ctl, Cx, section_label};

/// Scales the dialog offers (the command takes 0.25 to 4).
pub const SCALES: [f64; 3] = [1.0, 2.0, 3.0];

/// A file name for a document: its name with the characters that cannot be in a file name replaced.
pub(crate) fn file_name(document: &str, format: ExportFormat) -> String {
    let cleaned: String =
        document.trim().chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let stem = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let stem = if stem.is_empty() { "drawing" } else { stem };
    format!("{stem}.{}", format.extension())
}

/// The arguments of `doc.export` for the dialog's options. The options a format does not use are left out.
pub(crate) fn export_args(o: &ExportState, file: &str) -> Value {
    let mut args = json!({
        "format": o.format.arg(),
        "scope": if o.selection_only { "selection" } else { "all" },
        "to": { "download": file },
    });
    if let Some(map) = args.as_object_mut() {
        if o.format.is_picture() {
            map.insert("scale".into(), json!(o.scale));
            map.insert("background".into(), json!(o.background));
            map.insert("dark".into(), json!(o.dark));
        }
        if o.format.is_picture() {
            map.insert("embed_scene".into(), json!(o.embed_scene));
        }
    }
    args
}

/// The arguments of `clipboard.copy_as` for the dialog's options (a picture format).
pub(crate) fn copy_args(o: &ExportState) -> Value {
    json!({
        "format": o.format.arg(),
        "scope": if o.selection_only { "selection" } else { "all" },
        "scale": o.scale,
        "background": o.background,
        "dark": o.dark,
    })
}

/// The dialog's options as the settings give them, for a document with or without a selection.
fn options(m: &Model, format: ExportFormat) -> ExportState {
    ExportState {
        format,
        selection_only: m.selection_count > 0,
        scale: m.setting_f64("export.scale").unwrap_or(2.0),
        background: m.setting("export.background").and_then(Value::as_bool).unwrap_or(true),
        dark: m.setting_bool("export.dark"),
        embed_scene: m.setting("export.embed_scene").and_then(Value::as_bool).unwrap_or(true),
        loaded: true,
    }
}

/// Takes the options from the settings the first time the dialog is drawn.
fn load(o: &mut ExportState, m: &Model) {
    if !o.loaded {
        *o = options(m, o.format);
    }
}

/// The name of the active document.
fn active_name(m: &Model) -> String {
    m.tabs.iter().find(|t| t.active).map(|t| t.name.clone()).unwrap_or_default()
}

/// The arguments of a `doc.export` that a person started without the dialog (a menu row, `Mod+E`, the palette):
/// what the person gave, and for everything missing what the dialog would send for a document that is exported
/// at once. So every way to export a picture is the same call and writes the same file (a download on the web,
/// a file in the downloads folder on the desktop); only the `inline` default is for agents, who read the bytes.
/// Other commands and arguments that are not an object come back unchanged.
pub(crate) fn complete_args(model: &Model, id: &str, args: Value) -> Value {
    let completes_export = model.command(id).is_some_and(|c| c.host.effect == HostEffect::CompletesExport);
    let Value::Object(given) = args else {
        return args;
    };
    if !completes_export {
        return Value::Object(given);
    }
    let format = given.get("format").and_then(Value::as_str).and_then(ExportFormat::from_arg).unwrap_or_default();
    let mut full = export_args(&options(model, format), &file_name(&active_name(model), format));
    if let Some(map) = full.as_object_mut() {
        map.extend(given);
    }
    full
}

/// Draws the dialog.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(o) = st.export.as_mut() else {
        return;
    };
    load(o, cx.model);
    let document = active_name(cx.model);
    let pal = cx.pal;
    show_modal(ctx, cx, Modal::new(Id::unique("export_dialog")), "Export", 420.0, &Close::Overlays, |ui, cx| {
        section_label(ui, &pal, "Format");
        ui.horizontal_wrapped(|ui| {
            for f in ExportFormat::ALL {
                if cx.choice(ui, f.title(), o.format == f, None).clicked() {
                    o.format = f;
                }
            }
        });
        let picture = o.format.is_picture();
        // A picture option that the chosen format ignores is greyed with what the engine says the option is for.
        let off = |arg: &str| -> Option<String> {
            (!picture).then(|| arg_doc("doc.export", arg).unwrap_or_else(|| format!("`{arg}` is for pictures.")))
        };
        let (off_scale, off_background, off_dark, off_embed) =
            (off("scale"), off("background"), off("dark"), off("embed_scene"));
        section_label(ui, &pal, "What");
        let none_selected = cx.model.selection_count == 0;
        ui.horizontal(|ui| {
            // The engine's text for a command that needs a selection and has none.
            let reason = if none_selected { cx.model.refusal_text("clipboard.copy") } else { None };
            if cx.choice(ui, "Selection only", o.selection_only && !none_selected, reason.as_deref()).clicked() {
                o.selection_only = !o.selection_only;
            }
        });
        section_label(ui, &pal, "Scale");
        ui.horizontal(|ui| {
            for scale in SCALES {
                if cx.choice(ui, &format!("{scale}x"), (o.scale - scale).abs() < 0.01, off_scale.as_deref()).clicked() {
                    o.scale = scale;
                }
            }
        });
        section_label(ui, &pal, "Options");
        ui.horizontal_wrapped(|ui| {
            if cx.choice(ui, "Background", o.background, off_background.as_deref()).clicked() {
                o.background = !o.background;
            }
            if cx.choice(ui, "Dark", o.dark, off_dark.as_deref()).clicked() {
                o.dark = !o.dark;
            }
            if cx.choice(ui, "Embed scene", o.embed_scene, off_embed.as_deref()).clicked() {
                o.embed_scene = !o.embed_scene;
            }
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("File: {}", file_name(&document, o.format)))
                .size(TextSize::Small.points())
                .color(pal.muted),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
            if picture && matches!(o.format, ExportFormat::Png | ExportFormat::Svg) {
                let copy = Ctl::new("clipboard.copy_as").args(copy_args(o)).label("Copy to clipboard");
                cx.text_button(ui, &copy, "Copy", false);
            }
            let export = Ctl::new("doc.export").args(export_args(o, &file_name(&document, o.format))).label("Export");
            cx.text_button(ui, &export, "Export", true);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_has_no_path_characters_and_the_format_extension() {
        assert_eq!(file_name("Plan", ExportFormat::Png), "Plan.png");
        assert_eq!(file_name("a/b\\c", ExportFormat::Svg), "a_b_c.svg");
        assert_eq!(file_name("  ", ExportFormat::Json), "drawing.json");
        assert_eq!(file_name("..", ExportFormat::Excalidraw), "drawing.excalidraw");
    }

    #[test]
    fn a_format_without_pictures_sends_no_picture_options() {
        let o = ExportState { format: ExportFormat::Json, ..ExportState::default() };
        let args = export_args(&o, "x.json");
        assert_eq!(args, json!({"format": "json", "scope": "all", "to": {"download": "x.json"}}));
    }
}
