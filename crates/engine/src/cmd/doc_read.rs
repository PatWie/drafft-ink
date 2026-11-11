//! `doc.outline` and `doc.inspect`: the read-only views of the document that an agent uses to look before it acts
//! (docs/commands.md §2.1 and §3). The other `doc` commands (open, save, export, ...) belong to another family
//! module; these two only read, so they live here with the other introspection of this task.
//!
//! `doc.outline` is deterministic: the same document always gives the same text, in reading order (top to bottom,
//! then left to right), whatever order the elements were made in.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_edit::query::{self, Outline, OutlineOpts, OutlineScope};
use drafftink_edit::query_cmd::ALL_FIELDS;
use drafftink_geom::kurbo::Rect;
use drafftink_scene::{ElementId, GroupId, Kind, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::text::support::RectOut;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{CommandSpec, Facts, Typed};

// ---------------------------------------------------------------------------------------------
// doc.outline
// ---------------------------------------------------------------------------------------------

/// What `doc.outline` returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutlineFormat {
    /// The compact text only.
    Text,
    /// The structure as JSON only.
    Json,
    /// Both (the default).
    #[default]
    Both,
}

/// Which part of the document the outline covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutlineScopeArg {
    /// Everything (the default).
    #[default]
    All,
    /// The selected elements.
    Selection,
    /// Elements that touch the visible area.
    Visible,
    /// The children of `frame`.
    Frame,
}

/// Arguments of `doc.outline`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct OutlineArgs {
    /// `text`, `json` or `both`.
    pub format: OutlineFormat,
    /// `all`, `selection`, `visible` or `frame`.
    pub scope: OutlineScopeArg,
    /// With scope `frame`: the frame (an id or `@name`).
    pub frame: Option<Ref>,
    /// Show at most this many nodes and as many edges (1 to 5000). The text says when it cut something.
    pub max_nodes: i64,
}

impl Default for OutlineArgs {
    fn default() -> Self {
        Self { format: OutlineFormat::Both, scope: OutlineScopeArg::All, frame: None, max_nodes: 500 }
    }
}

/// Output of `doc.outline`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OutlineOut {
    /// One line per node and per edge, in a fixed order. Absent for format `json`.
    pub text: Option<String>,
    /// Nodes, edges, loose texts, groups and frames. Absent for format `text`.
    pub outline: Option<Outline>,
}

fn outline(s: &mut Session, a: OutlineArgs) -> Result<OutlineOut, Refusal> {
    let cmd = s.cmd();
    let max = args::in_range(cmd, "max_nodes", a.max_nodes as f64, 1.0, 5000.0)? as usize;
    let mut scope = OutlineScope::default();
    match (a.scope, &a.frame) {
        (OutlineScopeArg::Frame, None) => {
            return Err(Refusal::bad_arg(cmd, "frame", "is needed when scope is \"frame\". Give the frame's id or @name"));
        }
        (OutlineScopeArg::Frame, Some(f)) => {
            let id = s.resolve_ref(f)?;
            let kind = s.element(&id).map(|e| e.tag());
            if kind != Some(Kind::Frame) {
                return Err(Refusal::wrong_kind(cmd, kind.map_or("missing", Kind::name), "frame elements"));
            }
            scope.frame = Some(id);
        }
        (_, Some(_)) => {
            return Err(Refusal::bad_arg(cmd, "frame", "only works with scope \"frame\". Set scope=\"frame\", or drop `frame`"));
        }
        (OutlineScopeArg::Selection, None) => {
            let sel: BTreeSet<ElementId> = s.selection().iter().cloned().collect();
            if sel.is_empty() {
                return Err(Refusal::nothing_to_do(cmd, "outline", "nothing is selected. Select elements, or use scope \"all\""));
            }
            scope.only = Some(sel);
        }
        (OutlineScopeArg::Visible, None) => {
            let (x, y, w, h) = s.view().visible();
            scope.region = Some(Rect::new(x, y, x + w, y + h));
        }
        (OutlineScopeArg::All, None) => {}
    }
    let o = query::outline(s.store(), &OutlineOpts { max_nodes: max, scope });
    let text = (a.format != OutlineFormat::Json).then(|| o.to_text());
    let outline = (a.format != OutlineFormat::Text).then_some(o);
    Ok(OutlineOut { text, outline })
}

// ---------------------------------------------------------------------------------------------
// doc.inspect
// ---------------------------------------------------------------------------------------------

/// How much `doc.inspect` returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InspectDetail {
    /// Counts and bounds of the document (the default).
    #[default]
    Summary,
    /// Every field of each element.
    Full,
}

