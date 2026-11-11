//! `style`: stroke, fill, font and the other look properties, the defaults for new elements, and themes
//! (docs/commands.md §2.7).
//!
//! `style.set` replaces the old `SetStrokeColor`, `SetFillColor`, `SetStrokeWidth`, `SetCornerRadius`,
//! `SetSloppiness`, `SetFillPattern`, `SetStrokeStyle`, `SetOpacity` and the font actions. The rules (which
//! property applies to which kind, ranges, bound labels, the defaults, the themes) are in
//! `drafftink_edit::style`; this file maps arguments to those functions and shapes the output.
//!
//! * The defaults for new elements live in `Session::tool` (state class T, rolled back with a refusal or a
//!   cancelled gesture). The persisted copy is the setting `style.defaults`; it is written last and never by a
//!   live gesture frame, so a cancelled drag leaves it alone.
//! * `style.set` is a gesture command: a slider or a colour scrub runs `gesture.begin`, `gesture.update` and
//!   `gesture.end`, and the result is one undo step.
//! * Fonts change the stored text size only through this command's properties; the text box is not re-measured
//!   here (see the module docs of `drafftink_edit::style`).

use std::collections::BTreeMap;

use drafftink_edit::style::{
    Prop, StyleError, StyleProps, StyleReport, Theme, apply, read, theme, theme_names, themes,
};
use drafftink_scene::{Color, Element, ElementId, Head, Kind, SceneRead, SceneTx};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::element_common::target_refusal;
use crate::refusal::{Refusal, echo};
use crate::session::{Ref, Session};
use crate::spec::{Host};
use crate::spec::{
    CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// The key of the persisted defaults in the settings.
pub const DEFAULTS_KEY: &str = "style.defaults";

/// Where `style.set` and `style.apply_theme` write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// With a target (given `ids`, else the selection): the target and the defaults. Without one: the defaults.
    #[default]
    Auto,
    /// The target only. Refuses with `no_target` when there is none.
    Selection,
    /// The defaults for new elements only.
    Defaults,
    /// The target and the defaults. Refuses with `no_target` when there is no target.
    Both,
}

/// Where `style.get` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GetScope {
    /// The target if there is one, else the defaults.
    #[default]
    Auto,
    /// The target only. Refuses with `no_target` when there is none.
    Selection,
    /// The defaults for new elements.
    Defaults,
}

fn yes() -> bool {
    true
}

// ---- shared plumbing ----

/// Who a style call is about.
struct Plan {
    targets: Vec<ElementId>,
    defaults: bool,
}

fn plan(s: &Session, ids: Option<&[Ref]>, scope: Scope) -> Result<Plan, Refusal> {
    let id = s.cmd();
    match scope {
        Scope::Defaults => {
            if ids.is_some() {
                return Err(Refusal::bad_arg(
                    id,
                    "ids",
                    "is not used with scope \"defaults\". Omit `ids`, or use scope \"selection\" or \"both\"",
                ));
            }
            Ok(Plan { targets: Vec::new(), defaults: true })
        }
        Scope::Selection => Ok(Plan { targets: s.targets(ids)?, defaults: false }),
        Scope::Both => Ok(Plan { targets: s.targets(ids)?, defaults: true }),
        Scope::Auto => match (ids, s.targets(None)) {
            (Some(_), _) => Ok(Plan { targets: s.targets(ids)?, defaults: true }),
            (None, Ok(targets)) => Ok(Plan { targets, defaults: true }),
            (None, Err(_)) => Ok(Plan { targets: Vec::new(), defaults: true }),
        },
    }
}

fn refusal(cmd: &str, e: StyleError) -> Refusal {
    match e {
        StyleError::Empty => Refusal::bad_arg(
            cmd,
            "props",
            "give at least one style property, for example {\"stroke_color\": \"blue-500\"}",
        ),
        StyleError::OutOfRange { prop, expected } => Refusal::bad_arg(cmd, &format!("props.{}", prop.name()), expected),
        StyleError::Target(t) => target_refusal(cmd, t),
        StyleError::NotApplicable { props } => {
            let named = props.iter().map(|p| format!("`{}`", p.name())).collect::<Vec<_>>().join(", ");
            let mut kinds: Vec<&str> = Vec::new();
            for p in &props {
                if !kinds.contains(&p.applies_to()) {
                    kinds.push(p.applies_to());
                }
            }
            Refusal::not_applicable(cmd, &named, &kinds.join("; "))
        }
        StyleError::Store(e) => Refusal::internal(cmd, e.to_string()),
    }
}

