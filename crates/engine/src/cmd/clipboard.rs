//! `clipboard`: copy, cut, paste, copy and paste style, copy as image (docs/commands.md §2.6).
//!
//! One path for every surface. The keyboard, the context menu and the palette all call these commands; the only
//! difference is the `at` argument, which a shortcut fills with the pointer position.
//!
//! * `copy` and `cut` build the payload with `drafftink_io::clipboard::pack` (the native subset export of the
//!   selected elements, their bound text, frame children and pictures, plus the Excalidraw flavour when the
//!   setting `clipboard.excalidraw_compat` is on). The payload is kept in the session buffer and queued for the
//!   `Clipboard` port, so it reaches the system clipboard only when the command succeeded.
//! * `paste` finds what to paste (see [`paste`]) and puts it in with `drafftink_edit::paste` (new ids, new groups,
//!   remapped bindings), `image.insert` logic or a text element.
//! * `copy_style` and `paste_style` use `drafftink_edit::style::{read, apply}`: the same properties and the same
//!   skipping rules as `style.set`.
//! * `copy_as` writes a PNG (through the `Renderer` port), an SVG or the text of the selection.
//!
//! Session state kept here (outside the rollback, so the commands write it last): the copy buffer and the style
//! buffer, both in `Session::ext`.

pub(crate) mod common;
mod copy_as;
mod paste;

use std::collections::HashSet;

use drafftink_edit::group::units::{Relations, with_companions};
use drafftink_edit::lifecycle::{self, ChildPolicy, LifecycleError};
use drafftink_edit::style::{self, StyleError, StyleProps};
use drafftink_io::{clipboard as wire, native};
use drafftink_scene::{ElementId, Kind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cmd::element_common::target_refusal;
use crate::ports::{ClipPayload, PortWrite};
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed,
    Undo,
};

pub use common::{At, AtWord};

/// The settings key of the Excalidraw flavour of a copy.
pub const COMPAT_KEY: &str = "clipboard.excalidraw_compat";

/// The last copy made in this session, as the envelope of `drafftink_io::clipboard`.
#[derive(Default)]
pub(crate) struct CopyBuffer {
    pub envelope: Option<Value>,
}

/// The style remembered by `clipboard.copy_style`.
#[derive(Default)]
pub(crate) struct StyleBuffer {
    pub props: Option<StyleProps>,
}

fn yes() -> bool {
    true
}

fn lifecycle_refusal(cmd: &str, e: LifecycleError) -> Refusal {
    match e {
        LifecycleError::Target(t) => target_refusal(cmd, t),
        LifecycleError::NoTarget => Refusal::no_target(cmd),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

// ---- copy and cut ----

/// What a copy holds.
struct Packed {
    payload: Value,
    count: usize,
}

/// Builds the payload for `ids` and their companions (bound text, frame children).
fn pack(s: &mut Session, ids: &[ElementId]) -> Result<Packed, Refusal> {
    let cmd = s.cmd();
    let store = s.store();
    let rel = Relations::build(store);
    let all = with_companions(store, &rel, ids, true);
    if all.is_empty() {
        return Err(Refusal::no_target(cmd));
    }
    let keep: HashSet<ElementId> = all.iter().cloned().collect();
    let doc = native::subset(&s.tab().doc.snapshot(), &keep);
    let compat = s.settings.bool_or(COMPAT_KEY, true);
    let packed = wire::pack(&doc, compat).map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    for w in packed.report.warnings.iter().take(5) {
        s.warn(w.clone());
    }
    Ok(Packed { payload: packed.payload, count: all.len() })
}

/// Keeps the payload in the session buffer and queues the system clipboard write. Call it last.
fn publish(s: &mut Session, payload: Value, system: bool) -> bool {
    s.ext.with::<CopyBuffer, _>(|b| b.envelope = Some(payload.clone()));
    if !system {
        return false;
    }
    if s.ports.clipboard.is_none() {
        s.warn("The system clipboard is not available here, so the copy stays inside drafft.ink. Paste it with `clipboard.paste` and source `internal`.");
        return false;
    }
    s.queue_write(PortWrite::Clipboard(ClipPayload::Elements(payload)));
    true
}

/// Arguments of `clipboard.copy` and `clipboard.cut`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyArgs {
    /// The elements to copy. Default: the selection. Bound text and the children of a copied frame come along.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Also write to the system clipboard. Without a clipboard port the copy stays in the session and a warning
    /// says so.
    #[serde(default = "yes")]
    pub system: bool,
}

impl Default for CopyArgs {
    fn default() -> Self {
        Self { ids: None, system: true }
    }
}

/// Output of `clipboard.copy` and `clipboard.cut`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CopyOut {
    /// Elements in the copy, bound text and frame children included.
    pub count: usize,
    /// True when the payload was handed to the system clipboard.
    pub system: bool,
}