/// Arguments of `doc.inspect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct InspectArgs {
    /// With detail `full`: only these elements (ids or `@name`).
    pub ids: Option<Vec<Ref>>,
    /// `summary` or `full`.
    pub detail: InspectDetail,
    /// With detail `full`: also list deleted elements (they carry `deleted: true`).
    pub include_deleted: bool,
    /// With detail `full`: page size, 1 to 1000.
    pub limit: i64,
    /// With detail `full`: elements to skip.
    pub offset: i64,
}

impl Default for InspectArgs {
    fn default() -> Self {
        Self { ids: None, detail: InspectDetail::Summary, include_deleted: false, limit: 200, offset: 0 }
    }
}

fn inspect(s: &mut Session, a: InspectArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let limit = args::in_range(cmd, "limit", a.limit as f64, 1.0, 1000.0)? as usize;
    let offset = args::in_range(cmd, "offset", a.offset as f64, 0.0, 1.0e9)? as usize;
    let store = s.store();
    if a.detail == InspectDetail::Summary {
        if a.ids.is_some() || a.include_deleted {
            return Err(Refusal::bad_args(
                cmd,
                "`ids` and `include_deleted` only work with detail \"full\". Set detail=\"full\", or drop them",
            ));
        }
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        let mut groups: BTreeMap<&GroupId, usize> = BTreeMap::new();
        let (mut frames, mut locked, mut count, mut labels, mut bound_arrows) = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut bounds: Option<Rect> = None;
        for e in store.iter() {
            // A bound label follows its container: it is counted apart, as `element.query` leaves it out.
            if e.container_id().is_some() {
                labels += 1;
                continue;
            }
            *kinds.entry(e.tag().name()).or_default() += 1;
            for g in &e.group_ids {
                *groups.entry(g).or_default() += 1;
            }
            frames += usize::from(e.tag() == Kind::Frame);
            bound_arrows += usize::from(e.tag() == Kind::Arrow && e.binding_targets().iter().any(Option::is_some));
            locked += usize::from(e.locked);
            let b = query::element_bounds(e);
            bounds = Some(bounds.map_or(b, |x| x.union(b)));
            count += 1;
        }
        let tab = s.tab();
        return Ok(json!({
            "name": store.meta().name,
            "tab": tab.id.to_string(),
            "revision": tab.doc.revision(),
            "dirty": tab.dirty(),
            "save_target": Value::Null,
            "background": store.meta().background,
            "elements": count,
            "labels": labels,
            "kinds": kinds,
            "bounds": bounds.map(RectOut::from),
            "groups": groups.values().filter(|n| **n >= 2).count(),
            "frames": frames,
            "bound_arrows": bound_arrows,
            "locked": locked,
        }));
    }
    let wanted: Vec<ElementId> = match &a.ids {
        Some(refs) => {
            let mut out = Vec::with_capacity(refs.len());
            for r in refs {
                let id = match s.resolve_ref(r) {
                    Ok(id) => id,
                    Err(e) if a.include_deleted => match ElementId::new(&r.0).ok().filter(|i| store.get(i).is_some()) {
                        Some(id) => id,
                        None => return Err(e),
                    },
                    Err(e) => return Err(e),
                };
                out.push(id);
            }
            out
        }
        None => store.iter_all().filter(|e| a.include_deleted || !e.deleted).map(|e| e.id.clone()).collect(),
    };
    let total = wanted.len();
    let page: Vec<ElementId> = wanted.into_iter().skip(offset).take(limit).collect();
    let mut items = query::describe_all(store, &page, &ALL_FIELDS);
    for (item, id) in items.iter_mut().zip(&page) {
        if let (Some(o), Some(e)) = (item.as_object_mut(), store.get(id))
            && e.deleted
        {
            o.insert("deleted".to_owned(), json!(true));
        }
    }
    let truncated = offset.saturating_add(items.len()) < total;
    Ok(json!({ "total": total, "items": items, "truncated": truncated }))
}

/// The introspection commands of the `doc` family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "doc.inspect",
        "doc",
        "Inspect document",
        "Returns a summary of the document (counts, bounds, revision) or every field of its elements.",
        &Typed::new(inspect),
        Facts { palette: true, ..Facts::READ_ONLY },
    )
    .example(r#"{"detail":"summary"}"#),
    CommandSpec::new(
        "doc.outline",
        "doc",
        "Outline document",
        "Returns the structure of the document in a compact, deterministic form: nodes, edges, groups and frames.",
        &Typed::new(outline),
        Facts { palette: true, ..Facts::READ_ONLY },
    )
    .refusals(&["nothing_to_do"])
    .example(r#"{"format":"text","scope":"all"}"#),
];

#[cfg(test)]
#[path = "doc_read_tests.rs"]
mod tests;