/// The properties that were skipped for an element.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Skipped {
    /// The element.
    pub id: String,
    /// The properties it does not support.
    pub props: Vec<String>,
}

fn names(set: &std::collections::BTreeSet<Prop>) -> Vec<String> {
    set.iter().map(|p| p.name().to_owned()).collect()
}

struct Done {
    changed: bool,
    applied: BTreeMap<String, Vec<String>>,
    skipped: Vec<Skipped>,
    defaults_updated: bool,
    background_set: bool,
}

/// Applies `props` as `plan` says. Validates first, edits the document, then the defaults, then the setting.
fn run(
    s: &mut Session,
    props: &StyleProps,
    plan: &Plan,
    include_bound_text: bool,
    background: Option<Color>,
) -> Result<Done, Refusal> {
    let cmd = s.cmd();
    props.validate().map_err(|e| refusal(cmd, e))?;

    let mut report = StyleReport::default();
    let mut background_changed = false;
    if !plan.targets.is_empty() || background.is_some() {
        let (r, _) = s
            .tab_mut()
            .doc
            .edit::<(StyleReport, bool), StyleError>(|tx| {
                let report = if plan.targets.is_empty() {
                    StyleReport::default()
                } else {
                    apply(tx, &plan.targets, props, include_bound_text)?
                };
                let mut bg = false;
                if let Some(c) = background
                    && tx.meta().background != c
                {
                    tx.set_background(c);
                    bg = true;
                }
                Ok((report, bg))
            })
            .map_err(|e| refusal(cmd, e))?;
        report = r.0;
        background_changed = r.1;
    }

    let mut defaults_changed = false;
    if plan.defaults {
        let mut d = s.tool.defaults();
        defaults_changed = d.apply(props);
        if defaults_changed {
            s.tool.set_defaults(d);
        }
        // Gesture frames are rolled back by the gesture machinery, which does not roll settings back, so only a
        // final run persists the defaults.
        if !s.live() {
            let stored = serde_json::to_value(s.tool.defaults().to_props()).unwrap_or(Value::Null);
            s.settings.set_raw(DEFAULTS_KEY, stored);
        }
    }

    Ok(Done {
        changed: !report.changed.is_empty() || background_changed || defaults_changed,
        applied: report.applied.iter().map(|(id, set)| (id.as_str().to_owned(), names(set))).collect(),
        skipped: report
            .skipped
            .iter()
            .map(|(id, set)| Skipped { id: id.as_str().to_owned(), props: names(set) })
            .collect(),
        defaults_updated: plan.defaults,
        background_set: background_changed,
    })
}

// ---- style.set ----

/// Arguments of `style.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The properties to set. At least one. Absent properties stay as they are; `fill_color: null` removes the fill.
    pub props: StyleProps,
    /// The elements to change. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Where to write: the elements, the defaults for new elements, or both. `auto` writes both when there is a
    /// target and the defaults alone when there is none.
    #[serde(default)]
    pub scope: Scope,
    /// Text properties of a shape or arrow also reach its label.
    #[serde(default = "yes")]
    pub include_bound_text: bool,
}

/// Output of `style.set`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetOut {
    /// False when everything already had the requested values.
    pub changed: bool,
    /// The properties each element took, by element id. A label reached through its container is listed under its
    /// own id.
    pub applied: BTreeMap<String, Vec<String>>,
    /// The properties an element does not support.
    pub skipped: Vec<Skipped>,
    /// True when the defaults for new elements were part of this call.
    pub defaults_updated: bool,
}

fn set(s: &mut Session, a: SetArgs) -> Result<SetOut, Refusal> {
    let p = plan(s, a.ids.as_deref(), a.scope)?;
    let d = run(s, &a.props, &p, a.include_bound_text, None)?;
    Ok(SetOut { changed: d.changed, applied: d.applied, skipped: d.skipped, defaults_updated: d.defaults_updated })
}

// ---- style.get ----

/// Arguments of `style.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetArgs {
    /// The elements to read. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// Read the elements, the defaults, or the elements if there are any and the defaults if not.
    pub scope: GetScope,
    /// The properties to report, by name. Default: all of them.
    pub props: Option<Vec<String>>,
}