fn copy(s: &mut Session, a: CopyArgs) -> Result<CopyOut, Refusal> {
    let ids = s.targets(a.ids.as_deref())?;
    let p = pack(s, &ids)?;
    let system = publish(s, p.payload, a.system);
    Ok(CopyOut { count: p.count, system })
}

fn cut(s: &mut Session, a: CopyArgs) -> Result<CopyOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let p = pack(s, &ids)?;
    let (report, _) = s
        .tab_mut()
        .doc
        .edit::<lifecycle::DeleteReport, LifecycleError>(|tx| lifecycle::delete(tx, &ids, ChildPolicy::Delete))
        .map_err(|e| lifecycle_refusal(cmd, e))?;
    let gone: HashSet<&ElementId> = report.deleted.iter().collect();
    let obs = &mut s.tab_mut().observed;
    obs.selection.retain(|i| !gone.contains(i));
    if obs.selection.is_empty() {
        obs.selected_groups.clear();
    }
    obs.linear_editor = None;
    let system = publish(s, p.payload, a.system);
    Ok(CopyOut { count: p.count, system })
}

// ---- copy_style and paste_style ----

/// Arguments of `clipboard.copy_style`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CopyStyleArgs {
    /// The element to take the style from. Default: the first selected element in z-order.
    pub from: Option<Ref>,
}

/// Output of `clipboard.copy_style`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CopyStyleOut {
    /// The element the style came from.
    pub from: String,
    /// Its kind.
    pub kind: Kind,
    /// The remembered properties. Text properties of a shape with a label come from the label.
    pub props: StyleProps,
}

fn style_error(cmd: &str, e: StyleError) -> Refusal {
    match e {
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
        other => Refusal::internal(cmd, other.to_string()),
    }
}

fn copy_style(s: &mut Session, a: CopyStyleArgs) -> Result<CopyStyleOut, Refusal> {
    let cmd = s.cmd();
    let from = match &a.from {
        Some(r) => s.resolve_ref(r)?,
        None => {
            let mut sel = s.targets(None)?;
            sel.sort_by(|x, y| {
                let key = |i: &ElementId| s.element(i).map(|e| e.index.clone());
                key(x).cmp(&key(y)).then_with(|| x.cmp(y))
            });
            sel.into_iter().next().ok_or_else(|| Refusal::no_target(cmd))?
        }
    };
    let read = style::read(s.store(), std::slice::from_ref(&from), true).map_err(|e| style_error(cmd, e))?;
    let Some((kind, props)) = read.into_iter().next() else { return Err(Refusal::not_found(cmd, from.as_str())) };
    s.ext.with::<StyleBuffer, _>(|b| b.props = Some(props.clone()));
    Ok(CopyStyleOut { from: from.as_str().to_owned(), kind, props })
}

/// Arguments of `clipboard.paste_style`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PasteStyleArgs {
    /// The elements to restyle. Default: the selection.
    pub ids: Option<Vec<Ref>>,
}

/// Output of `clipboard.paste_style`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PasteStyleOut {
    /// The properties each element took, by element id.
    pub applied: std::collections::BTreeMap<String, Vec<String>>,
    /// The properties an element does not support.
    pub skipped: Vec<Skipped>,
}

/// The properties that were skipped for an element.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Skipped {
    /// The element.
    pub id: String,
    /// The properties it does not support.
    pub props: Vec<String>,
}

fn paste_style(s: &mut Session, a: PasteStyleArgs) -> Result<PasteStyleOut, Refusal> {
    let cmd = s.cmd();
    let targets = s.targets(a.ids.as_deref())?;
    let Some(props) = s.ext.with::<StyleBuffer, _>(|b| b.props.clone()) else {
        return Err(Refusal::custom(
            "nothing_to_do",
            cmd,
            "No style has been copied yet. Copy one with `clipboard.copy_style` first.",
        ));
    };
    let (report, _) = s
        .tab_mut()
        .doc
        .edit::<style::StyleReport, StyleError>(|tx| style::apply(tx, &targets, &props, true))
        .map_err(|e| style_error(cmd, e))?;
    let names = |set: &std::collections::BTreeSet<style::Prop>| set.iter().map(|p| p.name().to_owned()).collect();
    Ok(PasteStyleOut {
        applied: report.applied.iter().map(|(id, set)| (id.as_str().to_owned(), names(set))).collect(),
        skipped: report
            .skipped
            .iter()
            .map(|(id, set)| Skipped { id: id.as_str().to_owned(), props: names(set) })
            .collect(),
    })
}

