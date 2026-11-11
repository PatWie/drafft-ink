//! `element` family, links: `element.set_link` and `element.open_link` (docs/commands.md §2.4).
//!
//! The rules (what a link may contain, web link or element link, the second check before opening) are in
//! `drafftink_edit::link`. The commands resolve `@name` and the selection, write the field, and for
//! `element.open_link` hand the result back: a web link comes back as a value for the shell to open, because the
//! engine never opens a browser; an element link selects the element and brings it into view.

use drafftink_edit::link::{self, Input, LinkError, Stored};
use drafftink_scene::{ElementId, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::element_common::target_refusal;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Host, HostEffect};
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo};

/// The refusal code for text that is not a usable link.
const INVALID_LINK: &str = "invalid_link";
/// The refusal code for an element that has no link to open.
const NO_LINK: &str = "no_link";

/// The one element a command acts on: the given reference, else the single selected element.
fn one_target(s: &Session, id: Option<&Ref>) -> Result<ElementId, Refusal> {
    if let Some(r) = id {
        return s.resolve_ref(r);
    }
    let live: Vec<ElementId> = s.selection().iter().filter(|i| s.element(i).is_some()).cloned().collect();
    match live.as_slice() {
        [one] => Ok(one.clone()),
        other => Err(Refusal::needs_single(s.cmd(), "element", other.len())),
    }
}

fn link_refusal(cmd: &str, e: LinkError) -> Refusal {
    match e {
        LinkError::Target(t) => target_refusal(cmd, t),
        LinkError::Store(m) => Refusal::internal(cmd, m),
        other => Refusal::custom(INVALID_LINK, cmd, other.to_string()),
    }
}

/// The live element an element link names: `e12` or `@name`.
fn linked_element(s: &Session, target: &str) -> Result<ElementId, Refusal> {
    let cmd = s.cmd();
    let id = if target.starts_with('@') {
        s.resolve_ref(&Ref(target.to_owned()))?
    } else {
        ElementId::new(target).map_err(|_| Refusal::not_found(cmd, target))?
    };
    if link::target_exists(s.store(), &id) {
        Ok(id)
    } else {
        Err(Refusal::not_found(cmd, target))
    }
}

// ---- element.set_link ---------------------------------------------------------------------------------------

/// Arguments of `element.set_link`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetLinkArgs {
    /// The element: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
    /// A web address (`https://example.com`, `mailto:a@b.example`; a bare `example.com` gets `https://`), or an
    /// element in this document (`#e3`, `@name`). `null`, absent or empty removes the link.
    pub link: Option<String>,
}

/// Result of `element.set_link`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetLinkOut {
    /// The element.
    pub id: String,
    /// The link as stored (`#e3` for an element link), `null` when removed.
    pub link: Option<String>,
    /// `url`, `element`, or `null` when removed.
    pub kind: Option<&'static str>,
    /// False when the element already had this link.
    pub changed: bool,
}

fn set_link(s: &mut Session, a: SetLinkArgs) -> Result<SetLinkOut, Refusal> {
    let cmd = s.cmd();
    let id = one_target(s, a.id.as_ref())?;
    let (stored, kind) = match link::parse_input(a.link.as_deref().unwrap_or("")).map_err(|e| link_refusal(cmd, e))? {
        Input::Clear => (None, None),
        Input::Url(u) => (Some(u), Some("url")),
        Input::Element(r) => {
            let target = linked_element(s, &r)?;
            (Some(link::element_link(&target)), Some("element"))
        }
    };
    let (changed, _) = s
        .tab_mut()
        .doc
        .edit::<bool, LinkError>(|tx| link::set(tx, &id, stored.clone()))
        .map_err(|e| link_refusal(cmd, e))?;
    Ok(SetLinkOut { id: id.as_str().to_owned(), link: stored, kind, changed })
}

// ---- element.open_link --------------------------------------------------------------------------------------