/// One property over the targets.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PropInfo {
    /// True when at least one target supports the property.
    pub applicable: bool,
    /// The common value, or the word `"mixed"` when the supporting targets differ. Absent when not applicable. A
    /// removed fill is `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Why the property cannot be set now: the text `style.set` refuses with for this property alone. Only when
    /// the targets are elements and none of them supports it. A control greys itself with this text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The heads at the two ends of the lines and arrows among the targets. Heads are not style properties
/// (`arrow.set_head` writes them), so they are reported beside `props`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct HeadsInfo {
    /// The start head, as the name `arrow.set_head` takes (`none`, `arrow`, `bar`, ...), or `"mixed"` when the
    /// lines and arrows differ.
    pub start: String,
    /// The end head, in the same words.
    pub end: String,
}

/// The common head of one end, or `"mixed"`.
fn common_head(heads: impl Iterator<Item = Head>) -> String {
    let mut heads = heads;
    let Some(first) = heads.next() else { return "mixed".to_owned() };
    if heads.all(|h| h == first) { head_name(first) } else { "mixed".to_owned() }
}

/// The snake_case name of a head, as it is serialised.
fn head_name(head: Head) -> String {
    match serde_json::to_value(head) {
        Ok(Value::String(name)) => name,
        _ => "none".to_owned(),
    }
}

/// The heads over the lines and arrows among `targets`; `None` when there are none.
fn heads_of(s: &Session, targets: &[ElementId]) -> Option<HeadsInfo> {
    let store = s.store();
    let linear: Vec<(Head, Head)> = targets
        .iter()
        .filter_map(|id| store.get(id))
        .filter_map(Element::linear_data)
        .map(|d| (d.start_head, d.end_head))
        .collect();
    if linear.is_empty() {
        return None;
    }
    Some(HeadsInfo {
        start: common_head(linear.iter().map(|h| h.0)),
        end: common_head(linear.iter().map(|h| h.1)),
    })
}

/// Output of `style.get`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GetOut {
    /// Always false: reading changes nothing.
    pub changed: bool,
    /// `selection` (the targets were read) or `defaults`.
    pub source: String,
    /// The elements that were read.
    pub ids: Vec<String>,
    /// The kinds among them, without repeats.
    pub kinds: Vec<Kind>,
    /// The requested properties.
    pub props: BTreeMap<String, PropInfo>,
    /// The start and end heads of the lines and arrows among the targets. Absent when there are none (and for
    /// the defaults, whose arrow heads are `tool.set_arrow`'s settings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heads: Option<HeadsInfo>,
}

fn get(s: &mut Session, a: GetArgs) -> Result<GetOut, Refusal> {
    let cmd = s.cmd();
    let wanted: Vec<Prop> = match &a.props {
        None => Prop::ALL.to_vec(),
        Some(list) => {
            let mut out = Vec::new();
            for n in list {
                let p = Prop::from_name(n).ok_or_else(|| {
                    let valid: Vec<&str> = Prop::ALL.iter().map(|p| p.name()).collect();
                    Refusal::bad_arg(
                        cmd,
                        "props",
                        format!("unknown style property `{}`. Valid properties: {}", echo(n), valid.join(", ")),
                    )
                })?;
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            out
        }
    };
    let scope = match a.scope {
        GetScope::Auto => Scope::Auto,
        GetScope::Selection => Scope::Selection,
        GetScope::Defaults => Scope::Defaults,
    };
    let p = plan(s, a.ids.as_deref(), scope)?;
    let (source, ids, kinds, rows): (&str, Vec<String>, Vec<Kind>, Vec<StyleProps>) = if p.targets.is_empty() {
        ("defaults", Vec::new(), Vec::new(), vec![s.tool.defaults().to_props()])
    } else {
        let read = read(s.store(), &p.targets, true).map_err(|e| refusal(cmd, e))?;
        let mut kinds: Vec<Kind> = read.iter().map(|(k, _)| *k).collect();
        kinds.sort();
        kinds.dedup();
        (
            "selection",
            p.targets.iter().map(|i| i.as_str().to_owned()).collect(),
            kinds,
            read.into_iter().map(|(_, p)| p).collect(),
        )
    };
    let props = wanted
        .into_iter()
        .map(|prop| {
            let values: Vec<Value> = rows.iter().filter_map(|r| r.value(prop)).collect();
            let value = match values.split_first() {
                None => None,
                Some((first, rest)) if rest.iter().all(|v| v == first) => Some(first.clone()),
                Some(_) => Some(json!("mixed")),
            };
            let applicable = !values.is_empty();
            let reason = (!applicable && source == "selection").then(|| {
                Refusal::not_applicable("style.set", &format!("`{}`", prop.name()), prop.applies_to()).message
            });
            (prop.name().to_owned(), PropInfo { applicable, value, reason })
        })
        .collect();
    let heads = heads_of(s, &p.targets);
    Ok(GetOut { changed: false, source: source.to_owned(), ids, kinds, props, heads })
}

// ---- style.apply_theme ----

/// Arguments of `style.apply_theme`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyThemeArgs {
    /// The theme: default, monochrome, pastel, blueprint or high_contrast. See `style.list_themes`.
    pub name: String,
    /// The elements to change. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Where to write; the same as for `style.set`.
    #[serde(default)]
    pub scope: Scope,
    /// Also set the page colour to the theme's background.
    #[serde(default)]
    pub set_background: bool,
}

