//! `element.query` and `element.at`: the read-only half of the `element` family (docs/commands.md §2.4).
//!
//! Both commands are pure reads over the working store. The matching, ordering, paging, describing and
//! hit-testing rules are in `drafftink_edit::query`; the filter that accepts `@name` references is
//! `drafftink_edit::query::RefQuery`. The commands validate the arguments and shape the output.

use drafftink_edit::query::{
    AtOpts, DEFAULT_FIELDS, Field, MAX_LIMIT, Order, QueryCtx, QueryError, QueryOpts, describe, describe_all,
    elements_at, run_query, RefQuery, hit_members,
};
use drafftink_geom::kurbo::Point;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::args;
use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::{Host, HostEffect};
use crate::spec::{CommandSpec, Facts, Typed};

use super::element_common::{id_strings, target_refusal};

/// The refusal for a query error. `key` names the argument that held the filter.
pub(crate) fn query_refusal(cmd: &str, key: &str, e: QueryError) -> Refusal {
    match e {
        QueryError::Target(t) => target_refusal(cmd, t),
        QueryError::UnknownName(n) => Refusal::not_found(cmd, &format!("@{n}")),
        QueryError::BadRegex { pattern, reason } => Refusal::bad_arg(
            cmd,
            key,
            format!(
                "the text regex `{}` is not valid: {}. Fix the pattern, or use `contains` for a plain phrase",
                echo(&pattern),
                echo(&reason)
            ),
        ),
        QueryError::BadRef(r) => Refusal::bad_arg(
            cmd,
            key,
            format!("`{}` is not an element id or an @name. Ids use only letters, digits, '_' and '-'", echo(&r)),
        ),
        QueryError::BadRegion => {
            Refusal::bad_arg(cmd, key, "the region must have finite numbers for x, y, w and h")
        }
        QueryError::BadLimit => Refusal::bad_arg(cmd, key, format!("must be a whole number from 1 to {MAX_LIMIT}")),
    }
}

/// A world point from `[x, y]`, each coordinate finite and within the world limit.
pub(crate) fn world_pt(cmd: &str, key: &str, p: [f64; 2]) -> Result<Point, Refusal> {
    Ok(Point::new(args::world(cmd, &format!("{key}[0]"), p[0])?, args::world(cmd, &format!("{key}[1]"), p[1])?))
}

const READ: Facts = Facts::READ_ONLY;

// ---- element.query ----

/// Arguments of `element.query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct QueryArgs {
    /// Which elements to find. Every given field must match. Absent: every element. Elements are ids or `@name`.
    pub filter: Option<RefQuery>,
    /// Most elements to return, 1 to 1000.
    pub limit: usize,
    /// Elements to skip, for paging.
    pub offset: usize,
    /// What to say about each element. Default: id, kind, name, text, bounds.
    pub fields: Vec<Field>,
    /// `z` back to front, `reading` top to bottom then left to right, `position` left to right then top to bottom.
    pub order: Order,
}

impl Default for QueryArgs {
    fn default() -> Self {
        Self { filter: None, limit: 100, offset: 0, fields: DEFAULT_FIELDS.to_vec(), order: Order::Z }
    }
}

/// Result of `element.query`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct QueryOut {
    /// How many elements match the filter.
    pub total: usize,
    /// The requested page, one object per element with the requested fields.
    pub items: Vec<Value>,
    /// More matches exist after this page: call again with a larger `offset`.
    pub truncated: bool,
}

/// The most fields one call may ask for (there are 13 distinct ones).
const MAX_FIELDS: usize = 32;

fn query(s: &mut Session, a: QueryArgs) -> Result<QueryOut, Refusal> {
    let cmd = s.cmd();
    if a.limit == 0 || a.limit > MAX_LIMIT {
        return Err(query_refusal(cmd, "limit", QueryError::BadLimit));
    }
    if a.fields.is_empty() || a.fields.len() > MAX_FIELDS {
        return Err(Refusal::bad_arg(cmd, "fields", format!("must list 1 to {MAX_FIELDS} fields")));
    }
    let store = s.store();
    let q = match &a.filter {
        Some(f) => f.resolve(store).map_err(|e| query_refusal(cmd, "filter", e))?,
        None => Default::default(),
    };
    let ctx = QueryCtx { selected: Some(s.selection()) };
    let opts = QueryOpts { limit: a.limit, offset: a.offset, order: a.order };
    let r = run_query(store, &q, &ctx, &opts).map_err(|e| query_refusal(cmd, "filter", e))?;
    Ok(QueryOut { total: r.total, items: describe_all(store, &r.ids, &a.fields), truncated: r.truncated })
}

// ---- element.at ----

/// Arguments of `element.at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtArgs {
    /// World point `[x, y]`.
    pub point: [f64; 2],
    /// How far from an outline still counts as a hit, in screen pixels (divided by the zoom).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Return every hit, top to bottom, instead of the topmost.
    #[serde(default)]
    pub all: bool,
    /// Report group members as themselves instead of as their outermost group.
    #[serde(default)]
    pub deep: bool,
    /// Locked elements can be hit.
    #[serde(default)]
    pub include_locked: bool,
}

/// Default hit tolerance in screen pixels.
pub(crate) const DEFAULT_TOLERANCE: f64 = 6.0;
/// Largest hit tolerance in screen pixels.
pub(crate) const MAX_TOLERANCE: f64 = 1000.0;

fn default_tolerance() -> f64 {
    DEFAULT_TOLERANCE
}

/// Result of `element.at`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AtOut {
    /// How many hits.
    pub count: usize,
    /// The hits, top to bottom. Each has id, kind, name and bounds. A hit that counts as a whole group also has
    /// `group` (its id) and `members` (the element ids of the group).
    pub hits: Vec<Value>,
}

fn at(s: &mut Session, a: AtArgs) -> Result<AtOut, Refusal> {
    let cmd = s.cmd();
    let p = world_pt(cmd, "point", a.point)?;
    let tol = args::in_range(cmd, "tolerance", a.tolerance, 0.0, MAX_TOLERANCE)?;
    let opts = AtOpts { tolerance: tol / s.view().zoom, all: a.all, deep: a.deep, include_locked: a.include_locked };
    let store = s.store();
    let hits: Vec<Value> = elements_at(store, p, &opts)
        .into_iter()
        .map(|h| {
            let mut v = describe(store, &h.id, &DEFAULT_FIELDS);
            if let (Some(g), Value::Object(m)) = (&h.group, &mut v) {
                m.insert("group".to_owned(), Value::String(g.as_str().to_owned()));
                m.insert("members".to_owned(), serde_json::json!(id_strings(hit_members(store, &h).iter())));
            }
            v
        })
        .collect();
    Ok(AtOut { count: hits.len(), hits })
}

/// The commands of this part of the `element` family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "element.query",
        "element",
        "Find elements",
        "Finds elements by id, text, kind, style, region, connection, group, frame, name, lock or selection.",
        &Typed::new(query),
        READ,
    )
    .host(Host::effect(HostEffect::ShowsSearchResults))
    .example(r#"{"filter":{"kind":["rectangle","ellipse"],"text":"login"},"limit":20}"#),
    CommandSpec::new(
        "element.at",
        "element",
        "Element at point",
        "Hit-tests a world point and returns the topmost unit under it, or every hit with all=true.",
        &Typed::new(at),
        READ,
    )
    .example(r#"{"point":[120,80]}"#),
];

#[cfg(test)]
#[path = "element_query_tests.rs"]
mod tests;