/// Arguments of `element.open_link`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct OpenLinkArgs {
    /// The element: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
}

/// Result of `element.open_link`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OpenLinkOut {
    /// The element that holds the link.
    pub id: String,
    /// `url` (the shell opens `url`) or `element` (the view moved to `target`).
    pub kind: &'static str,
    /// The web address to open, for `kind: url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The element the link points to, for `kind: element`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// True when the selection or the view changed.
    pub changed: bool,
}

fn open_link(s: &mut Session, a: OpenLinkArgs) -> Result<OpenLinkOut, Refusal> {
    let cmd = s.cmd();
    let id = one_target(s, a.id.as_ref())?;
    let stored = s.element(&id).and_then(|e| e.link.clone()).ok_or_else(|| {
        Refusal::custom(
            NO_LINK,
            cmd,
            format!("Element `{id}` has no link. Add one with `element.set_link`.", id = id.as_str()),
        )
        .with_details(json!({ "id": id.as_str() }))
    })?;
    // A document from outside can hold any text in `link`: check it again before it reaches a browser.
    match link::classify_stored(&stored).map_err(|e| link_refusal(cmd, e))? {
        Stored::Url(url) => Ok(OpenLinkOut {
            id: id.as_str().to_owned(),
            kind: "url",
            url: Some(url),
            target: None,
            changed: false,
        }),
        Stored::Element(target) => {
            let target_id = linked_element(s, &target)?;
            let before = s.fingerprint();
            show_element(s, &target_id)?;
            Ok(OpenLinkOut {
                id: id.as_str().to_owned(),
                kind: "element",
                url: None,
                target: Some(target_id.as_str().to_owned()),
                changed: s.fingerprint() != before,
            })
        }
    }
}

/// Selects the element when it can be selected (a locked one cannot) and scrolls it into view, zooming out when
/// it does not fit but never in (Excalidraw: "scale-down").
fn show_element(s: &mut Session, id: &ElementId) -> Result<(), Refusal> {
    let locked = s.element(id).is_some_and(|e| e.locked);
    if !locked {
        s.run_nested("selection.select", json!({ "ids": [id.as_str()] }))?;
    }
    let zoom = s.view().zoom;
    s.run_nested("view.zoom_to_fit", json!({ "scope": "ids", "ids": [id.as_str()], "max_zoom": zoom }))?;
    Ok(())
}

/// `open_link` is offered when some element has a link.
fn any_link(s: &Session) -> Result<(), Refusal> {
    if s.store().iter().any(|e| e.link.is_some()) {
        Ok(())
    } else {
        Err(Refusal::nothing_to_do(s.cmd(), "open", "no element has a link. Add one with `element.set_link`"))
    }
}

// ---- registration -------------------------------------------------------------------------------------------

const SET: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::Soft,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const OPEN: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::OBSERVED.with(Mutates::VIEW),
    view_only: Gate::Allowed,
    redraw: Redraw::Camera,
    agent: false,
    doc_write: crate::spec::DocWrite::NotDocument,
    ..SET
};

/// Commands of this part of the family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "element.set_link",
        "element",
        "Link",
        "Sets or removes the hyperlink of an element: a web address, or a link to another element.",
        &Typed::new(set_link),
        SET,
    )
    .host(Host::effect(HostEffect::CompletesLinkEdit))
    .refusals(&[INVALID_LINK])
    .example(r##"{"id":"e1","link":"https://example.com"}"##),
    CommandSpec::new(
        "element.open_link",
        "element",
        "Open link",
        "Opens the hyperlink of an element: returns the web address for the shell to open, or goes to the linked element.",
        &Typed::new(open_link),
        OPEN,
    )
    .host(Host::effect(HostEffect::ReturnsUrlToOpen))
    .available(any_link)
    .refusals(&[NO_LINK])
    .example(r#"{"id":"e1"}"#),
];

#[cfg(test)]
#[path = "element_link_tests.rs"]
mod tests;