// ---- registration ----

/// Copying: reads the document, keeps the buffer and queues a clipboard write.
const READ: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::PORTS,
    needs_selection: Selection::Required,
    ports: PortSet::CLIPBOARD,
    batch: Batch::Allowed,
    view_only: Gate::Allowed,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Allowed,
    redraw: Redraw::None,
    ring: RingMode::Always,
    palette: true,
    agent: true,
};

/// Changing the document.
const WRITE: Facts = Facts {
    undo: Undo::Undoable,
    mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::PORTS),
    view_only: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ..READ
};

const COPY_KEYS: &[Binding] = &[Binding::new(&["Mod+C"], Scope::Canvas)];
const CUT_KEYS: &[Binding] = &[Binding::new(&["Mod+X"], Scope::Canvas)];
const PASTE_KEYS: &[Binding] = &[
    Binding::new(&["Mod+V"], Scope::Canvas).with_args(r#"{"at":"$cursor"}"#),
    Binding::new(&["Mod+Shift+V"], Scope::Canvas)
        .with_args(r#"{"at":"$cursor","plain_text":true}"#)
        .titled("Paste as plain text"),
];
const COPY_STYLE_KEYS: &[Binding] = &[Binding::new(&["Mod+Alt+C"], Scope::Canvas)];
const PASTE_STYLE_KEYS: &[Binding] = &[Binding::new(&["Mod+Alt+V"], Scope::Canvas)];
const COPY_AS_KEYS: &[Binding] = &[Binding::new(&["Mod+Shift+E", "Mod+Shift+C", "Shift+Alt+C"], Scope::Canvas)
    .with_args(r#"{"format":"png"}"#)
    .titled("Copy as PNG")];

/// The clipboard family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "clipboard.copy",
        "clipboard",
        "Copy",
        "Copies the selected elements (with bound text, frame children and pictures) to the session and the system clipboard.",
        &Typed::new(copy),
        READ,
    )
    .shortcuts(COPY_KEYS),
    CommandSpec::new(
        "clipboard.cut",
        "clipboard",
        "Cut",
        "Copies the selected elements to the clipboard and deletes them, as one undo step.",
        &Typed::new(cut),
        WRITE,
    )
    .shortcuts(CUT_KEYS),
    CommandSpec::new(
        "clipboard.paste",
        "clipboard",
        "Paste",
        "Pastes copied elements, Excalidraw data, a Mermaid diagram, an image or plain text, centred on a point.",
        &Typed::new(paste::paste),
        Facts { needs_selection: Selection::No, ..WRITE },
    )
    .refusals(&["clipboard_empty", "mermaid_unsupported"])
    .shortcuts(PASTE_KEYS)
    .example(r#"{"source":{"text":"hello"},"at":[0,0]}"#),
    CommandSpec::new(
        "clipboard.copy_style",
        "clipboard",
        "Copy style",
        "Remembers the style properties of one element so that clipboard.paste_style can apply them to others.",
        &Typed::new(copy_style),
        READ,
    )
    .shortcuts(COPY_STYLE_KEYS),
    CommandSpec::new(
        "clipboard.paste_style",
        "clipboard",
        "Paste style",
        "Applies the style remembered by clipboard.copy_style to the selected elements that support its properties.",
        &Typed::new(paste_style),
        Facts { mutates: Mutates::DOC, ..WRITE },
    )
    .shortcuts(PASTE_STYLE_KEYS),
    CommandSpec::new(
        "clipboard.copy_as",
        "clipboard",
        "Copy as image",
        "Writes the selection or the whole document to the system clipboard as a PNG, an SVG or plain text.",
        &Typed::new(copy_as::copy_as),
        Facts {
            needs_selection: Selection::Soft,
            ports: PortSet::RENDERER.with(PortSet::CLIPBOARD),
            ..READ
        },
    )
    .shortcuts(COPY_AS_KEYS),
];

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