/// Output of `style.apply_theme`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApplyThemeOut {
    /// False when everything already matched the theme.
    pub changed: bool,
    /// The theme that was applied.
    pub theme: String,
    /// The properties each element took, by element id.
    pub applied: BTreeMap<String, Vec<String>>,
    /// The properties an element does not support.
    pub skipped: Vec<Skipped>,
    /// True when the defaults for new elements were part of this call.
    pub defaults_updated: bool,
    /// True when the page colour changed.
    pub background_set: bool,
}

fn apply_theme(s: &mut Session, a: ApplyThemeArgs) -> Result<ApplyThemeOut, Refusal> {
    let cmd = s.cmd();
    let Some(t) = theme(&a.name) else {
        return Err(Refusal::custom(
            "unknown_theme",
            cmd,
            format!(
                "Unknown theme `{}`. Available themes: {}. See them with `style.list_themes`.",
                echo(&a.name),
                theme_names()
            ),
        )
        .suggesting("style.list_themes", None));
    };
    let p = plan(s, a.ids.as_deref(), a.scope)?;
    let background = if a.set_background { t.background.map(|c| c.0) } else { None };
    let d = run(s, &t.props, &p, true, background)?;
    Ok(ApplyThemeOut {
        changed: d.changed,
        theme: t.name.to_owned(),
        applied: d.applied,
        skipped: d.skipped,
        defaults_updated: d.defaults_updated,
        background_set: d.background_set,
    })
}

// ---- style.list_themes ----

/// Output of `style.list_themes`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListThemesOut {
    /// Always false: reading changes nothing.
    pub changed: bool,
    /// The built-in themes with their bundles.
    pub themes: Vec<Theme>,
}

fn list_themes(_: &mut Session, _: NoArgs) -> Result<ListThemesOut, Refusal> {
    Ok(ListThemesOut { changed: false, themes: themes() })
}

// ---- registration ----

/// `style.set`: the document, the defaults and the persisted defaults; one gesture or one step.
const WRITE: Facts = Facts {
    undo: Undo::Gesture,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::TOOL).with(Mutates::SETTINGS),
    needs_selection: Selection::Soft,
    ports: PortSet::NONE,
    batch: crate::spec::Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Refused,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

/// The style family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "style.set",
        "style",
        "Set style",
        "Sets style properties on elements and on the defaults for new elements; properties an element does not support are skipped and reported.",
        &Typed::new(set),
        WRITE,
    )
    .host(Host::text_editor_takes_style())
    .coalesce("style.set", 1000)
    .example(r#"{"props":{"stroke_color":"blue-500","fill_color":"blue-100","stroke_width":4}}"#),
    CommandSpec::new(
        "style.get",
        "style",
        "Get style",
        "Reads style properties of elements or of the defaults: the common value or \"mixed\", and whether each property applies.",
        &Typed::new(get),
        Facts { needs_selection: Selection::Soft, ..Facts::READ_ONLY },
    ),
    CommandSpec::new(
        "style.apply_theme",
        "style",
        "Apply theme",
        "Applies a named style bundle to elements and to the defaults for new elements, and optionally sets the page colour.",
        &Typed::new(apply_theme),
        Facts { undo: Undo::Undoable, palette: true, ..WRITE },
    )
    .refusals(&["unknown_theme"])
    .example(r#"{"name":"pastel","set_background":true}"#),
    CommandSpec::new(
        "style.list_themes",
        "style",
        "List themes",
        "Lists the built-in themes with the style properties and background each one sets.",
        &Typed::new(list_themes),
        Facts::READ_ONLY,
    ),
];

#[cfg(test)]
#[path = "style_tests.rs"]
mod tests;
